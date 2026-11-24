#!/usr/bin/env bash
# Runs the conformance tool against outside oracles, on a Linux x86_64 node:
# a static FFmpeg build (ffmpeg, ffprobe) and mkvalidator built from the
# Matroska project's published source. Both run only as separate
# processes; their source is never read. Cached in $WEBM_CI_CACHE
# (default .ci-cache in the repo).
set -euo pipefail
cache="${WEBM_CI_CACHE:-$PWD/.ci-cache}/oracles"
mkdir -p "$cache"

ff="$cache/ffmpeg"
if [ ! -x "$ff/bin/ffprobe" ]; then
  rm -rf "$ff" && mkdir -p "$ff"
  curl -fsSL -o "$cache/ffmpeg.tar.xz" \
    https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz
  tar -xJf "$cache/ffmpeg.tar.xz" -C "$ff" --strip-components=1
  rm -f "$cache/ffmpeg.tar.xz"
fi
export FFMPEG="$ff/bin/ffmpeg" FFPROBE="$ff/bin/ffprobe"
"$FFPROBE" -version | head -1

mkv_tag="${MKVALIDATOR_TAG:-mkvalidator-0.6.0}"
mkv="$cache/$mkv_tag-bin"
if [ ! -x "$mkv" ]; then
  rm -rf "$cache/foundation-source" "$cache/mkv-build"
  git clone -q --depth 1 --branch "$mkv_tag" https://github.com/Matroska-Org/foundation-source "$cache/foundation-source"
  cmake -S "$cache/foundation-source" -B "$cache/mkv-build" -DCMAKE_BUILD_TYPE=Release >/dev/null
  cmake --build "$cache/mkv-build" --target mkvalidator -j 4 >/dev/null
  cp "$(find "$cache/mkv-build" -type f -name mkvalidator -perm -u+x | head -1)" "$mkv"
  (cd "$cache/foundation-source" && echo "mkvalidator source commit: $(git rev-parse HEAD)") > "$cache/$mkv_tag.version"
fi
export MKVALIDATOR="$mkv"
cat "$cache/$mkv_tag.version"

cargo build --release -p webm-conformance
rm -rf out && mkdir -p out
"${CARGO_TARGET_DIR:-target}/release/webm-conformance" out || status=$?
exit "${status:-0}"
