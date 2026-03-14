#!/usr/bin/env bash
# Build the default unified kelix container image.
# Run from the repository root: ./docker/build.sh
# Optional: pass --all-targets to also tag the legacy per-role images.
# Requires: podman

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "${SCRIPT_DIR}")"
BUILD_ALL_TARGETS="${1:-}"

echo "Building kelix container image..."

podman build \
    --file "${SCRIPT_DIR}/Dockerfile.agent-runner" \
    --target agent-runner \
    --tag kelix:latest \
    "${REPO_ROOT}"

echo "Built kelix:latest"

if [[ "${BUILD_ALL_TARGETS}" == "--all-targets" ]]; then
    podman tag kelix:latest kelix-orchestrator:latest
    podman tag kelix:latest kelix-worker-claude:latest
    podman tag kelix:latest kelix-worker-codex:latest

    echo "Tagged aliases:"
    echo "  kelix-orchestrator:latest"
    echo "  kelix-worker-claude:latest"
    echo "  kelix-worker-codex:latest"
fi

echo ""
echo "Pruning dangling images..."
podman image prune --force --filter dangling=true >/dev/null
echo "Dangling image cleanup complete."

echo ""
echo "Image build complete. Verify with: podman images | grep keli"
echo ""

