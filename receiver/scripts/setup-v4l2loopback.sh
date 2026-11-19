#!/usr/bin/env bash
# Creates the "Pocket Webcam Camera" virtual webcam at /dev/video10 and makes it
# persist across reboots. Run with sudo. Requires v4l2loopback-dkms.
# Keep OPTIONS in sync with packaging/linux/modprobe.conf (package-linux.sh checks).
set -euo pipefail

DEVICE_NR=10
LABEL="Pocket Webcam Camera"

if [[ $EUID -ne 0 ]]; then
    echo "Run with sudo: sudo $0" >&2
    exit 1
fi

if ! modinfo v4l2loopback >/dev/null 2>&1; then
    echo "v4l2loopback is not installed (Ubuntu/Debian: v4l2loopback-dkms, Arch: v4l2loopback-dkms, Fedora: akmod-v4l2loopback from RPM Fusion)." >&2
    exit 1
fi

# --- helpers ---------------------------------------------------------------------------

# Settings files for v4l2loopback that aren't ours, one per line, from the given folders.
other_loopback_configs() {
    local dir file
    for dir in "$@"; do
        for file in "$dir"/*.conf; do
            [[ -f "$file" && "${file##*/}" != pocketwebcam.conf ]] || continue
            if grep -Eq '^[[:space:]]*options[[:space:]]+v4l2loopback([[:space:]]|$)' "$file"; then
                echo "$file"
            fi
        done
    done
}

# True if the camera exists under our name.
camera_is_set_up() {
    [[ -e /dev/video${DEVICE_NR} ]] &&
        [[ "$(cat "/sys/devices/virtual/video4linux/video${DEVICE_NR}/name" 2>/dev/null)" == "$LABEL" ]]
}

# Names of the programs that have a virtual camera open right now, e.g. "obs, chrome".
loopback_users() {
    local node fd pid names=()
    for node in /sys/devices/virtual/video4linux/video*; do
        [[ -e "$node" ]] || continue
        while IFS= read -r fd; do
            pid=${fd#/proc/}
            names+=("$(cat "/proc/${pid%%/*}/comm" 2>/dev/null || echo "process ${pid%%/*}")")
        done < <(find /proc/[0-9]*/fd -maxdepth 1 -lname "/dev/${node##*/}" 2>/dev/null)
    done
    ((${#names[@]})) && printf '%s\n' "${names[@]}" | sort -u | paste -sd, | sed 's/,/, /g'
    return 0
}

# Loads the driver, or loads it again if its settings changed, and checks for the camera.
load_camera() {
    if lsmod | grep -q '^v4l2loopback'; then
        if camera_is_set_up; then
            echo "Ready: /dev/video${DEVICE_NR} (${LABEL}) is already set up"
            return 0
        fi
        # Loading it again takes every v4l2loopback camera away for a moment, also the
        # ones other programs are using, and the driver refuses while any is open.
        if (($(cat /sys/module/v4l2loopback/refcnt 2>/dev/null || echo 0) > 0)); then
            local users
            users=$(loopback_users)
            echo "The driver has to be loaded again for the new settings, but a virtual camera is in use${users:+ by: ${users}}." >&2
            echo "Close those programs (in OBS: Stop Virtual Camera), then run this again." >&2
            return 1
        fi
        echo "Reloading v4l2loopback..."
        modprobe -r v4l2loopback
    fi
    modprobe v4l2loopback

    if camera_is_set_up; then
        echo "Ready: /dev/video${DEVICE_NR} (${LABEL})"
    else
        echo "Module loaded but /dev/video${DEVICE_NR} (${LABEL}) is missing; check dmesg" >&2
        return 1
    fi
}

# --- somebody else's settings? ----------------------------------------------------------

# The driver takes one set of settings: modprobe hands it every "options v4l2loopback"
# line it finds, and where two disagree the last one wins. A hand-written file (usually
# for OBS's virtual camera) plus ours would quietly cancel one of the two cameras out.
OTHERS=$(other_loopback_configs /etc/modprobe.d /run/modprobe.d /usr/local/lib/modprobe.d /usr/lib/modprobe.d)
if [[ -n "$OTHERS" ]]; then
    listed=no
    while IFS= read -r file; do
        if grep -E '^[[:space:]]*options[[:space:]]+v4l2loopback' "$file" | grep -qF "\"${LABEL}\""; then
            listed=yes
        fi
    done <<<"$OTHERS"

    if [[ $listed == yes ]]; then
        # That file already lists our camera (see the instructions below): it's in charge.
        # An empty file of ours in /etc also switches off a packaged one in /usr/lib.
        echo "Using your own v4l2loopback settings, which list \"${LABEL}\":"
        sed 's/^/  /' <<<"$OTHERS"
        echo "# Empty on purpose: \"${LABEL}\" is set up in another file here (see setup-v4l2loopback.sh)." \
            > /etc/modprobe.d/pocketwebcam.conf
        echo "v4l2loopback" > /etc/modules-load.d/pocketwebcam.conf
        load_camera
        exit
    fi

    {
        echo "v4l2loopback already has settings that Pocket Webcam didn't write (usually for OBS's virtual camera):"
        echo
        while IFS= read -r file; do
            echo "  $file"
            grep -E '^[[:space:]]*options[[:space:]]+v4l2loopback' "$file" | sed 's/^[[:space:]]*/      /'
        done <<<"$OTHERS"
        echo
        echo "The driver takes only one set of settings, so a second file from Pocket Webcam would"
        echo "cancel one of the cameras out. Nothing was changed."
        echo
        echo "Add the Pocket Webcam camera to that line instead: raise devices by one, and add one"
        echo "entry to the end of video_nr, card_label and exclusive_caps. For example,"
        echo
        echo "  options v4l2loopback devices=1 video_nr=0 card_label=\"OBS Virtual Camera\" exclusive_caps=1"
        echo "becomes"
        echo "  options v4l2loopback devices=2 video_nr=0,${DEVICE_NR} card_label=\"OBS Virtual Camera\",\"${LABEL}\" exclusive_caps=1,1"
        echo
        echo "Then run this script again: sudo $0"
    } >&2
    exit 1
fi

# exclusive_caps=1 is required for Chrome/WebRTC to list the device as a camera.
OPTIONS="devices=1 video_nr=${DEVICE_NR} card_label=\"${LABEL}\" exclusive_caps=1"

# The .deb, .rpm and AUR packages ship the same settings in /usr/lib (packaging/linux/*.conf);
# only the tarball install needs them written to /etc.
if [[ -e /usr/lib/modprobe.d/pocketwebcam.conf && -e /usr/lib/modules-load.d/pocketwebcam.conf ]]; then
    # A file of ours in /etc (from a tarball install, or the empty one written above
    # while another file was in charge) has the same name and would hide the packaged one.
    rm -f /etc/modprobe.d/pocketwebcam.conf /etc/modules-load.d/pocketwebcam.conf
    echo "Using the packaged settings in /usr/lib/modprobe.d/pocketwebcam.conf"
else
    echo "v4l2loopback" > /etc/modules-load.d/pocketwebcam.conf
    echo "options v4l2loopback ${OPTIONS}" > /etc/modprobe.d/pocketwebcam.conf
    echo "Wrote /etc/modules-load.d/pocketwebcam.conf and /etc/modprobe.d/pocketwebcam.conf"
fi

load_camera
