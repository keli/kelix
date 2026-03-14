# Claude Onboarding

Use this profile as a seed template when you want an existing local agent to generate or adapt a `kelix.toml` for your project.
The containerized orchestrator command in [kelix.toml](./kelix.toml) is an optional runnable example, not a required onboarding architecture.

This profile uses `Claude Code` as the orchestrator backend.

## What It Can Do

After you run the containerized onboarding orchestrator, it can:

- read mounted kelix context (for example `/prompts`, `/docs`, and files under the user-mounted `/workspace`)
- generate a new `kelix.toml` or minimally edit an existing one
- suggest backend/auth/approval choices and keep the config consistent with repo examples
- summarize exactly what it changed and what you should verify locally

Limit:

- it cannot directly modify the environment variables of your current parent shell process

## Auth

### Default (OAuth token)

1. Generate a long-lived token (one-time setup, opens a browser for authentication):

```sh
claude setup-token
```

2. Copy the printed token and export it:

```sh
export CLAUDE_CODE_OAUTH_TOKEN=<token>
```

### Alternative (API key)

Uncomment the API key variant in `kelix.toml` and export `ANTHROPIC_API_KEY`.

If you want a Codex-based onboarding profile, use `examples/codex-onboarding`.
