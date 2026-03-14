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
- Chat-assistant style orchestration through adapters, including OpenClaw-like usage patterns.
- Optional full automation loops via custom `approval-agent` policies when human gates are not required.
- Domain-specific workflows (ML training, trading pipeline, onboarding automation).

See [Design](docs/DESIGN.md) for architecture, config schema, and invariants.

## Documentation

- [Design](docs/DESIGN.md) — architecture, core concepts, config schema
- [Core Protocol](docs/CORE_PROTOCOL.md) — stdio protocol between core and orchestrator
- [Orchestrator Protocol](docs/ORCHESTRATOR_PROTOCOL.md) — protocol between orchestrator and worker subagents
- [Agent Conventions](docs/CODING_CONVENTIONS.md) — conventions for agents developing this project
- [Releasing](docs/RELEASING.md) — maintainer release and Homebrew tap automation flow
