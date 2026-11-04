#!/bin/sh
# Package a built minerve binary into a release archive plus its SHA-256 checksum.
# Usage: scripts/publish-flow.sh <target> <binary> <output-dir>
# Output: <output-dir>/minerve-<target>.tar.gz and <output-dir>/minerve-<target>.tar.gz.sha256
set -eu
TARGET=$1
BINARY=$2
OUTPUT_DIR=$3
ROOT=$(cd "$(dirname "$0")/.." && pwd)
test -f "$BINARY" || { echo "Missing executable: $BINARY" >&2; exit 1; }
test -f "$ROOT/LICENSE" || { echo "Missing LICENSE" >&2; exit 1; }
if command -v sha256sum >/dev/null; then
    SUM=sha256sum
else
    SUM='shasum -a 256'
fi
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' 0
# Explicit file list: no source tree, build cache, credentials or .env.
install -m 0755 "$BINARY" "$STAGE/minerve"
install -m 0644 "$ROOT/LICENSE" "$STAGE/LICENSE"
mkdir -p "$OUTPUT_DIR"
ARCHIVE="minerve-$TARGET.tar.gz"
tar -czf "$OUTPUT_DIR/$ARCHIVE" -C "$STAGE" --owner=0 --group=0 minerve LICENSE
(cd "$OUTPUT_DIR" && $SUM "$ARCHIVE" > "$ARCHIVE.sha256")
echo "$OUTPUT_DIR/$ARCHIVE"
