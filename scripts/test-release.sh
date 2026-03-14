#!/usr/bin/env bash
set -euo pipefail

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64)
        TARGET="aarch64-apple-darwin"
        ;;
    Darwin-x86_64)
        TARGET="x86_64-apple-darwin"
        ;;
    Linux-x86_64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
    Linux-aarch64)
        TARGET="aarch64-unknown-linux-gnu"
        ;;
    *)
        TARGET="$(uname -m)-$(uname -s | tr '[:upper:]' '[:lower:]')"
        ;;
esac

OUTPUT_DIR="dist/local-release"
ARCHIVE_DIR="${OUTPUT_DIR}/archive"
UNPACK_DIR="${OUTPUT_DIR}/bundle"

cargo build --release --bin kelix --bin kelix-orchestrator --bin kelix-worker --bin weave

mkdir -p "$ARCHIVE_DIR"
rm -rf "$UNPACK_DIR"
mkdir -p "$UNPACK_DIR"

bash scripts/package-release.sh \
    --target "$TARGET" \
    --version "test" \
    --output-dir "$ARCHIVE_DIR"

ARCHIVE_PATH="${ARCHIVE_DIR}/kelix-test-${TARGET}.tar.gz"

if [[ ! -f "$ARCHIVE_PATH" ]]; then
    echo "expected archive not found: $ARCHIVE_PATH" >&2
    exit 1
fi

tar -xzf "$ARCHIVE_PATH" -C "$UNPACK_DIR"

echo "Archive: ${ARCHIVE_PATH}"
echo "Bundle:  ${UNPACK_DIR}/kelix"
echo ""
echo "Run it with:"
echo "  cd ${UNPACK_DIR}/kelix"
echo "  ./bin/kelix start examples/codex-onboarding/kelix.toml
echo "  # or:"
echo "  ./bin/kelix start examples/claude-onboarding/kelix.toml
