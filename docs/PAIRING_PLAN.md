# Pairing + encryption (protocol v2): plan

Status: **implemented 2026-11-18** on all four sides (receiver, desktop app, iPhone, Android); not released yet.

Tested: receiver unit tests (both pairing paths, wrong proof, declines, cancels); terminal receiver against
`tools/fake_phone.py` (code pairing, QR pairing over real mDNS, reconnect, a phone that forgot the computer);
the terminal QR code decoded with zbarimg; the iOS certificate layout parsed by OpenSSL and rustls; iOS unit
tests on a simulator in CI (shared vectors, certificate, Keychain identity); Android unit tests and the
Android emulator (code pairing over TLS with the KeyStore key, reconnect, an unknown computer refused).

Not tested yet: the iPhone app on a real iPhone (scanner, Camera-app link, code pairing); the desktop app's
pairing dialogs by eye; Windows (CI build only); Android QR pairing by camera app (needs a real phone).

Known limits: Android has no in-app scanner (the camera app or Google Lens opens the pairing link); if the
phone cancels after tapping Pair, the computer only notices when its own user answers. Fixes review flaw 1: today any device on the
Wi-Fi can connect to a phone and watch the camera, and nothing is encrypted.

## Goals

- Every connection is encrypted (TLS 1.3).
- A phone only streams to computers it has paired with; a computer only connects to phones it has
  paired with. Strangers on the same Wi-Fi get nothing.
- Pairing once takes about a second: **scan a QR code** on the computer screen with the phone.
  Without a usable camera view, **compare a 6-digit code** on both screens instead.
- No accounts, no server, nothing leaves the local network (privacy policy stays true).

## How it works

Each side makes a key pair and a self-signed certificate once, on first start:

| Side | Key | Stored in |
|---|---|---|
| Desktop | P-256, `rcgen` | `~/.config/pocketwebcam/identity.pem` (0600), `%APPDATA%` on Windows |
| iPhone | P-256, `SecKey`, cert built with Apple's `swift-certificates` | Keychain (identity) |
| Android | P-256 in AndroidKeyStore (it writes the self-signed cert itself) | AndroidKeyStore |

A peer is identified by the **SHA-256 fingerprint of its certificate**. Pairing = both sides storing
the other's fingerprint. After that, every connection is mutual TLS and each side checks that the
other's certificate is one it has paired with ("pinning", like KDE Connect).

The phone stays the TLS server and the desktop the client, as in v1.

### Pairing with the QR code

1. Desktop: **Pair a phone**. The app makes a one-time secret (16 random bytes) and shows a QR code:
   `pocketwebcam://pair?v=2&fp=<desktop fingerprint>&s=<secret>&n=<computer name>`.
2. Phone: **Pair with a computer** opens a scanner (iOS: `AVCaptureMetadataOutput` on the camera
   session the app already runs; Android: ZXing core, no Google Play services).
3. After the scan the phone adds `pair=<first 8 bytes of SHA-256(secret)>` to its mDNS TXT record, so
   the desktop knows which phone scanned its code, and connects to it.
4. TLS handshake. Both sides accept an unknown certificate **only for this pairing connection**.
5. Phone checks that the desktop's client certificate has the fingerprint from the QR code.
   A man in the middle can't pass this: it doesn't have the desktop's private key.
6. Phone sends `PAIR_PROOF` = HMAC-SHA256(secret, phone fp ‖ desktop fp). Desktop checks it with its
   secret and the phone certificate it just saw. Both store the other's fingerprint. Done; streaming
   starts on the same connection.

The secret is valid for 5 minutes and for one phone.

### Pairing with a code (fallback)

Desktop picks an unpaired phone in the phone list and clicks **Pair**. After the TLS handshake both
screens show the same 6-digit code, derived from SHA-256(desktop fp ‖ phone fp) (the method
Bluetooth calls numeric comparison). The user taps **Pair** on the phone and **Confirm** on the
computer. If an attacker sat in between, the two codes would differ.

### After pairing

- Phones that aren't paired show up in the desktop list as "Not paired · Pair", and never auto-connect.
- A phone closes any connection whose certificate it doesn't know, right after the handshake,
  before sending a single frame.
- Several computers per phone and several phones per computer are fine.
- Forget: phone Settings → Paired computers; desktop phone menu → Forget this phone.

## Protocol changes (v1 → v2)

- TXT `v=2`, plus `pair=` while pairing. The TCP port stays 7310.
- The whole v1 message stream runs inside TLS, unchanged.
- New messages: `PAIR_REQUEST 0x30` (desktop → phone: name, id, mode `qr`/`code`),
  `PAIR_PROOF 0x31` (phone → desktop), `PAIR_RESULT 0x32` (both: accepted/declined).
- **No v1 support**: v1 phones and receivers can't talk to v2 ones. This is fine before launch.
  Everyone on TestFlight gets the update, and the desktop names the problem ("Update the phone app").

## Steps

1. PROTOCOL.md v2 + fixtures (pairing messages, QR URL, fingerprint and code test vectors that all
   three implementations must match).
2. Receiver: identity, rustls client with pinning, pairing state machine, `pairs` in settings,
   tests against a Rust test phone; `fake_phone.py` speaks TLS + pairing (Python `ssl`).
3. Desktop app: "Pair a phone" dialog (QR + code), "Not paired" phones in the picker, Forget.
   Terminal receiver: `pocketwebcam pair` prints the QR code in the terminal.
4. iOS: Keychain identity, `NWListener` with TLS + verify block, scanner screen, code screen,
   Paired computers list.
5. Android: KeyStore identity, `SSLServerSocket` with client auth + pinning TrustManager, ZXing
   scanner, same screens.
6. Docs, privacy policy (the certificate fingerprints), website/marketing copy (series-2 says
   "No pairing codes" → "Scan once and you're paired").

## What can be checked where

- Receiver + fake phone: fully tested here, including a man-in-the-middle test (wrong fingerprint
  must fail).
- Android: emulator, code pairing (QR scanning on the emulator is awkward; the virtual scene can show an image).
- iOS: builds in CI only; the real test is on your iPhone (pair by QR, pair by code, forget, a stranger
  computer gets refused).

## Decisions (2026-11-17)

- v1 is dropped completely: phones and receivers only speak v2.
- Paired computers and phones stay trusted until the user forgets them (no expiry).
