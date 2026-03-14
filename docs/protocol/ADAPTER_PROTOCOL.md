# Adapter Protocol

Status: Proposal
Last updated: March 4, 2026

## 1. Overview

This document defines how an external adapter connects to `kelix` core to provide alternate frontends such as chat bots or web UIs.

See [DESIGN.md](../DESIGN.md) for the overall architecture and [CORE_PROTOCOL.md](CORE_PROTOCOL.md) for the core–orchestrator protocol.

## 2. Headless Mode

In normal operation, core presents a TUI. Adapter-driven runs use `--headless`:

```
kelix --headless start   [--session-id <id>] [--enabled-subagents <name,...>]
kelix --headless resume <id>
```

In headless mode:
- No TUI is rendered.
- Core reads the adapter event stream on stdin and writes it on stdout.
- The orchestrator connection is unchanged and remains internal to core.
- Core stays alive until `session_end` or a signal.

## 3. Adapter–Core Interface

The adapter event stream is a bidirectional newline-delimited JSON channel over core's stdin/stdout in headless mode.

```
adapter process ──stdin──► kelix core (headless)
                ◄──stdout── kelix core (headless)
```

All messages share the same envelope as the core–orchestrator protocol:

```json
{ "id": "<uuid>", "type": "<message-type>", ... }
```

### 3.1 Adapter → Core

#### `user_message`

Submit a user message. Core forwards it to the orchestrator as a `user_input` event (CORE_PROTOCOL.md §5.1).

```json
{
  "id": "adp-001",
  "type": "user_message",
  "text": "Skip the database migration task.",
  "sender_id": "user-42",
  "channel_id": "chan-project-alpha"
}
```

`sender_id` and `channel_id` are opaque. Core forwards them unchanged in `user_input.metadata`.

Response:

```json
{ "id": "adp-001", "type": "user_message_ack" }
```

#### `approval_response`

Submit a human decision in response to an `approval_required` event.

```json
{
  "id": "adp-002",
  "type": "approval_response",
  "request_id": "req-002",
  "choice": "yes"
}
```

`request_id` must match the `id` of a pending `approval_required` event. Core returns an error if no such pending approval exists.

Response: `{ "id": "adp-002", "type": "approval_response_ack" }`.

#### `session_end`

Request a clean shutdown of the core process. Core sends `session_abort` to the orchestrator, marks the session `suspended`, and exits.

```json
{ "id": "adp-003", "type": "session_end" }
```

No response is sent; core exits after cleanup.

#### `debug_mode`

Request a runtime debug-mode change for the current session.

```json
{
  "id": "adp-004",
  "type": "debug_mode",
  "enabled": true
}
```

`enabled` is optional:
- `true`: force debug on
- `false`: force debug off
- omitted: toggle current state

No direct response is sent. Core emits a `notify` event with the effective state.

### 3.2 Core → Adapter

#### `agent_message`

Emitted when the orchestrator sends a `blocked` request that requires human input. The adapter forwards this to the appropriate channel.

```json
{
  "id": "evt-001",
  "type": "agent_message",
  "text": "task/002 has an unresolvable conflict. How should I proceed?",
  "session_id": "sess-abc123",
  "channel_id": "chan-project-alpha"
}
```

`channel_id` is copied from the most recent `user_message` for the session. If none exists yet, it is `null`.

The adapter must reply with `approval_response`, using this event's `id` as `request_id`.

#### `notify`

Emitted when the orchestrator sends `notify` (see CORE_PROTOCOL.md §4.7). No reply is required.

```json
{
  "id": "evt-002",
  "type": "notify",
  "text": "Dispatching task-003 (rate limiter) to coding-agent.",
  "session_id": "sess-abc123",
  "channel_id": "chan-project-alpha",
  "level": "info"
}
```

`level` values: `info` | `warning` | `error`. No reply is expected.

Core also emits `notify` for worker lifecycle transitions. These may include an `event` field for resource accounting.

Worker started (emitted immediately after `spawn_ack`):

```json
{
  "id": "evt-010",
  "type": "notify",
  "level": "info",
  "text": "Worker started: coding-agent (req-011)",
  "session_id": "sess-abc123",
  "channel_id": "chan-project-alpha",
  "event": "worker_started",
  "spawn_id": "req-011",
  "subagent": "coding-agent"
}
```

Worker finished (emitted after `spawn_result` or `spawn_error` is delivered to the orchestrator):

```json
{
  "id": "evt-011",
  "type": "notify",
  "level": "info",
  "text": "Worker finished: coding-agent (req-011), exit_code=0",
  "session_id": "sess-abc123",
  "channel_id": "chan-project-alpha",
  "event": "worker_finished",
  "spawn_id": "req-011",
  "subagent": "coding-agent",
  "exit_code": 0
}
```

`exit_code` is `null` for `spawn_error` (unclean termination).

#### `approval_required`

Emitted when an approval gate is set to `human`. The adapter must surface it and reply with `approval_response`.

```json
{
  "id": "evt-003",
  "type": "approval_required",
  "kind": "merge",
  "message": "Merge task/001 into main?",
  "options": ["yes", "no", "skip"],
  "session_id": "sess-abc123",
  "channel_id": "chan-project-alpha"
}
```

#### `session_complete`

Emitted when the orchestrator sends `complete`. The adapter should notify the channel that the session has finished.

```json
{
  "id": "evt-004",
  "type": "session_complete",
  "summary": "All tasks finished. 3 branches merged to main.",
  "session_id": "sess-abc123"
}
```

#### `session_error`

Emitted when core encounters an unrecoverable error (e.g. repeated orchestrator crashes). The adapter should notify the channel.

```json
{
  "id": "evt-005",
  "type": "session_error",
  "reason": "orchestrator crashed 3 consecutive times",
  "session_id": "sess-abc123"
}
```

## 4. Multi-Session Process Model

Core is single-session: one process manages one session. Multi-session adapters therefore manage one core process per active session.

```
adapter process
    ├── core process  (session sess-abc123, channel chan-project-alpha)
    ├── core process  (session sess-def456, channel chan-project-beta)
    └── core process  (session sess-ghi789, channel chan-project-gamma)
```

The adapter is responsible for:

- **Process lifecycle**: spawning `kelix --headless start` or `resume <id>`, and tracking PIDs.
- **Routing inbound messages**: mapping each incoming chat message to the correct core process by `channel_id` (or equivalent platform identifier).
- **Routing outbound events**: forwarding `agent_message`, `notify`, `approval_required`, and `session_complete` events from each core process to the correct channel.
- **Crash detection**: detecting when a core process exits unexpectedly and deciding whether to restart it (e.g. by running `kelix --headless resume <id>` again).

Core processes are independent. The adapter is the only component with a cross-session view.

**Session discovery.** The adapter may use `kelix list --json` to enumerate known sessions (reads the session index at `~/.kelix/sessions/index.json` by default, or `$KELIX_DATA_DIR/sessions/index.json` if `KELIX_DATA_DIR` is set) and resume any `suspended` sessions on startup.

### 4.1 Resource Management

Resource limits operate at three layers:

| Layer | Mechanism | Controls |
|-------|-----------|---------|
| Container runtime (podman/k8s) | `--cpus`, `--memory` per container | Hard per-worker CPU and memory cap |
| Core config (`[agent]`) | `max_concurrent_spawns`, `max_spawns`, `max_wall_time_secs` | Per-session worker concurrency and lifetime |
| Adapter | Session queue + global worker counter | Cross-session totals |

The adapter is responsible for enforcing global limits across all sessions. It does so using the `worker_started` and `worker_finished` lifecycle events emitted in `notify` messages (see §3.2).

**Recommended adapter resource model:**

```
global_worker_count: int          # currently running workers across all sessions
active_session_count: int         # currently active core processes
session_start_queue: Queue        # pending start/resume requests waiting for capacity
```

On `worker_started`: increment `global_worker_count`.
On `worker_finished`: decrement `global_worker_count`; drain `session_start_queue` if capacity is available.

**Session admission.** When a new session is requested and `active_session_count >= max_active_sessions`, the adapter queues the start request rather than spawning a new core process immediately. It starts the queued session when an existing session reaches `complete` or `suspended`.

**Global worker cap.** `max_global_workers` is adapter-level, not core-level. Because the adapter cannot intercept in-process `spawn` requests, it enforces this indirectly by setting per-session `max_concurrent_spawns`. A simple static partition is `floor(max_global_workers / max_active_sessions)`.

**Container resource limits.** CPU and memory limits are applied at the container runtime level, not by the adapter. The `command` field in `[subagents.<name>]` config should include the appropriate flags:

```toml
[subagents.coding-agent]
command = "podman run --rm -i --cpus=2 --memory=4g my-coding-agent-image"
```

The container runtime enforces these limits directly; the adapter does not.

### 4.2 Per-Session Subagent Filtering

The adapter may restrict which registered subagents a session can use. `enabled_subagents` filters; it does not register new agents.

The adapter passes the desired subagent list via the `--enabled-subagents` flag when starting a core process:

```
kelix --headless start --enabled-subagents research-agent,knowledge-agent
```

Core computes the effective set as the intersection of registered agents and the supplied list, then exposes it in `session_start.config.subagents`. Spawning an unlisted subagent returns `unknown_subagent`.

If `--enabled-subagents` is omitted, all registered subagents are available (existing behavior).

On `resume`, the flag is optional. If supplied, it overrides the subagent set for the resumed session. If omitted, the subagent set from the original `start` invocation is restored from the session index.

**Typical adapter usage.** The adapter selects `enabled_subagents` based on group or channel configuration:

```python
# group config maps group_id → list of allowed subagents
enabled = group_config[group_id].get("subagents", None)
flag = f"--enabled-subagents {','.join(enabled)}" if enabled else ""
subprocess.Popen(f"kelix --headless start {flag}")
```

This allows different channels to use different capability sets without separate binaries or config files.

## 5. Multi-Agent Routing in a Single Channel

A single chat channel may host multiple sessions. The adapter must route each message to the correct one.

**Recommended routing convention**: messages are addressed to a specific agent by mention or slash command prefix:

```
@keli-alpha implement the login flow
@keli-beta fix the CI pipeline
/alpha: what is the current plan?
```

The adapter extracts the target session from the prefix, strips it, and forwards the remainder as `user_message`.

**Reply attribution**: `agent_message`, `notify`, and `approval_required` include `session_id`. The adapter should label outbound messages so users can distinguish sessions.

**Unaddressed messages**: if a message does not match any routing prefix, the adapter may:
- Ignore it (recommended for channels with heavy non-agent traffic).
- Forward it to a default session (suitable for single-agent channels).
- Reply with a help message listing active agents.

This routing convention is adapter-only; core and orchestrator are unaware of it.

## 6. Adapter Implementation Notes

These are recommendations, not core requirements.

- **Message serialization**: if users send messages faster than the orchestrator can process them, the adapter should queue `user_message` calls and send them to core one at a time (wait for `user_message_ack` before sending the next). Core and the orchestrator process `user_input` events sequentially.
- **Approval timeout**: if a pending `approval_required` event receives no response within a configurable timeout, the adapter may auto-respond with the first option (if `none`-gate semantics are desired) or escalate to a channel admin.
- **Reconnection**: if a core process is restarted (e.g. after a crash), the adapter should re-attach to the new process's stdin/stdout and re-register any pending approvals by re-emitting the stored `approval_required` events to the channel.
- **Idempotency**: the adapter should not re-send a `user_message` that has already been acknowledged, even after a reconnect. Use the `id` field for deduplication.
