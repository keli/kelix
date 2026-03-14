#!/usr/bin/env bash

set -euo pipefail

AUTH_ROOT="${AGENT_AUTH_DIR:-/auth}"
export XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
export XDG_DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$HOME/.cache}"

mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

# @chunk entrypoint/prompts-guard
# Prompts are bundled into the image at /prompts. Users may override by
# mounting their own /prompts. Fail fast if required prompt files are missing.
verify_prompts() {
    local prompts_root="/prompts"
    local required=(
        "orchestrator.md"
        "coding-agent.md"
        "coding-guardrails.md"
    )

    if [[ ! -d "$prompts_root" ]]; then
        echo "ERROR: missing prompts directory: $prompts_root" >&2
        exit 1
    fi

    for rel in "${required[@]}"; do
        if [[ ! -f "$prompts_root/$rel" ]]; then
            echo "ERROR: missing required prompt file: $prompts_root/$rel" >&2
            exit 1
        fi
    done
}
# @end-chunk

verify_prompts


# @chunk entrypoint/auth-symlink
# Auth material should be mounted and referenced in-place to avoid
# startup copies and preserve live state from the mounted host paths.
link_auth() {
    local src="$1"
    local dest="$2"
    if [[ -e "$src" ]]; then
        mkdir -p "$(dirname "$dest")"
        rm -rf "$dest"
        ln -s "$src" "$dest"
    fi
}

link_auth "$AUTH_ROOT/claude" "$HOME/.claude"
link_auth "$AUTH_ROOT/claude.json" "$HOME/.claude.json"
link_auth "$AUTH_ROOT/codex" "$HOME/.codex"
link_auth "$AUTH_ROOT/codex-config" "$XDG_CONFIG_HOME/codex"
link_auth "$AUTH_ROOT/openai-config" "$XDG_CONFIG_HOME/openai"
link_auth "$AUTH_ROOT/codex-share" "$XDG_DATA_HOME/codex"

# @chunk entrypoint/home-overlay
# Mount arbitrary files/dirs into $HOME by placing them under /auth/home/.
# e.g. -v $HOME/.gitconfig:/auth/home/.gitconfig:ro
#      -v $HOME/.ssh:/auth/home/.ssh:ro
# Each entry is symlinked into the container user's $HOME automatically.
if [[ -d "$AUTH_ROOT/home" ]]; then
    for src in "$AUTH_ROOT/home"/* "$AUTH_ROOT/home"/.[!.]*; do
        [[ -e "$src" ]] || continue
        link_auth "$src" "$HOME/$(basename "$src")"
    done
fi
# @end-chunk

if [[ "$#" -eq 0 ]]; then
    exec bash
fi

exec "$@"
