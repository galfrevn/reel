#!/usr/bin/env bash
# reel installer — https://github.com/galfrevn/reel
#
#   curl -fsSL https://raw.githubusercontent.com/galfrevn/reel/main/setup.sh | bash
#
# Installs the `reel` binary: downloads a prebuilt release when one exists
# for this platform, otherwise builds from source with cargo.

set -euo pipefail

REPO="galfrevn/reel"
BIN="reel"
INSTALL_DIR="${REEL_INSTALL_DIR:-$HOME/.local/bin}"

info()  { printf '\033[1;36mreel\033[0m %s\n' "$*"; }
error() { printf '\033[1;31mreel\033[0m %s\n' "$*" >&2; exit 1; }

# ── platform ────────────────────────────────────────────────────────────────
os="$(uname -s)"; arch="$(uname -m)"
case "$os" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-gnu" ;;
  *) error "unsupported OS: $os (Windows support is on the roadmap)" ;;
esac
case "$arch" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x86_64" ;;
  *) error "unsupported architecture: $arch" ;;
esac
target="${arch}-${os}"

# ── try a prebuilt release ──────────────────────────────────────────────────
try_release() {
  local url="https://github.com/${REPO}/releases/latest/download/${BIN}-${target}.tar.gz"
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN
  if curl -fsSL "$url" -o "$tmp/reel.tar.gz" 2>/dev/null; then
    tar -xzf "$tmp/reel.tar.gz" -C "$tmp"
    mkdir -p "$INSTALL_DIR"
    install -m 755 "$tmp/$BIN" "$INSTALL_DIR/$BIN"
    return 0
  fi
  return 1
}

# ── fall back to building from source ───────────────────────────────────────
build_from_source() {
  command -v cargo >/dev/null 2>&1 || error \
"no prebuilt binary for ${target} yet, and cargo (Rust) is not installed.
Install Rust first — https://rustup.rs — then re-run this script:

  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  info "no prebuilt binary for ${target} — building from source (takes a minute)…"
  cargo install --git "https://github.com/${REPO}" reel-cli --root "${INSTALL_DIR%/bin}" --quiet
}

info "installing reel for ${target}"
if try_release; then
  info "installed prebuilt binary"
else
  build_from_source
fi

# ── PATH check ──────────────────────────────────────────────────────────────
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) info "note: add $INSTALL_DIR to your PATH:
    export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

"$INSTALL_DIR/$BIN" --version
info "done. Try:  reel render session.cast"
info "tip: let your coding agent drive reel —  npx skills add ${REPO}"
