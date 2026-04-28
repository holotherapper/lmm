//! HF Cache scanning and adoption candidate discovery.
use std::fs;
use std::path::{Path, PathBuf};

use crate::artifacts::{
    FormatDetection, artifact_signature, default_alias, detect_format, selected_runtime_files,
};
use crate::error::{AppError, Result};
use crate::hf::{RepoFile, candidate_files};
use crate::lock::LockFile;
use crate::model::{
    Artifact, ArtifactFile, FormatKind, HfCacheLocation, Locations, ModelEntry, Ownership, Source,
};

#[derive(Clone)]
pub(crate) struct AdoptionCandidate {
    pub id: String,
    pub snapshot_root: PathBuf,
    pub repo_files: Vec<RepoFile>,
    pub model: ModelEntry,
}

pub(crate) fn adoption_candidates(
    repo_dirs: &[PathBuf],
    lock: &LockFile,
    take_ownership: bool,
) -> Result<Vec<AdoptionCandidate>> {
    let mut best: std::collections::HashMap<String, AdoptionCandidate> =
        std::collections::HashMap::new();

    for repo_dir in repo_dirs {
        let Some(repo) = repo_from_cache_dir(repo_dir) else {
            continue;
        };
        let snapshots_dir = repo_dir.join("snapshots");
        if !snapshots_dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&snapshots_dir).map_err(|source| AppError::Read {
            path: snapshots_dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| AppError::Read {
                path: snapshots_dir.clone(),
                source,
            })?;
            let snapshot_root = entry.path();
            if !snapshot_root.is_dir() {
                continue;
            }
            let is_tracked = lock.models.values().any(|m| {
                m.locations
                    .hf_cache
                    .as_ref()
                    .is_some_and(|loc| loc.snapshot_path == snapshot_root)
            });
            if is_tracked {
                continue;
            }
            if let Some(candidate) =
                adoption_candidate_for_snapshot(&repo, &snapshot_root, take_ownership)?
            {
                best.entry(repo.clone())
                    .and_modify(|existing| {
                        if candidate.model.artifact.size_bytes > existing.model.artifact.size_bytes
                        {
                            *existing = candidate.clone();
                        }
                    })
                    .or_insert(candidate);
            }
        }
    }
    Ok(best.into_values().collect())
}

fn adoption_candidate_for_snapshot(
    repo: &str,
    snapshot_root: &Path,
    take_ownership: bool,
) -> Result<Option<AdoptionCandidate>> {
    let commit = snapshot_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    if commit.is_empty() {
        return Ok(None);
    }

    let repo_files = snapshot_repo_files(snapshot_root)?;
    if repo_files.is_empty() {
        return Ok(None);
    }
    let candidate_files = candidate_files(&repo_files);

    let (format, selected_files) = match detect_format(&candidate_files, None, None) {
        FormatDetection::Detected(fmt) => {
            match selected_runtime_files(&candidate_files, fmt, None) {
                Ok(selected) => {
                    let resolved =
                        crate::artifacts::resolve_selected_repo_files(&repo_files, &selected)
                            .map_err(AppError::InvalidInput)?;
                    (fmt, resolved)
                }
                Err(_) => (fmt, repo_files),
            }
        }
        _ => {
            if !has_downloaded_weights(snapshot_root, &repo_files) {
                return Ok(None);
            }
            let mut fmt = detect_format_kind(&repo_files);
            if fmt == FormatKind::Safetensors
                && (has_mlx_safetensors_metadata(snapshot_root, &repo_files)
                    || has_mlx_config_quantization(snapshot_root, &repo_files))
            {
                fmt = FormatKind::Mlx;
            }
            (fmt, repo_files)
        }
    };

    let signature = artifact_signature(repo, &commit, &format, &candidate_files);
    let artifact_files: Vec<ArtifactFile> = selected_files
        .iter()
        .map(|file| ArtifactFile {
            path: file.path.clone(),
            size_bytes: file.size_bytes,
            hf_blob: Some(file.oid.clone()),
            sha256: None,
            role: None,
        })
        .collect();
    let size_bytes = artifact_files.iter().map(|file| file.size_bytes).sum();
    let ownership = if take_ownership {
        Ownership::OwnedAdopted
    } else {
        Ownership::Adopted
    };
    let model = ModelEntry {
        name: default_alias(repo, format, None),
        source: Source {
            kind: "hf".to_string(),
            repo: repo.to_string(),
            revision: commit.clone(),
            commit,
            ownership,
        },
        format,
        artifact: Artifact {
            signature: signature.clone(),
            size_bytes,
            full_snapshot: false,
            files: artifact_files,
        },
        locations: Locations {
            hf_cache: Some(HfCacheLocation {
                snapshot_path: snapshot_root.to_path_buf(),
            }),
        },
        exposures: std::collections::BTreeMap::new(),
    };

    Ok(Some(AdoptionCandidate {
        id: format!("hf:{signature}"),
        snapshot_root: snapshot_root.to_path_buf(),
        repo_files: selected_files,
        model,
    }))
}

fn is_weight_extension(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".safetensors")
        || p.ends_with(".gguf")
        || p.ends_with(".npz")
        || p.ends_with(".bin")
        || p.ends_with(".pt")
        || p.ends_with(".pth")
}

pub(crate) fn has_downloaded_weights(snapshot_root: &Path, files: &[RepoFile]) -> bool {
    files.iter().any(|f| {
        if !is_weight_extension(&f.path) {
            return false;
        }
        let full_path = snapshot_root.join(&f.path);
        full_path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    })
}

fn has_mlx_safetensors_metadata(snapshot_root: &Path, files: &[RepoFile]) -> bool {
    use std::io::Read;
    for f in files {
        if !f.path.ends_with(".safetensors") {
            continue;
        }
        let full_path = snapshot_root.join(&f.path);
        let Ok(mut file) = fs::File::open(&full_path) else {
            continue;
        };
        let mut size_buf = [0u8; 8];
        if file.read_exact(&mut size_buf).is_err() {
            continue;
        }
        let header_size = u64::from_le_bytes(size_buf) as usize;
        if header_size > 1_000_000 {
            continue;
        }
        let mut header_buf = vec![0u8; header_size];
        if file.read_exact(&mut header_buf).is_err() {
            continue;
        }
        let header_str = String::from_utf8_lossy(&header_buf);
        if header_str.contains("\"format\":\"mlx\"") || header_str.contains("\"format\": \"mlx\"") {
            return true;
        }
    }
    false
}

fn has_mlx_config_quantization(snapshot_root: &Path, files: &[RepoFile]) -> bool {
    let has_config = files.iter().any(|f| f.path == "config.json");
    if !has_config {
        return false;
    }
    let config_path = snapshot_root.join("config.json");
    let Ok(content) = fs::read_to_string(&config_path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    let Some(q) = json.get("quantization") else {
        return false;
    };
    q.is_object() && q.get("quant_type").is_none()
}

fn detect_format_kind(files: &[RepoFile]) -> FormatKind {
    match detect_format_by_extension(files).as_str() {
        "gguf" => FormatKind::Gguf,
        "mlx" => FormatKind::Mlx,
        "safetensors" | "pytorch" => FormatKind::Safetensors,
        _ => FormatKind::Unknown,
    }
}

pub(crate) fn snapshot_repo_files_pub(snapshot_root: &Path) -> Result<Vec<RepoFile>> {
    snapshot_repo_files(snapshot_root)
}

fn snapshot_repo_files(snapshot_root: &Path) -> Result<Vec<RepoFile>> {
    let mut files = Vec::new();
    let mut stack = vec![snapshot_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|source| AppError::Read {
            path: dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| AppError::Read {
                path: dir.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| AppError::Read {
                path: path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(relative) = relative_snapshot_path(snapshot_root, &path)? else {
                continue;
            };
            let metadata = fs::metadata(&path).map_err(|source| AppError::Read {
                path: path.clone(),
                source,
            })?;
            let oid = snapshot_file_oid(&path).unwrap_or_else(|| relative.replace('/', "-"));
            files.push(RepoFile {
                path: relative,
                size_bytes: metadata.len(),
                oid,
            });
        }
    }
    Ok(files)
}

fn relative_snapshot_path(snapshot_root: &Path, path: &Path) -> Result<Option<String>> {
    let Ok(relative) = path.strip_prefix(snapshot_root) else {
        return Ok(None);
    };
    let Some(value) = relative.to_str() else {
        return Err(AppError::InvalidInput(format!(
            "snapshot path is not valid UTF-8: {}",
            path.display()
        )));
    };
    Ok(Some(value.replace(std::path::MAIN_SEPARATOR, "/")))
}

fn snapshot_file_oid(path: &Path) -> Option<String> {
    fs::read_link(path)
        .ok()
        .and_then(|target| target.file_name().map(|name| name.to_os_string()))
        .and_then(|name| name.to_str().map(ToString::to_string))
}

fn repo_from_cache_dir(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let repo = name.strip_prefix("models--")?;
    let mut parts = repo.split("--");
    let org = parts.next()?;
    let model = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{org}/{model}"))
}

#[derive(Clone)]
pub(crate) struct HfCacheEntry {
    pub repo: String,
    pub format: String,
    pub size_bytes: u64,
    pub snapshot_path: PathBuf,
    pub commit: String,
}

pub(crate) fn hf_cache_entries(repo_dirs: &[PathBuf], lock: &LockFile) -> Vec<HfCacheEntry> {
    let mut best_per_repo: std::collections::HashMap<String, HfCacheEntry> =
        std::collections::HashMap::new();

    for repo_dir in repo_dirs {
        let Some(repo) = repo_from_cache_dir(repo_dir) else {
            continue;
        };
        let snapshots_dir = repo_dir.join("snapshots");
        if !snapshots_dir.exists() {
            continue;
        }
        let Ok(dir_iter) = fs::read_dir(&snapshots_dir) else {
            continue;
        };
        for entry in dir_iter.flatten() {
            let snapshot_root = entry.path();
            if !snapshot_root.is_dir() {
                continue;
            }
            let commit = snapshot_root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if commit.is_empty() {
                continue;
            }
            let is_tracked = lock.models.values().any(|m| {
                m.locations
                    .hf_cache
                    .as_ref()
                    .is_some_and(|loc| loc.snapshot_path == snapshot_root)
            });
            if is_tracked {
                continue;
            }
            let files = match snapshot_repo_files(&snapshot_root) {
                Ok(f) => f,
                Err(_) => continue,
            };
            if files.is_empty() {
                continue;
            }
            let format = if has_downloaded_weights(&snapshot_root, &files) {
                detect_format_by_extension(&files)
            } else {
                "incomplete".to_string()
            };
            let size_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
            let candidate = HfCacheEntry {
                repo: repo.clone(),
                format,
                size_bytes,
                snapshot_path: snapshot_root,
                commit,
            };
            best_per_repo
                .entry(repo.clone())
                .and_modify(|existing| {
                    if candidate.size_bytes > existing.size_bytes {
                        *existing = candidate.clone();
                    }
                })
                .or_insert(candidate);
        }
    }
    best_per_repo.into_values().collect()
}

fn detect_format_by_extension(files: &[RepoFile]) -> String {
    let has_gguf = files.iter().any(|f| f.path.ends_with(".gguf"));
    let has_safetensors = files.iter().any(|f| f.path.ends_with(".safetensors"));
    let has_npz = files.iter().any(|f| f.path.ends_with(".npz"));
    let has_bin = files
        .iter()
        .any(|f| f.path.ends_with(".bin") || f.path.ends_with(".pt"));

    if has_gguf {
        "gguf".to_string()
    } else if has_npz {
        "mlx".to_string()
    } else if has_safetensors {
        "safetensors".to_string()
    } else if has_bin {
        "pytorch".to_string()
    } else {
        "unknown".to_string()
    }
}

pub(crate) fn hf_cache_repo_dirs(cache_dir: &Path) -> Result<Vec<PathBuf>> {
    if !cache_dir.exists() {
        return Ok(Vec::new());
    }

    let mut repos = Vec::new();
    for entry in fs::read_dir(cache_dir).map_err(|source| AppError::Read {
        path: cache_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| AppError::Read {
            path: cache_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let is_repo = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("models--"));
        if path.is_dir() && is_repo {
            repos.push(path);
        }
    }
    Ok(repos)
}
