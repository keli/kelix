# Claude + Codex Team

Local profile:

- `Claude Code` as orchestrator
- `Codex` as coding-agent
- `Claude Code` as review-agent

## Prerequisites

You need:

- `podman`
- a built `kelix` binary
- a built `kelix:latest` image
- working local auth for `Codex` and `Claude Code`, or valid API keys
- `~/.gitconfig` with `user.name` and `user.email` set (mounted read-only into each container for git identity)

Build from repo root:

```sh
./docker/build.sh
cargo build --release
```

## Start

From repo root:

```sh
./target/release/kelix start examples/claude-codex-team/kelix.toml --prompt "Help me onboard this workspace"
```

This uses the default TUI.


## Adjust The Config

Start from [kelix.toml](./kelix.toml).

Usually you only need to change:

- auth mounts
- whether to use API keys or local-login mounts
- enabled subagents
- approval policy

Use `Codex` or `Claude Code` directly to adapt the file for your machine.

Suggested prompt:

```text
Modify examples/claude-codex-team/kelix.toml for my machine.

Keep:
- Claude Code as orchestrator
- Codex as coding-agent
- Claude Code as review-agent

Requirements:
- use only auth paths that exist on my machine
- prefer minimal changes
- do not require knowledge-agent

Before editing, inspect:
- docs/DESIGN.md
- docs/CODING_CONVENTIONS.md
- examples/claude-codex-team/kelix.toml
```

## Troubleshooting

If startup fails, check in this order:

1. `Codex` works on the host by itself.
2. `Claude Code` works on the host by itself.
3. The auth paths in `kelix.toml` actually exist.
4. `podman` can run `kelix:latest`.

If orchestrator starts but a worker fails, fix the worker config and restart the session. The first priority is getting the orchestrator up.

For detailed runtime diagnostics, enable debug mode:

```sh
./target/release/kelix --debug start examples/claude-codex-team/kelix.toml --prompt "Help me onboard this workspace"
```

or:

```sh
KELIX_DEBUG=1 ./target/release/kelix start examples/claude-codex-team/kelix.toml --prompt "Help me onboard this workspace"
```

Debug mode prints verbose orchestrator I/O and process diagnostics to stderr.
