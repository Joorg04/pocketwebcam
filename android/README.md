# Pocket Webcam for Android

Kotlin + Camera2 + MediaCodec + Jetpack Compose. Speaks the same protocol as the iPhone app
(`../PROTOCOL.md`), so the desktop receiver works unchanged.

| Path | What |
|---|---|
| `net/Protocol.kt`, `net/StreamServer.kt` | wire format, TCP server, mDNS (`NsdManager`), backpressure |
| `camera/CameraController.kt` | Camera2: virtual lenses by zoom ratio, formats, fps, torch, stabilization |
| `encode/VideoEncoder.kt` | H.264/HEVC surface encoder, parameter sets in-band on keyframes |
| `audio/Microphone.kt` | `AudioRecord` (CAMCORDER source) → 10 ms chunks of 48 kHz mono s16le on the camera's clock |
| `ThermalLimit.kt` | steps the stream down to 1080p30 / 720p30 while the phone is hot |
| `AppModel.kt` | wires camera → encoder → network, mic, thermal step-down, config, state |
| `ui/MainScreen.kt` | preview, status, controls |

## Build

Toolchain used on this machine: JDK 21 + Gradle 8.10.2 (mise) and the Android SDK in `~/Android/Sdk`
(`source ~/Android/env.sh` sets the variables).

```bash
cd android && ./gradlew :app:assembleDebug     # app/build/outputs/apk/debug/app-debug.apk
./gradlew test lint                            # unit tests (protocol, thermal step-down) and lint
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

## Test without a phone (emulator)

```bash
emulator -avd pw_test -no-window -no-audio -gpu swiftshader_indirect &   # virtual camera scene
adb forward tcp:7310 tcp:7310                                           # emulator is behind NAT: no mDNS
receiver/target/release/pocketwebcam --host 127.0.0.1:7310
```

The emulator only offers 720p and has no real lenses, so lens switching, 4K and thermal behaviour are untested.
`adb forward` is also how USB-cable mode will work (planned).

## Protocol additions

`STREAM_FORMAT.mirror` (optional): Android can't mirror at capture, so the receiver flips instead.

## Microphone

Same as the iPhone (see `../docs/PHASE4_MICROPHONE.md`): the mic starts off on every connection and
turns on when the computer sends `SET_CONFIG {"audio": true}` (the desktop does that by default).
Android asks for `RECORD_AUDIO` then, the first time, not at app start. A denial is reported as
`micPermission: "denied"`; the mic button then asks once more or opens the app's system settings.
The mic only records while the app is on screen (like the camera), and the phone's mic button wins
over the computer.

## Thermal step-down

At `THERMAL_STATUS_SEVERE` (`thermal: "serious"`) the stream drops to at most 1080p 30 fps, at
`CRITICAL` and above to 720p 30 fps; a quality preset follows the smaller size. The full stream
comes back after a minute below the threshold. The saved and requested config stays as chosen;
`STATE` reports what is actually sent. Test on a device with
`adb shell cmd thermalservice override-status 3` (and `reset` afterwards).
