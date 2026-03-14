# Knowledge Pipeline

Status: Proposal
Last updated: March 3, 2026

## Overview

The knowledge pipeline turns source annotations and user-provided documents into a retrievable corpus that agents can query. It has three components:

- **`weave`**: extracts `@chunk` annotations from source files into structured index units.
- **knowledge volume**: a persistent store holding both raw documents and extracted chunk indexes.
- **knowledge-agent**: answers queries against the volume.

These components are independent. `weave` can run without knowledge-agent (e.g. to generate static documentation). knowledge-agent can serve user-provided documents that have no chunk annotations.

## 1. weave Tool

`weave` is a dependency-free CLI script. It scans source files for `@chunk`/`@end-chunk` annotations and writes one JSON file per chunk into an output directory.

**Invocation:**

```
weave <source-root> <output-dir>
```

**Output per chunk** (`<output-dir>/<module>/<concern>.json`):

```json
{
  "chunk": "auth/token-validator",
  "prose_format": "markdown+latex",
  "prose": "Validates JWT tokens against the session key. Rejects expired or malformed tokens without leaking timing information.\n\nLatency bound: $t_{reject} \\approx t_{accept}$.",
  "code": "fn validate_token(token: &str, key: &SessionKey) -> Result<Claims, AuthError> { ... }",
  "lang": "rust",
  "source_file": "src/auth.rs",
  "source_line": 42
}
```

`prose` is the canonical machine-readable documentation field. It is stored as a UTF-8 string containing Markdown text with inline LaTeX math (`$...$`) and display LaTeX math (`$$...$$`) allowed verbatim. `weave` does not parse or normalize math expressions; it preserves the source text exactly apart from JSON string escaping.

`prose_format` is required and currently fixed to `markdown+latex`. Consumers must treat `prose` as presentation-capable text, not plain text. Retrieval systems may index the raw string directly or strip Markdown syntax as a preprocessing step, but they must not discard math content.

**Schema contract:**

- `chunk`: stable chunk identifier (`<module>/<concern>`).
- `prose_format`: syntax tag for `prose`; currently `markdown+latex`.
- `prose`: Markdown body plus optional LaTeX math, intended for both LLM consumption and human-facing rendering.
- `code`: source text inside the chunk boundary, excluding annotation comments.
- `lang`: source language label used when rendering fenced code blocks.
- `source_file`: repository-relative path to the source file.
- `source_line`: 1-based line number of the `@chunk` annotation.

`weave` overwrites existing output files. It does not delete files for chunks that no longer exist — stale index cleanup is the caller's responsibility.

**When to run:**

- Post-merge hook in git-backed sessions: `weave` runs after each squash merge to main, keeping the index current with the integrated codebase.
- On demand: agents or humans may invoke `weave` at any time via the shell policy gate (`weave` must be in `allowed_commands`).

`weave` output is the only input knowledge-agent needs for code knowledge. It does not need direct access to the git repository or source files.

## 2. Knowledge Volume

The volume at `/knowledge/` holds two kinds of content:

| Path | Content |
|------|---------|
| `/knowledge/docs/<name>` | Raw documents loaded by the user or orchestrator (PDF, Markdown, plain text, etc.) |
| `/knowledge/chunks/<module>/<concern>.json` | Chunk index units written by `weave` |

Documents and chunks are indexed independently. A query searches both.

## 3. Retrieval Strategy

The appropriate retrieval strategy depends on corpus size. knowledge-agent selects the strategy based on what is available in its environment.

| Corpus size | Strategy |
|-------------|----------|
| Small (tens of documents / hundreds of chunks) | Load all into context; let the model retrieve inline |
| Medium | Keyword search via `rg` over `/knowledge/`; feed matching sections to the model |
| Large | Vector index (e.g. SQLite-vec, Chroma); embed queries and chunk prose at load time |

The keyword strategy requires only `rg`, which is already in the default `allowed_commands`. Vector search requires the knowledge-agent container to include an embedding model and vector store — this is a deployment-layer concern.

For chunk queries, prose is the primary retrieval target. Code is returned alongside prose when a chunk matches, but embedding or keyword matching is done against the prose description, not the code body. When `prose_format` is `markdown+latex`, retrieval should preserve math tokens; stripping `$` delimiters is allowed only if the underlying TeX content remains searchable.

## 4. Synchronization

In git-backed sessions, the canonical sync point is post-merge:

```
coding-agent commits @chunk-annotated source
    → squash merge to main
    → post-merge hook: weave src/ /knowledge/chunks/
    → knowledge volume updated
```

knowledge-agent does not poll for changes. It reads the volume at query time. Stale chunks (from deleted or renamed code) remain in the volume until `weave` is re-run and stale files are pruned. Pruning is optional — stale chunks degrade retrieval quality but do not cause errors.

User documents are loaded explicitly via knowledge-agent `load` mode, triggered by the orchestrator or directly by the user.
