# Example: Telegram Chat Assistant

Status: Example
Last updated: March 15, 2026

## Overview

This profile is for running a Telegram-connected kelix assistant with the current built-in adapter.

Current adapter scope in this repository:

- Provider support: Telegram only.
- Adapter runtime: runs on the host process (not inside kelix core).
- Session model: adapter binds Telegram groups to **existing** kelix sessions by group title.

## What This Example Includes

- `kelix.toml`: core/subagent runtime profile for chat-assistant style workloads.
- Top-level `[adapter]` provider selection + `[adapter.providers.<name>]` registry.

`examples/chat-assistant/adapter.toml` from earlier drafts is not part of the active runtime path.

## Prerequisites

- `podman` installed and daemon running
- `kelix` binary available in your `PATH`
- `kelix:latest` image built locally (for example `./docker/build.sh`)
- Claude auth configured (OAuth token or API key)
- Telegram bot token (`TELEGRAM_BOT_TOKEN`)

## Authentication Setup

Option A (OAuth token):

```bash
export CLAUDE_CODE_OAUTH_TOKEN=...
```

Option B (API key):

```bash
export ANTHROPIC_API_KEY=...
```

## Run Flow (Current Implementation)

1. Start gateway (adapter connects to this WebSocket endpoint):

```bash
kelix gateway --listen-addr 127.0.0.1:9000
```

2. Create a session with an explicit ID (this ID must match Telegram group title):

```bash
kelix core start examples/chat-assistant/kelix.toml --session_id my-group
```

3. Start adapter on host:

```bash
export TELEGRAM_BOT_TOKEN=...
kelix adapter --provider telegram --gateway-url ws://127.0.0.1:9000
```

4. In Telegram:

- Add the bot into a `group` or `supergroup`.
- Set group title to `my-group` (exact match to session ID).
- Send `/rebind` once, or just send a normal message to trigger auto-bind.

## Telegram Command Surface

- Any text message: forwarded to bound session.
- `/ask <text>` or `/run <text>`: explicit prompt.
- `@bot <text>`: mention-style prompt.
- `/status`: show current binding.
- `/rebind`: retry title-based binding.
- `/approve <request_id> <choice|index>`: answer approval request.
- `/help`: show commands.

Only text messages are forwarded. Media messages are ignored.

## Behavioral Notes

- The adapter does not auto-create sessions on first group message.
- The adapter does not read per-group subagent config from TOML.
- Group-to-session mapping is persisted in adapter state (`~/.kelix/adapters/telegram-state.json` by default).
- Binding uses session existence checks via `kelix list --json`.
- `adapter.autostart=true` can be used to auto-launch the selected provider when running `kelix start <config>`.

## Difference From OpenClaw-Style Usage

Compared to common OpenClaw workflows, this example is intentionally narrower:

- Telegram only (no Slack/Discord adapters in-tree yet).
- Explicit pre-created kelix sessions instead of automatic per-group provisioning.
- Session binding by group title/session ID, not by adapter-managed workspace abstraction.
- No active adapter-level scheduling/capacity policy config in this example.
