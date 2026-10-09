#!/bin/sh
# Installs mongo-compare on macOS or Linux from the latest GitHub release.
#
#   curl -fsSL https://github.com/SankethJain/CanonicalJsonCompare/releases/latest/download/install.sh | sh
#
# Optional settings (environment variables):
#   MONGO_COMPARE_VERSION      a release tag such as v0.1.0 (default: latest)
#   MONGO_COMPARE_INSTALL_DIR  where to put the program (default: ~/.local/bin)
set -eu

REPO="SankethJain/CanonicalJsonCompare"
BIN="mongo-compare"
VERSION="${MONGO_COMPARE_VERSION:-latest}"
INSTALL_DIR="${MONGO_COMPARE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Linux) os="unknown-linux-musl" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "this installer supports macOS and Linux. On Windows use install.ps1." ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) fail "unsupported processor: $(uname -m)" ;;
esac
target="$arch-$os"
archive="$BIN-$target.tar.gz"
if [ -n "${MONGO_COMPARE_DOWNLOAD_URL:-}" ]; then
  base="$MONGO_COMPARE_DOWNLOAD_URL"
elif [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$1" -O "$2"
  else
    fail "please install curl or wget first"
  fi
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading $BIN ($target)..."
download "$base/$archive" "$tmp/$archive" || fail "download failed: $base/$archive"

if download "$base/$archive.sha256" "$tmp/$archive.sha256" 2>/dev/null; then
  expected="$(cut -d ' ' -f 1 < "$tmp/$archive.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$archive" | cut -d ' ' -f 1)"
  else
    actual="$(shasum -a 256 "$tmp/$archive" | cut -d ' ' -f 1)"
  fi
  [ "$expected" = "$actual" ] || fail "checksum mismatch, the download may be damaged. Please try again."
fi

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$INSTALL_DIR"
cp "$tmp/$BIN-$target/$BIN" "$INSTALL_DIR/$BIN"
chmod 755 "$INSTALL_DIR/$BIN"
if [ "$os" = "apple-darwin" ]; then
  # Allow macOS to run a program that was not downloaded through a browser.
  xattr -d com.apple.quarantine "$INSTALL_DIR/$BIN" 2>/dev/null || true
fi
say "Installed to $INSTALL_DIR/$BIN"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    line="export PATH=\"$INSTALL_DIR:\$PATH\""
    case "${SHELL:-}" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) [ "$os" = "apple-darwin" ] && profile="$HOME/.bash_profile" || profile="$HOME/.bashrc" ;;
      *) profile="$HOME/.profile" ;;
    esac
    if ! grep -qs "$INSTALL_DIR" "$profile"; then
      printf '\n# Added by the mongo-compare installer\n%s\n' "$line" >> "$profile"
      say "Added $INSTALL_DIR to your PATH in $profile."
    fi
    say "Open a new terminal window before using it."
    ;;
esac

say ""
say "Done! Start it by typing:  $BIN"
say "Try it with sample data:   $BIN --demo"
