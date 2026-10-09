#!/bin/sh
# Removes mongo-compare installed by install.sh (macOS and Linux).
#
# The installer puts this script on your PATH as `mongo-compare-uninstall`.
# Your compare reports (compare-report-* folders) and export files are not touched.
set -eu

BIN="mongo-compare"
INSTALL_DIR="${MONGO_COMPARE_INSTALL_DIR:-$HOME/.local/bin}"
MARKER="# Added by the mongo-compare installer"

rm -f "$INSTALL_DIR/$BIN" "$INSTALL_DIR/$BIN-uninstall"

# Take out the PATH line the installer added (the marker and the line after it).
for profile in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
  if [ -f "$profile" ] && grep -qF "$MARKER" "$profile"; then
    tmp="$(mktemp)"
    awk -v marker="$MARKER" 'skip { skip = 0; next } $0 == marker { skip = 1; next } { print }' "$profile" > "$tmp"
    # Write back in place so file permissions and symlinks are kept.
    cat "$tmp" > "$profile"
    rm -f "$tmp"
    echo "Removed the PATH entry from $profile."
  fi
done

# Remembered settings and the sample files made by Ctrl+D / --demo.
case "$(uname -s)" in
  Darwin) rm -rf "$HOME/Library/Application Support/$BIN" ;;
  *) rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/$BIN" ;;
esac
rm -rf "${TMPDIR:-/tmp}/mongo-compare-demo"

echo "mongo-compare has been removed."
