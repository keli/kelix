# kelix Design

Status: Proposal
Last updated: March 5, 2026

## 1. Purpose

`kelix` is a meta-agent runtime for orchestrating subagents through shell execution.

This document is intentionally high-level:

- It explains architecture, boundaries, and invariants.
- It does not duplicate wire-level protocol or message-field details.

Protocol specifics live in:

- [INFRA_BOOTSTRAP.md](INFRA_BOOTSTRAP.md)
- [protocol/CORE_PROTOCOL.md](protocol/CORE_PROTOCOL.md)
- [protocol/ADAPTER_PROTOCOL.md](protocol/ADAPTER_PROTOCOL.md)
- [protocol/ORCHESTRATOR_PROTOCOL.md](protocol/ORCHESTRATOR_PROTOCOL.md)
- [protocol/INFRA_BOOTSTRAP_PROTOCOL.md](protocol/INFRA_BOOTSTRAP_PROTOCOL.md)

## 2. Goals

- Keep core minimal, auditable, and tool-agnostic.
- Support long-lived, resumable sessions.
- Make autonomy policy-driven (`human`, `agent`, `none` gates).
- Keep deployment concerns (sandboxing, runtime hardening) outside core.
- Stay cross-platform (Linux, macOS, Windows) where practical.

## 3. Architecture

The system is a strict layered stack; each layer speaks only to adjacent layers.

```text
external world (chat, webhook, cron, TUI)
─────────────────────────────── platform protocol
adapter
─────────────────────────────── ADAPTER_PROTOCOL
core
─────────────────────────────── CORE_PROTOCOL
orchestrator
─────────────────────────────── ORCHESTRATOR_PROTOCOL
workers
─────────────────────────────── shell (exec + stdout)
container runtime / OS
```

Layer roles:

- `adapter`: channel integration, routing, multi-session process management.
- `core`: lifecycle control, policy enforcement, spawn/cancel execution, frontend plumbing.
- `orchestrator`: planning, dispatch, retries, recovery logic, work-item decisions.
- `workers`: task execution only (typically stateless per invocation).

## 4. Design Invariants

- Core exposes shell execution as its only execution primitive.
- Exactly one orchestrator is active per session at a time (handover allowed).
- Core enforces policy; orchestrator and workers own task intelligence.
- Session state is durable and resumable.
- Shared session infrastructure must be explicit before dependent work runs.
- Prompt conventions (including `@chunk`) are prompt-layer policy, not core semantics.

## 5. Responsibility Boundary

Core is responsible for:

- session lifecycle and indexing
- orchestrator process supervision
- worker spawn/cancel execution
- shell gate enforcement
- approval routing and limit enforcement

Orchestrator and workers are responsible for:

- planning and decomposition
- execution/retry/review strategy
- work-item state transitions beyond protocol minimum
- domain-specific recovery behavior and handover content

## 6. Session Model (High-Level)

A session is the durable unit of orchestration (`start`, `resume`, `list`).

High-level states:

- `active`
- `suspended`
- `complete`

Work is tracked as work items (for long-running, multi-goal sessions), while task-level behavior is orchestrator-defined.

For state schemas and message sequencing, see the protocol docs.

## 7. Configuration Model (High-Level)

Core consumes:

- `[agent]` limits and runtime bounds
- `[subagents.*]` executable registry
- `[tools.shell]` shell gate policy
- `[approval]` gate routing policy
- `[budget]` runtime budget policy

Config defines runtime policy and available executables; behavioral strategy stays in prompts.

## 8. Recovery Model (High-Level)

- Core restarts orchestrator processes on resume/handover according to protocol.
- Orchestrator reconstructs domain/task context from its persisted session artifacts.
- Planned handover and crash recovery share the same re-entry path at the core boundary.

Detailed restart and handover semantics are protocol-defined.

## 9. Non-Goals (Core)

- Built-in MCP support
- Built-in skills system
- Plugin runtime inside core
- Provider-specific orchestration logic in core

These can be implemented at prompt/worker/deployment layers.

## 10. Example Deployments

Concrete deployment examples live in `examples/`:

- [codex-onboarding](../examples/codex-onboarding/)
- [claude-onboarding](../examples/claude-onboarding/)
- [software-project](../examples/software-project/)
- [chat-assistant](../examples/chat-assistant/)
- [infra-management](../examples/infra-management/)
- [ml-training](../examples/ml-training/)
- [trading-pipeline](../examples/trading-pipeline/)
- [claude-codex-team](../examples/claude-codex-team/)
