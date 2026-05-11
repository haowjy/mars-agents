#!/usr/bin/env bash
set -euo pipefail

unset GIT_DIR GIT_WORK_TREE

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-full}"

run_step() {
  printf 'preflight: %s\n' "$*" >&2
  "$@"
}

case "$MODE" in
  fast)
    cd "$ROOT_DIR"
    run_step cargo fmt --check
    ;;
  full)
    cd "$ROOT_DIR"
    run_step cargo fmt --check
    run_step cargo clippy --all-targets -- -D warnings
    run_step cargo test --quiet
    run_step cargo build --release --quiet
    ;;
  *)
    printf 'Usage: preflight.sh [fast|full]\n' >&2
    exit 1
    ;;
esac
