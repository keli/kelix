# Codex Onboarding

Use this profile as a seed template when you want an existing local agent to generate or adapt a `kelix.toml` for your project.
The containerized orchestrator command in [kelix.toml](./kelix.toml) is an optional runnable example, not a required onboarding architecture.

This profile uses `Codex` as the orchestrator backend.

## What It Can Do

After you run the containerized onboarding orchestrator, it can:

- read mounted kelix context (for example `/prompts`, `/docs`, and files under the user-mounted `/workspace`)
- generate a new `kelix.toml` or minimally edit an existing one
- suggest backend/auth/approval choices and keep the config consistent with repo examples
- summarize exactly what it changed and what you should verify locally

Limit:

- it cannot directly modify the environment variables of your current parent shell process

## Auth

Requires prior `codex login` on this machine.

If you want a Claude-based onboarding profile, use `examples/claude-onboarding`.
