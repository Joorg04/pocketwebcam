#!/usr/bin/env bash
# Installs Pocket Webcam for Linux:
#   1. the video libraries and the virtual camera driver, with your package manager (asks for
#      your password once),
#   2. the "Pocket Webcam Camera" virtual webcam, loaded now and at every start,
#   3. the app itself, for your user only (~/.local), with an entry in your app launcher.
# Run it from the unpacked folder:  ./install.sh
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[1;31m%s\033[0m\n' "$*" >&2; exit 1; }

[[ $EUID -ne 0 ]] || fail "Run this as your normal user, not with sudo. It asks for your password when it needs it."
[[ "$(uname -m)" == "x86_64" ]] || fail "This download is for 64-bit Intel/AMD computers (x86_64); this one is $(uname -m)."

say "1/3  Installing video libraries and the virtual camera driver"
if command -v apt-get >/dev/null; then
    # Ubuntu, Debian, Linux Mint, Pop!_OS, elementary, Zorin …
    sudo apt-get update
    sudo apt-get install -y gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad gstreamer1.0-libav v4l2loopback-dkms
    # The driver is built for the running kernel and needs its headers.
    sudo apt-get install -y "linux-headers-$(uname -r)" ||
        echo "Couldn't install headers for kernel $(uname -r); the driver may not build. See README.txt."
elif command -v pacman >/dev/null; then
    # Arch, Manjaro, EndeavourOS, CachyOS …
    kernel_pkg=$(pacman -Qqo "/usr/lib/modules/$(uname -r)/vmlinuz" 2>/dev/null || echo linux)
    sudo pacman -S --needed --noconfirm gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav \
        v4l2loopback-dkms "${kernel_pkg}-headers"
elif command -v dnf >/dev/null; then
    # Fedora: the H.264 decoder and the driver come from RPM Fusion.
    if ! dnf repolist --enabled 2>/dev/null | grep -qi rpmfusion-free; then
        fail "Fedora needs the RPM Fusion repositories first (free + nonfree):
  https://rpmfusion.org/Configuration
Then run ./install.sh again."
    fi
    sudo dnf install -y gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-plugins-bad-free \
        gstreamer1-libav akmod-v4l2loopback
    sudo akmods --force || true
else
    fail "Couldn't find apt, pacman or dnf. Install these yourself, then run ./install.sh again:
  GStreamer plugins base, good, bad and libav (avdec_h264), and the v4l2loopback driver."
fi

say "2/3  Setting up the virtual camera (\"Pocket Webcam Camera\")"
sudo ./setup-v4l2loopback.sh ||
    fail "The virtual camera driver didn't load. See \"If the camera doesn't load\" in README.txt."

say "3/3  Installing the app"
bin="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/1024x1024/apps"
mkdir -p "$bin" "$apps" "$icons"
install -m755 bin/pocketwebcam-app "$bin/pocketwebcam-app"
install -m755 bin/pocketwebcam "$bin/pocketwebcam"
install -m644 share/pocketwebcam.png "$icons/pocketwebcam.png"
# The app's "set up the virtual camera" hint points here.
install -Dm755 setup-v4l2loopback.sh "$HOME/.local/share/pocketwebcam/setup-v4l2loopback.sh"
# An absolute path, so the launcher finds the app even before ~/.local/bin is on PATH.
sed "s|^Exec=.*|Exec=$bin/pocketwebcam-app|" share/pocketwebcam.desktop > "$apps/pocketwebcam.desktop"
chmod 644 "$apps/pocketwebcam.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" || true

say "Done!"
echo "Open \"Pocket Webcam\" from your app launcher (or run $bin/pocketwebcam-app),"
echo "then open the Pocket Webcam app on your iPhone. Both need to be on the same Wi-Fi."
