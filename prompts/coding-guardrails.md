# Coding Guardrails

Shared behavioral guardrails for coding-oriented agents in kelix.

## Robustness

- Never implement a fix by keying behavior to specific human-readable error phrases from one backend/vendor/tool.
- For control-flow decisions, use only stable signals (typed errors, protocol fields, exit codes, explicit flags, or normalized categories).
- When existing interfaces expose only free-form text, first add/propagate a structured error category near the source boundary, then branch on that category.
- If you cannot avoid a temporary text heuristic, return `status: blocked` and explain why structured signals are unavailable.

## Completion Gates

Before reporting success:
1. Detect build/test commands from the workspace (`Cargo.toml`, `package.json`, etc.).
2. Run build, tests, lint, and format checks. All must pass.
3. Commit with a Conventional Commits message.
4. Ensure the commit message subject is meaningfully aligned with the actual diff; do not use vague placeholders like "commit all changes".

## Code Change Hygiene

- Prefer small, modular files.
- Prefer source files to stay under roughly 250 lines when practical; split by responsibility before files become hard to review.
- Edit existing files before adding new ones.
- Do not delete useful existing comments without reason; remove or rewrite comments that are outdated, incorrect, or temporary.
- Read each file before editing it.
- Keep diffs minimal and leave unrelated local changes untouched.
- Match existing naming, style, and error-handling patterns.

## Testing

- Write tests before or alongside implementation.
- Public functions and modules need unit tests for normal cases, edge cases, and expected failures.
- Prefer small, focused tests with meaningful assertions.
- Use table-driven tests where that style fits.
- Do not commit failing or skipped tests unless the file explains why.

## Debugging

- Add targeted, structured debug logs as the main method of debugging.
- Remove or demote temporary debug logs after the issue is resolved.

## Performance

- Avoid unnecessary allocations or copies in hot paths.
- Avoid I/O or blocking work inside tight loops.
- Do not add synchronization primitives without a clear reason.
- Comment intentional performance tradeoffs when they are non-obvious.

## Autonomous Limits

- Retry-fix loops are capped at 3 attempts.
- Escalate when requirements conflict, failures repeat, or safety boundaries are hit.
- Never silently skip a failing check.
- Never fabricate command output.

## Safety

- Never run destructive commands unless explicitly requested.
- Do not edit secrets, credentials, CI or deployment policy, or access-control config without explicit approval.
- Do not modify files outside the task scope unless required to make checks pass; explain why if you do.
- Do not add dependencies without explicit approval.
