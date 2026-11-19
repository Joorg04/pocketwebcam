#!/usr/bin/env bash
# Builds the .deb (Ubuntu/Debian) and .rpm (Fedora) packages from the tarball that
# package-linux.sh made:
#   target/dist/pocketwebcam_<version>_amd64.deb
#   target/dist/pocketwebcam-<version>-1.x86_64.rpm
# Needs dpkg-deb and rpmbuild (Ubuntu: apt install rpm). Pass --deb or --rpm to build only one.
#
# The packages install the app to /usr, the virtual camera settings to /usr/lib/modprobe.d and
# /usr/lib/modules-load.d, and depend on the distro's GStreamer plugins, pactl and v4l2loopback.
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
name="pocketwebcam-linux-x86_64-$version"
tarball="target/dist/$name.tar.gz"
dist="$PWD/target/dist"
work="$PWD/target/distro"
want_deb=1 want_rpm=1
case "${1:-}" in
    --deb) want_rpm=0 ;;
    --rpm) want_deb=0 ;;
    "") ;;
    *) echo "usage: $0 [--deb|--rpm]" >&2; exit 2 ;;
esac
[[ -f "$tarball" ]] || { echo "$tarball is missing; run scripts/package-linux.sh first" >&2; exit 1; }

summary="Use your iPhone as a webcam over Wi-Fi"
description="Pocket Webcam receives the camera of the Pocket Webcam iPhone app over the local
network and shows it as the \"Pocket Webcam Camera\" virtual webcam (v4l2loopback) in Zoom,
Meet, Teams, Discord, OBS and browsers. It can also make the phone's microphone a PipeWire
or PulseAudio microphone."
maintainer="Pocket Webcam <support@pocketwebcam.com>"

rm -rf "$work"
mkdir -p "$work"
tar -C "$work" -xzf "$tarball"
src="$work/$name"

# The file tree both packages install. The AUR PKGBUILD installs the same files.
stage() {
    local root=$1 licenses=$2
    install -Dm755 "$src/bin/pocketwebcam-app" "$root/usr/bin/pocketwebcam-app"
    install -Dm755 "$src/bin/pocketwebcam" "$root/usr/bin/pocketwebcam"
    install -Dm644 "$src/share/pocketwebcam.desktop" "$root/usr/share/applications/pocketwebcam.desktop"
    for size in 256 512; do
        install -Dm644 "$src/share/pocketwebcam-$size.png" \
            "$root/usr/share/icons/hicolor/${size}x$size/apps/pocketwebcam.png"
    done
    install -Dm755 "$src/setup-v4l2loopback.sh" "$root/usr/share/pocketwebcam/setup-v4l2loopback.sh"
    install -Dm644 "$src/share/modprobe.conf" "$root/usr/lib/modprobe.d/pocketwebcam.conf"
    install -Dm644 "$src/share/modules-load.conf" "$root/usr/lib/modules-load.d/pocketwebcam.conf"
    install -Dm644 -t "$root$licenses" "$src/licenses/LICENSE" "$src/licenses/Inter-LICENSE.txt" \
        "$src/licenses/THIRD-PARTY-NOTICES.txt"
}

# Load the camera now if the driver is already built (a later restart loads it anyway).
# Never fails the install, and never reloads a loaded driver: an app may be using it.
postinst='#!/bin/sh
if [ -e /proc/modules ] && ! grep -q "^v4l2loopback " /proc/modules; then
    modprobe v4l2loopback 2>/dev/null || true
fi
if [ ! -e /dev/video10 ]; then
    echo "Pocket Webcam: the virtual camera starts after the next restart."
    echo "To start it now: sudo /usr/share/pocketwebcam/setup-v4l2loopback.sh"
fi
exit 0'

if ((want_deb)); then
    root="$work/deb"
    stage "$root" /usr/share/doc/pocketwebcam
    # Debian keeps the license in doc/<package>/copyright.
    {
        echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"
        echo "Upstream-Name: Pocket Webcam"
        echo "Source: https://pocketwebcam.com"
        echo
        echo "Files: *"
        echo "Copyright: $(sed -n 's/^Copyright (c) //p' "$src/licenses/LICENSE")"
        echo "License: MIT"
        echo "Comment: pocketwebcam-app embeds the Inter font, under the OFL-1.1 license below, and"
        echo " other people's libraries and data: their licenses are in"
        echo " /usr/share/doc/pocketwebcam/THIRD-PARTY-NOTICES.txt."
        echo
        echo "License: MIT"
        sed 's/^$/./; s/^/ /' "$src/licenses/LICENSE"
        echo
        echo "License: OFL-1.1"
        sed 's/^$/./; s/^/ /' "$src/licenses/Inter-LICENSE.txt"
    } > "$root/usr/share/doc/pocketwebcam/copyright"
    rm "$root/usr/share/doc/pocketwebcam/LICENSE" "$root/usr/share/doc/pocketwebcam/Inter-LICENSE.txt"
    mkdir -p "$root/DEBIAN"
    {
        echo "Package: pocketwebcam"
        echo "Version: $version"
        echo "Architecture: amd64"
        echo "Maintainer: $maintainer"
        echo "Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)"
        # The GStreamer plugin packages pull in libgstreamer and glib; the window needs EGL and
        # xkbcommon (loaded at run time); pactl makes the phone's microphone.
        echo "Depends: gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav, v4l2loopback-dkms, pulseaudio-utils, libegl1, libxkbcommon0"
        echo "Section: video"
        echo "Priority: optional"
        echo "Homepage: https://pocketwebcam.com"
        echo "Description: $summary"
        sed 's/^/ /' <<<"$description"
    } > "$root/DEBIAN/control"
    printf '%s\n' "$postinst" > "$root/DEBIAN/postinst"
    chmod 755 "$root/DEBIAN/postinst"
    deb="$dist/pocketwebcam_${version}_amd64.deb"
    dpkg-deb --root-owner-group -Zxz --build "$root" "$deb" >/dev/null
    echo "Built $deb ($(du -h "$deb" | cut -f1))"
fi

if ((want_rpm)); then
    root="$work/rpm"
    stage "$root" /usr/share/licenses/pocketwebcam
    spec="$work/pocketwebcam.spec"
    cat > "$spec" <<EOF
Name: pocketwebcam
Version: $version
Release: 1
Summary: $summary
License: MIT AND OFL-1.1
URL: https://pocketwebcam.com
Packager: $maintainer
# H.264/HEVC decoding (gstreamer1-libav) and the v4l2loopback driver come from RPM Fusion.
Requires: gstreamer1-plugins-base
Requires: gstreamer1-plugins-good
Requires: gstreamer1-plugins-bad-free
Requires: (gstreamer1-libav or gstreamer1-plugin-libav)
Requires: akmod-v4l2loopback
Requires: pulseaudio-utils
Requires: libglvnd-egl
Requires: libxkbcommon
# The binaries are prebuilt and already stripped.
AutoReqProv: no
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

%description
$description

%install
cp -a "$root/." %{buildroot}/

%post
$(tail -n +2 <<<"$postinst")

%files
/usr/bin/pocketwebcam-app
/usr/bin/pocketwebcam
/usr/share/applications/pocketwebcam.desktop
/usr/share/icons/hicolor/256x256/apps/pocketwebcam.png
/usr/share/icons/hicolor/512x512/apps/pocketwebcam.png
/usr/share/pocketwebcam
/usr/lib/modprobe.d/pocketwebcam.conf
/usr/lib/modules-load.d/pocketwebcam.conf
%license /usr/share/licenses/pocketwebcam/LICENSE
%license /usr/share/licenses/pocketwebcam/Inter-LICENSE.txt
%license /usr/share/licenses/pocketwebcam/THIRD-PARTY-NOTICES.txt
EOF
    rpmbuild -bb --quiet --nodeps --define "_topdir $work/rpmbuild" --define "_rpmdir $work/rpmbuild/out" "$spec"
    rpm="$dist/pocketwebcam-$version-1.x86_64.rpm"
    mv "$work/rpmbuild/out/x86_64/pocketwebcam-$version-1.x86_64.rpm" "$rpm"
    echo "Built $rpm ($(du -h "$rpm" | cut -f1))"
fi
