#!/usr/bin/env bash
# Fuzzes the API with cargo-fuzz (nightly) for FUZZ_SECS seconds (default
# 1200). The corpus lives in $K2_JOB_CACHE so later runs build on it.
# -malloc_limit_mb is below what one input's frames can add up to in the
# large-frame mode, so any buffering of frames fails the run.
set -euo pipefail
secs="${FUZZ_SECS:-1200}"
cache="${K2_JOB_CACHE:-$PWD/.fuzz-cache}"
mkdir -p "$cache/corpus" "$cache/bin"
export PATH="$cache/bin/bin:$PATH"
if ! command -v cargo-fuzz >/dev/null; then
  cargo install --locked --root "$cache/bin" cargo-fuzz
fi
rustc --version
cd fuzz
status=0
cargo fuzz run api "$cache/corpus" -- -max_total_time="$secs" -rss_limit_mb=2048 -malloc_limit_mb=64 \
  -max_len=65536 -print_final_stats=1 || status=$?
mkdir -p ../out
cp -r artifacts ../out/ 2>/dev/null || true
ls "$cache/corpus" | wc -l | xargs echo "corpus entries:"
exit $status
