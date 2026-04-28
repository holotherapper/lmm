//! Path validation, sanitization, and traversal prevention.
use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, Result};

pub fn validate_repo_id(repo: &str) -> Result<()> {
    let parts = repo.split('/').collect::<Vec<_>>();
    if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
        return Err(AppError::InvalidInput(format!(
            "repository must be in org/name form: {repo}"
        )));
    }

    for part in parts {
        if !part
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        {
            return Err(AppError::InvalidInput(format!(
                "repository contains unsupported characters: {repo}"
            )));
        }
    }

    Ok(())
}

pub fn validate_relative_repo_path(path: &str) -> Result<()> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(AppError::InvalidInput(
            "repository file path must be relative".to_string(),
        ));
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(AppError::InvalidInput(format!(
                    "unsafe repository file path: {}",
                    path.display()
                )));
            }
        }
    }

    Ok(())
}

pub fn ensure_child_path(root: &Path, child: &Path) -> Result<()> {
    let root = normalize_without_existing_requirement(root);
    let child = normalize_without_existing_requirement(child);
    if child.starts_with(&root) {
        return Ok(());
    }

    Err(AppError::InvalidInput(format!(
        "path `{}` is outside root `{}`",
        child.display(),
        root.display()
    )))
}

pub fn sanitize_alias(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut previous_separator = false;

    for ch in input.chars().flat_map(char::to_lowercase) {
        let is_allowed = ch.is_ascii_alphanumeric();
        if is_allowed {
            output.push(ch);
            previous_separator = false;
            continue;
        }

        if !previous_separator && !output.is_empty() {
            output.push('-');
            previous_separator = true;
        }
    }

    output.trim_matches('-').to_string()
}

pub fn repo_cache_dir_name(repo: &str) -> String {
    format!("models--{}", repo.replace('/', "--"))
}

/// Reject paths that contain control characters (newlines, tabs, etc.).
///
/// Ollama Modelfile directives are newline-delimited, so a path containing
/// `\n` could inject extra directives (FROM, TEMPLATE, SYSTEM …).  The path
/// originates from the HF cache whose directory names incorporate
/// user-controlled repository and commit strings – making injection feasible.
pub fn validate_path_no_control_chars(path: &Path) -> Result<()> {
    let s = path.to_string_lossy();
    if let Some(pos) = s.find(|ch: char| ch.is_control()) {
        return Err(AppError::InvalidInput(format!(
            "path contains control character at byte {pos}: {s:?}"
        )));
    }
    Ok(())
}

pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_repo_path(relative)?;
    let joined = root.join(relative);
    ensure_child_path(root, &joined)?;
    Ok(joined)
}

fn normalize_without_existing_requirement(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            Component::Normal(part) => output.push(part),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        ensure_child_path, safe_join, sanitize_alias, validate_path_no_control_chars,
        validate_relative_repo_path, validate_repo_id,
    };

    #[test]
    fn validate_path_no_control_chars_accepts_normal_path() {
        assert!(validate_path_no_control_chars(Path::new("/tmp/models/model.gguf")).is_ok());
    }

    #[test]
    fn validate_path_no_control_chars_rejects_newline() {
        assert!(validate_path_no_control_chars(Path::new("/tmp/models\n/model.gguf")).is_err());
    }

    #[test]
    fn validate_path_no_control_chars_rejects_null() {
        assert!(validate_path_no_control_chars(Path::new("/tmp/models\0/file")).is_err());
    }

    #[test]
    fn validate_relative_repo_path_rejects_parent_components() {
        assert!(validate_relative_repo_path("../config.json").is_err());
    }

    #[test]
    fn ensure_child_path_rejects_sibling_path() {
        assert!(ensure_child_path(Path::new("/tmp/root"), Path::new("/tmp/root2/file")).is_err());
    }

    #[test]
    fn sanitize_alias_collapses_untrusted_characters() {
        assert_eq!(sanitize_alias("DeepSeek R1/Q4_K_M"), "deepseek-r1-q4-k-m");
    }

    #[test]
    fn validate_repo_id_accepts_valid() {
        assert!(validate_repo_id("org/repo").is_ok());
        assert!(validate_repo_id("mlx-community/Qwen3-0.6B").is_ok());
    }

    #[test]
    fn validate_repo_id_rejects_invalid() {
        assert!(validate_repo_id("noslash").is_err());
        assert!(validate_repo_id("a/b/c").is_err());
        assert!(validate_repo_id("/repo").is_err());
        assert!(validate_repo_id("org/").is_err());
    }

    #[test]
    fn validate_relative_path_accepts_normal() {
        assert!(validate_relative_repo_path("model.safetensors").is_ok());
        assert!(validate_relative_repo_path("subdir/file.json").is_ok());
    }

    #[test]
    fn safe_join_prevents_escape() {
        let root = Path::new("/tmp/root");
        assert!(safe_join(root, "../escape").is_err());
        assert!(safe_join(root, "normal/file.txt").is_ok());
    }

    #[test]
    fn validate_path_no_control_chars_accepts_normal() {
        let path = std::path::PathBuf::from("/tmp/normal/path/model.gguf");
        assert!(validate_path_no_control_chars(&path).is_ok());
    }
}
