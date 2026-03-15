# Example: Multi-Group Chat Assistant

Status: Proposal
Last updated: February 24, 2026

## Overview

This example describes how to build a chat assistant similar to OpenClaw on top of kelix: one persistent session per group or channel, reachable via existing messaging platforms (Telegram, Slack, Discord, etc.), with simple questions answered directly by the orchestrator and complex tasks handled as short-lived work items delegated to worker subagents.

## Architecture

```
Telegram / Slack / Discord
        │
    adapter process
  ┌─────────────────────────────────────────┐
  │  group A → core (sess-A) → orchestrator │
  │  group B → core (sess-B) → orchestrator │
  │  group C → core (sess-C) → orchestrator │
  └─────────────────────────────────────────┘
        │
   (per session, on demand)
   planning-agent / coding-agent / research-agent / ...
```

One core process per group. The adapter maps each incoming message to the correct core process by group/channel ID and routes replies back.

## Session Lifecycle

- **First message from a group**: adapter runs `kelix --headless start --enabled-subagents <group-subagents>` and records the resulting `session_id` against the group ID.
- **Subsequent messages**: adapter routes to the existing core process via its stdin.
- **Adapter restart**: adapter calls `kelix list --json` on startup, finds all `suspended` sessions, and resumes each with `kelix --headless resume <id>`.
- **Idle groups**: sessions remain `active` indefinitely (orchestrator is long-lived). If the host machine restarts, sessions are automatically resumed on adapter startup.

## Simple vs. Complex Requests

The orchestrator handles simple questions directly without spawning any worker:

- "What time is it in Tokyo?" → orchestrator replies inline.
- "Summarize what we discussed last week" → orchestrator reads session history and replies.

Complex requests trigger worker subagents and usually create a new work item within the group session:

- "Research the latest papers on RAG" → orchestrator spawns research-agent.
- "Write a script to parse these logs" → orchestrator opens a work item for the request, then may spawn planning-agent followed by coding-agent.

No code change is needed to distinguish these cases — the orchestrator's system prompt defines the threshold. See `prompts/orchestrator.md`.

## Per-Group Subagent Configuration

Different groups may have access to different capabilities. The adapter selects `--enabled-subagents` at session start based on group configuration:

```toml
# adapter config (not kelix core config)
[groups.dev-team]
subagents = ["research-agent", "coding-agent", "review-agent", "knowledge-agent"]

[groups.ops-team]
subagents = ["research-agent", "knowledge-agent"]

[groups.general]
# no entry → all registered subagents available
```

All subagents listed here must be registered in the core config. The adapter passes the intersection as `--enabled-subagents` when starting the session.

## Adapter Resource Configuration

```toml
# adapter config
max_active_sessions   = 50   # maximum simultaneously active core processes
max_global_workers    = 20   # maximum simultaneously running worker containers
```

The adapter enforces `max_active_sessions` via session admission queuing (see ADAPTER_PROTOCOL.md §4.1). `max_global_workers` is enforced indirectly by setting `max_concurrent_spawns` in core config before session start:

```
max_concurrent_spawns = floor(max_global_workers / active_session_count)
```

## Core Config

```toml
[agent]
max_spawns            = 100
max_concurrent_spawns = 4    # set dynamically by adapter at session start
max_wall_time_secs    = 0    # sessions are long-lived; no wall-clock limit

[subagents.orchestrator]
start_command = "podman run --rm -i my-orchestrator-image"
lifecycle = "session"

[subagents.research-agent]
start_command = "podman run --rm -i --cpus=1 --memory=2g my-research-agent-image"
lifecycle = "task"

[subagents.coding-agent]
start_command = "podman run --rm -i --cpus=2 --memory=4g my-coding-agent-image"
lifecycle = "task"

[subagents.review-agent]
start_command = "podman run --rm -i --cpus=1 --memory=2g my-review-agent-image"
lifecycle = "task"

[subagents.knowledge-agent]
start_command = "podman run --rm -i my-knowledge-agent-image"
lifecycle = "task"
volume    = "knowledge-vol"

[approval]
shell_gate = "none"   # chat assistant: auto-approve shell commands
plan_gate  = "none"   # auto-approve plans
merge_gate = "human"  # require human confirmation before merging code

[budget]
max_tokens        = 500000
on_budget_exceeded = "reject_spawn"  # let orchestrator degrade gracefully
```

## Message Routing (Adapter Pseudocode)

```python
on_message(group_id, sender_id, text):
    session = sessions.get(group_id)
    if session is None:
        subagents = group_config.get(group_id, {}).get("subagents")
        flag = f"--enabled-subagents {','.join(subagents)}" if subagents else ""
        session = start_core(f"kelix --headless start {flag}")
        sessions[group_id] = session

    session.send_user_message(sender_id=sender_id, channel_id=group_id, text=text)

on_core_event(session_id, event):
    group_id = session_map[session_id]
    if event.type == "agent_message":
        send_to_channel(group_id, event.text)
    elif event.type == "approval_required":
        send_to_channel(group_id, format_approval_prompt(event))
    elif event.type == "session_complete":
        send_to_channel(group_id, event.summary)
```
