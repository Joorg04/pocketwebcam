#!/usr/bin/env bash
# Points the AUR package at the release in receiver/Cargo.toml: sets pkgver, resets pkgrel,
# fills in the tarball's sha256 and regenerates .SRCINFO. Then copy PKGBUILD,
# pocketwebcam.install and .SRCINFO into your clone of the AUR repo (docs/LINUX_PACKAGING.md).
#
#   packaging/aur/update-aur.sh            # checksum of the published file on downloads.pocketwebcam.com
#   packaging/aur/update-aur.sh FILE       # checksum of a local tarball (before it's uploaded)
set -euo pipefail
local_file=${1:+$(realpath "$1")}
cd "$(dirname "$0")"

version=$(grep -m1 '^version' ../../Cargo.toml | cut -d'"' -f2)
file="pocketwebcam-linux-x86_64-$version.tar.gz"
if [[ -n "$local_file" ]]; then
    sha=$(sha256sum "$local_file" | cut -d' ' -f1)
else
    sha=$(curl -fsSL "https://downloads.pocketwebcam.com/linux/$file" | sha256sum | cut -d' ' -f1)
fi

cd pocketwebcam-bin
old=$(sed -n 's/^pkgver=//p' PKGBUILD)
sed -i "s/^pkgver=.*/pkgver=$version/; s/^sha256sums_x86_64=.*/sha256sums_x86_64=('$sha')/" PKGBUILD
[[ "$old" == "$version" ]] || sed -i 's/^pkgrel=.*/pkgrel=1/' PKGBUILD
makepkg --printsrcinfo > .SRCINFO
echo "pocketwebcam-bin $version: sha256 $sha"
