# OpenCode Team

Local profile using OpenCode as both the orchestrator and worker backend.

## Prerequisites

- `podman`
- a built `kelix` binary
- a built `kelix:latest` image
- provider API keys exported in your host shell (for example `OPENAI_API_KEY`)
- model defaults set in `examples/opencode-team/opencode.json`

Build from repo root:

```sh
./docker/build.sh
cargo build --release
```

## Authentication

OpenCode authentication depends on the LLM provider you configure inside the container. Set the appropriate environment variable:

```sh
# Examples
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
export GOOGLE_GENERATIVE_AI_API_KEY=...
export OPENROUTER_API_KEY=...

# kelix.toml passes these variables through to each container.
```

## Model Selection

This example mounts `examples/opencode-team/opencode.json` into `/workspace/opencode.json`
for orchestrator and worker containers.

Edit that file to choose the default model:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "model": "openai/gpt-5",
  "small_model": "openai/gpt-5-nano"
}
```

To inspect available model IDs:

```sh
podman run --rm --env OPENAI_API_KEY=$OPENAI_API_KEY kelix:latest opencode models --refresh
podman run --rm --env OPENAI_API_KEY=$OPENAI_API_KEY kelix:latest opencode models openai
```

## Start

```sh
./target/release/kelix start examples/opencode-team/kelix.toml --prompt "your task here"
```

## How It Works

| Role | Backend | CLI invoked |
|------|---------|-------------|
| orchestrator | `kelix-orchestrator --agent opencode` | `opencode run "<prompt>"` per turn |
| coding-agent | `kelix-worker --agent opencode` | `opencode run "<prompt>"` |
| review-agent | `kelix-worker --agent opencode` | `opencode run "<prompt>"` |

The orchestrator uses a stateless-turn model: each incoming message from kelix core
triggers a new `opencode run` invocation whose prompt includes the accumulated
conversation history. This matches the Codex backend model.

The worker backend collects stdout from `opencode run`, tries to parse the last
non-empty line as a JSON `WorkerResult`, and falls back to treating the full output
as a plain-text summary.

## Troubleshooting

1. Confirm `opencode run "hello"` works on the host before using it inside a container.
2. Check that auth credentials are mounted or injected via environment variables.
3. If the orchestrator starts but workers fail, fix the worker config and resume the session.
4. Enable debug mode for verbose I/O diagnostics:

```sh
./target/release/kelix --debug start examples/opencode-team/kelix.toml --prompt "test"
```
