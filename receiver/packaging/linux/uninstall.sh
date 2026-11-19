#!/usr/bin/env bash
# Removes the Pocket Webcam app and its virtual camera setup. Leaves the video libraries and
# the v4l2loopback driver installed (other apps such as OBS may use them), and keeps your
# recordings and snapshots.
set -euo pipefail

pkill -x pocketwebcam-ap 2>/dev/null || true
rm -f "$HOME/.local/bin/pocketwebcam-app" "$HOME/.local/bin/pocketwebcam" \
    "$HOME/.local/share/applications/pocketwebcam.desktop" \
    "$HOME/.local/share/icons/hicolor/1024x1024/apps/pocketwebcam.png" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/pocketwebcam.desktop"
rm -rf "$HOME/.local/share/pocketwebcam" "${XDG_STATE_HOME:-$HOME/.local/state}/pocketwebcam"
echo "Removed the app. Your settings are in ${XDG_CONFIG_HOME:-$HOME/.config}/pocketwebcam (delete that folder too if you like)."

if [[ -e /etc/modprobe.d/pocketwebcam.conf || -e /etc/modules-load.d/pocketwebcam.conf ]]; then
    echo "Removing the virtual camera setup (asks for your password)…"
    # Without an "options" line, the camera is set up in the user's own v4l2loopback
    # settings (see setup-v4l2loopback.sh): those, and their other cameras, stay.
    own_settings=no
    if [[ -e /etc/modprobe.d/pocketwebcam.conf ]] && ! grep -qs '^options v4l2loopback' /etc/modprobe.d/pocketwebcam.conf; then
        own_settings=yes
    fi
    sudo rm -f /etc/modprobe.d/pocketwebcam.conf /etc/modules-load.d/pocketwebcam.conf
    if [[ $own_settings == yes ]]; then
        echo "The camera is listed in your own v4l2loopback settings; take it out there if you like."
    else
        sudo modprobe -r v4l2loopback 2>/dev/null || echo "The virtual camera goes away after the next restart."
    fi
fi
echo "Done."
