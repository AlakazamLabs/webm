#!/usr/bin/env bash
# The crate's gates: fmt, clippy (-D warnings), tests, docs (-D warnings),
# examples and the package list. Runs every step and fails if any failed.
set -uo pipefail
fail=0
step() {
  echo "=== $*"
  if ! "$@"; then
    echo "!!! FAILED: $*"
    fail=1
  fi
}
rustc --version
cargo --version
step cargo fmt --all --check
step cargo clippy --workspace --all-targets -- -D warnings
step cargo clippy -p webm-muxer --all-targets --all-features -- -D warnings
step cargo test --workspace
step cargo test -p webm-muxer --all-features
step cargo test -p webm-muxer --no-default-features
step env RUSTDOCFLAGS="-D warnings" cargo doc -p webm-muxer --no-deps
step env RUSTDOCFLAGS="-D warnings" cargo doc -p webm-muxer --no-deps --all-features
step cargo build -p webm-muxer --examples
step cargo package -p webm-muxer --list --allow-dirty
exit $fail
