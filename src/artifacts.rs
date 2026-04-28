//! Model artifact detection, format classification, and file selection.
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::FormatKind;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CandidateFile {
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FormatDetection {
    Detected(FormatKind),
    Ambiguous(String),
    Unsupported(String),
}

impl FormatDetection {
    pub fn into_format(self) -> Option<FormatKind> {
        match self {
            Self::Detected(fmt) => Some(fmt),
            _ => None,
        }
    }
}

pub fn detect_format(
    files: &[CandidateFile],
    selected_file: Option<&str>,
    library_hint: Option<&str>,
) -> FormatDetection {
    if files.iter().any(|file| is_split_gguf(&file.path)) {
        return FormatDetection::Unsupported("split GGUF is unsupported in v1".to_string());
    }

    if let Some(selected) = selected_file
        && selected.ends_with(".gguf")
    {
        return FormatDetection::Detected(FormatKind::Gguf);
    }

    let gguf_count = files
        .iter()
        .filter(|file| file.path.ends_with(".gguf"))
        .count();
    if gguf_count > 1 {
        return FormatDetection::Ambiguous("multiple GGUF files found".to_string());
    }
    if gguf_count == 1 {
        return FormatDetection::Detected(FormatKind::Gguf);
    }

    let has_config = files.iter().any(|file| file.path == "config.json");
    let has_safetensors = files.iter().any(|file| file.path.ends_with(".safetensors"));
    if has_config && has_safetensors {
        if let Some(library) = library_hint
            && library == "mlx"
        {
            return FormatDetection::Detected(FormatKind::Mlx);
        }
        return FormatDetection::Detected(FormatKind::Safetensors);
    }

    FormatDetection::Ambiguous("format could not be detected".to_string())
}

pub fn is_split_gguf(path: &str) -> bool {
    let Some(file_name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.ends_with(".gguf") && file_name.contains("-of-") && file_name.contains("-00001-")
}

pub fn artifact_signature(
    repo: &str,
    commit: &str,
    format: &FormatKind,
    files: &[CandidateFile],
) -> String {
    let mut paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    format!("{repo}@{commit}#{format}:{}", paths.join(","))
}

pub fn default_alias(repo: &str, format: FormatKind, selected_file: Option<&str>) -> String {
    let model_name = repo.rsplit('/').next().unwrap_or(repo);
    match (format, selected_file) {
        (FormatKind::Gguf, Some(file)) => {
            let stem = file
                .rsplit('/')
                .next()
                .unwrap_or(file)
                .trim_end_matches(".gguf");
            crate::security::sanitize_alias(stem)
        }
        (FormatKind::Mlx, _) => format!("{}-mlx", crate::security::sanitize_alias(model_name)),
        (FormatKind::Safetensors, _) => {
            format!(
                "{}-safetensors",
                crate::security::sanitize_alias(model_name)
            )
        }
        (FormatKind::GgufSplit, _) => {
            format!("{}-gguf-split", crate::security::sanitize_alias(model_name))
        }
        (FormatKind::Gguf, None) => format!("{}-gguf", crate::security::sanitize_alias(model_name)),
        (FormatKind::Unknown, _) => crate::security::sanitize_alias(model_name),
    }
}

pub fn selected_runtime_files(
    files: &[CandidateFile],
    format: FormatKind,
    selected_file: Option<&str>,
) -> Result<Vec<CandidateFile>, String> {
    if let Some(selected) = selected_file {
        let matches = files
            .iter()
            .filter(|file| wildcard_match(selected, &file.path))
            .cloned()
            .collect::<Vec<_>>();
        return match matches.len() {
            0 => Err(format!("no files match `{selected}`")),
            1 if format == FormatKind::Gguf => Ok(matches),
            _ if format == FormatKind::Gguf => {
                Err(format!("multiple GGUF files match `{selected}`"))
            }
            _ => {
                validate_runtime_closure(&matches, format)?;
                Ok(matches)
            }
        };
    }

    match format {
        FormatKind::Gguf => {
            let matches = files
                .iter()
                .filter(|file| file.path.ends_with(".gguf"))
                .cloned()
                .collect::<Vec<_>>();
            match matches.len() {
                1 => Ok(matches),
                0 => Err("no GGUF file found".to_string()),
                _ => Err("multiple GGUF files found; pass --file".to_string()),
            }
        }
        FormatKind::Mlx | FormatKind::Safetensors => {
            let selected = files
                .iter()
                .filter(|file| is_runtime_closure_file(&file.path))
                .cloned()
                .collect::<Vec<_>>();
            if selected.is_empty() {
                Err("no runtime closure files found".to_string())
            } else {
                validate_runtime_closure(&selected, format)?;
                Ok(selected)
            }
        }
        FormatKind::GgufSplit => Err("split GGUF is unsupported in v1".to_string()),
        FormatKind::Unknown => Err("unknown format; cannot select runtime files".to_string()),
    }
}

fn validate_runtime_closure(files: &[CandidateFile], format: FormatKind) -> Result<(), String> {
    let label = match format {
        FormatKind::Mlx => "MLX",
        FormatKind::Safetensors => "Safetensors",
        FormatKind::Gguf | FormatKind::GgufSplit | FormatKind::Unknown => return Ok(()),
    };

    if !has_file_name(files, "config.json") {
        return Err(format!("{label} snapshot requires config.json"));
    }
    if !files.iter().any(|file| file.path.ends_with(".safetensors")) {
        return Err(format!(
            "{label} snapshot requires at least one .safetensors weight file"
        ));
    }
    if !has_tokenizer_files(files) {
        return Err(format!(
            "{label} snapshot requires tokenizer files such as tokenizer.json, tokenizer.model, or vocab/merges"
        ));
    }
    if format == FormatKind::Mlx && !has_file_name(files, "config.json") {
        return Err("MLX snapshot requires config.json and safetensors weight files".to_string());
    }

    Ok(())
}

fn is_runtime_closure_file(path: &str) -> bool {
    let Some(file_name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    matches!(
        file_name,
        "config.json"
            | "generation_config.json"
            | "tokenizer.json"
            | "tokenizer_config.json"
            | "special_tokens_map.json"
            | "added_tokens.json"
            | "vocab.json"
            | "vocab.txt"
            | "merges.txt"
            | "tokenizer.model"
            | "spiece.model"
            | "sentencepiece.bpe.model"
            | "chat_template.jinja"
            | "model.safetensors.index.json"
    ) || path.ends_with(".safetensors")
}

fn has_file_name(files: &[CandidateFile], expected: &str) -> bool {
    files.iter().any(|file| {
        Path::new(&file.path)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(expected)
    })
}

pub fn resolve_selected_repo_files(
    files: &[crate::hf::RepoFile],
    selected: &[CandidateFile],
) -> Result<Vec<crate::hf::RepoFile>, String> {
    selected
        .iter()
        .map(|candidate| {
            files
                .iter()
                .find(|file| file.path == candidate.path)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "selected file not found in repository metadata: {}",
                        candidate.path
                    )
                })
        })
        .collect()
}

fn has_tokenizer_files(files: &[CandidateFile]) -> bool {
    has_file_name(files, "tokenizer.json")
        || has_file_name(files, "tokenizer.model")
        || has_file_name(files, "spiece.model")
        || has_file_name(files, "sentencepiece.bpe.model")
        || has_file_name(files, "vocab.txt")
        || (has_file_name(files, "vocab.json") && has_file_name(files, "merges.txt"))
        || (has_file_name(files, "tokenizer_config.json") && has_file_name(files, "vocab.json"))
}

pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    if pattern == value {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() < 2 {
        return false;
    }
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !value.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == parts.len() - 1 {
            if !value[pos..].ends_with(part) {
                return false;
            }
        } else if let Some(found) = value[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use crate::model::FormatKind;

    use super::{
        CandidateFile, FormatDetection, default_alias, detect_format, is_split_gguf,
        resolve_selected_repo_files, selected_runtime_files,
    };

    #[test]
    fn split_gguf_is_detected_before_single_file_gguf() {
        assert!(is_split_gguf("model-00001-of-00003.gguf"));
        assert_eq!(
            detect_format(
                &[CandidateFile {
                    path: "model-00001-of-00003.gguf".to_string(),
                    size_bytes: 1
                }],
                None,
                None
            ),
            FormatDetection::Unsupported("split GGUF is unsupported in v1".to_string())
        );
    }

    #[test]
    fn single_selected_gguf_is_supported() {
        assert_eq!(
            detect_format(
                &[CandidateFile {
                    path: "model-q4_k_m.gguf".to_string(),
                    size_bytes: 1
                }],
                Some("model-q4_k_m.gguf"),
                None
            ),
            FormatDetection::Detected(FormatKind::Gguf)
        );
    }

    #[test]
    fn safetensors_snapshot_requires_tokenizer_files() {
        let files = vec![
            CandidateFile {
                path: "config.json".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "model.safetensors".to_string(),
                size_bytes: 1,
            },
        ];

        let error = selected_runtime_files(&files, FormatKind::Safetensors, None).unwrap_err();

        assert!(error.contains("requires tokenizer files"));
    }

    #[test]
    fn safetensors_snapshot_selects_runtime_closure() {
        let files = vec![
            CandidateFile {
                path: "README.md".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "config.json".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "model.safetensors".to_string(),
                size_bytes: 3,
            },
            CandidateFile {
                path: "tokenizer.json".to_string(),
                size_bytes: 2,
            },
        ];

        let selected = selected_runtime_files(&files, FormatKind::Safetensors, None).unwrap();

        assert_eq!(selected.len(), 3);
        assert!(selected.iter().any(|file| file.path == "config.json"));
        assert!(selected.iter().any(|file| file.path == "model.safetensors"));
        assert!(selected.iter().any(|file| file.path == "tokenizer.json"));
    }

    #[test]
    fn detect_single_gguf() {
        let files = vec![CandidateFile {
            path: "model.gguf".to_string(),
            size_bytes: 1000,
        }];
        assert_eq!(
            detect_format(&files, None, None),
            FormatDetection::Detected(FormatKind::Gguf)
        );
    }

    #[test]
    fn detect_multiple_gguf_is_ambiguous() {
        let files = vec![
            CandidateFile {
                path: "a.gguf".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "b.gguf".to_string(),
                size_bytes: 1,
            },
        ];
        assert!(matches!(
            detect_format(&files, None, None),
            FormatDetection::Ambiguous(_)
        ));
    }

    #[test]
    fn detect_selected_gguf() {
        let files = vec![
            CandidateFile {
                path: "a.gguf".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "b.gguf".to_string(),
                size_bytes: 1,
            },
        ];
        assert_eq!(
            detect_format(&files, Some("a.gguf"), None),
            FormatDetection::Detected(FormatKind::Gguf)
        );
    }

    #[test]
    fn detect_safetensors_layout() {
        let files = vec![
            CandidateFile {
                path: "config.json".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "model.safetensors".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "model-00001-of-00002.safetensors".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "model-00002-of-00002.safetensors".to_string(),
                size_bytes: 1,
            },
            CandidateFile {
                path: "tokenizer.json".to_string(),
                size_bytes: 1,
            },
        ];
        // Multiple safetensors shards -> not MLX heuristic -> detected as Safetensors
        assert_eq!(
            detect_format(&files, None, None),
            FormatDetection::Detected(FormatKind::Safetensors)
        );
    }

    #[test]
    fn default_alias_gguf_uses_filename() {
        let alias = default_alias("org/repo", FormatKind::Gguf, Some("model-Q4_K_M.gguf"));
        assert_eq!(alias, "model-q4-k-m");
    }

    #[test]
    fn default_alias_mlx_appends_suffix() {
        let alias = default_alias("org/Qwen3-0.6B", FormatKind::Mlx, None);
        assert_eq!(alias, "qwen3-0-6b-mlx");
    }

    #[test]
    fn resolve_selected_repo_files_finds_match() {
        let repo_files = vec![
            crate::hf::RepoFile {
                path: "a.txt".to_string(),
                size_bytes: 10,
                oid: "o1".to_string(),
            },
            crate::hf::RepoFile {
                path: "b.txt".to_string(),
                size_bytes: 20,
                oid: "o2".to_string(),
            },
        ];
        let selected = vec![CandidateFile {
            path: "b.txt".to_string(),
            size_bytes: 20,
        }];
        let result = resolve_selected_repo_files(&repo_files, &selected).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].oid, "o2");
    }

    #[test]
    fn resolve_selected_repo_files_missing_returns_error() {
        let repo_files = vec![crate::hf::RepoFile {
            path: "a.txt".to_string(),
            size_bytes: 10,
            oid: "o1".to_string(),
        }];
        let selected = vec![CandidateFile {
            path: "missing.txt".to_string(),
            size_bytes: 10,
        }];
        assert!(resolve_selected_repo_files(&repo_files, &selected).is_err());
    }
}
