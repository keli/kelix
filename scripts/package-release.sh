#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage:
  scripts/package-release.sh --target <triple> [options]

Options:
  --target <triple>         Rust target triple used in the archive name
  --version <value>         Version label in the archive name (default: dev)
  --output-dir <path>       Output directory for archives (default: dist)
  --bundle-name <name>      Top-level bundle directory name (default: kelix)

The generated archive layout is:
  <bundle-name>-<version>-<target>.tar.gz
    <bundle-name>/
      bin/kelix
      bin/kelix-orchestrator
      bin/kelix-worker
      bin/weave
      README.md
      docs/
      examples/
      prompts/
EOF
}

TARGET=""
VERSION="dev"
OUTPUT_DIR="dist"
BUNDLE_NAME="kelix"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# All binaries to include in the release bundle.
BINS=(kelix kelix-orchestrator kelix-worker weave)

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            TARGET="${2:-}"
            shift 2
            ;;
        --version)
            VERSION="${2:-}"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="${2:-}"
            shift 2
            ;;
        --bundle-name)
            BUNDLE_NAME="${2:-}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "$TARGET" ]]; then
    echo "--target is required" >&2
    usage >&2
    exit 1
fi

# Resolve build output directory from target triple. Prefer target-specific
# output produced by `cargo build --target <triple>`, then fall back to host
# target output for local/manual packaging workflows.
BIN_DIR="${REPO_ROOT}/target/${TARGET}/release"
if [[ ! -d "$BIN_DIR" ]]; then
    BIN_DIR="${REPO_ROOT}/target/release"
fi

# Windows targets use `.exe` binaries regardless of the host OS.
EXT=""
[[ "$TARGET" == *-windows-* ]] && EXT=".exe"

for bin in "${BINS[@]}"; do
    if [[ ! -f "${BIN_DIR}/${bin}${EXT}" ]]; then
        echo "binary not found: ${BIN_DIR}/${bin}${EXT}" >&2
        exit 1
    fi
done

if [[ ! -d "${REPO_ROOT}/examples" ]]; then
    echo "missing examples/ directory" >&2
    exit 1
fi

if [[ ! -d "${REPO_ROOT}/prompts" ]]; then
    echo "missing prompts/ directory" >&2
    exit 1
fi

if [[ ! -d "${REPO_ROOT}/docs" ]]; then
    echo "missing docs/ directory" >&2
    exit 1
fi

mkdir -p "$OUTPUT_DIR"

ARCHIVE_BASENAME="${BUNDLE_NAME}-${VERSION}-${TARGET}"
STAGE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/kelix-package.XXXXXX")"
ROOT_DIR="${STAGE_DIR}/${BUNDLE_NAME}"

cleanup() {
    rm -rf "$STAGE_DIR"
}
trap cleanup EXIT

mkdir -p "${ROOT_DIR}/bin"
for bin in "${BINS[@]}"; do
    cp "${BIN_DIR}/${bin}${EXT}" "${ROOT_DIR}/bin/${bin}${EXT}"
done
cp "${REPO_ROOT}/README.md" "${ROOT_DIR}/README.md"
cp -R "${REPO_ROOT}/docs" "${ROOT_DIR}/docs"
cp -R "${REPO_ROOT}/examples" "${ROOT_DIR}/examples"
cp -R "${REPO_ROOT}/prompts" "${ROOT_DIR}/prompts"

ARCHIVE_PATH="${OUTPUT_DIR}/${ARCHIVE_BASENAME}.tar.gz"
tar -C "$STAGE_DIR" -czf "$ARCHIVE_PATH" "$BUNDLE_NAME"

echo "Created ${ARCHIVE_PATH}"
