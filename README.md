# kelix

A general-purpose meta-agent that orchestrates subagents to complete any task.

## Overview

`kelix` is a protocol-first runtime for long-running multi-agent execution. It intentionally separates **core control-plane responsibilities** from **agent intelligence**, so the runtime can evolve with SOTA models/agent frameworks without rewriting core safety and lifecycle logic, while directly tackling the inherently non-deterministic nature of LLM behavior.

## Quick Install (macOS)

Install with Homebrew:

```sh
brew tap keli/kelix
brew install kelix
```

Prerequisites:

- For containerized subagent configs (recommended examples): Podman installed and running.
- Install and authenticate at least one agent runtime on your machine (Claude Code or Codex recommended; OpenCode is optional and currently less validated in this repo).

```sh
brew install podman
podman machine init
podman machine start
```

Verify:

```sh
kelix --help
```

Use an onboarding example to generate a project-specific config first, then switch to a fuller profile if needed.

## Quick Start Profiles

Before running a profile, read the profile-specific README under `examples/<profile>/README.md` (especially `examples/codex-onboarding/README.md` and `examples/claude-onboarding/README.md`) to confirm auth, mounts, and backend prerequisites.

List available bundled example configs:

```sh
kelix start --list-examples
```

Start directly from an example alias (no path lookup needed):

```sh
kelix start --example codex-onboarding
kelix start --example claude-onboarding
```

Try the Claude + Codex team profile (from repo root):

```sh
kelix start --example claude-codex-team
```

`claude-codex-team` uses:

- Claude Code as orchestrator
- Codex as coding-agent
- Claude Code as review-agent


## Telegram Adapter

The Telegram adapter lets you drive a kelix session from a Telegram group chat.

### Setup

1. Create a bot via [@BotFather](https://t.me/BotFather) and copy the token. Disable Group Privacy so the bot can receive all group messages: send `/setprivacy` to BotFather, select your bot, and choose **Disable**. 
2. Start a kelix session locally with the session name you want to use:

```sh
kelix start path/to/config.toml --session my-project
```

3. Create a Telegram group and set its title to exactly the session ID (e.g. `my-project`), then add the bot to the group.
4. Start the adapter:

```sh
export TELEGRAM_BOT_TOKEN=<your-token>
kelix adapter
```

Or pass the token directly:

```sh
kelix adapter --bot-token <your-token>
```

The adapter auto-binds a group to a session when the group title matches a known session ID. Send `/rebind` in the group to re-trigger binding after a rename.

### First-run: claiming the whitelist

On first run with no whitelist set, the adapter prints a one-time claim code:

```
kelix-adapter: whitelist is empty
kelix-adapter: send '/claim abc123def456' to the bot to claim admin access
```

Send `/claim <code>` to the bot (in the group or as a direct message). The first user to send the correct code is added to the whitelist. The code expires when the adapter process exits.

### Usage

Once claimed, whitelisted users can interact in the group:

| Command | Description |
|---|---|
| Any text message | Sent as input to the bound session |
| `/ask <text>` or `/run <text>` | Explicit prompt (useful to avoid mention stripping) |
| `@bot <text>` | Mention-style prompt |
| `/status` | Show which session the group is bound to |
| `/rebind` | Re-bind to a session matching the group title |
| `/approve <request_id> <choice\|index>` | Respond to an approval request |
| `/claim <code>` | Claim whitelist on first run |
| `/help` | List commands |

Only text messages are forwarded; voice, images, and other media are silently ignored.

### Resetting state

To clear all chat bindings and the whitelist:

```sh
kelix adapter --reset
```

State is stored at `~/.kelix/adapters/telegram-state.json` by default. Use `--state-path` to override.

## Build from Source

The reference implementation is written in Rust.

**Requirements:** Rust (see `rust-toolchain.toml`)

```sh
cargo build --release
cargo test
./target/release/kelix start path/to/config.toml
./target/release/kelix resume <session-id>
./target/release/kelix list
```

`kelix core ...` is kept as an advanced/debug namespace when you need explicit core flags (for example `--tui` or `--debug`).

## Design Notes

Design principles and why they matter:

- Keep core small and deterministic (lifecycle, policy gates, approvals, spawn/cancel, durable session state) so failures are easier to reason about and recover.
- Keep task intelligence in orchestrator/workers/prompts so strategy can change without destabilizing core runtime behavior.
- Use one execution primitive (shell command execution) so policy enforcement, auditing, and sandbox boundaries are uniform.
- Use structured protocol contracts (NDJSON messages, explicit worker status/failure categories, fixed exit codes) so non-deterministic model outputs are normalized into predictable control flow.
- Make handover/recovery first-class so long-running sessions can survive crashes, restarts, and context limits.
- Prefer native agent runtimes (Claude Code, Codex, OpenCode) so `kelix` can track SOTA tooling with low integration churn.
- Use literate `@chunk` annotations with `weave` extraction so code knowledge remains queryable for review, RAG, and ongoing maintenance automation.
- Keep infra bootstrap and deployment policy explicit in workflow design so teams can integrate existing DevOps/IaC processes instead of rewriting them.

Target workflow directions (see `examples/` for current templates and design sketches):

- Software project execution with planner/coder/reviewer loops.
- Infra and DevOps change management with explicit risk gates.
- Chat-assistant style orchestration through adapters (currently Telegram in-tree).
- Optional full automation loops via custom `approval-agent` policies when human gates are not required.
- Domain-specific workflows (ML training, trading pipeline, onboarding automation).

See [Design](docs/DESIGN.md) for architecture, config schema, and invariants.

## Documentation

- [Design](docs/DESIGN.md) — architecture, core concepts, config schema
- [Core Protocol](docs/CORE_PROTOCOL.md) — stdio protocol between core and orchestrator
- [Orchestrator Protocol](docs/ORCHESTRATOR_PROTOCOL.md) — protocol between orchestrator and worker subagents
- [Agent Conventions](docs/CODING_CONVENTIONS.md) — conventions for agents developing this project
- [Releasing](docs/RELEASING.md) — maintainer release and Homebrew tap automation flow
