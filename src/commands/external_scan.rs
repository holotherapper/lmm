//! Discovery and scanning of external (non-lmm-managed) model exposures.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::paths::expand_tilde;

#[derive(Clone, Debug)]
pub(crate) struct ExternalExposure {
    #[allow(dead_code)]
    pub id: String,
    pub name: String,
    pub tool: String,
    #[allow(dead_code)]
    pub path: PathBuf,
    pub size_hint: u64,
}

pub(crate) fn discover_external_exposures(
    config: &Config,
    lock: &crate::lock::LockFile,
) -> Result<Vec<ExternalExposure>> {
    let tracked_paths = tracked_exposure_paths(lock);
    let mut exposures = Vec::new();

    if let Some(root) = config.paths.lmstudio.as_deref() {
        let root = expand_tilde(root);
        exposures.extend(scan_lmstudio_exposures(&root, &tracked_paths)?);
    }
    if let Some(root) = config.paths.jan.as_deref() {
        let root = expand_tilde(root);
        exposures.extend(scan_direct_tool_exposures("jan", &root, &tracked_paths)?);
    }

    let tracked_ollama_names: std::collections::HashSet<String> = lock
        .models
        .values()
        .filter_map(|m| {
            m.exposures
                .get("ollama")
                .and_then(|e| e.path.as_ref())
                .map(|p| p.to_string_lossy().to_string())
        })
        .collect();
    exposures.extend(scan_ollama_models(&tracked_ollama_names));

    Ok(exposures)
}

fn tracked_exposure_paths(lock: &crate::lock::LockFile) -> BTreeSet<PathBuf> {
    lock.models
        .values()
        .flat_map(|model| model.exposures.values())
        .filter_map(|entry| entry.path.clone())
        .collect()
}

pub(crate) fn scan_lmstudio_exposures(
    root: &Path,
    tracked_paths: &BTreeSet<PathBuf>,
) -> Result<Vec<ExternalExposure>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut exposures = Vec::new();
    for author in std::fs::read_dir(root).map_err(|source| AppError::Read {
        path: root.to_path_buf(),
        source,
    })? {
        let author = author.map_err(|source| AppError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        let author_path = author.path();
        if !author_path.is_dir() {
            continue;
        }
        for model in std::fs::read_dir(&author_path).map_err(|source| AppError::Read {
            path: author_path.clone(),
            source,
        })? {
            let model = model.map_err(|source| AppError::Read {
                path: author_path.clone(),
                source,
            })?;
            let path = model.path();
            if !path.is_dir() || tracked_paths.contains(&path) {
                continue;
            }
            let Some(name) = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(ToString::to_string)
            else {
                continue;
            };
            exposures.push(ExternalExposure {
                id: external_exposure_id("lmstudio", &path),
                name,
                tool: "lmstudio".to_string(),
                path,
                size_hint: 0,
            });
        }
    }
    Ok(exposures)
}

pub(crate) fn scan_direct_tool_exposures(
    tool: &str,
    root: &Path,
    tracked_paths: &BTreeSet<PathBuf>,
) -> Result<Vec<ExternalExposure>> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut exposures = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|source| AppError::Read {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| AppError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if !path.is_dir() || tracked_paths.contains(&path) {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToString::to_string)
        else {
            continue;
        };
        exposures.push(ExternalExposure {
            id: external_exposure_id(tool, &path),
            name,
            tool: tool.to_string(),
            path,
            size_hint: 0,
        });
    }
    Ok(exposures)
}

pub(crate) fn external_exposure_id(tool: &str, path: &Path) -> String {
    format!("external:{tool}:{}", path.display())
}

pub(crate) fn scan_ollama_models(
    tracked_names: &std::collections::HashSet<String>,
) -> Vec<ExternalExposure> {
    let ollama_dir = dirs_ollama_manifests();
    let Some(library_dir) = ollama_dir else {
        return Vec::new();
    };
    let Ok(models) = std::fs::read_dir(&library_dir) else {
        return Vec::new();
    };

    let mut exposures = Vec::new();
    for model_entry in models.flatten() {
        let model_path = model_entry.path();
        if !model_path.is_dir() {
            continue;
        }
        let Some(model_name) = model_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let Ok(tags) = std::fs::read_dir(&model_path) else {
            continue;
        };
        for tag_entry in tags.flatten() {
            let tag_path = tag_entry.path();
            if !tag_path.is_file() {
                continue;
            }
            let Some(tag) = tag_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            let full_name = format!("{model_name}:{tag}");
            if tracked_names.contains(&full_name) {
                continue;
            }

            let size = ollama_manifest_size(&tag_path);
            exposures.push(ExternalExposure {
                id: format!("external:ollama:{full_name}"),
                name: full_name,
                tool: "ollama".to_string(),
                path: tag_path,
                size_hint: size,
            });
        }
    }
    exposures
}

fn dirs_ollama_manifests() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = home.join(".ollama/models/manifests/registry.ollama.ai/library");
    if dir.is_dir() { Some(dir) } else { None }
}

pub(crate) fn ollama_manifest_size(manifest_path: &Path) -> u64 {
    let Ok(bytes) = std::fs::read(manifest_path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return 0;
    };
    value
        .get("layers")
        .and_then(|l| l.as_array())
        .map(|layers| {
            layers
                .iter()
                .filter_map(|l| l.get("size").and_then(|s| s.as_u64()))
                .sum()
        })
        .unwrap_or(0)
}
