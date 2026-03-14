use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// @chunk weave-bin/cli
// Minimal standalone CLI for the proposed weave tool. Keep the interface aligned
// with the design docs so the prototype can later be replaced without changing
// calling conventions.
#[derive(Parser, Debug)]
#[command(
    name = "weave",
    about = "Extract @chunk annotations into JSON documents"
)]
struct Cli {
    /// Root directory to scan for annotated source files.
    source_root: PathBuf,

    /// Output directory where per-chunk JSON files will be written.
    output_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChunkDocument {
    chunk: String,
    prose_format: String,
    prose: String,
    code: String,
    lang: String,
    source_file: String,
    source_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingChunk {
    chunk: String,
    prose_lines: Vec<String>,
    code_lines: Vec<String>,
    source_line: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    run(&cli.source_root, &cli.output_dir)
}
// @end-chunk

// @chunk weave-bin/run
// Walk the source tree, extract annotated chunks, and write each chunk to its
// own JSON file. Existing files are overwritten; stale output files are left in
// place to match the documented contract.
fn run(source_root: &Path, output_dir: &Path) -> Result<()> {
    let source_root = source_root
        .canonicalize()
        .with_context(|| format!("failed to resolve source root: {}", source_root.display()))?;

    if !source_root.is_dir() {
        anyhow::bail!("source root is not a directory: {}", source_root.display());
    }

    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create output dir: {}", output_dir.display()))?;

    visit_dir(&source_root, &source_root, output_dir)
}
// @end-chunk

// @chunk weave-bin/directory-walk
// Recursively visit files under the source root. Non-UTF-8 files are ignored so
// binary artifacts in the tree do not abort the whole run.
fn visit_dir(root: &Path, current: &Path, output_dir: &Path) -> Result<()> {
    for entry in fs::read_dir(current)
        .with_context(|| format!("failed to read directory: {}", current.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", current.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to stat path: {}", path.display()))?;

        if file_type.is_dir() {
            visit_dir(root, &path, output_dir)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("failed to read file: {}", path.display()))
            }
        };

        for chunk in extract_chunks(&contents, &path, root)? {
            write_chunk(output_dir, &chunk)?;
        }
    }

    Ok(())
}
// @end-chunk

// @chunk weave-bin/chunk-parser
// Parse a file line-by-line. The prose block is the consecutive comment block
// immediately following `@chunk`; the remaining lines until `@end-chunk` are
// emitted as code, excluding the annotation markers themselves.
fn extract_chunks(contents: &str, path: &Path, root: &Path) -> Result<Vec<ChunkDocument>> {
    let mut chunks = Vec::new();
    let mut pending: Option<PendingChunk> = None;
    let mut collecting_prose = false;

    for (idx, raw_line) in contents.lines().enumerate() {
        let line_no = idx + 1;

        if let Some(chunk_name) = parse_chunk_start(raw_line) {
            if let Some(open) = pending.take() {
                anyhow::bail!(
                    "nested @chunk detected in {} at line {} before closing chunk '{}'",
                    path.display(),
                    line_no,
                    open.chunk
                );
            }

            pending = Some(PendingChunk {
                chunk: chunk_name,
                prose_lines: Vec::new(),
                code_lines: Vec::new(),
                source_line: line_no,
            });
            collecting_prose = true;
            continue;
        }

        if parse_chunk_end(raw_line) {
            let open = pending.take().ok_or_else(|| {
                anyhow::anyhow!(
                    "orphan @end-chunk in {} at line {}",
                    path.display(),
                    line_no
                )
            })?;
            chunks.push(finalize_chunk(open, path, root)?);
            collecting_prose = false;
            continue;
        }

        let Some(open) = pending.as_mut() else {
            continue;
        };

        if collecting_prose {
            if let Some(comment_text) = parse_comment_text(raw_line) {
                open.prose_lines.push(comment_text.to_string());
                continue;
            }

            collecting_prose = false;
        }

        open.code_lines.push(raw_line.to_string());
    }

    if let Some(open) = pending {
        anyhow::bail!(
            "unterminated chunk '{}' in {} opened at line {}",
            open.chunk,
            path.display(),
            open.source_line
        );
    }

    Ok(chunks)
}
// @end-chunk

// @chunk weave-bin/chunk-finalize
// Normalize the parsed chunk into the documented JSON schema. The source path is
// made relative to the current working directory when possible so repository
// paths remain stable when weave is run from the repo root.
fn finalize_chunk(pending: PendingChunk, path: &Path, root: &Path) -> Result<ChunkDocument> {
    let source_file = relative_source_path(path, root)?;

    Ok(ChunkDocument {
        chunk: pending.chunk,
        prose_format: "markdown+latex".to_string(),
        prose: pending.prose_lines.join("\n"),
        code: pending.code_lines.join("\n"),
        lang: infer_language(path),
        source_file,
        source_line: pending.source_line,
    })
}
// @end-chunk

// @chunk weave-bin/comment-parser
// Recognize annotation and prose comment lines across a small set of common
// source comment prefixes. This keeps the prototype language-agnostic enough for
// the documented format without pulling in a real parser.
fn parse_chunk_start(line: &str) -> Option<String> {
    let body = parse_comment_text(line)?;
    let chunk = body.trim();
    if chunk == "@chunk" {
        None
    } else if let Some(rest) = chunk.strip_prefix("@chunk ") {
        Some(rest.trim().to_string())
    } else if let Some(rest) = chunk.strip_prefix("@chunk\t") {
        Some(rest.trim().to_string())
    } else {
        None
    }
}

fn parse_chunk_end(line: &str) -> bool {
    parse_comment_text(line)
        .map(|body| body.trim() == "@end-chunk")
        .unwrap_or(false)
}

fn parse_comment_text(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let prefixes = ["//", "#", "--", ";", "%", "/*", "*"];

    for prefix in prefixes {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let rest = rest.trim_start();
            let rest = rest.strip_suffix("*/").unwrap_or(rest).trim_end();
            return Some(rest);
        }
    }

    None
}
// @end-chunk

// @chunk weave-bin/output-writer
// Write one JSON document per chunk using the chunk name as the output path.
// Chunk names must contain at least one `/` so the final path is
// `<output>/<module>/<concern>.json`.
fn write_chunk(output_dir: &Path, chunk: &ChunkDocument) -> Result<()> {
    let (module, concern) = chunk.chunk.rsplit_once('/').ok_or_else(|| {
        anyhow::anyhow!(
            "invalid chunk name '{}': expected module/concern",
            chunk.chunk
        )
    })?;

    let chunk_dir = output_dir.join(module);
    fs::create_dir_all(&chunk_dir)
        .with_context(|| format!("failed to create chunk dir: {}", chunk_dir.display()))?;

    let output_path = chunk_dir.join(format!("{concern}.json"));
    let json = serde_json::to_string_pretty(chunk).context("failed to serialize chunk")?;
    fs::write(&output_path, json)
        .with_context(|| format!("failed to write chunk file: {}", output_path.display()))?;

    Ok(())
}
// @end-chunk

// @chunk weave-bin/path-and-lang
// Keep path normalization and language labeling predictable for downstream
// renderers. Unknown extensions fall back to a plain text label.
fn relative_source_path(path: &Path, root: &Path) -> Result<String> {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(relative) = path.strip_prefix(&cwd) {
            return Ok(relative.to_string_lossy().replace('\\', "/"));
        }
    }

    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("failed to relativize source path: {}", path.display()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn infer_language(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let lang = match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "jsx" => "jsx",
        "go" => "go",
        "java" => "java",
        "rb" => "ruby",
        "sh" => "bash",
        "zsh" => "zsh",
        "md" => "markdown",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" => "json",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "" => "text",
        other => other,
    };

    lang.to_string()
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // @chunk weave-bin/tests-parse
    // Validate the core parser contract: prose stays in markdown+latex form and
    // code excludes the annotation comment block.
    #[test]
    fn test_extract_chunks_preserves_markdown_and_latex() {
        let source = "\
// @chunk math/newton-step\n\
// Compute the next Newton step.\n\
//\n\
// Update rule: $x_{n+1} = x_n - f(x_n)/f'(x_n)$.\n\
fn newton_step() {}\n\
// @end-chunk\n";

        let root = Path::new("/repo");
        let path = root.join("src/newton.rs");
        let chunks = extract_chunks(source, &path, root).expect("chunk parse should succeed");

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].chunk, "math/newton-step");
        assert_eq!(chunks[0].prose_format, "markdown+latex");
        assert_eq!(
            chunks[0].prose,
            "Compute the next Newton step.\n\nUpdate rule: $x_{n+1} = x_n - f(x_n)/f'(x_n)$."
        );
        assert_eq!(chunks[0].code, "fn newton_step() {}");
        assert_eq!(chunks[0].lang, "rust");
        assert_eq!(chunks[0].source_file, "src/newton.rs");
        assert_eq!(chunks[0].source_line, 1);
    }
    // @end-chunk

    // @chunk weave-bin/tests-run
    // Exercise the end-to-end file writer on a temporary directory so the CLI
    // behavior stays pinned to the documented output layout.
    #[test]
    fn test_run_writes_one_json_file_per_chunk() {
        let temp = tempdir().expect("tempdir should be created");
        let source_root = temp.path().join("src");
        let output_dir = temp.path().join("out");
        fs::create_dir_all(&source_root).expect("source root should be created");

        let source_path = source_root.join("sample.rs");
        fs::write(
            &source_path,
            "\
// @chunk demo/example\n\
// Example prose with $a^2 + b^2 = c^2$.\n\
fn example() {}\n\
// @end-chunk\n",
        )
        .expect("source file should be written");

        run(&source_root, &output_dir).expect("weave run should succeed");

        let output_path = output_dir.join("demo/example.json");
        let written = fs::read_to_string(&output_path).expect("output should be written");
        let parsed: ChunkDocument =
            serde_json::from_str(&written).expect("output JSON should deserialize");

        assert_eq!(parsed.chunk, "demo/example");
        assert_eq!(parsed.prose_format, "markdown+latex");
        assert_eq!(parsed.prose, "Example prose with $a^2 + b^2 = c^2$.");
        assert_eq!(parsed.code, "fn example() {}");
        assert_eq!(parsed.lang, "rust");
        assert!(parsed.source_file.ends_with("sample.rs"));
    }
    // @end-chunk
}
