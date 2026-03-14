use crate::paths::resolve_kelix_home_path;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

// @chunk main/example-config-discovery
// Discover and print example config files from common locations so users can
// start from bundled templates without manually browsing the filesystem.
pub fn print_example_configs() -> Result<()> {
    let entries = discover_example_entries()?;
    if entries.is_empty() {
        let roots = example_search_roots();
        println!("No example .toml files found.");
        if roots.is_empty() {
            println!("Checked no roots (could not resolve current directory).");
        } else {
            println!("Checked:");
            for root in roots {
                println!("  {}", root.display());
            }
        }
        return Ok(());
    }

    for entry in entries {
        println!("{} -> {}", entry.alias, entry.path.display());
    }

    Ok(())
}

pub fn resolve_example_config(name: &str) -> Result<PathBuf> {
    let entries = discover_example_entries()?;
    if let Some(entry) = entries.into_iter().find(|e| e.alias == name) {
        return Ok(entry.path);
    }
    anyhow::bail!(
        "unknown example '{}'. Run `kelix start --list-examples` to see available aliases.",
        name
    );
}

#[derive(Debug, Clone)]
struct ExampleEntry {
    alias: String,
    path: PathBuf,
}

fn discover_example_entries() -> Result<Vec<ExampleEntry>> {
    let mut entries = Vec::<ExampleEntry>::new();
    let mut seen = HashSet::<String>::new();

    for root in example_search_roots() {
        for file in collect_toml_files(&root)? {
            for alias in aliases_for_example_file(&root, &file) {
                if seen.insert(alias.clone()) {
                    entries.push(ExampleEntry {
                        alias,
                        path: file.clone(),
                    });
                }
            }
        }
    }

    entries.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(entries)
}

fn aliases_for_example_file(root: &Path, file: &Path) -> Vec<String> {
    let rel = file.strip_prefix(root).unwrap_or(file);
    let rel_unix = rel.to_string_lossy().replace('\\', "/");
    let mut aliases = Vec::<String>::new();
    let dir_name = rel
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");

    if stem == "kelix" && !dir_name.is_empty() {
        aliases.push(dir_name.to_string());
    }

    if dir_name == "onboarding" && stem == "kelix" {
        aliases.push("onboarding".to_string());
        aliases.push("codex-onboarding".to_string());
    }
    if dir_name == "onboarding" && stem == "kelix.claude" {
        aliases.push("claude-onboarding".to_string());
    }
    if dir_name == "codex-onboarding" && stem == "kelix" {
        aliases.push("onboarding".to_string());
        aliases.push("codex-onboarding".to_string());
    }
    if dir_name == "claude-onboarding" && stem == "kelix" {
        aliases.push("claude-onboarding".to_string());
    }

    if !stem.is_empty() && stem != "kelix" && stem != "kelix.claude" {
        aliases.push(stem.to_string());
    }
    if aliases.is_empty() {
        aliases.push(rel_unix);
    }
    aliases
}

pub fn example_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        let examples = cwd.join("examples");
        if examples.is_dir() {
            roots.push(examples);
        }
    }

    if let Ok(kelix_home) = resolve_kelix_home_path() {
        let examples = kelix_home.join("examples");
        if examples.is_dir() && !roots.contains(&examples) {
            roots.push(examples);
        }
    }

    roots
}

pub fn collect_toml_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_toml_files_rec(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_toml_files_rec(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("failed to read directory: {}", dir.display()))?
    {
        let entry = entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
        let path = entry.path();

        if path.is_dir() {
            collect_toml_files_rec(&path, files)?;
            continue;
        }

        let is_toml = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("toml"))
            .unwrap_or(false);
        if is_toml {
            files.push(path);
        }
    }
    Ok(())
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::collect_toml_files;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn collect_toml_files_finds_nested_and_filters_extensions() {
        let dir = tempdir().expect("create tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("a/b")).expect("create nested dirs");
        fs::write(root.join("root.toml"), "x = 1").expect("write root toml");
        fs::write(root.join("a/b/nested.TOML"), "x = 2").expect("write nested toml");
        fs::write(root.join("ignore.md"), "# nope").expect("write non toml");

        let files = collect_toml_files(root).expect("collect toml files");
        let rels: Vec<String> = files
            .into_iter()
            .map(|p| {
                p.strip_prefix(root)
                    .expect("strip prefix")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert_eq!(rels, vec!["a/b/nested.TOML", "root.toml"]);
    }
}
