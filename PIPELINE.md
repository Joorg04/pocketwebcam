# PIPELINE.md

Rules for the /pipeline agents (planner, workers, reviewer) in this repository.
See `~/Work/Agentic pipeline/README.md` for how the pipeline works.

## Project

Pocket Webcam: use a phone as a wireless webcam. Folders: `receiver/` (Rust: terminal receiver
`pocketwebcam` + desktop app `pocketwebcam-app` in `src/bin/app/`), `ios/` (Swift, project generated
by XcodeGen from `ios/project.yml`), `android/` (Kotlin, Compose, Camera2), `website/` (plain
HTML/CSS), `docs/` (roadmap, launch plan), `PROTOCOL.md` (phone ↔ desktop protocol, shared by all apps).

## Environment setup

- Rust: none. To avoid rebuilding all dependencies in every worktree, use a shared target dir.
  Set it on each command, not with a separate `export X=...; cd ...` (the sandbox refuses that form):
  `cd receiver && CARGO_TARGET_DIR="$HOME/.cache/pocketwebcam-target" cargo test`
- With that target dir, binaries are in `~/.cache/pocketwebcam-target/release/`, not `receiver/target/`.
- Android: `source ~/Android/env.sh` before any `./gradlew` command (otherwise Java is missing).
- iOS: cannot be built on this Linux machine. Only GitHub Actions builds it, and the pipeline never
  pushes, so iOS changes can only be checked by reading the code carefully. Say so in reports.

## Build and verify

| Area | Command (from repo root) | Notes |
|------|--------------------------|-------|
| Receiver + desktop app | `cd receiver && cargo build --release` | first build in a new target dir takes minutes |
| Receiver tests | `cd receiver && cargo test` | |
| Terminal-only receiver | `cd receiver && cargo build --release --no-default-features` | must keep working |
| Lint | `cd receiver && cargo clippy --all-targets -- -D warnings` | clean on master since 153b0ad; must stay clean |
| Android | `cd android && ./gradlew -q :app:assembleDebug` | ~1-2 min |
| Android release | `cd android && ./gradlew -q :app:bundleRelease` | needs the upload key; skip if absent |
| Website | open the changed pages' HTML and check links/assets exist | `python3 tools/preview_site.py` serves it locally |
| End-to-end (no phone) | `python3 tools/fake_phone.py` + `~/.cache/pocketwebcam-target/release/pocketwebcam --output window` | needs a display; usually skip in workers |

Agents can't launch the desktop app (`pocketwebcam-app`): the user's running copy holds the
single-instance lock, and killing it is off-limits. UI changes are verified by the user after merge
(`install-app.sh`, then a phone or the Android emulator `pw_test` via `adb forward tcp:7310 tcp:7310`
and "Connect by IP address" → `127.0.0.1:7310`). Say so in reports.

## Off-limits

- Never: `git push`, deploying the website (Cloudflare), `receiver/scripts/install-app.sh` (installs
  into the user's system), killing the user's running desktop app, `adb install` on a real phone.
- Never read, print or edit: `~/.gradle/gradle.properties`, `~/keys/`, `*.jks`, `*.p12`, `*.p8`,
  `~/Documents/pocketwebcam-notes.txt`, anything with API keys or SMTP credentials.
- Don't change version numbers (`receiver/Cargo.toml`, `ios/project.yml`, `android/app/build.gradle.kts`)
  unless the task is a release: they're bumped together (README "Versions").
- `PROTOCOL.md` changes affect all three apps: only if the task asks for a protocol change, and then
  every app must be updated in the same run.
- Generated or local folders: `receiver/target/`, `android/build/`, `android/app/build/`, `ios/*.xcodeproj`.

## Security-sensitive

Changes to these paths also get a security review (pipeline-security-reviewer). The phone and desktop
talk over the local network, so anything that parses network input matters:
- `PROTOCOL.md`, `receiver/src/protocol.rs`, `session.rs`, `discovery.rs`, `engine.rs`
- `ios/PocketWebcam/Network/`, `android/app/src/main/kotlin/com/pocketwebcam/app/net/`
- Permission and privacy files: `ios/project.yml` (Info.plist keys), `ios/PocketWebcam/Resources/PrivacyInfo.xcprivacy`,
  `android/app/src/main/AndroidManifest.xml`
- Website security: `website/_headers`, `website/.well-known/`, forms and anything with `<script>`
- Build and signing setup: `android/app/build.gradle.kts`, `.github/workflows/`

## Conventions

- Commit messages: `Area: what changed` (e.g. `Desktop app: ...`, `Android: ...`, `Website: ...`,
  `Docs: ...`), imperative, with a short body explaining why when it isn't obvious.
- User-facing wording is phone-neutral ("phone", not "iPhone") unless it's iOS-specific.
- Desktop app style: dark theme with brand yellow `#ffd60a` (`receiver/src/bin/app/theme.rs`).

## Good parallel splits

`receiver/`, `android/`, `ios/` and `website/` don't share files, so steps in different folders can
run in the same wave. Within `receiver/src/bin/app/`, `controls.rs`, `window.rs`, `tray.rs` and
`guide.rs` are fairly independent; `main.rs` and `theme.rs` are touched by most UI changes, so give
them to one step only.
