#!/bin/sh
# Removes minerve installed by `make install` (cargo, ~/.cargo/bin) or install.sh (~/.local/bin).
# Only removes the minerve executable; never touches config or history directories.
set -eu
CARGO_BIN="${CARGO_HOME:-"$HOME/.cargo"}/bin/minerve"
LOCAL_BIN="${MINERVE_INSTALL_DIR:-"$HOME/.local/bin"}/minerve"
removed=0
# Use cargo uninstall for the cargo copy so cargo's .crates.toml metadata stays in sync.
if [ -f "$CARGO_BIN" ] && command -v cargo >/dev/null 2>&1; then
    cargo uninstall minerve
    removed=$((removed + 1))
fi
if [ -f "$LOCAL_BIN" ] && [ ! -L "$LOCAL_BIN" ]; then
    rm -f "$LOCAL_BIN"
    echo "Removed $LOCAL_BIN"
    removed=$((removed + 1))
fi
if [ "$removed" -eq 0 ]; then
    echo 'No minerve binary found in ~/.cargo/bin or ~/.local/bin'
fi
if command -v minerve >/dev/null 2>&1; then
    echo "Warning: minerve is still on PATH at $(command -v minerve)" >&2
    exit 1
fi
echo 'Minerve uninstalled.'
