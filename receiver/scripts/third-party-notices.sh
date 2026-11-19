#!/usr/bin/env bash
# Writes packaging/THIRD-PARTY-NOTICES.txt: the licenses of everything the desktop app
# ships that we didn't write. The app shows it (Settings → Open-source licenses) and the
# Linux and Windows packages include it.
#
# Run it after changing dependencies; CI fails if the committed file is out of date.
# Needs cargo-about 0.9 (cargo install cargo-about --locked --features cli).
set -euo pipefail
cd "$(dirname "$0")/.."

out=packaging/THIRD-PARTY-NOTICES.txt
rust=$(cargo about generate --workspace packaging/about.hbs)

# Plain LF line endings everywhere, so the file is the same on Windows (where Git checks
# the license files out with CRLF) and CI can compare it.

{
cat <<'EOF'
Pocket Webcam: third-party notices
==================================

Pocket Webcam's desktop app includes software and data made by other people. Their
licenses are below. Pocket Webcam itself is under the MIT license (LICENSE).

Contents
  1. Inter (font)
  2. MediaPipe Selfie Segmentation (background blur model)
  3. GStreamer and the libraries it uses (Windows)
  4. Rust libraries


================================================================================
1. Inter (font)
================================================================================

The app's font. SIL Open Font License 1.1.

EOF
cat assets/fonts/Inter-LICENSE.txt
cat <<'EOF'


================================================================================
2. MediaPipe Selfie Segmentation (background blur model)
================================================================================

Used for background blur and virtual backgrounds. Copyright Google LLC, Apache
License 2.0 (text below). Source: the MediaPipe models page,
https://storage.googleapis.com/mediapipe-assets/selfie_segmentation_landscape.tflite

Changes made by Pocket Webcam: the model was converted from TensorFlow Lite to ONNX
(tf2onnx), and its last layer, a MediaPipe-specific operation, was replaced with
standard ONNX operations that compute the same result.

EOF
cat models/LICENSE-MediaPipe.txt
cat <<'EOF'


================================================================================
3. GStreamer and the libraries it uses (Windows)
================================================================================

On Linux, Pocket Webcam uses the GStreamer that your distribution installs, so the
Linux packages don't include it.

On Windows, the installer includes GStreamer 1.28 and the libraries it depends on
(GLib, FFmpeg's libavcodec, libavformat and libavutil, Pango, HarfBuzz, FreeType,
fontconfig, cairo, and others), unchanged, as built by the GStreamer project.
GStreamer and most of these libraries are under the GNU Lesser General Public
License (LGPL) 2.1 or later; the rest are under permissive licenses. Pocket Webcam
loads them as separate DLLs, and you may replace them with your own builds.

Each library's license text is in the "licenses\gstreamer" folder of the Windows
installation (C:\Program Files\Pocket Webcam\licenses\gstreamer).

Source code: the GStreamer project publishes the exact source of these builds,
including every library, at https://gstreamer.freedesktop.org/download/ and
https://gitlab.freedesktop.org/gstreamer/cerbero (the tag for the bundled version).
If you can't get it there, write to hello@pocketwebcam.com and we'll send it to you,
for at least three years after we last ship this version.

GNU Lesser General Public License: https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html


EOF
printf '%s\n' "$rust"
} | tr -d '\r' > "$out"

echo "Wrote $out ($(wc -c < "$out" | tr -d ' ') bytes)"
