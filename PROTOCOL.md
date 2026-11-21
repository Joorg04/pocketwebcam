# Pocket Webcam Protocol v2

How the phone app and the desktop receiver talk to each other over the local network.

v2 (November 2026) puts every connection inside TLS 1.3 and only lets paired devices talk. v1 (plain TCP, anyone on the network could connect) is gone: v1 and v2 apps can't connect to each other.

## Roles

- **Phone** = server. It listens on a TCP port and advertises itself over Bonjour/mDNS.
- **Desktop receiver** = client. It discovers phones and connects to one.

One phone serves one receiver at a time. A second connection is rejected with an `ERROR` message.

## Discovery

- Service type: `_pocketwebcam._tcp`
- Instance name: the phone's user-visible name (e.g. `Nic's iPhone`)
- Default port: `7310` (the phone may pick another; always read it from the mDNS record)
- TXT record keys:
  - `v` — protocol version (`2`). A receiver lists phones with another version but doesn't connect to them ("update the app").
  - `fp` — the phone's certificate fingerprint (see [Encryption](#encryption)), so the receiver can tell paired phones from others before connecting. Only a hint: the TLS handshake checks the real certificate.
  - `pair` (only while pairing) — after scanning a computer's QR code, the first 8 bytes of SHA-256 of the code's secret, as 16 lowercase hex digits. Tells that computer which phone to connect to.
  - `model` — device model (e.g. `iPhone16,1`, `Pixel 8`)
  - `id` (optional) — a random identifier the phone app makes up once. It stays the same when the phone is renamed, so receivers remember a phone by it. Phone apps from before the identifier leave it out, and receivers then go by the name.

The phone only serves the local network (or a USB cable): it refuses connections that arrive over mobile data.

Sample messages for every type are in [`protocol-fixtures/`](protocol-fixtures/); the receiver's and the Android app's tests read them.

If mDNS is blocked on a network, the receiver can connect directly with `--host <ip>[:port]`.

## Encryption

Every connection is TLS 1.3 (no older versions) over the TCP port above. The phone is the TLS server and asks for a client certificate; the desktop must send one.

Each app makes an ECDSA P-256 key and a self-signed certificate once, when it first needs one, and keeps it (desktop: `identity.pem` in its config folder, readable only by the user; iPhone: the Keychain; Android: AndroidKeyStore). Certificate names and dates mean nothing here.

A device is known by its **fingerprint**: SHA-256 of its certificate (DER), written as base64url without padding (43 characters).

During the handshake both sides accept any certificate, but TLS still checks that the other side holds that certificate's private key. Right after the handshake, each side looks up the other's fingerprint in its list of paired devices:

- Desktop: the phone isn't paired, and the user didn't ask to pair it: close the connection.
- Phone: the computer isn't paired and its first message isn't `PAIR_REQUEST`: send `ERROR` `not_paired` and close. Nothing else is sent to it, not even `HELLO`.

## Pairing

Pairing stores the other side's fingerprint, on both sides. It lasts until the user forgets the device. A phone can be paired with several computers, and a computer with several phones.

All values below have test vectors in [`protocol-fixtures/pairing.json`](protocol-fixtures/pairing.json).

### With the QR code (the usual way)

1. The desktop makes a **secret** of 16 random bytes and shows a QR code with this link:
   `pocketwebcam://pair?v=2&fp=<desktop fingerprint>&s=<secret, base64url>&n=<computer name, %-encoded UTF-8>`
   The secret is good for 5 minutes and one pairing.
2. The phone scans it and adds `pair=<tag>` to its TXT record (tag: first 8 bytes of SHA-256(secret), hex).
3. The desktop sees the tag, connects, and sends `PAIR_REQUEST` `{"mode": "qr"}`.
4. The phone checks that the desktop's certificate has the fingerprint from the QR code. If not (another computer, or a man in the middle, who can't have the desktop's key), it sends `PAIR_RESULT` `{"accepted": false, "reason": "wrong_computer"}`. Without a scanned code it answers `"reason": "no_code"`.
5. The phone sends `PAIR_PROOF` with HMAC-SHA256(key = secret, message = `pocketwebcam pairing proof` ‖ phone fingerprint ‖ desktop fingerprint), with the fingerprints as raw 32-byte hashes.
6. The desktop checks the proof against its secret and the phone certificate it sees. Then it sends `PAIR_RESULT` `{"accepted": true}` (or `false` with `"reason": "wrong_proof"`). Both save the pairing, the phone drops the `pair` TXT key, and the session goes on with `HELLO`.

### With a 6-digit code (no camera, or connecting by address)

1. The desktop connects to a phone that isn't paired and sends `PAIR_REQUEST` `{"mode": "code"}`.
2. Both show the code: SHA-256(`pocketwebcam pairing code` ‖ desktop fingerprint ‖ phone fingerprint), first 4 bytes as a big-endian number, modulo 1 000 000, as 6 digits with leading zeros. A man in the middle would have a different certificate on each side, so the two codes would differ.
3. The phone's user taps Pair: the phone sends `PAIR_RESULT` `{"accepted": true}` (or `false`, `"reason": "declined"`).
4. The desktop's user confirms that the codes match: the desktop sends `PAIR_RESULT` `{"accepted": true}`. Both save the pairing, and the session goes on with `HELLO`.

Either side waits up to 90 seconds for its user; after that it sends `PAIR_RESULT` `{"accepted": false, "reason": "timeout"}` and closes.

## Framing

Every message on the TCP stream has the same 5-byte header:

```
+--------+----------------------+-------------------+
| type   | length               | payload           |
| 1 byte | 4 bytes, big-endian  | `length` bytes    |
+--------+----------------------+-------------------+
```

- Maximum payload: 16 MiB. A receiver must close the connection if it sees a larger length.
- Everything a receiver sends is small. A phone closes the connection if it is sent a payload larger than 1 MiB.
- JSON payloads are UTF-8, no trailing newline.

## Message types

| Type   | Name           | Direction        | Payload |
|--------|----------------|------------------|---------|
| `0x01` | `HELLO`        | both             | JSON    |
| `0x02` | `CAPABILITIES` | phone → desktop  | JSON    |
| `0x03` | `SET_CONFIG`   | desktop → phone  | JSON    |
| `0x04` | `STATE`        | phone → desktop  | JSON    |
| `0x10` | `STREAM_FORMAT`| phone → desktop  | JSON    |
| `0x11` | `VIDEO_FRAME`  | phone → desktop  | binary  |
| `0x12` | `KEYFRAME_REQ` | desktop → phone  | empty   |
| `0x13` | `AUDIO_FORMAT` | phone → desktop  | JSON    |
| `0x14` | `AUDIO_FRAME`  | phone → desktop  | binary  |
| `0x30` | `PAIR_REQUEST` | desktop → phone  | JSON    |
| `0x31` | `PAIR_PROOF`   | phone → desktop  | JSON    |
| `0x32` | `PAIR_RESULT`  | both             | JSON    |
| `0x20` | `PING`         | both             | JSON    |
| `0x21` | `PONG`         | both             | JSON    |
| `0x7F` | `ERROR`        | both             | JSON    |

Unknown message types must be ignored (skip `length` bytes). This keeps receivers working with newer phones of the same protocol version.

## Session flow

```
desktop                                   phone
   | ---- TCP connect -------------------> |
   | ==== TLS 1.3 handshake (both certs) == |
   |      (both check the other is paired;  |
   |       if not: Pairing, see above)      |
   | ---- HELLO --------------------------> |
   | <--- HELLO --------------------------- |
   | <--- CAPABILITIES -------------------- |
   | <--- STATE --------------------------- |
   | <--- STREAM_FORMAT ------------------- |
   | <--- VIDEO_FRAME (keyframe first) ---- |
   | <--- VIDEO_FRAME ... ----------------- |
   | ---- SET_CONFIG ---------------------> |   (any time)
   | <--- STATE --------------------------- |
   | <--- STREAM_FORMAT ------------------- |   (if codec/size/fps changed)
   | <--- VIDEO_FRAME (keyframe) ---------- |
   | ---- SET_CONFIG {"audio": true} -----> |   (mic on)
   | <--- STATE --------------------------- |
   | <--- AUDIO_FORMAT -------------------- |
   | <--- AUDIO_FRAME ... ----------------- |   (every 10 ms, between video frames)
```

### HELLO

```json
{ "protocol": 2, "name": "Nic's iPhone", "app": "Pocket Webcam", "appVersion": "0.1.0", "platform": "ios" }   // or "android"
```

The desktop sends `"platform": "linux"` (or `windows` / `macos`). If the protocol major versions differ, the side that notices sends `ERROR` and closes.

The phone's `HELLO` also carries `"id"`, the same identifier as in its mDNS record (optional, see Discovery).

Until the phone has a valid `HELLO` it answers `PING` and ignores everything else (`SET_CONFIG`, `KEYFRAME_REQ`), and it closes a connection that hasn't sent one within 10 s.

### CAPABILITIES

Everything the phone can do. Sent once after `HELLO`, and again if it changes (e.g. a lens becomes unavailable).

```json
{
  "cameras": [
    {
      "id": "com.apple.avfoundation.avcapturedevice.built-in_video:0",
      "name": "Wide",
      "position": "back",
      "lens": "wide",
      "zoom": { "min": 1.0, "max": 10.0 },
      "formats": [
        { "width": 3840, "height": 2160, "fps": [24, 25, 30, 60] },
        { "width": 1920, "height": 1080, "fps": [24, 25, 30, 60, 120, 240] },
        { "width": 1280, "height": 720,  "fps": [24, 25, 30, 60, 120, 240] }
      ],
      "features": ["torch", "stabilization", "focusLock", "exposureLock"]
    }
  ],
  "codecs": ["h264", "hevc"],
  "bitrate": { "min": 1000000, "max": 50000000 },
  "audio": { "codecs": ["pcm_s16le"], "sampleRates": [48000], "channels": [1] }
}
```

- `position`: `back` | `front` | `external`
- `lens`: `ultraWide` | `wide` | `telephoto` | `dual` | `dualWide` | `triple` | `trueDepth` | `external`
- `fps`: discrete frame rates the receiver may request for that resolution.
- `audio` (optional): the phone can send its microphone. Phones without it leave the key out, and receivers hide their mic controls.

### SET_CONFIG

Partial update. Only the keys present are changed.

```json
{
  "camera": "com.apple.avfoundation.avcapturedevice.built-in_video:0",
  "width": 1920, "height": 1080, "fps": 30,
  "codec": "h264",
  "bitrate": 12000000,
  "zoom": 1.0,
  "torch": false,
  "stabilization": true,
  "mirror": false,
  "audio": true
}
```

- `audio`: turn the phone's microphone on or off. It isn't remembered between connections: the mic starts off on every new connection, and the receiver sends `"audio": true` if it wants sound. The person holding the phone can turn the mic off at any time; the phone then reports `"audio": false` in `STATE`.

The phone clamps invalid values to the nearest supported option and reports the result in `STATE`.

### STATE

The phone's full current configuration. Same keys as `SET_CONFIG`, all present, plus:

```json
{ "...": "...", "thermal": "nominal", "battery": 0.82, "charging": true, "micPermission": "granted" }
```

- `thermal`: `nominal` | `fair` | `serious` | `critical`
- A hot phone may lower `width`, `height` and `fps` on its own (the Android app steps down to at most 1080p 30 fps at `serious` and 720p 30 fps at `critical`, and goes back up after a minute of cooler readings). `STATE` always shows what is actually sent, followed by a new `STREAM_FORMAT`.
- `audio` is `false` unless sound is actually being sent (for example, it stays `false` while the microphone permission is denied).
- `micPermission` (optional): `granted` | `denied` | `undetermined`. Lets the receiver explain a denied permission instead of staying silent.
- `cameraPaused` (optional): why the phone's camera has stopped for now: `background` (the app isn't on screen) | `inUse` (another app has the camera) | `call` | `overheated` | `unknown`. Left out while the camera runs. No video arrives while it's set; the connection stays up.

### STREAM_FORMAT

Sent before the first frame and whenever the encoded stream changes shape. The receiver rebuilds its decoder when it gets one.

```json
{ "codec": "h264", "width": 1920, "height": 1080, "fps": 30, "rotation": 0, "mirror": false }
```

- `rotation`: clockwise degrees the receiver should rotate the image (`0`, `90`, `180`, `270`).
- `mirror` (optional, default `false`): after rotating, flip the image horizontally. Phones send their Mirror setting here instead of mirroring at capture time: capture-time mirroring flips the frame before the receiver rotates it, which turns into an upside-down image for any rotation of 90 or 270 degrees.

### VIDEO_FRAME

One complete encoded access unit (one picture).

```
+-------+------------------+------------------------------+
| flags | pts              | Annex-B data                 |
| 1 B   | 8 B, big-endian, | NAL units, each prefixed     |
|       | microseconds     | with 00 00 00 01             |
+-------+------------------+------------------------------+
```

- `flags` bit 0 = keyframe. Other bits reserved, must be 0.
- Keyframes must carry parameter sets in-band (SPS/PPS for H.264; VPS/SPS/PPS for HEVC) so the receiver can start decoding from any keyframe.
- `pts` is the capture time on the phone's monotonic clock. Only differences matter.

### AUDIO_FORMAT

Sent before the first `AUDIO_FRAME` and whenever the audio format changes.

```json
{ "codec": "pcm_s16le", "sampleRate": 48000, "channels": 1 }
```

Receivers only play `pcm_s16le`, 48000 Hz, 1 channel, and ignore audio in any other format.

### AUDIO_FRAME

A chunk of audio, normally 10 ms (480 samples, 960 bytes).

```
+------------------+----------------------------------+
| pts              | samples                          |
| 8 B, big-endian, | signed 16-bit little-endian,     |
| microseconds     | interleaved if more than 1 channel |
+------------------+----------------------------------+
```

- `pts` is on the same clock as `VIDEO_FRAME`, so the two can be lined up (for example, in recordings).
- The payload length minus 8 must be a multiple of 2 × `channels`.

### KEYFRAME_REQ

Asks the phone to encode the next frame as a keyframe. The receiver sends it after connecting late, after a decode error, or after it dropped frames.

### PING / PONG

```json
{ "t": 123456789 }
```

`t` is the sender's monotonic clock in microseconds. The other side echoes the same payload in `PONG`. The receiver pings every 2 s to measure round-trip time. A connection with no messages for 10 s is considered dead.

### ERROR

```json
{ "code": "busy", "message": "Another computer is already connected." }
```

Codes: `busy`, `version`, `not_paired` (the phone doesn't know this computer; it must be paired again), `camera`, `encoder`, `internal`.

## Latency and backpressure

TCP can stall on bad Wi-Fi. To keep latency low, the **phone** must never queue video:

- If the previous frame is still being written to the socket, drop the new non-keyframe.
- After dropping, keep dropping until the next keyframe, and request one from the encoder immediately.
- Control messages (`STATE`, `PONG`, …) are never dropped.

Audio follows a different rule, because a gap in sound is audible while a dropped video frame isn't:

- Send `AUDIO_FRAME`s even while a video frame is still being written. Don't wait for it.
- If more than 200 ms of audio is already waiting to be sent, drop new audio until the backlog clears. A short gap is better than a permanent delay.
- Never drop or reorder audio in any other case.
