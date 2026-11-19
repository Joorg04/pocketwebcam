# Linux packaging

How the Linux desktop app (`pocketwebcam-app`, plus the terminal receiver `pocketwebcam`) is
packaged, built, released and published.

## Downloads

| File | For | Install |
|---|---|---|
| `pocketwebcam_<version>_amd64.deb` | Ubuntu 22.04+, Debian 12+, Mint 21+, Pop!_OS, elementary, Zorin | `sudo apt install ./pocketwebcam_<version>_amd64.deb` |
| `pocketwebcam-<version>-1.x86_64.rpm` | Fedora 36+ (with [RPM Fusion](https://rpmfusion.org/Configuration) free + nonfree) | `sudo dnf install ./pocketwebcam-<version>-1.x86_64.rpm`, then restart |
| AUR `pocketwebcam-bin` | Arch, Manjaro, EndeavourOS, CachyOS | `yay -S pocketwebcam-bin` (plus the headers for your kernel, e.g. `linux-headers`) |
| `pocketwebcam-linux-x86_64-<version>.tar.gz` | any other x86_64 distro with glibc 2.35+ | unpack, `./install.sh` (uses apt, pacman or dnf; installs the app to `~/.local`) |

All files live in the R2 bucket under `linux/` and are served at
`https://downloads.pocketwebcam.com/linux/<file>` (see [WEBSITE.md](WEBSITE.md)). x86_64 only.

Only Arch has been tested on a real system so far. Keep the website's "tested" labels honest.

## Why these formats

The app needs three things from the system that it can't bring along:

1. **The v4l2loopback kernel module**, loaded with `devices=1 video_nr=10
   card_label="Pocket Webcam Camera" exclusive_caps=1` at every boot. Building it (DKMS/akmod),
   signing it for Secure Boot and writing `modprobe.d`/`modules-load.d` all need root and the
   distro's own packages.
2. **GStreamer plugins** (good, bad, libav for H.264/HEVC decoding, VA-API from bad). The H.264/HEVC
   decoders are packaged differently per distro (Fedora: RPM Fusion only).
3. **`pactl`** and the user's PipeWire/PulseAudio server, for the phone microphone
   (`module-pipe-source`).

That rules out the self-contained formats as the main download:

- **Flatpak** can't load kernel modules or write `/etc`/`/usr/lib/modprobe.d`, needs
  `--device=all` to reach `/dev/video10`, and would bundle its own GStreamer runtime without the
  distro's codec choices. Users would still have to install v4l2loopback by hand, which is the
  hard part. Flathub also discourages apps that need host setup.
- **AppImage** has the same gap (no module, no system config) and would also have to bundle
  GStreamer and its plugins.
- **Snap**: same, plus confinement around `/dev/video*`.

Native packages fix this in one step: the package manager installs v4l2loopback (DKMS on
Debian/Ubuntu/Arch, akmod on Fedora) and the GStreamer plugins as dependencies, and the package
ships the module settings. So:

- **.deb** (Ubuntu/Debian and derivatives: most desktop Linux users) and **.rpm** (Fedora) are
  built from the same file tree by one script.
- **AUR `pocketwebcam-bin`** for Arch: the binary package from the release tarball. A source
  package (`pocketwebcam`, built with cargo) isn't possible while the GitHub repository is
  private, because the AUR needs a public source URL. If the repo goes public, add one next to
  `pocketwebcam-bin` (same `package()` lines, `build()` running `cargo build --release --locked`).
- **The tarball** stays as the fallback for everything else, with `install.sh`.

No apt/dnf repository for now: users download the file again for updates. A repository (or a
PPA / COPR) is the next step if updates become frequent.

## What the packages install

The .deb, .rpm and AUR package install the same files (from the tarball):

| Path | What |
|---|---|
| `/usr/bin/pocketwebcam-app`, `/usr/bin/pocketwebcam` | the desktop app and the terminal receiver |
| `/usr/share/applications/pocketwebcam.desktop` | app launcher entry (`receiver/packaging/pocketwebcam.desktop`) |
| `/usr/share/icons/hicolor/{256x256,512x512}/apps/pocketwebcam.png` | icon (`receiver/packaging/linux/icons/`, made from `brand/profile-1024.png`) |
| `/usr/lib/modprobe.d/pocketwebcam.conf` | v4l2loopback options (`receiver/packaging/linux/modprobe.conf`) |
| `/usr/lib/modules-load.d/pocketwebcam.conf` | loads v4l2loopback at boot (`receiver/packaging/linux/modules-load.conf`) |
| `/usr/share/pocketwebcam/setup-v4l2loopback.sh` | (re)loads the camera now; the app shows this command when the camera is missing |
| `/usr/share/licenses/pocketwebcam*/` (Debian: `/usr/share/doc/pocketwebcam/copyright`) | MIT license and the Inter font's OFL |

After install the package loads the module if it's already built and not loaded yet. It never
reloads a loaded module (an app could be using it), and on a first install DKMS (Arch) or akmods
(Fedora) usually builds the module afterwards, so the camera appears after the next restart or
after `sudo /usr/share/pocketwebcam/setup-v4l2loopback.sh`. The package prints that.

Settings in `/usr/lib/modprobe.d` can be overridden by a file with the same name in
`/etc/modprobe.d`, which is what the tarball's `setup-v4l2loopback.sh` writes; both have the same
content. `package-linux.sh` checks that `modprobe.conf` matches the script.

Dependencies:

| | .deb | .rpm | AUR |
|---|---|---|---|
| GStreamer | `gstreamer1.0-plugins-{base,good,bad}`, `gstreamer1.0-libav` | `gstreamer1-plugins-{base,good,bad-free}`, `gstreamer1-libav` or `gstreamer1-plugin-libav` | `gstreamer`, `gst-plugins-{base,base-libs,good,bad}`, `gst-libav` |
| v4l2loopback | `v4l2loopback-dkms` | `akmod-v4l2loopback` (RPM Fusion) | `V4L2LOOPBACK-MODULE` (= `v4l2loopback-dkms`) |
| microphone (`pactl`) | `pulseaudio-utils` | `pulseaudio-utils` | `libpulse` |
| window | `libegl1`, `libxkbcommon0` | `libglvnd-egl`, `libxkbcommon` | `libglvnd`, `libxkbcommon`, `hicolor-icon-theme` |

## Build locally

```bash
cd receiver
scripts/package-linux.sh          # tarball (cargo-zigbuild for glibc 2.35 on a newer system)
scripts/package-linux-distro.sh   # .deb + .rpm from that tarball (needs dpkg-deb and rpmbuild)
```

Output in `receiver/target/dist/`. `package-linux.sh` needs `mise install zig@0.15.2 && cargo
install --locked cargo-zigbuild` when the system's glibc is newer than 2.35; on glibc 2.35 itself
(the CI container) it uses plain cargo. On Arch, dpkg-deb and rpmbuild come from the `dpkg` and
`rpm-tools` packages; `--deb` or `--rpm` builds only one.

Check the packages without installing them:

```bash
dpkg-deb --info target/dist/pocketwebcam_0.1.0_amd64.deb
dpkg-deb --contents target/dist/pocketwebcam_0.1.0_amd64.deb
rpm -qip target/dist/pocketwebcam-0.1.0-1.x86_64.rpm
rpm -qlp --requires --scripts target/dist/pocketwebcam-0.1.0-1.x86_64.rpm
```

AUR package from a local tarball (no install):

```bash
cd receiver/packaging/aur
./update-aur.sh ../../target/dist/pocketwebcam-linux-x86_64-0.1.0.tar.gz
cd pocketwebcam-bin
cp ../../../target/dist/pocketwebcam-linux-x86_64-0.1.0.tar.gz .   # makepkg uses it instead of downloading
makepkg -f && namcap PKGBUILD && namcap pocketwebcam-bin-*.pkg.tar.zst
rm -rf src pkg *.tar.gz *.pkg.tar.zst
```

namcap warns that the GStreamer plugins, `libpulse`, `libglvnd`, `libxkbcommon` and
`V4L2LOOPBACK-MODULE` "may not be needed": they're used at run time (plugins, `pactl`, dlopen,
the kernel module), which namcap can't see. Keep them.

## Release

1. Bump the version everywhere (README "Versions"), commit, tag and push the tag:
   `git tag -a v0.2.0 -m "..." && git push origin v0.2.0`.
2. The **Linux release** workflow (`.github/workflows/linux-release.yml`) builds the tarball,
   .deb and .rpm in an `ubuntu:22.04` container, checks the AUR package builds and passes namcap
   in an Arch container, and uploads everything as the artifact `pocketwebcam-linux-<version>`
   (with `SHA256SUMS`). It fails if the tag doesn't match `receiver/Cargo.toml`. It can also be
   started by hand (Actions → Linux release → Run workflow). It publishes nothing.
3. Download it: `gh run download -R Joorg04/pocketwebcam -n pocketwebcam-linux-0.2.0`
4. Upload the three files to R2, each with a new name (never overwrite a published file):

   ```bash
   v=0.2.0
   for f in "pocketwebcam-linux-x86_64-$v.tar.gz:application/gzip" \
            "pocketwebcam_${v}_amd64.deb:application/vnd.debian.binary-package" \
            "pocketwebcam-$v-1.x86_64.rpm:application/x-rpm"; do
     name=${f%%:*} type=${f#*:}
     npx wrangler r2 object put "pocketwebcam-downloads/linux/$name" --remote --file "$name" \
       --content-type "$type" --cache-control "public, max-age=31536000, immutable" \
       --content-disposition "attachment; filename=\"$name\""
   done
   ```

   (PowerShell: `npx.cmd`, one command per file as in WEBSITE.md.) Then update the links and
   sizes in `website/download.html` and `website/linux.html`, and delete the oldest version.
5. Publish the AUR package (below), after the tarball is uploaded.

## Publish to the AUR

One-time setup: an account on [aur.archlinux.org](https://aur.archlinux.org) with your SSH public
key (My Account → SSH Public Key), then:

```bash
git clone ssh://aur@aur.archlinux.org/pocketwebcam-bin.git ~/aur/pocketwebcam-bin   # empty the first time
```

Each release:

```bash
receiver/packaging/aur/update-aur.sh     # sets pkgver, pkgrel=1 and the sha256 of the uploaded tarball
cp receiver/packaging/aur/pocketwebcam-bin/{PKGBUILD,pocketwebcam.install,.SRCINFO} ~/aur/pocketwebcam-bin/
cd ~/aur/pocketwebcam-bin
makepkg -f && namcap PKGBUILD *.pkg.tar.zst && rm -rf src pkg *.tar.gz *.pkg.tar.zst
git add PKGBUILD pocketwebcam.install .SRCINFO && git commit -m "Update to 0.2.0" && git push
```

Commit the updated PKGBUILD and .SRCINFO in this repo too. To fix the package without a new app
version, bump `pkgrel` by hand, regenerate `.SRCINFO` (`makepkg --printsrcinfo > .SRCINFO`) and
push again. The AUR repo must only ever contain `PKGBUILD`, `.SRCINFO` and `pocketwebcam.install`.

## Moving from the tarball install to a package

The tarball installs to `~/.local`, which comes first on PATH and in the app launcher, so it would
hide the packaged app. Run `./uninstall.sh` from the tarball folder first (it also unloads the
virtual camera; the package loads it again at the next restart).

## Before telling users it works

Test on a clean machine or VM per distro (none of this has been installed yet):

- Ubuntu 22.04 and 24.04, with Secure Boot on: `sudo apt install ./pocketwebcam_*.deb` asks
  for a MOK password, the restart enrolls the key, `/dev/video10` shows up as "Pocket Webcam
  Camera", the app launches from the menu with its icon, the phone connects, Chrome/Zoom list
  the camera, the microphone works.
- Debian 12, Fedora (with RPM Fusion), Arch with `yay -S pocketwebcam-bin`: same checks.
- `apt remove pocketwebcam` / `dnf remove` / `pacman -R` leaves no files behind.

Known limitation: Ubuntu 22.04/24.04 and Debian 12 ship v4l2loopback 0.12.7. The app's
"keep the camera listed while stopped" feature uses v4l2loopback's `timeout_image_io` control,
which most likely arrived after 0.12.7 (its upstream commits are from late 2023), so on those systems the camera may still disappear from other apps'
lists while the app isn't streaming. Check this when testing.
