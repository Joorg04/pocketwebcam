#!/usr/bin/env python3
"""Fake Pocket Webcam phone for testing the desktop receiver without an iPhone.

Speaks PROTOCOL.md exactly like the real app: advertises over mDNS (via
avahi-publish), accepts one receiver, and streams ffmpeg test patterns encoded
as H.264 or HEVC. Each fake "lens" shows a different pattern so switching is
visible. Its "microphone" is a 440 Hz tone that beeps twice a second.

Usage:
    python3 tools/fake_phone.py [--port 7310] [--name "Fake Phone"] [--id ID] [--no-mdns] [--no-audio] [--no-id]
                                [--scan URL] [--decline] [--forget]

Protocol v2: every connection is TLS 1.3 with a certificate the fake phone makes
once (kept in ~/.cache/pocketwebcam-fake-phone/<name>/, with its paired
computers). Unlike a real phone it can't accept any computer's certificate (Python's
ssl module has no hook for that), so it trusts the one in --desktop-cert, by default
this computer's own Pocket Webcam identity.

Pairing: --scan takes the link from the desktop's QR code (copy it from the app's
pairing dialog or the terminal receiver's output) and pairs like a phone that scanned
it. A computer that asks to pair by code is accepted after printing the code, unless
--decline is given. --forget forgets all paired computers first.

--no-audio behaves like a phone app from before audio support (no `audio` in
CAPABILITIES, SET_CONFIG audio is ignored).
--no-id behaves like a phone app from before identifiers (no `id` in the mDNS
record or in HELLO). --id sets the identifier (by default it's made from the
name); keep it and change --name to act like a phone that was renamed.

While a receiver is connected, `kill -USR1 <pid>` pauses the camera the way iOS
does when another app takes it (STATE says "cameraPaused": "inUse" and the video
stops); the same signal again resumes it.

Requires: ffmpeg (with libx264/libx265), python3-cryptography, avahi-publish (optional).
"""

import argparse
import base64
import hashlib
import hmac
import json
import os
import math
import queue
import shutil
import signal
import socket
import ssl
import struct
import subprocess
import threading
import time
import urllib.parse

PROTOCOL_VERSION = 2

HELLO, CAPABILITIES, SET_CONFIG, STATE = 0x01, 0x02, 0x03, 0x04
STREAM_FORMAT, VIDEO_FRAME, KEYFRAME_REQ = 0x10, 0x11, 0x12
AUDIO_FORMAT, AUDIO_FRAME = 0x13, 0x14
PAIR_REQUEST, PAIR_PROOF, PAIR_RESULT = 0x30, 0x31, 0x32
PING, PONG, ERROR = 0x20, 0x21, 0x7F

MAX_PAYLOAD = 16 * 1024 * 1024

FORMATS = [
    {"width": 3840, "height": 2160, "fps": [24, 30]},
    {"width": 1920, "height": 1080, "fps": [24, 30, 60]},
    {"width": 1280, "height": 720, "fps": [24, 30, 60]},
]

# Each fake camera maps to an ffmpeg lavfi source pattern.
CAMERAS = [
    {"id": "fake:ultrawide", "name": "Ultra Wide", "position": "back", "lens": "ultraWide",
     "pattern": "smptehdbars"},
    {"id": "fake:wide", "name": "Wide", "position": "back", "lens": "wide",
     "pattern": "testsrc2"},
    {"id": "fake:tele", "name": "Telephoto", "position": "back", "lens": "telephoto",
     "pattern": "mandelbrot"},
    {"id": "fake:front", "name": "Front", "position": "front", "lens": "trueDepth",
     "pattern": "rgbtestsrc"},
]


AUDIO_RATE = 48000
CHUNK = 480  # 10 ms


def capabilities(audio):
    caps = {
        "cameras": [
            {
                "id": c["id"], "name": c["name"], "position": c["position"], "lens": c["lens"],
                "zoom": {"min": 1.0, "max": 1.0},
                "formats": FORMATS,
                "features": [],
            }
            for c in CAMERAS
        ],
        "codecs": ["h264", "hevc"],
        "bitrate": {"min": 1_000_000, "max": 50_000_000},
    }
    if audio:
        caps["audio"] = {"codecs": ["pcm_s16le"], "sampleRates": [AUDIO_RATE], "channels": [1]}
    return caps


def now_us():
    return time.monotonic_ns() // 1000


class Encoder:
    """Runs ffmpeg and yields (is_keyframe, annexb_bytes) per access unit."""

    def __init__(self, config, on_frame):
        self.config = config
        self.on_frame = on_frame
        self.proc = None
        self.thread = None

    def start(self):
        c = self.config
        pattern = next(cam["pattern"] for cam in CAMERAS if cam["id"] == c["camera"])
        size = f'{c["width"]}x{c["height"]}'
        src = f"{pattern}=size={size}:rate={c['fps']}"
        if c["mirror"]:
            src += ",hflip"
        common = [
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-re",
            "-f", "lavfi", "-i", src,
            "-pix_fmt", "yuv420p", "-g", str(c["fps"]), "-bf", "0",
            "-b:v", str(c["bitrate"]), "-maxrate", str(c["bitrate"]),
            "-bufsize", str(c["bitrate"] // 2),
        ]
        if c["codec"] == "hevc":
            codec = ["-c:v", "libx265", "-preset", "ultrafast", "-tune", "zerolatency",
                     "-x265-params", "repeat-headers=1:aud=1:log-level=error", "-f", "hevc", "-"]
        else:
            codec = ["-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency",
                     "-x264-params", "repeat-headers=1:aud=1", "-f", "h264", "-"]
        self.proc = subprocess.Popen(common + codec, stdout=subprocess.PIPE)
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()

    def stop(self):
        if self.proc:
            self.proc.kill()
            self.proc.wait()
            self.proc = None

    def _nal_type(self, nal):
        if self.config["codec"] == "hevc":
            return (nal[0] >> 1) & 0x3F
        return nal[0] & 0x1F

    def _is_aud(self, nal_type):
        return nal_type == (35 if self.config["codec"] == "hevc" else 9)

    def _is_key(self, nal_type):
        if self.config["codec"] == "hevc":
            return 16 <= nal_type <= 21
        return nal_type == 5

    def _read(self):
        proc = self.proc
        buf = bytearray()
        au = bytearray()
        key = False
        while True:
            chunk = proc.stdout.read1(65536)
            if not chunk:
                break
            buf += chunk
            # Split on 3- or 4-byte start codes (encoders mix both); keep the last,
            # possibly partial, NAL in buf. Output always uses 4-byte start codes.
            while True:
                start = buf.find(b"\x00\x00\x01")
                if start < 0:
                    break
                nxt = buf.find(b"\x00\x00\x01", start + 3)
                if nxt < 0:
                    break
                # A 4-byte start code's leading zero lands at the end of the previous NAL.
                nal = bytes(buf[start + 3:nxt]).rstrip(b"\x00")
                del buf[:nxt]
                if not nal:
                    continue
                nal_type = self._nal_type(nal)
                if self._is_aud(nal_type) and au:
                    self.on_frame(key, bytes(au))
                    au.clear()
                    key = False
                if self._is_key(nal_type):
                    key = True
                au += b"\x00\x00\x00\x01" + nal


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def unb64(text):
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def fingerprint(cert_der):
    return b64(hashlib.sha256(cert_der).digest())


class Identity:
    """The fake phone's certificate and the computers it's paired with."""

    def __init__(self, name):
        slug = "".join(c if c.isalnum() else "-" for c in name.lower())
        self.dir = os.path.join(os.path.expanduser("~/.cache/pocketwebcam-fake-phone"), slug)
        os.makedirs(self.dir, exist_ok=True)
        self.pem = os.path.join(self.dir, "phone.pem")
        if not os.path.exists(self.pem):
            self._generate()
        cert = open(self.pem).read()
        cert = cert[: cert.index("-----END CERTIFICATE-----") + 25]
        self.fingerprint = fingerprint(ssl.PEM_cert_to_DER_cert(cert))
        self.paired_path = os.path.join(self.dir, "paired.json")
        try:
            self.paired = json.load(open(self.paired_path))
        except (OSError, ValueError):
            self.paired = {}

    def _generate(self):
        import datetime
        from cryptography import x509
        from cryptography.hazmat.primitives import hashes, serialization
        from cryptography.hazmat.primitives.asymmetric import ec
        from cryptography.x509.oid import NameOID

        key = ec.generate_private_key(ec.SECP256R1())
        subject = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Pocket Webcam")])
        cert = (x509.CertificateBuilder().subject_name(subject).issuer_name(subject)
                .public_key(key.public_key()).serial_number(x509.random_serial_number())
                .not_valid_before(datetime.datetime(2026, 1, 1))
                .not_valid_after(datetime.datetime(4000, 1, 1)).sign(key, hashes.SHA256()))
        with open(self.pem, "wb") as f:
            os.fchmod(f.fileno(), 0o600)
            f.write(cert.public_bytes(serialization.Encoding.PEM))
            f.write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                      serialization.NoEncryption()))

    def pair(self, desktop_fp, name):
        self.paired[desktop_fp] = name
        json.dump(self.paired, open(self.paired_path, "w"), indent=2)

    def forget_all(self):
        self.paired = {}
        json.dump(self.paired, open(self.paired_path, "w"))


class TlsConn:
    """One TLS connection that a reader and writers can share (like the receiver's Link).

    The socket is read and written outside the lock; the TLS state is only touched under it.
    """

    def __init__(self, raw, context):
        self.raw = raw
        self.incoming, self.outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
        self.tls = context.wrap_bio(self.incoming, self.outgoing, server_side=True)
        self.lock = threading.Lock()
        self.plain = bytearray()
        while True:
            try:
                self.tls.do_handshake()
                break
            except ssl.SSLWantReadError:
                self._flush()
                data = self.raw.recv(65536)
                if not data:
                    raise ConnectionError("receiver left during the handshake")
                self.incoming.write(data)
        self._flush()
        self.peer = fingerprint(self.tls.getpeercert(binary_form=True))

    def _flush(self):
        out = self.outgoing.read()
        if out:
            self.raw.sendall(out)

    def sendall(self, data):
        with self.lock:
            self.tls.write(data)
            self._flush()

    def recv(self, n):
        while not self.plain:
            data = self.raw.recv(65536)
            with self.lock:
                if not data:
                    return b""
                self.incoming.write(data)
                while True:
                    try:
                        self.plain += self.tls.read(65536)
                    except ssl.SSLWantReadError:
                        break
                    except ssl.SSLZeroReturnError:
                        return b""
                self._flush()
        chunk = bytes(self.plain[:n])
        del self.plain[:n]
        return chunk

    def setsockopt(self, *a):
        self.raw.setsockopt(*a)

    def close(self):
        try:
            self.raw.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.raw.close()


def pairing_code(desktop_fp, phone_fp):
    d = hashlib.sha256(b"pocketwebcam pairing code" + unb64(desktop_fp) + unb64(phone_fp)).digest()
    return f"{int.from_bytes(d[:4], 'big') % 1_000_000:06d}"


def parse_scan(url):
    """The pairing link from a desktop's QR code."""
    if not url.startswith("pocketwebcam://pair?"):
        raise SystemExit("--scan wants the pocketwebcam://pair?... link from the QR code")
    q = dict(urllib.parse.parse_qsl(url.split("?", 1)[1]))
    if q.get("v") != "2":
        raise SystemExit("that pairing link is for another protocol version")
    return {"desktop": q["fp"], "secret": unb64(q["s"]), "name": q.get("n", "Computer"),
            "until": time.time() + 300}


class Session:
    def __init__(self, sock, phone_name, has_audio=True, phone_id=None, identity=None, scanned=None,
                 decline=False, on_paired=None):
        self.sock = sock
        self.phone_name = phone_name
        self.phone_id = phone_id
        self.identity = identity
        self.scanned = scanned
        self.decline = decline
        self.on_paired = on_paired
        self.paused = False
        self.has_audio = has_audio
        self.audio_on = False
        self.audio_thread = None
        self.write_lock = threading.Lock()
        self.alive = True
        self.encoder = None
        self.frames = queue.Queue(maxsize=1)
        self.stream_lock = threading.Lock()
        self.generation = 0
        self.waiting_for_key = False
        self.dropped = 0
        self.sent = 0
        self.config = {
            "camera": "fake:wide", "width": 1280, "height": 720, "fps": 30,
            "codec": "h264", "bitrate": 6_000_000, "zoom": 1.0,
            "torch": False, "stabilization": False, "mirror": False,
        }

    # --- I/O helpers -------------------------------------------------------

    def send(self, msg_type, payload=b""):
        if isinstance(payload, (dict, list)):
            payload = json.dumps(payload).encode()
        with self.write_lock:
            self.sock.sendall(struct.pack(">BI", msg_type, len(payload)) + payload)

    def recv_exact(self, n):
        data = bytearray()
        while len(data) < n:
            chunk = self.sock.recv(n - len(data))
            if not chunk:
                raise ConnectionError("receiver disconnected")
            data += chunk
        return bytes(data)

    def recv(self):
        msg_type, length = struct.unpack(">BI", self.recv_exact(5))
        if length > MAX_PAYLOAD:
            raise ConnectionError(f"payload too large: {length}")
        return msg_type, self.recv_exact(length)

    # --- video -------------------------------------------------------------

    def on_frame(self, gen, key, data):
        """Called from the encoder thread. Never blocks: drops instead (see PROTOCOL.md)."""
        if gen != self.generation:
            return
        if self.waiting_for_key and not key:
            self.dropped += 1
            return
        self.waiting_for_key = False
        try:
            self.frames.put_nowait((gen, key, data))
        except queue.Full:
            self.dropped += 1
            self.waiting_for_key = True

    def video_sender(self):
        while self.alive:
            try:
                gen, key, data = self.frames.get(timeout=0.5)
            except queue.Empty:
                continue
            payload = struct.pack(">BQ", 1 if key else 0, now_us()) + data
            with self.stream_lock:
                if gen != self.generation:
                    continue
                try:
                    self.send(VIDEO_FRAME, payload)
                    self.sent += 1
                except OSError:
                    self.alive = False

    def restart_encoder(self):
        if self.encoder:
            self.encoder.stop()
        c = self.config
        # Bumping the generation makes the sender discard frames from the old stream,
        # so nothing from it can follow the new STREAM_FORMAT.
        with self.stream_lock:
            self.generation += 1
            gen = self.generation
            self.waiting_for_key = True
            self.send(STREAM_FORMAT, {"codec": c["codec"], "width": c["width"],
                                      "height": c["height"], "fps": c["fps"], "rotation": 0})
        self.encoder = Encoder(dict(c), lambda key, data: self.on_frame(gen, key, data))
        self.encoder.start()
        print(f"  streaming {c['camera']} {c['width']}x{c['height']}@{c['fps']} "
              f"{c['codec']} {c['bitrate'] / 1e6:.1f} Mbps")

    def state(self):
        state = dict(self.config, thermal="nominal", battery=1.0, charging=True)
        if self.has_audio:
            state.update(audio=self.audio_on, micPermission="granted")
        if self.paused:
            state["cameraPaused"] = "inUse"
        return state

    def toggle_pause(self):
        """The camera is taken away (no video, STATE says why) or given back."""
        self.paused = not self.paused
        print(f"  camera {'paused' if self.paused else 'running again'}")
        if self.paused:
            if self.encoder:
                self.encoder.stop()
            self.send(STATE, self.state())
        else:
            self.send(STATE, self.state())
            self.restart_encoder()

    # --- audio -------------------------------------------------------------

    def audio_sender(self):
        """10 ms chunks paced in real time, like the phone's microphone."""
        self.send(AUDIO_FORMAT, {"codec": "pcm_s16le", "sampleRate": AUDIO_RATE, "channels": 1})
        start = time.monotonic()
        n = 0
        while self.alive and self.audio_on:
            samples = []
            for i in range(CHUNK):
                t = (n + i) / AUDIO_RATE
                on = (t % 0.5) < 0.25
                samples.append(int(on * 8000 * math.sin(2 * math.pi * 440 * t)))
            try:
                self.send(AUDIO_FRAME, struct.pack(">Q", now_us()) + struct.pack(f"<{CHUNK}h", *samples))
            except OSError:
                self.alive = False
            n += CHUNK
            time.sleep(max(0.0, start + n / AUDIO_RATE - time.monotonic()))

    def set_audio(self, on):
        if not self.has_audio or on == self.audio_on:
            return
        self.audio_on = on
        print(f"  microphone {'on' if on else 'off'}")
        if on:
            self.audio_thread = threading.Thread(target=self.audio_sender, daemon=True)
            self.audio_thread.start()

    # --- config ------------------------------------------------------------

    def apply_config(self, update):
        new = dict(self.config)
        for key in ("camera", "width", "height", "fps", "codec", "bitrate",
                    "zoom", "torch", "stabilization", "mirror"):
            if key in update:
                new[key] = update[key]
        # Clamp to what we actually support.
        if new["camera"] not in {c["id"] for c in CAMERAS}:
            new["camera"] = self.config["camera"]
        if new["codec"] not in ("h264", "hevc"):
            new["codec"] = self.config["codec"]
        fmt = min(FORMATS, key=lambda f: abs(f["width"] - new["width"]) + abs(f["height"] - new["height"]))
        new["width"], new["height"] = fmt["width"], fmt["height"]
        new["fps"] = min(fmt["fps"], key=lambda f: abs(f - new["fps"]))
        new["bitrate"] = max(1_000_000, min(50_000_000, int(new["bitrate"])))
        if "audio" in update:
            self.set_audio(bool(update["audio"]))

        stream_keys = ("camera", "width", "height", "fps", "codec", "bitrate", "mirror")
        restart = any(new[k] != self.config[k] for k in stream_keys)
        self.config = new
        self.send(STATE, self.state())
        if restart and not self.paused:
            self.restart_encoder()

    # --- main loop ---------------------------------------------------------

    def pair(self, request):
        """Answers a PAIR_REQUEST like the phone apps do. True when paired."""
        desktop = self.sock.peer
        print(f"  {request.get('name')} asks to pair ({request.get('mode')})")
        if request.get("mode") == "qr":
            scan = self.scanned
            if not scan or scan["until"] < time.time():
                self.send(PAIR_RESULT, {"accepted": False, "reason": "no_code"})
                return False
            if scan["desktop"] != desktop:
                # Someone in the middle, or another computer: not the one in the QR code.
                self.send(PAIR_RESULT, {"accepted": False, "reason": "wrong_computer"})
                return False
            mac = hmac.new(scan["secret"], b"pocketwebcam pairing proof" + unb64(self.identity.fingerprint)
                           + unb64(desktop), hashlib.sha256).digest()
            self.send(PAIR_PROOF, {"proof": b64(mac)})
        else:
            code = pairing_code(desktop, self.identity.fingerprint)
            print(f"  pairing code: {code[:3]} {code[3:]}  -> {'declining' if self.decline else 'accepting'}")
            if self.decline:
                self.send(PAIR_RESULT, {"accepted": False, "reason": "declined"})
                return False
            self.send(PAIR_RESULT, {"accepted": True})
        msg_type, payload = self.recv()
        answer = json.loads(payload) if msg_type == PAIR_RESULT else {}
        if not answer.get("accepted"):
            print(f"  the computer didn't accept: {answer.get('reason')}")
            return False
        self.identity.pair(desktop, request.get("name", "Computer"))
        print(f"  paired with {request.get('name')}")
        if self.on_paired:
            self.on_paired()
        return True

    def run(self):
        msg_type, payload = self.recv()
        if msg_type == PAIR_REQUEST:
            if not self.pair(json.loads(payload)):
                return
            msg_type, payload = self.recv()
        elif self.sock.peer not in self.identity.paired:
            print("  a computer that isn't paired: refused")
            self.send(ERROR, {"code": "not_paired", "message": "This phone isn't paired with that computer."})
            return
        if msg_type != HELLO:
            self.send(ERROR, {"code": "internal", "message": "expected HELLO"})
            return
        hello = json.loads(payload)
        print(f"  receiver says hello: {hello}")
        if hello.get("protocol") != PROTOCOL_VERSION:
            self.send(ERROR, {"code": "version", "message": "unsupported protocol version"})
            return

        hello = {"protocol": PROTOCOL_VERSION, "name": self.phone_name,
                 "app": "Pocket Webcam", "appVersion": "fake", "platform": "fake"}
        if self.phone_id:
            hello["id"] = self.phone_id
        self.send(HELLO, hello)
        self.send(CAPABILITIES, capabilities(self.has_audio))
        self.send(STATE, self.state())
        threading.Thread(target=self.video_sender, daemon=True).start()
        threading.Thread(target=self.stats, daemon=True).start()
        self.restart_encoder()

        while self.alive:
            msg_type, payload = self.recv()
            if msg_type == SET_CONFIG:
                update = json.loads(payload)
                print(f"  SET_CONFIG {update}")
                self.apply_config(update)
            elif msg_type == KEYFRAME_REQ:
                # ffmpeg can't be poked mid-stream; the 1 s GOP covers it.
                pass
            elif msg_type == PING:
                self.send(PONG, payload)
            elif msg_type == ERROR:
                print(f"  receiver error: {payload.decode(errors='replace')}")

    def stats(self):
        while self.alive:
            time.sleep(5)
            print(f"  sent {self.sent} frames, dropped {self.dropped}")

    def close(self):
        self.alive = False
        if self.encoder:
            self.encoder.stop()
        self.sock.close()


def advertise(name, port, phone_id, phone_fp, pair_tag=None):
    if not shutil.which("avahi-publish"):
        print("avahi-publish not found; mDNS disabled (use --host on the receiver)")
        return None
    txt = [f"v={PROTOCOL_VERSION}", "model=fake", f"fp={phone_fp}"] + ([f"id={phone_id}"] if phone_id else [])
    if pair_tag:
        txt.append(f"pair={pair_tag}")
    return subprocess.Popen(
        ["avahi-publish", "-s", name, "_pocketwebcam._tcp", str(port)] + txt,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=7310)
    ap.add_argument("--name", default="Fake Phone")
    ap.add_argument("--no-mdns", action="store_true")
    ap.add_argument("--no-audio", action="store_true", help="act like a phone without audio support")
    ap.add_argument("--id", help="the phone's identifier (default: made from the name)")
    ap.add_argument("--no-id", action="store_true", help="act like a phone app from before identifiers")
    ap.add_argument("--scan", metavar="URL", help="pair like a phone that scanned this QR code link")
    ap.add_argument("--decline", action="store_true", help="decline pairing by code")
    ap.add_argument("--forget", action="store_true", help="forget all paired computers first")
    config_home = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    ap.add_argument("--desktop-cert", default=os.path.join(config_home, "pocketwebcam", "identity.pem"),
                    help="certificate of the computer to accept (default: this computer's)")
    args = ap.parse_args()
    identity = Identity(args.name)
    if args.forget:
        identity.forget_all()
    scanned = [parse_scan(args.scan)] if args.scan else [None]
    try:
        desktop_pem = open(args.desktop_cert).read()
    except OSError:
        raise SystemExit(f"no desktop certificate at {args.desktop_cert}; start the receiver once first")
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_3
    context.load_cert_chain(identity.pem)
    context.verify_mode = ssl.CERT_REQUIRED
    context.load_verify_locations(cadata=desktop_pem[: desktop_pem.index("-----END CERTIFICATE-----") + 25])
    print(f"phone fingerprint {identity.fingerprint}; paired with {list(identity.paired.values()) or 'nobody'}")
    # Stable per name, like the real app's identifier is stable per install.
    phone_id = args.id or "fake-" + "".join(c if c.isalnum() else "-" for c in args.name.lower())
    if args.no_id:
        phone_id = None

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", args.port))
    server.listen(2)
    pair_tag = hashlib.sha256(scanned[0]["secret"]).digest()[:8].hex() if scanned[0] else None
    mdns = [None if args.no_mdns else advertise(args.name, args.port, phone_id, identity.fingerprint, pair_tag)]

    def paired_by_scan():
        # The scanned code is used up: announce without it again.
        scanned[0] = None
        if mdns[0]:
            mdns[0].terminate()
            mdns[0] = advertise(args.name, args.port, phone_id, identity.fingerprint)
    # Treat `kill` like Ctrl+C so the avahi-publish child is always cleaned up.
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt))
    print(f"Fake phone '{args.name}' listening on :{args.port}")

    busy = threading.Lock()
    current = []  # the connected session, if any

    def toggle_pause(*_):
        for session in current:
            try:
                session.toggle_pause()
            except OSError:
                pass

    signal.signal(signal.SIGUSR1, toggle_pause)

    def handle(conn, addr):
        try:
            conn = TlsConn(conn, context)
        except (ssl.SSLError, OSError, ConnectionError) as e:
            print(f"refused a connection from {addr[0]}: {e}")
            conn.close()
            return
        if not busy.acquire(blocking=False):
            s = Session(conn, args.name)
            s.send(ERROR, {"code": "busy", "message": "Another computer is already connected."})
            conn.close()
            return
        print(f"receiver connected from {addr[0]}")
        conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        session = Session(conn, args.name, has_audio=not args.no_audio, phone_id=phone_id, identity=identity,
                          scanned=scanned[0], decline=args.decline, on_paired=paired_by_scan if scanned[0] else None)
        current.append(session)
        try:
            session.run()
        except (ConnectionError, OSError) as e:
            print(f"  connection ended: {e}")
        finally:
            current.remove(session)
            session.close()
            busy.release()
            print("receiver disconnected")

    try:
        while True:
            conn, addr = server.accept()
            threading.Thread(target=handle, args=(conn, addr), daemon=True).start()
    except KeyboardInterrupt:
        pass
    finally:
        if mdns[0]:
            mdns[0].terminate()


if __name__ == "__main__":
    main()
