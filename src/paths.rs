use std::path::{Path, PathBuf};

// @chunk paths/kelix-home-resolution
// Resolve KELIX_HOME across package layouts:
// - explicit env var wins
// - bundle root next to `bin/`
// - package-managed `share/kelix` locations relative to the executable
pub fn resolve_kelix_home_path() -> Result<PathBuf, String> {
    if let Ok(val) = std::env::var("KELIX_HOME") {
        if !val.is_empty() {
            return Ok(PathBuf::from(val));
        }
    }

    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot resolve binary path for KELIX_HOME: {e}"))?;
    let candidates = kelix_home_candidates(&exe);
    if candidates.is_empty() {
        return Err("binary path has no parent directory".to_string());
    }

    if let Some(found) = candidates
        .iter()
        .find(|path| looks_like_kelix_home(path.as_path()))
    {
        return Ok(found.clone());
    }

    Ok(candidates[0].clone())
}

fn looks_like_kelix_home(path: &Path) -> bool {
    path.join("prompts").is_dir() || path.join("examples").is_dir() || path.join("docs").is_dir()
}

fn kelix_home_candidates(exe: &Path) -> Vec<PathBuf> {
    let mut out = Vec::<PathBuf>::new();
    let Some(bin_dir) = exe.parent() else {
        return out;
    };

    let is_bin_dir = bin_dir.file_name().map(|n| n == "bin").unwrap_or(false);
    if is_bin_dir {
        if let Some(root) = bin_dir.parent() {
            push_unique(&mut out, root.to_path_buf());
            push_unique(&mut out, root.join("share").join("kelix"));
        } else {
            push_unique(&mut out, bin_dir.to_path_buf());
        }
    } else {
        push_unique(&mut out, bin_dir.to_path_buf());
        push_unique(&mut out, bin_dir.join("share").join("kelix"));
        if let Some(parent) = bin_dir.parent() {
            push_unique(&mut out, parent.join("share").join("kelix"));
        }
    }

    out
}

fn push_unique(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !paths.contains(&candidate) {
        paths.push(candidate);
    }
}
// @end-chunk

#[cfg(test)]
mod tests {
    use super::kelix_home_candidates;
    use std::path::PathBuf;

    #[test]
    fn candidates_include_bundle_root_for_bin_layout() {
        let exe = PathBuf::from("/opt/kelix/bin/kelix");
        let c = kelix_home_candidates(&exe);
        assert_eq!(c[0], PathBuf::from("/opt/kelix"));
        assert_eq!(c[1], PathBuf::from("/opt/kelix/share/kelix"));
    }

    #[test]
    fn candidates_include_share_kelix_for_prefix_bin_layout() {
        let exe = PathBuf::from("/usr/local/bin/kelix");
        let c = kelix_home_candidates(&exe);
        assert_eq!(c[0], PathBuf::from("/usr/local"));
        assert_eq!(c[1], PathBuf::from("/usr/local/share/kelix"));
    }

    #[test]
    fn candidates_include_self_dir_for_non_bin_layout() {
        let exe = PathBuf::from("/tmp/kelix/kelix");
        let c = kelix_home_candidates(&exe);
        assert_eq!(c[0], PathBuf::from("/tmp/kelix"));
        assert_eq!(c[1], PathBuf::from("/tmp/kelix/share/kelix"));
    }
}
