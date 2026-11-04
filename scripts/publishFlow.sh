#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
if [[ $# -gt 1 || ( $# -eq 1 && $1 != --resume ) ]]; then
    echo 'Usage: scripts/publishFlow.sh [--resume]' >&2
    exit 2
fi
# release.py needs Python 3.11+ (tomllib, hashlib.file_digest); the default python3 may be older.
for candidate in python3.14 python3.13 python3.12 python3.11 python3; do
    if command -v "$candidate" >/dev/null 2>&1 \
        && "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
        exec "$candidate" scripts/release/release.py publish "$@"
    fi
done
if command -v uv >/dev/null 2>&1; then
    exec uv run --python 3.12 python scripts/release/release.py publish "$@"
fi
echo 'Python 3.11+ is required: install it or uv (https://docs.astral.sh/uv/).' >&2
exit 1
