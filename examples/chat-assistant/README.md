# Example: Telegram Chat Assistant

Status: Example
Last updated: March 16, 2026

## Overview

This profile runs a Telegram-connected kelix assistant: Codex as orchestrator,
Claude as coding-agent, with the Telegram adapter auto-started alongside the session.

Current adapter scope:

- Provider support: Telegram only.
- Session model: adapter binds Telegram groups to kelix sessions by group title.
- Adapter state: `~/.kelix/adapters/telegram-state.json`

## Prerequisites

- `podman` installed and daemon running
- `kelix` binary built (`cargo build --release`)
- `kelix:latest` image built (`./docker/build.sh`)
- Codex auth configured (`~/.codex`)
- Claude auth configured (`~/.claude`, `~/.claude.json`, `CLAUDE_CODE_OAUTH_TOKEN`)
- `TELEGRAM_BOT_TOKEN` set in the environment
- `~/.gitconfig` with `user.name` and `user.email` set

## Start

```sh
export TELEGRAM_BOT_TOKEN=...
export CLAUDE_CODE_OAUTH_TOKEN=...
./target/release/kelix start examples/chat-assistant/kelix.toml --session my-group
```

`kelix start` automatically:
1. Starts a gateway (if not already running) at `127.0.0.1:9000`.
2. Launches the Telegram adapter (`adapter.autostart = true`).
3. Opens the TUI for the session.

The session ID must match the Telegram group title for auto-binding to work.

## Telegram Bot Setup

1. Create a bot via [@BotFather](https://t.me/botfather) and get a token.
2. Add the bot to a group or supergroup.
3. Set the group title to match your `--session` value.
4. On first run the adapter prints a claim code to stderr. Send `/claim <code>` to the bot
   to whitelist yourself. Until claimed, session messages are dropped.

## Telegram Commands

- Any plain text message (or `@botname <text>`): forwarded to the bound session.
- `/ask <text>` or `/run <text>`: explicit prompt.
- `/status`: show the current session binding for this group.
- `/rebind`: retry title-based binding (use after renaming the group or restarting).
- `/approve <request_id> <choice|index>`: answer an approval request.
- `/claim <code>`: whitelist yourself on first run.
- `/help`: show commands.

Only text messages are forwarded. Media messages are ignored.

## Adjust The Config

Start from [kelix.toml](./kelix.toml).

Common changes:

- auth mounts (swap codex/claude paths or use API keys via env vars)
- `max_concurrent_spawns` (raise for more parallel tasks)
- `budget.max_tokens` / `on_budget_exceeded`
- `adapter.autostart` (set to `false` to manage the adapter separately)

## Troubleshooting

If startup fails, check in this order:

1. `Codex` works on the host by itself.
2. `Claude Code` works on the host by itself.
3. `TELEGRAM_BOT_TOKEN` is set and valid.
4. The auth paths in `kelix.toml` exist on your machine.
5. `podman` can run `kelix:latest`.

If the adapter starts but messages are dropped, you have not claimed the bot yet.
Check stderr for the claim code and send `/claim <code>` to the bot.

For detailed diagnostics:

```sh
./target/release/kelix start examples/chat-assistant/kelix.toml --session my-group --debug
```
