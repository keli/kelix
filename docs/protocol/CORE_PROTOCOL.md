# Core Protocol

Status: Proposal
Last updated: March 1, 2026

## 1. Overview

This document defines the stdio-based control protocol between kelix core and the orchestrator subagent. See [DESIGN.md](../DESIGN.md) for the overall architecture and [ORCHESTRATOR_PROTOCOL.md](ORCHESTRATOR_PROTOCOL.md) for the protocol between the orchestrator and its workers.

The orchestrator is a long-running process connected to kelix core via stdin/stdout. It communicates by exchanging newline-delimited JSON messages. kelix core is the executor: it acts on requests from the orchestrator and returns results. The orchestrator drives the session; kelix core enforces policy.

This protocol requires the subagent backend to support persistent stdio connections. Async job backends (e.g. cloud batch APIs) are not supported.

Spawn requests are non-blocking: the orchestrator may issue multiple spawns before receiving results. kelix core executes workers concurrently and returns each `spawn_result` as a separate event correlated by request `id`. The orchestrator–core stdio connection remains one-message-at-a-time on the wire; concurrency is at the spawn execution level, not the transport level.

## 2. Transport

- **Encoding**: newline-delimited JSON (one JSON object per line).
- **Orchestrator → kelix**: requests written to stdout.
- **kelix → orchestrator**: responses and events written to stdin.
- **Framing**: each message is a single JSON object terminated by `\n`. No multi-line messages.

## 3. Message Structure

All messages share a common envelope:

```json
{ "id": "<uuid>", "type": "<message-type>", ... }
```

- `id`: unique message ID. Responses echo the request `id` for correlation.
- `type`: determines the message type and payload shape.

## 4. Request Types (orchestrator → kelix)

### 4.1 `spawn`

Launch a subagent. kelix starts the process using the command configured in `[subagents.<name>]`, passes `input` to its stdin, and immediately acknowledges with `spawn_ack`. The actual result is delivered later as a `spawn_result` event (see §5.3) when the worker exits. Multiple spawns may be outstanding simultaneously; each `spawn_result` carries the original request `id` for correlation.

```json
{
  "id": "req-001",
  "type": "spawn",
  "subagent": "coding-agent",
  "input": { "prompt": "...", "context": { ... } }
}
```

Acknowledgement (immediate):

```json
{
  "id": "req-001",
  "type": "spawn_ack"
}
```

If kelix cannot start the worker (e.g. subagent not in config, budget exceeded), it returns an `error` response instead of `spawn_ack`. An `error` here means the worker never started; it does not mean the worker exited with failure.

`error` response shape (used for all request types):

```json
{
  "id": "req-001",
  "type": "error",
  "code": "unknown_subagent",
  "message": "subagent 'foo' is not defined in config"
}
```

`code` values: `unknown_subagent`, `budget_exceeded`, `spawn_limit_exceeded`, `unknown_spawn_id`, `invalid_request`.

### 4.2 `approve`

Surface a decision for approval. kelix routes the request according to the gate configured for `shell_gate` in `[approval]`: `human` blocks until the user responds via TUI; `none` auto-approves immediately with `decided_by: "auto"`. Either way, execution blocks until a decision is returned.

`kind` values:
- `shell`: a shell command requires approval (routed by `shell_gate`)

```json
{
  "id": "req-002",
  "type": "approve",
  "kind": "shell",
  "message": "Run: git push origin task/001?",
  "options": ["yes", "no"]
}
```

Response:

```json
{
  "id": "req-002",
  "type": "approve_result",
  "choice": "yes",
  "decided_by": "human | auto"
}
```

`options` must be a non-empty array of strings. If `options` is missing or empty, kelix returns an `error` response with code `invalid_request` and does not surface the approval.

`blocked` requests (free-form human input required) are always routed to the human. All approval decisions are recorded in the session log.

### 4.3 `config_get`

Read a kelix config value.

```json
{
  "id": "req-003",
  "type": "config_get",
  "key": "tools.shell.timeout_secs"
}
```

Response:

```json
{
  "id": "req-003",
  "type": "config_result",
  "key": "tools.shell.timeout_secs",
  "value": 120
}
```

If the key does not exist in config, `value` is `null`. kelix does not return an error for unknown keys.

### 4.4 `complete`

Signal that the current task is complete. kelix displays the summary to the user and suspends the session. The session remains resumable: if the user sends a follow-up message, core resumes with `recovery: true` and the orchestrator reconstructs context from session state.

```json
{
  "id": "req-005",
  "type": "complete",
  "summary": "All tasks finished. 3 branches merged to main."
}
```

No response is sent; kelix suspends the session and the core process exits after receiving this message.

### 4.5 `blocked`

Signal that the orchestrator cannot proceed and requires human input beyond a simple approval. kelix surfaces the message to the user via TUI and waits for the user to respond with free-form text.

```json
{
  "id": "req-006",
  "type": "blocked",
  "message": "task/002 has an unresolvable merge conflict in src/config.rs. How should I proceed?"
}
```

Response:

```json
{
  "id": "req-006",
  "type": "blocked_result",
  "input": "Discard task/002 and retry with a narrower scope."
}
```

### 4.6 `notify`

Send a progress update or status message that does not require a response. In TUI mode, kelix renders it in the terminal output stream. In headless mode, core forwards it to the adapter as a `notify` event (see ADAPTER_PROTOCOL.md §3.2). `notify` is fire-and-forget: no response is sent.

```json
{
  "id": "req-008",
  "type": "notify",
  "message": "Dispatching task-003 (rate limiter) to coding-agent.",
  "level": "info"
}
```

`level` values: `info` | `warning` | `error`. If absent, treated as `info`.

### 4.7 `cancel_spawn`

Request cancellation of a previously acknowledged spawn. kelix sends SIGTERM to the worker process and waits up to `grace_period_secs` (default: 10) for it to exit cleanly, then sends SIGKILL if still running. The response is returned once the worker process has terminated.

```json
{
  "id": "req-007",
  "type": "cancel_spawn",
  "spawn_id": "req-010",
  "grace_period_secs": 10
}
```

`grace_period_secs` is optional; if omitted, the configured default (10s) is used.

Response:

```json
{
  "id": "req-007",
  "type": "cancel_result",
  "spawn_id": "req-010",
  "status": "cancelled | already_done"
}
```

- `cancelled`: the worker was running and has been terminated. No `spawn_result` will be delivered for this spawn.
- `already_done`: the worker had already exited before the cancel arrived. A `spawn_result` (or `spawn_error`) was or will be delivered normally for this spawn.

If `spawn_id` does not refer to a known in-flight spawn (e.g. already completed, never acknowledged, or unknown), kelix returns an `error` response. `cancel_spawn` is synchronous: the orchestrator must not send another synchronous request until `cancel_result` (or `error`) is received.

## 5. Event Types (kelix → orchestrator, unsolicited)

kelix may send events to the orchestrator outside of request-response cycles.

### 5.1 `user_input`

The user typed a message in the TUI, or an adapter forwarded a message from an external front-end (e.g. a chat platform).

```json
{
  "id": "evt-001",
  "type": "user_input",
  "text": "Actually, skip the database migration task.",
  "metadata": {}
}
```

`metadata` is an optional opaque object. In headless mode, core copies it from the adapter's `user_message` payload (e.g. `sender_id`, `channel_id`). The orchestrator may include `metadata` fields in `blocked` replies so the adapter can route the response to the correct channel. Core does not interpret `metadata`; it is passed through verbatim.

### 5.2 `session_abort`

kelix is shutting down (e.g. user pressed Ctrl-C, `max_spawns` or `max_wall_time_secs` reached). The orchestrator should clean up and exit.

```json
{
  "id": "evt-002",
  "type": "session_abort",
  "reason": "wall_time_exceeded"
}
```

### 5.3 `spawn_result`

Delivered asynchronously when a previously acknowledged spawn completes. The `id` matches the original `spawn` request `id`.

```json
{
  "id": "req-001",
  "type": "spawn_result",
  "exit_code": 0,
  "output": { ... }
}
```

`exit_code` values are defined in ORCHESTRATOR_PROTOCOL.md §5 Worker Output Contract (0 = success, 1 = failure, 2 = blocked). `output` is produced by reading the worker's entire stdout, then attempting to parse it as a single JSON object. If parsing succeeds, `output` is that object. If parsing fails (invalid JSON, empty stdout, or output truncated by `max_output_bytes`), `output` is the raw stdout string. kelix does not attempt line-by-line parsing.

**Truncation.** If the worker's raw stdout exceeds `max_output_bytes`, core truncates it before parsing. Truncation always occurs on a newline boundary: core walks backward from the byte limit to the nearest preceding `\n` and cuts there, ensuring no multi-byte character or line is split mid-stream. If no newline exists within the limit, the output is truncated at the last valid UTF-8 character boundary before the limit. A `truncated: true` field is added to `spawn_result` alongside `output` whenever truncation occurs.

### 5.4 `spawn_error`

Delivered when a worker that was previously acknowledged fails at the process level (crash, signal, unreadable output). Distinct from `exit_code: 1`, which is a clean failure the worker reported itself. The orchestrator should treat `spawn_error` as an unclean failure and may retry or escalate.

```json
{
  "id": "req-001",
  "type": "spawn_error",
  "reason": "worker process terminated with signal 9"
}
```

## 6. Session Lifecycle

Core operates in two modes:

- **TUI mode** (default): renders output to the terminal and reads user input interactively.
- **Headless mode** (`--headless`): suppresses the TUI; exposes a structured JSON event stream on core's own stdin/stdout for an adapter process. See [ADAPTER_PROTOCOL.md](ADAPTER_PROTOCOL.md).

The core–orchestrator protocol (this document) is identical in both modes.

```
kelix [--headless] start / kelix [--headless] resume <id>
    │
    ├── kelix spawns orchestrator process
    ├── kelix sends session_start via stdin:
    │     { "type": "session_start", "prompt": "...", "recovery": false|true,
    │       "session_id": "...", "handover": null|{...}, "config": { ... } }
    │
    ├── orchestrator processes requests, kelix executes them
    │   (spawn → spawn_ack ... spawn_result [async])
    │   (approve / config_get / blocked / cancel_spawn — synchronous)
    │
    ├── orchestrator sends { "type": "complete" }
    │   → session marked suspended; core exits (resumable on next user message)
    │
    ├── kelix sends { "type": "session_abort" }
    │   → session marked suspended; orchestrator should clean up and exit
    │
    ├── [planned handover] orchestrator exits with code 3
    │   ├── core reads handover payload from orchestrator stdout
    │   ├── core buffers pending spawn_result events
    │   └── core re-spawns orchestrator immediately with recovery: true, handover: {...}
    │         (no user prompt; handover counter does NOT increment crash counter)
    │
    └── [crash path] orchestrator exits without "complete" or code 3
            ├── kelix buffers pending spawn_result events
            ├── increments consecutive-crash counter
            ├── if counter < 3: prompts user: Restart? [yes / no / abort]
            │     on yes: re-spawns orchestrator with recovery: true, handover: null
            └── if counter >= 3: requires explicit user action before restart
```

Only `spawn` is asynchronous. All other request types (`approve`, `config_get`, `blocked`, `cancel_spawn`, `complete`) remain synchronous: kelix sends exactly one response before the next request is processed. The orchestrator must not send a second synchronous request until the previous one is acknowledged, but it may send additional `spawn` requests at any time.

The initial `session_start` message is sent by kelix before the orchestrator makes any requests:

```json
{
  "id": "init-000",
  "type": "session_start",
  "prompt": "user's original prompt",
  "config": {
    "subagents": ["orchestrator", "coding-agent", "review-agent", "research-agent", "knowledge-agent"],
    "max_spawns": 0,
    "max_concurrent_spawns": 0,
    "max_wall_time_secs": 0,
    "protocol": {
      "request_types": ["spawn", "approve", "config_get", "complete", "blocked", "notify", "cancel_spawn"],
      "request_fields": {
        "spawn":        ["id", "subagent", "input"],
        "approve":      ["id", "kind", "message", "options"],
        "config_get":   ["id", "key"],
        "complete":     ["id", "summary"],
        "blocked":      ["id", "message"],
        "notify":       ["id", "message"],
        "cancel_spawn": ["id", "spawn_id"]
      },
      "instructions": [
        "Only send requests whose `type` value appears in `config.protocol.request_types`. This list is generated by core and is exhaustive; any other `type` value will be ignored.",
        "For each request type, include only the fields listed in `config.protocol.request_fields[type]`. Do not use any other field names; unrecognised fields cause the message to be dropped.",
        "Only spawn subagents whose name appears in `config.subagents`. Spawn requests for any other name are rejected with `unknown_subagent`."
      ]
    }
  },
  "recovery": false,
  "session_id": "sess-abc123",
  "handover": null
}
```

`config.protocol.request_types` is the exhaustive list of valid `type` values the orchestrator may send to core. This list is generated by core from its internal enum and is the authoritative source — the orchestrator must treat it as such rather than relying on externally maintained documentation or prompts.

`config.protocol.request_fields` maps each type to its required field names. This mapping is also generated by core from its internal enum and is the authoritative source for field names. The orchestrator must use only the field names listed here; any other field names will cause the message to be rejected.

`config.protocol.instructions` is a list of constraint rules generated by core. These rules are the authoritative source for protocol behaviour constraints and take precedence over any description in the orchestrator system prompt. The orchestrator must read and follow them on every `session_start`.

`prompt` is the user's initial goal. When `kelix start` is run without `--prompt` and stdout is a TTY, core collects the prompt interactively via the TUI before spawning the orchestrator; the field is always populated by the time `session_start` is sent. On `resume`, `prompt` is an empty string; the orchestrator reconstructs its working context from `.kelix/session-state.json` and `handover` instead.

`config.subagents` lists the subagents available to this session. Core populates this from the intersection of all registered `[subagents]` entries in config and the `enabled_subagents` list supplied by the adapter at session start (see ADAPTER_PROTOCOL.md §4.2). The orchestrator must only spawn subagents that appear in this list; attempts to spawn unlisted subagents are rejected with `unknown_subagent`.

`recovery` is `false` on a normal session start. It is `true` in all cases where a new orchestrator instance continues a previously started session:

- **Crash recovery**: the orchestrator exited unexpectedly and the user confirmed restart.
- **Session resume**: the user ran `kelix resume <id>` after core itself had exited.
- **Planned handover**: the orchestrator sent exit code 3 to signal it was approaching its context window limit.

In all three cases, core sends `recovery: true` and the original `session_id`. The `handover` field carries the summary payload from the orchestrator's exit output when the cause was a planned handover; it is `null` otherwise. The orchestrator uses this to enter recovery startup (see DESIGN.md §11). If `recovery` is absent, treat it as `false`.

**Planned handover.** When the orchestrator exits with code 3 (the same `handover` exit code used by worker agents), core treats it as a planned context-window handover rather than a crash. Core immediately re-spawns a new orchestrator instance with `recovery: true` and sets the `handover` field in `session_start` to the orchestrator's exit output payload. The new orchestrator reads its persisted state and the `handover` summary to reconstruct working context, then continues. Core does not prompt the user when a handover is planned; it is transparent. Planned handovers do not increment the consecutive-unclean-exit counter (see crash recovery behavior below).

All repo configuration, credentials, working directories, and concrete infra descriptors remain outside the core protocol. They are either provided by the deployment layer or established by the orchestrator when the session adopts a bootstrap workflow, and are not transmitted in `session_start`.

## 7. Policy Enforcement

kelix enforces policy on all requests:

- `spawn`: subagent name must be in `[subagents]` config. Unknown subagents are rejected.
- `approve` routing: the only supported `kind` is `shell`. kelix routes it according to `shell_gate` in `[approval]` config (`human` or `none`).
- Result gates: after a worker exits, if `[approval.result_gates.<subagent-name>]` is configured, kelix intercepts the `spawn_result` before delivering it to the orchestrator and routes it through the configured gate. See §7.1.
- Budget: if cumulative token usage, as tracked by kelix from worker `output` payloads that include a `usage` field, exceeds `[budget].max_tokens`, kelix rejects further `spawn` requests with a `budget_exceeded` error response and sends `session_abort`. If `[budget].on_budget_exceeded` is `reject_spawn`, kelix returns an error response to the spawn but does not send `session_abort`; the orchestrator may handle the refusal and continue non-spawn operations (e.g. `complete` or `blocked`). After sending `session_abort`, all subsequent `spawn` requests are rejected immediately regardless of message ordering.
- Spawn limit: if `max_spawns` is non-zero and the total number of acknowledged spawns reaches it, kelix rejects further `spawn` requests with a `spawn_limit_exceeded` error and sends `session_abort`.
- Wall-clock limit: if `max_wall_time_secs` is non-zero and the session has been running for that duration, kelix sends `session_abort` with reason `wall_time_exceeded` and ignores further requests.

Policy violations return an `error` response and do not terminate the session; the orchestrator decides how to handle them.

### 7.1 Result gates

Result gates let the user configure whether a subagent's output must pass an approval step before the orchestrator receives it. This is the mechanism for wiring review-agents, human checkpoints, or other approval subagents into the spawn result flow without baking those concepts into the core protocol.

Configuration (in `kelix.toml`):

```toml
[approval.result_gates.coding-agent]
gate = "agent:review-agent"   # human | agent:<subagent-name> | none (default)
```

Gate values:
- `none` (default): `spawn_result` is delivered to the orchestrator immediately.
- `human`: core intercepts the result, presents the worker's summary and output to the user via TUI, and waits for confirmation. On confirm: delivers the original `spawn_result`. On deny: delivers a `spawn_result` with `exit_code: 1` and a failure output indicating the result was rejected by the user.
- `agent:<name>`: core intercepts the result, spawns the named subagent with the original worker output as its stdin input, and waits for that subagent to exit. If the gate agent exits with code 0 (success): delivers the original `spawn_result`. If the gate agent exits with non-zero: delivers a `spawn_result` with `exit_code: 1` whose output includes the gate agent's output as the error context.

The gate agent receives the intercepted `spawn_result` output as its stdin. It must produce a standard worker result (see ORCHESTRATOR_PROTOCOL.md §5). The orchestrator does not see the gate agent spawn — it is handled entirely by core.

Cycle prevention: a subagent may not be its own gate agent. If `[approval.result_gates.X]` sets `gate = "agent:X"`, kelix rejects the config at startup with a descriptive error. Gate agent spawns are not themselves subject to result gate interception, preventing chains.

### Shell command execution

Preferred input is an argv array (`["git", "clone", "url"]`), executed directly without a shell where the platform supports it. String-form commands are passed through shell-like tokenization and then executed directly (not via a shell).

Shell gate policy applies only to commands executed through this core shell execution path (`approve kind="shell"`). It does not constrain commands an agent process may execute directly in its own runtime environment.

For string-form commands, core expands `$VAR` and `${VAR}` references from the process environment before execution. Unset variables expand to an empty string. The tilde shorthands `~` and `~/...` are expanded using `HOME`. If `$KELIX_HOME` is unset, core derives it from executable-relative candidates (bundle root next to `bin/`, then package-style `share/kelix` paths) and prefers the first location that contains bundled kelix assets (`prompts/`, `examples/`, or `docs/`). Core does not invoke a shell and does not support broader shell semantics such as command substitution, pipelines, globbing, or redirection.

Allowlist matching is command-level only: for argv input, match the first element; for string input, match the first whitespace-delimited token. If `git` is allowed, any `git` subcommand is allowed. Finer restrictions belong in the runtime or host policy, not core.
