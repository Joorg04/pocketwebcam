#!/usr/bin/env bash
# Builds the desktop app and installs it for the current user (no sudo needed).
# Puts the binary in ~/.local/bin and adds an app-launcher entry with the logo.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build --release

bin="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/1024x1024/apps"
mkdir -p "$bin" "$apps" "$icons"

install -m755 target/release/pocketwebcam-app "$bin/pocketwebcam-app"
install -m755 target/release/pocketwebcam "$bin/pocketwebcam"
install -m644 ../brand/profile-1024.png "$icons/pocketwebcam.png"
# The app's "set up the virtual camera" hint points here.
install -Dm755 scripts/setup-v4l2loopback.sh "$HOME/.local/share/pocketwebcam/setup-v4l2loopback.sh"
install -m644 packaging/pocketwebcam.desktop "$apps/pocketwebcam.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true

echo "Installed. Open \"Pocket Webcam\" from your app launcher, or run: pocketwebcam-app"
