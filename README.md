# Pocket Webcam

Use your iPhone as a webcam over Wi-Fi: free, full resolution, every lens.

Website: [pocketwebcam.com](https://pocketwebcam.com)

```
iPhone app ──Wi-Fi──▶ desktop receiver ──▶ virtual webcam ──▶ Zoom / Meet / OBS / browser
 camera                 decode (GStreamer)    /dev/video10
 H.264/HEVC encode
```

## Project layout

| Path | What it is |
|---|---|
| [`PROTOCOL.md`](PROTOCOL.md) | The phone ↔ desktop wire protocol. **Read this first.** |
| [`protocol-fixtures/`](protocol-fixtures/) | Sample messages for every part of the protocol. The receiver's and the Android app's tests read them, so the apps can't drift apart unnoticed. |
| [`ios/`](ios/) | iPhone app (Swift / SwiftUI, no dependencies). Built on the Mac. |
| [`receiver/`](receiver/) | Desktop receiver (Rust + GStreamer). Linux today. |
| [`website/`](website/) | pocketwebcam.com: plain HTML/CSS, hosted on Cloudflare. See [`docs/WEBSITE.md`](docs/WEBSITE.md). |
| [`brand/`](brand/) | Logo, profile pictures, colors and bio. See [`brand/README.md`](brand/README.md). |
| [`tools/`](tools/) | `fake_phone.py` (simulated iPhone for testing) and `preview_site.py` (local website preview). |
| [`docs/`](docs/) | Signing, packaging, pairing, microphone and Windows notes, website hosting. |

### iPhone app (`ios/PocketWebcam/`)

| Folder | Contents |
|---|---|
| `App/` | App entry point, `AppModel` (wires camera → encoder → network, single source of truth for the UI) |
| `Camera/` | `CameraManager`: lens discovery, formats/fps, zoom, torch, stabilization, mirroring, rotation |
| `Encoding/` | `VideoEncoder`: hardware H.264/HEVC via VideoToolbox, outputs Annex-B |
| `Network/` | Protocol types, `StreamServer` (Bonjour + TCP, frame dropping), Wi-Fi address helper |
| `UI/` | SwiftUI views: preview, status bar, lens picker and menus, settings, onboarding |
| `Resources/` | Asset catalog, privacy manifest (`Info.plist` is generated) |

### Receiver (`receiver/src/`)

| File | Contents |
|---|---|
| `main.rs` | CLI flags, reconnect loop |
| `protocol.rs` | Message framing and types (with unit tests) |
| `discovery.rs` | mDNS browsing for phones: one long-running search that keeps a live list |
| `logging.rs` | Log output; the desktop app also writes a log file |
| `session.rs` | One connection: handshake, message loop, keepalive |
| `video.rs` | GStreamer: persistent virtual-camera output + per-stream decoder |
| `console.rs` | Interactive commands (`lens`, `res`, `fps`, `bitrate`, …) |
| `engine.rs` | Discovery + connection loop + virtual camera behind one handle (used by the desktop app) |
| `settings.rs` | Saved settings (`~/.config/pocketwebcam/settings.json`) |
| `autostart.rs` | "Start on login" (XDG autostart file) |
| `bin/app/` | The desktop window (egui): `main.rs` preview and header, `controls.rs` sidebar and settings, `theme.rs` colors and widgets, `tray.rs` tray icon and menu, `window.rs` show/hide, `guide.rs` first-run guide |

## Linux: first-time setup

```bash
# 1. Dependencies (Arch / Omarchy)
sudo pacman -S --needed v4l2loopback-dkms v4l2loopback-utils \
    gst-plugins-good gst-plugins-bad gst-libav rustup
rustup default stable

# 2. Create the "Pocket Webcam Camera" virtual webcam (persists across reboots)
sudo receiver/scripts/setup-v4l2loopback.sh

# 3. Build the receiver
cd receiver && cargo build --release
```

## Desktop app

```bash
receiver/scripts/install-app.sh     # builds, installs to ~/.local/bin, adds "Pocket Webcam" to the app launcher
pocketwebcam-app                    # or run it directly
```

Pick your phone, watch the live picture, and change lens, resolution, frame rate, codec, quality, zoom,
mirror, stabilization and light from the sidebar. Settings (virtual camera size, GPU decoding, device,
auto-connect) are in the Settings window.

**First run.** A three-step guide opens the first time (get the iPhone app with a QR code, open it on the
phone, pick "Pocket Webcam Camera" in your video app). Reopen it any time with the **Guide** button. The QR code
points at `GET_APP_URL` in `receiver/src/bin/app/guide.rs`: switch it to the App Store link at launch.
Only one copy of the app runs; starting a second one brings the first to the front.

**Which phone connects.** The first time, a phone that is alone on the network connects by itself. From
then on only that phone does (it's remembered by an identifier the phone app makes up, so renaming the
phone doesn't matter); any other phone waits until you pick it from the menu at the top, and then becomes
the remembered one. That way the app never picks up somebody else's phone on a shared network.

**When something goes wrong.** The app writes a log to `~/.local/state/pocketwebcam/pocketwebcam.log`
(Windows: `%LOCALAPPDATA%\Pocket Webcam\pocketwebcam.log`); the tray menu has **Open log folder**. The
file starts over at 1 MB and the one before is kept as `pocketwebcam.old.log`. Start the app with
`RUST_LOG=debug` for more detail.

**Tray and login.** The app puts an icon in the system tray (menu: status, show/hide window, disconnect,
quit). Closing the window keeps streaming in the tray; turn that off in Settings. "Start when I log in"
adds `~/.config/autostart/pocketwebcam.desktop`, which starts the app hidden (`--minimized`). On Hyprland
the window is hidden by moving it to a special workspace, because Wayland doesn't let apps hide their own
window; on a desktop with no tray, closing the window just quits.

Release packages for users (.deb, .rpm, AUR `pocketwebcam-bin`, tarball): see
[docs/LINUX_PACKAGING.md](docs/LINUX_PACKAGING.md).

Build the terminal-only receiver with
`cargo build --release --no-default-features`.

## Running (terminal)

```bash
receiver/target/release/pocketwebcam            # find a phone and stream to /dev/video10
receiver/target/release/pocketwebcam --list     # list phones on the network
receiver/target/release/pocketwebcam --host 192.168.1.42   # skip discovery
receiver/target/release/pocketwebcam --size 3840x2160      # 4K virtual camera (default 1080p)
receiver/target/release/pocketwebcam --hw-decode           # GPU decoding via VA-API
receiver/target/release/pocketwebcam --background blur     # blur what's behind you
receiver/target/release/pocketwebcam --background-image ~/Pictures/office.jpg   # virtual background
```

`--background-image builtin:studio` picks a built-in background instead (also `warm-wall`, `dusk`,
`ocean`, `mint`). Builds with `--no-default-features` leave background support out; add
`--features background` to put it back in the terminal-only receiver.

While it runs, type commands into the terminal:

```
lens            list lenses          lens 2 / lens tele    switch lens
res             list resolutions     res 4k / res 1920x1080
fps 60          codec hevc           bitrate 20            zoom 2
torch on        mirror on            stab on               status
```

Then pick **Pocket Webcam Camera** in Zoom, Meet, OBS, or your browser.

## Testing without an iPhone

```bash
python3 tools/fake_phone.py                                  # terminal 1
receiver/target/release/pocketwebcam --output window           # terminal 2: preview window
```

The fake phone advertises over mDNS like the real app and streams test patterns.
Each fake "lens" shows a different pattern, so switching is visible.

`--no-id` and `--no-audio` make it behave like an older phone app, `--id` with another `--name` like a
phone that was renamed, and `kill -USR1 <pid>` pauses and resumes its camera the way iOS does when another
app takes the camera.

Two receiver tests use the real network (multicast, under a service type of their own) and are skipped by
default. Run them after touching `discovery.rs`:

```bash
cd receiver && cargo test --lib -- --ignored discovery
```

## iPhone: building on the Mac

```bash
brew install xcodegen
cd ios && xcodegen          # generates PocketWebcam.xcodeproj
open PocketWebcam.xcodeproj
```

In Xcode: select the **PocketWebcam** target → *Signing & Capabilities* → choose your team.
Plug in your iPhone, select it as the run destination, press ⌘R.
The simulator has no camera, so always run on a real device.

## Versions

All three apps share one version number (desktop `receiver/Cargo.toml`, iOS
`MARKETING_VERSION` in `ios/project.yml`, Android `versionName` in
`android/app/build.gradle.kts`). Bump them together and tag the commit:

```
git tag -a v0.2.0 -m "short description" && git push origin v0.2.0
```

`0.x` = before launch; `1.0.0` = first store release. `git tag -n` lists the
milestones; `git checkout v0.1.0` (then `git switch master`) visits an old one.

| Tag | Milestone |
|-----|-----------|
| v0.0.1 | First checkpoint: receiver, iOS app source, website |
| v0.0.2 | Desktop app: preview, phone controls, tray, first-run guide |
| v0.0.3 | Android app tested on a real phone; signup, SEO, iOS CI build |
| v0.1.0 | All apps on one version number (0.1.0) |

## License

Pocket Webcam is open source under the [MIT license](LICENSE): the iPhone, Android and
desktop apps, the website and the tools in this repository. The libraries and data the
desktop app ships with keep their own licenses, listed in
[receiver/packaging/THIRD-PARTY-NOTICES.txt](receiver/packaging/THIRD-PARTY-NOTICES.txt)
(regenerate it with `receiver/scripts/third-party-notices.sh` after changing dependencies).

The name "Pocket Webcam" and the logo (`brand/`) aren't covered by the MIT license. You're
welcome to fork the code, but please give your version its own name and icon, so nobody
mistakes it for this app.

Found a security problem? See [SECURITY.md](SECURITY.md).
