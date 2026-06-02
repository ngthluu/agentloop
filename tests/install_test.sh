#!/usr/bin/env bash
# Sources install.sh and verifies detect_target() maps OS/arch to rust targets.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../scripts/install.sh
# shellcheck disable=SC1091
source "$SCRIPT_DIR/scripts/install.sh"
set +e   # install.sh enabled errexit on source; the test accumulates failures instead

fail=0

assert_target() {
  local os="$1" arch="$2" want="$3" got
  got="$(detect_target "$os" "$arch" 2>/dev/null)" || got="<err>"
  if [ "$got" != "$want" ]; then
    printf 'FAIL: detect_target %s %s => %s (want %s)\n' "$os" "$arch" "$got" "$want"
    fail=1
  else
    printf 'ok:   detect_target %s %s => %s\n' "$os" "$arch" "$got"
  fi
}

assert_unsupported() {
  local os="$1" arch="$2"
  if detect_target "$os" "$arch" >/dev/null 2>&1; then
    printf 'FAIL: detect_target %s %s should be unsupported\n' "$os" "$arch"
    fail=1
  else
    printf 'ok:   detect_target %s %s rejected\n' "$os" "$arch"
  fi
}

assert_target Darwin arm64   aarch64-apple-darwin
assert_target Darwin aarch64 aarch64-apple-darwin
assert_target Darwin x86_64  x86_64-apple-darwin
assert_target Linux  x86_64  x86_64-unknown-linux-gnu
assert_target Linux  amd64   x86_64-unknown-linux-gnu
assert_unsupported Linux   aarch64
assert_unsupported Windows x86_64

if [ "$fail" -eq 0 ]; then echo "ALL PASS"; else echo "TESTS FAILED"; exit 1; fi
