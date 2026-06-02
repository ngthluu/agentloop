#!/usr/bin/env bash
# agentloop installer.
#   curl -fsSL https://raw.githubusercontent.com/ngthluu/agentloop/main/scripts/install.sh | bash
# Downloads the prebuilt binary for the host platform from the latest GitHub
# Release and installs it to ~/.local/bin (override with AGENTLOOP_INSTALL_DIR).
set -euo pipefail

REPO="ngthluu/agentloop"
BIN="agentloop"
DEFAULT_INSTALL_DIR="$HOME/.local/bin"

err() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

# detect_target <os> <arch> -> prints the rust target triple, or returns 1.
detect_target() {
  local os="$1" arch="$2"
  case "$os" in
    Darwin)
      case "$arch" in
        arm64|aarch64) echo "aarch64-apple-darwin" ;;
        x86_64)        echo "x86_64-apple-darwin" ;;
        *)             return 1 ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
        # linux/arm64 (aarch64) not yet supported — no release artifact built
        *)            return 1 ;;
      esac
      ;;
    *)
      return 1
      ;;
  esac
}

main() {
  local os arch target install_dir url tmp
  os="$(uname -s)"
  arch="$(uname -m)"
  target="$(detect_target "$os" "$arch")" || err "unsupported platform: $os $arch"

  install_dir="${AGENTLOOP_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
  url="https://github.com/${REPO}/releases/latest/download/${BIN}-${target}.tar.gz"

  printf 'Installing %s (%s) to %s\n' "$BIN" "$target" "$install_dir"

  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064  # intentional: expand $tmp now so trap fires even after main() returns
  trap "rm -rf '$tmp'" EXIT

  curl -fsSL "$url" -o "$tmp/${BIN}.tar.gz" || err "download failed: $url"
  tar -xzf "$tmp/${BIN}.tar.gz" -C "$tmp" || err "extract failed"
  [ -f "$tmp/$BIN" ] || err "archive did not contain '$BIN'"

  mkdir -p "$install_dir"
  install -m 0755 "$tmp/$BIN" "$install_dir/$BIN"

  printf '\n\xe2\x9c\x93 Installed %s to %s/%s\n' "$BIN" "$install_dir" "$BIN"
  # shellcheck disable=SC2016
  case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) printf '\n! %s is not on your PATH. Add to your shell profile:\n    export PATH="%s:$PATH"\n' "$install_dir" "$install_dir" ;;
  esac
}

# Run main only when executed/piped (curl | bash), not when sourced by tests.
if ! (return 0 2>/dev/null); then
  main "$@"
fi
