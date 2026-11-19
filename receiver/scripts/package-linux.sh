#!/usr/bin/env bash
# Builds the Linux download: target/dist/pocketwebcam-linux-x86_64-<version>.tar.gz
# (with its own install.sh, and the input for the .deb/.rpm/AUR packages).
#
# The binaries need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 and newer). On a
# system with a newer glibc they are built with cargo-zigbuild, so they still run on those.
# Needs (user-level, no sudo):  mise install zig@0.15.2 && cargo install --locked cargo-zigbuild
# On glibc 2.35 itself (CI builds in an ubuntu:22.04 container) plain cargo is enough.
set -euo pipefail
cd "$(dirname "$0")/.."

GLIBC=2.35
version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
name="pocketwebcam-linux-x86_64-$version"
out="target/dist/$name"

host_glibc=$(getconf GNU_LIBC_VERSION | cut -d' ' -f2)
if [[ "$(printf '%s\n' "$host_glibc" "$GLIBC" | sort -V | tail -1)" == "$GLIBC" ]]; then
    bin="target/portable/release"
    cargo build --release --locked --target-dir target/portable
else
    bin="target/portable/x86_64-unknown-linux-gnu/release"
    # pkg-config leaves out /usr/lib, which zig doesn't search by itself.
    PKG_CONFIG_ALLOW_CROSS=1 RUSTFLAGS="-L native=/usr/lib" mise exec zig@0.15.2 -- \
        cargo zigbuild --release --locked --target "x86_64-unknown-linux-gnu.$GLIBC" --target-dir target/portable
fi

newest=$(objdump -T "$bin/pocketwebcam-app" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
[[ "$newest" == "GLIBC_$GLIBC" || "$(printf '%s\n' "$newest" "GLIBC_$GLIBC" | sort -V | tail -1)" == "GLIBC_$GLIBC" ]] ||
    { echo "binary needs $newest, newer than glibc $GLIBC" >&2; exit 1; }

# The setup script (tarball installs) and modprobe.conf (packages) must load the same camera.
nr=$(sed -n 's/^DEVICE_NR=//p' scripts/setup-v4l2loopback.sh)
label=$(sed -n 's/^LABEL="\(.*\)"$/\1/p' scripts/setup-v4l2loopback.sh)
grep -qx "options v4l2loopback devices=1 video_nr=$nr card_label=\"$label\" exclusive_caps=1" packaging/linux/modprobe.conf ||
    { echo "packaging/linux/modprobe.conf doesn't match scripts/setup-v4l2loopback.sh" >&2; exit 1; }

rm -rf "$out" "$out.tar.gz"
mkdir -p "$out/bin" "$out/share" "$out/licenses"
install -m755 "$bin/pocketwebcam-app" "$bin/pocketwebcam" "$out/bin/"
install -m644 packaging/pocketwebcam.desktop "$out/share/"
install -m644 ../brand/profile-1024.png "$out/share/pocketwebcam.png"
# For the .deb, .rpm and AUR packages, which install from this tarball (package-linux-distro.sh).
install -m644 packaging/linux/icons/pocketwebcam-256.png packaging/linux/icons/pocketwebcam-512.png \
    packaging/linux/modprobe.conf packaging/linux/modules-load.conf "$out/share/"
install -m755 packaging/linux/install.sh packaging/linux/uninstall.sh scripts/setup-v4l2loopback.sh "$out/"
install -m644 packaging/linux/README.txt "$out/"
install -m644 ../LICENSE assets/fonts/Inter-LICENSE.txt packaging/THIRD-PARTY-NOTICES.txt "$out/licenses/"
tar -C target/dist --owner=0 --group=0 --numeric-owner -czf "$out.tar.gz" "$name"
echo "Built $out.tar.gz ($(du -h "$out.tar.gz" | cut -f1), needs glibc ${newest#GLIBC_} or newer)"
