#!/usr/bin/env bash
# Install kelix for local development (editable install).
#
# - Builds the binary in release mode
# - Symlinks the binary to ~/.local/bin/kelix
# - Symlinks prompts/, examples/, and docs/ into ~/.local/share/kelix/
#
# After running this script, re-run `cargo build --release` to pick up code
# changes — no reinstall needed because the binary path is symlinked.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${HOME}/.local/share/kelix"

BINS=(kelix kelix-orchestrator kelix-worker weave)

cargo build --release "${BINS[@]/#/--bin=}"

mkdir -p "$BIN_DIR"
for bin in "${BINS[@]}"; do
    ln -sf "${REPO_ROOT}/target/release/${bin}" "${BIN_DIR}/${bin}"
    echo "Linked binary: ${BIN_DIR}/${bin} -> ${REPO_ROOT}/target/release/${bin}"
done

mkdir -p "$DATA_DIR"
ln -sfn "${REPO_ROOT}/prompts"  "${DATA_DIR}/prompts"
ln -sfn "${REPO_ROOT}/examples" "${DATA_DIR}/examples"
ln -sfn "${REPO_ROOT}/docs"     "${DATA_DIR}/docs"
echo "Linked data:   ${DATA_DIR}/prompts  -> ${REPO_ROOT}/prompts"
echo "Linked data:   ${DATA_DIR}/examples -> ${REPO_ROOT}/examples"
echo "Linked data:   ${DATA_DIR}/docs     -> ${REPO_ROOT}/docs"

echo ""
echo "Make sure ${BIN_DIR} is in your PATH."
echo ""
echo "KELIX_HOME is not set automatically. Add to your shell rc:"
echo "  bash/zsh:  export KELIX_HOME=\"${DATA_DIR}\""
echo "  fish:      set -gx KELIX_HOME \"${DATA_DIR}\""
echo ""
echo "Installed: ${BINS[*]}"
