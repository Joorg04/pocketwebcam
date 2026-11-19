# Protocol fixtures

Sample messages for the protocol in [`../PROTOCOL.md`](../PROTOCOL.md), exactly as they
travel between the phone and the computer.

The protocol is written by hand four times: the receiver (Rust), the iPhone app (Swift),
the Android app (Kotlin) and the fake phone (Python). These files are the one place where
"what a message looks like" is written down for all of them, so a change on one side that
the others don't understand shows up in a test instead of on somebody's desk.

| File | Message |
|------|---------|
| `hello_desktop.json`, `hello_phone.json` | `HELLO` from each side |
| `capabilities.json` | `CAPABILITIES` |
| `set_config.json`, `set_config_partial.json` | `SET_CONFIG` with every key, and with one |
| `state.json` | `STATE` with every key a phone may send |
| `state_minimal.json` | `STATE` from the oldest phone app (required keys only) |
| `stream_format.json`, `stream_format_mirror.json` | `STREAM_FORMAT` (iPhone style, Android style) |
| `audio_format.json` | `AUDIO_FORMAT` |
| `error.json`, `ping.json` | `ERROR`, `PING` / `PONG` |
| `pair_request.json`, `pair_proof.json`, `pair_result.json` | The pairing messages |
| `pairing.json` | Test vectors for pairing: certificate fingerprint, QR link, TXT tag, 6-digit code, proof |
| `frames.json` | Whole messages as bytes (hex), with what they mean: the 5-byte header, `VIDEO_FRAME`, `AUDIO_FRAME` |

Who reads them:

- Receiver: `receiver/src/protocol.rs` (`cargo test`)
- Android: `android/app/src/test/kotlin/com/pocketwebcam/app/net/FixturesTest.kt` (`./gradlew :app:testDebugUnitTest`)
- iPhone: nothing yet. The iOS project has no test target; add one that decodes these files
  when it gets one.

When `PROTOCOL.md` changes, change the fixtures in the same commit and run both test suites.
New keys must stay optional: `state_minimal.json` and the `hello_*.json` files are what
already-installed apps send, and they have to keep working.
