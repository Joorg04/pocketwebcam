# Phase 4: iPhone microphone as a virtual mic

Status: **approved** (2026-10-21) with these decisions: the mic turns on automatically
when the phone connects, recordings include audio, and Android comes later.

Goal: the phone's microphone shows up on the computer as a normal microphone called
**"Pocket Webcam Microphone"**, so Zoom, Discord, Teams, OBS and browsers can pick it
like any other mic.

## Summary of the changes

| Part | Change | Size |
|------|--------|------|
| Protocol | 2 new message types + a few optional JSON keys. Stays **v1** (old apps ignore it). | small |
| iOS app | Capture mic audio, convert to 48 kHz mono, send it. Mic permission, mic toggle, mic indicator. | medium |
| Desktop | Receive audio, smooth out Wi-Fi jitter, play it into a virtual microphone. Mic button, level meter, settings. | medium |
| Android | Not in this phase. The protocol is ready for it; it's a later, separate change. (Done later on the `launch/android-parity` branch, see `android/README.md`; untested on a real device.) | — |

## 1. Protocol changes (PROTOCOL.md)

All additions are optional, so **the protocol version stays 1**:

- An old desktop app talking to a new phone ignores the unknown audio messages (the
  spec already says "unknown message types must be ignored").
- A new desktop app talking to an old phone sees no `audio` in `CAPABILITIES` and hides
  the mic controls.

### New message types

| Type | Name | Direction | Payload |
|------|------|-----------|---------|
| `0x13` | `AUDIO_FORMAT` | phone → desktop | JSON |
| `0x14` | `AUDIO_FRAME` | phone → desktop | binary |

`AUDIO_FORMAT` is sent before the first audio frame and whenever the format changes:

```json
{ "codec": "pcm_s16le", "sampleRate": 48000, "channels": 1 }
```

`AUDIO_FRAME` carries one 10 ms chunk of audio:

```
+------------------+-------------------------------+
| pts              | samples                       |
| 8 B, big-endian, | 480 x int16 little-endian     |
| microseconds     | (960 bytes at 48 kHz mono)    |
+------------------+-------------------------------+
```

`pts` uses the same phone clock as `VIDEO_FRAME`, so audio and video can be lined up
later (for example, in recordings).

### New JSON keys

- `CAPABILITIES.audio` (missing means the phone has no audio support):
  `{ "codecs": ["pcm_s16le"], "sampleRates": [48000], "channels": [1] }`
- `SET_CONFIG.audio` / `STATE.audio`: `true` or `false`, meaning the mic is on or off.
- `STATE.micPermission`: `"granted"`, `"denied"` or `"undetermined"`. This lets the
  desktop say "Allow the microphone in iPhone Settings" instead of failing silently.

### Why uncompressed PCM, not Opus or AAC

- **Bandwidth is not a problem.** Mono 48 kHz PCM is 0.77 Mbit/s, compared with 10 to
  50 Mbit/s for the video.
- **Lowest delay.** There's no encoder or decoder buffering, and no codec setup on
  either phone.
- **Simplest on Android later.** `AudioRecord` produces PCM directly.
- The `codec` field leaves room to add Opus later (for example, for a future USB or
  remote mode) without changing the protocol version.

### Backpressure (an addition to "Latency and backpressure")

Audio and video share one TCP connection, so audio can get stuck behind a large video
keyframe.

- **Audio is more sensitive than video.** A dropped video frame is invisible, but a gap
  in audio is audible. So the phone **sends audio even while a video frame is still in
  flight**, and doesn't follow the video rule of dropping frames when the socket is busy.
- **If more than 200 ms of audio is queued** (Wi-Fi has really stalled), the phone drops
  new audio until the backlog clears. A short gap is better than permanent delay. (Audio
  already handed to the socket can't be taken back, so it's the new audio that goes.)
- The desktop also **drops video sooner while audio is on**, so audio isn't held up by it.

## 2. iOS app changes

### Capture

- Add the microphone as an `AVCaptureDeviceInput` plus an `AVCaptureAudioDataOutput` to
  the **existing** `AVCaptureSession` in `CameraManager`. Audio and video then share one
  clock. The audio arrives on its own queue.
- Use `AVAudioConverter` to convert whatever the hardware delivers (usually 48 kHz
  float) to 48 kHz mono int16, cut into 10 ms chunks.
- Which microphone: iOS normally picks the mic that matches the camera (front mic with
  the front camera, back mic with the back cameras). **Verify this on the device.** If
  it doesn't, set it explicitly through `AVAudioSession` data sources.
- **Don't use iOS "voice processing" (echo cancellation) on the phone.** The sound from
  the computer's speakers never reaches the phone's audio system, so it has nothing to
  cancel against. Zoom, Discord and Chrome already cancel echo on the computer, and
  that works on our virtual mic like on any other mic.
- Audio only runs while the mic is on **and** a computer is connected. When it's off,
  the audio input is removed from the session, so the orange "mic in use" dot goes away.

### Permission and privacy

- Add `NSMicrophoneUsageDescription` to `project.yml`, for example: *"Pocket Webcam
  sends your microphone to your computer when you turn on the mic, so you can use it as
  a microphone in calls."*
- **The mic turns on automatically when a computer connects** (the desktop setting
  "Use the phone mic when it connects" is on by default and sends `audio: true`).
- iOS asks for permission the first time a computer connects, not when the app first
  starts, so the reason is obvious to the user. If permission is denied, `STATE` reports
  `micPermission: "denied"`, video keeps working, and the desktop shows how to fix it.
- **The phone always wins.** The phone user can turn the mic off at any time, and the
  desktop can't turn it back on without the phone showing it.

### UI

- **Mic button** in the icon column on the video's right edge (next to torch, mirror
  and sleep). It's on in yellow and off with a crossed-out mic.
- **Mic indicator** in the status bar while audio is streaming, plus a small level
  meter, so you can see the phone hears you.
- The landscape icon strips get the same button.

### Other

- The App Store privacy label doesn't change. Audio only goes from the phone to the
  user's own computer, never to us.
- A new TestFlight build is needed afterwards, which means another short Beta App
  Review, because the app gets a new permission.

### Files touched

`Camera/CameraManager.swift` (audio input and output), new `Encoding/AudioPacketizer.swift`
(converter and 10 ms chunks), `Network/Protocol.swift` (new types and keys),
`Network/StreamServer.swift` (`sendAudio` and its own backpressure rule),
`App/AppModel.swift` (mic state, permission, wiring), `UI/ContentView.swift` and
`UI/StatusBar.swift` (button, indicator), `project.yml` (usage string).

## 3. Desktop changes

### Virtual microphone

Tested on this machine on 2026-10-21 (PipeWire 1.6.8, GStreamer 1.28.6):

- `pipewiresink` with `media.class=Audio/Source/Virtual` **crashes** (SIGSEGV) in both
  modes, so it's not used.
- A null sink with `media.class=Audio/Source/Virtual` shows up as a mic, but a playback
  stream can't target it (PipeWire falls back to the speakers), so it's not used either.
- **Chosen: `module-pipe-source`.** It works and is the same on PipeWire and on
  older PulseAudio systems:
  `pactl load-module module-pipe-source source_name=pocketwebcam_mic
  file=$XDG_RUNTIME_DIR/pocketwebcam-mic.fifo format=s16le rate=48000 channels=1
  source_properties=device.description="Pocket Webcam Microphone"`.
  The app writes raw PCM into the FIFO. No GStreamer elements and no extra packages
  are needed.

Behaviour measured with a 440 Hz test tone:

- **With no writer, the mic keeps running and delivers silence**, so call apps never see
  a stalled device.
- **The mic stays until logout, like the virtual camera** (changed 2026-10-21 after the
  first real test). Removing it when the app quits made Discord (Chrome) fall back to
  another mic, and it **did not switch back** when the mic reappeared, not even when the
  mic was re-selected in Discord. PipeWire left the stream on the fallback although its
  `target.object` still named our mic; only `pactl move-source-output` fixed it. So the
  app never unloads the module. On start it reuses one that writes to the same FIFO and
  unloads only unusable ones (another path, duplicates, a FIFO that won't open). While
  the app isn't running, the mic plays silence. Tested: quit and restart kept Discord's
  stream on the mic.
- **Stale audio:** with the default 64 KiB pipe, the next app to open the mic heard
  180 ms of old audio. With the pipe shrunk to 4 KiB (`F_SETPIPE_SZ`, about 42 ms) and
  non-blocking writes that drop when the pipe is full, that went down to 50 ms. While
  an app is listening, the buffer stays at or below about 42 ms.
- **Clock drift between phone and computer:** if the phone runs fast, the pipe fills and
  one 10 ms chunk is dropped now and then. If it runs slow, the module inserts silence.
  That's good enough for v1; smarter resampling can come later if it's audible.
- The desktop still keeps a small jitter buffer (about 40 ms) before the pipe, to even
  out Wi-Fi hiccups.

### App UI

- **Mic button** next to Mirror and Light under the preview. It's labelled for screen
  readers and works from the keyboard with the yellow focus ring.
- **Level meter** while the mic is on, so you can see sound arriving.
- **Settings:** "Use the phone mic when it connects" (on by default) and a gain slider
  (0 to +12 dB). The mic button under the preview still turns it off for the session.
- **Stats popover:** add an audio row showing buffer delay and dropped or late audio.
- If permission is denied on the phone, a hint: "Allow the microphone for Pocket Webcam
  in iPhone Settings → Privacy → Microphone".
- Terminal receiver: the mic is on by default as well; `--no-mic` turns it off, and the
  console has `mic on|off`.

### Recording

The Phase 2 recorder used to record video only. Recordings from a phone that can send
audio now get an AAC track next to the copied video, in MP4 or MKV, lined up by the
phone's timestamps. The track is **silent wherever the mic is off**. A track can't be
added to a file once it has started, so this way turning the mic on mid-recording is
still captured. The encoder is the first available of fdkaacenc, avenc_aac (gst-libav,
which the tester installer already installs) or faac; with none, recordings have no
sound and a warning is logged.

### Linux tester package

No new packages are needed for the virtual mic: it needs `pactl`, which comes with
PipeWire (`pipewire-pulse`) and PulseAudio. The installer only checks that `pactl` exists.

## 4. Order of work

1. **Spike (desktop only, no phone needed): done 2026-10-21.** `module-pipe-source`
   chosen (see "Virtual microphone"). Still to check: the mic appears in Chrome and
   Discord.
2. Protocol: update PROTOCOL.md and the Rust and Swift protocol code, with tests.
3. Desktop: `audio.rs`, the jitter buffer, and engine wiring. Test it with a fake phone
   that sends a tone, which works without a phone.
4. iOS: capture, conversion, sending, permission and UI. Build in CI and install on the
   iPhone with ideviceinstaller.
5. Desktop UI: mic button, meter, settings, stats.
6. Audio in recordings.
7. Real test: a Discord or Zoom call using the iPhone mic, then a new TestFlight build.

As usual, I'll stop after the phase for you to test.

## 5. Risks and open questions

- **Delay:** the target is under 100 ms from phone mic to call app. A busy Wi-Fi
  network will push this up, and the jitter buffer setting is the trade-off.
- **Which mic iOS picks** needs checking on the device (step 4).
- **Lip sync:** in calls, the mic and camera are separate devices, so the call app
  doesn't line them up. We keep both as fast as possible, so they should be within a
  frame or two.
- **Flatpak apps** (for example, Discord installed as a Flatpak) can see PipeWire
  sources by default. Check this during the spike.
