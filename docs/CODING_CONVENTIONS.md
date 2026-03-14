# Agent Conventions

Default conventions for AI agents working in this repository.

## Scope

- These rules are language-agnostic.
- Detect the active toolchain from the workspace (`Cargo.toml`, `package.json`, `go.mod`, etc.) instead of relying on explicit config.

## Documentation

- Write code and docs in English unless the content is explicitly for localization.
- Use repository-relative paths in docs and Markdown links; do not commit machine-specific absolute paths such as `/Users/...` or `C:\...`.

## Git

- Use Conventional Commits: `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`, etc.
- Commit messages are single-line only.
- Small fixes may land on `main`.
- Features and larger refactors should use `feat/<name>` or `fix/<name>` branches, then squash-merge to `main`.

## Literate Annotations

Annotate each new or substantially changed logical unit with `@chunk` and `@end-chunk` comments. Do not expand a small fix into a large annotation-only diff unless the touched area is already being refactored. This is a source-level convention; the source file remains the build input.

Format:

```text
// @chunk <module>/<concern>
// <prose: responsibility and boundary contract>
<code>
// @end-chunk
```

Use the file's native comment syntax. Chunk names use lowercase kebab-case and must be unique within the repo, for example `auth/token-validator` or `rate-limiter/sliding-window`.

The `weave` tool extracts these annotations. `review-agent` and `knowledge-agent` also reference chunks by name, so keep names stable.
