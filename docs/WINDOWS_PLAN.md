# Windows desktop app

Status: **approved 2026-10-21, in progress.** Goal: the Windows app looks and works the
same as the Linux app (same window, same controls, same phone features), with Windows
equivalents for the parts that are Linux-only.

Decisions (2026-10-21):

- **Windows 11 first.** The virtual camera uses the built-in Windows 11 API
  (`MFCreateVirtualCamera`, build 22000+). Windows 10 (end of support October 2025) may
  get an older DirectShow camera later.
- **Microphone: our own driver, later, paid for by donations.** Windows can only add a
  microphone with a kernel audio driver signed by Microsoft (attestation signing in
  Partner Center, which needs an EV code-signing certificate, a few hundred dollars a
  year). VB-CABLE was rejected (decided 2026-10-21): users would need a second download
  and a reboot, and the mic would be called "CABLE Output". The goal is one download with
  app, camera and mic. Until the certificate is funded, Windows has no phone mic; the
  website's `/donate` page (Ko-fi) explains this. The same certificate signs the
  installer (no SmartScreen warning).
- **Testing:** builds come from GitHub Actions (`windows-latest`). The user tests on
  their Windows 11 PC.

## What's already portable

The whole window (egui, embedded Inter font, theme), the engine, the protocol, mDNS
discovery (`mdns-sd` is pure Rust), decoding, effects, capture and recordings
(GStreamer), file dialogs (`rfd`), settings, the single-instance lock (`File::try_lock`)
and the fake phone all work on Windows as they are, or with small path changes.

## What needs a Windows version

| Part | Linux | Windows |
|------|-------|---------|
| Virtual camera | `v4l2sink` → v4l2loopback | Media Foundation virtual camera: our own media source DLL (C++, adapted from [VCamSample](https://github.com/smourier/VCamSample), MIT) that reads frames the app writes to shared memory |
| Virtual mic | `module-pipe-source` FIFO | Own signed virtual audio driver (SYSVAD-based), fed through shared memory; funded by donations (see Decisions) |
| Tray | `ksni` (StatusNotifierItem) | `tray-icon` crate, same menu |
| Hide to tray | Hyprland special-workspace workaround | Native window hide and show |
| Autostart | XDG `.desktop` file | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` |
| Settings | `~/.config/pocketwebcam` | `%APPDATA%\Pocket Webcam` |
| Videos and Pictures folders | XDG user dirs | Known folders (Videos, Pictures) |
| Opening a capture | `xdg-open` + `close_range` | `ShellExecuteW` / `explorer` |
| Lock file and runtime dir | `XDG_RUNTIME_DIR` | `%LOCALAPPDATA%\Pocket Webcam` |
| GPU decode | VA-API (`vah264dec`) | Direct3D 11 (`d3d11h264dec`, `d3d11h265dec`) |
| Packaging | tarball + install.sh | Installer that bundles the GStreamer runtime and registers the camera DLL (needs admin once) |

## Virtual camera design

- `PocketWebcamCamera.dll` is a COM media source (one stream, NV12, 1920×1080 and
  1280×720, 30/60 fps, matching the app's Settings). It's installed under
  `Program Files` and registered in HKLM by the installer, because the Frame Server
  (svchost, LocalService) and the camera monitor (LocalSystem) load it, and they can't
  read files under a user profile.
- The app calls `MFCreateVirtualCamera(SoftwareCameraSource, System, CurrentUser,
  "Pocket Webcam", {CLSID})` and `Start`, with no admin rights needed. Windows shows the
  camera as "Pocket Webcam (Windows Virtual Camera)". Lifetime System means it stays
  in apps' lists even while the app is closed, like v4l2loopback on Linux.
- **Frames:** the app writes each finished frame (after effects) into a shared-memory
  ring (header with sequence number, size and timestamp, plus NV12 planes). The media
  source copies the newest frame into each sample. With no new frame for a while, it
  repeats the last one; with no writer at all, it shows a "Waiting for your phone"
  frame, like an idle webcam would.
- The shared memory is created by the source inside the Frame Server (session 0), with a
  security descriptor that lets the interactive user write. A normal user app isn't
  allowed to create `Global\` objects. **This is the first thing the spike checks.**
- On the app side, `Sink::V4l2` gets a Windows sibling `Sink::WinCamera`: the same
  `Output` pipeline ending in `appsink` (NV12), whose frames go into the shared memory.

## Order of work

Every step is built in CI and, where it shows something new, tested on the user's PC.

1. **Windows CI build** of the existing app and terminal receiver (preview only, no
   virtual camera yet): GStreamer MSVC runtime in CI, `cfg` gates for the Linux-only
   parts, a `.zip` artifact. Test: the app opens on the PC, finds the iPhone and shows
   the preview.
2. **Windows basics:** settings, capture folders, lock file, autostart, opening captures,
   D3D11 decoding, a tray with the same menu, native hide to tray.
3. **Virtual camera spike:** the media source DLL plus registration, at first showing a
   test pattern. Test: it appears in the Camera app, Chrome, Discord, Zoom and Teams,
   including whether Teams actually sends it to the other side.
4. **Virtual camera for real:** shared-memory frames from the app, Start/Stop virtual
   camera, sizes and frame rates, the idle frame.
5. **Microphone (after the certificate is funded):** a "Pocket Webcam Microphone" virtual
   audio driver based on Microsoft's SYSVAD sample, fed by the app through shared memory
   like the camera. Develop and test it in a Hyper-V VM with test signing, not on the
   user's PC. Same mic button, meter and settings as on Linux. Until then the app's mic
   settings on Windows explain this and link to `/donate` (done 2026-10-21).
6. **Installer:** bundles the app, the GStreamer runtime and the camera DLL; registers the
   DLL (one admin prompt); Start menu entry; a firewall rule so mDNS discovery works;
   uninstaller. Later: code signing, so SmartScreen doesn't warn.
   Started 2026-10-21: `receiver/packaging/windows/pocketwebcam.iss` (Inno Setup), built in
   CI as the `PocketWebcam-Setup` artifact (about 47 MB). Windows 11 only. The firewall
   rule allows incoming UDP 5353 (mDNS) on private and domain networks only. The
   uninstaller runs `pocketwebcam.exe --remove-camera` (Windows keeps the camera listed
   otherwise), unregisters the DLL, deletes the rule, the login entry and the plugin
   cache; settings in `%APPDATA%` stay. Hosting: the installer is too big for the website
   (Cloudflare's 25 MB file limit), so it's on Cloudflare R2 at
   `downloads.pocketwebcam.com` (2026-10-21; see `docs/WEBSITE.md` → Downloads).
7. **Polish and parity check** against the Linux app, screen by screen.

## Risks and open questions

- **Teams:** VCamSample reports that new Teams previews MF virtual cameras but doesn't
  send them to the other side. If that's also true for us, it needs investigating
  (possibly a missing attribute or format).
- **Shared memory across session 0** (see above): if the source can't create it with the
  right permissions, fall back to a named pipe or to a file mapping in `ProgramData`.
- **Code signing:** without a certificate, Windows SmartScreen warns on the installer.
  Options and costs to decide before a public release.
- **GStreamer size:** the runtime adds roughly 100 MB. It can be trimmed to the plugins
  we use.
