//! `lmm adopt` — bring untracked HF Cache models into lmm management.
use std::collections::BTreeMap;
use std::path::Path;

use crate::adapters::{ExposeRequest, create_exposures};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::lock::{LockFile, StateLock};
use crate::model::{ExposureEntry, ExposureStatus};
use crate::paths::{AppPaths, configured_hf_cache_dir, expand_tilde};
use crate::tui;

use super::scan::{AdoptionCandidate, adoption_candidates, hf_cache_repo_dirs};
use super::{AdoptInput, FormatSelection, resolve_tools, validate_tools};

pub fn adopt(paths: &AppPaths, input: &AdoptInput) -> Result<()> {
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let lock = LockFile::load(&paths.lock_path)?;
    let all_candidates =
        adoption_candidates(&hf_cache_repo_dirs(&hf_cache)?, &lock, input.take_ownership)?;

    if all_candidates.is_empty() {
        format::status(
            "\u{229c}",
            &format::dim("no adoptable HF Cache snapshots found"),
        );
        eprintln!(
            "  {}",
            format::dim(
                "All models with downloaded weights are already tracked, or no complete models exist in HF Cache."
            )
        );
        return Ok(());
    }

    let candidates = select_candidates(all_candidates, input)?;
    if candidates.is_empty() {
        return Ok(());
    }

    let _state_lock = StateLock::acquire(&paths.lock_path)?;
    let mut lock = LockFile::load(&paths.lock_path)?;
    let mut adopted = 0_usize;
    for candidate in candidates {
        if lock.models.contains_key(&candidate.id) {
            continue;
        }
        let mut exposures = detect_existing_exposures(&config, &candidate.snapshot_root);

        let candidate_tools = if input.tools.is_empty() {
            Vec::new()
        } else {
            let tools = resolve_tools(&input.tools, candidate.model.format, &config);
            validate_tools(&tools, FormatSelection::Format(candidate.model.format))?;
            tools
        };
        if !candidate_tools.is_empty() {
            let new_exposures = create_exposures(&ExposeRequest {
                tools: &candidate_tools,
                repo: &candidate.model.source.repo,
                alias: &candidate.model.name,
                format: candidate.model.format,
                snapshot_root: &candidate.snapshot_root,
                files: &candidate.repo_files,
                config: &config,
                replace: false,
                ollama_name: None,
            })?;
            exposures.extend(new_exposures);
        }

        let mut model = candidate.model;
        model.exposures = exposures;
        lock.models.insert(candidate.id, model);
        adopted += 1;
    }
    lock.save(&paths.lock_path)?;
    format::outro(&format!("Adopted {adopted} models"));
    Ok(())
}

fn detect_existing_exposures(
    config: &Config,
    snapshot_root: &Path,
) -> BTreeMap<String, ExposureEntry> {
    let mut exposures = BTreeMap::new();

    if let Some(lmstudio_path) = &config.paths.lmstudio {
        let root = expand_tilde(lmstudio_path);
        if let Some(entry) = scan_symlink_exposure(&root, snapshot_root) {
            exposures.insert("lmstudio".to_string(), entry);
        }
    }

    if let Some(jan_path) = &config.paths.jan {
        let root = expand_tilde(jan_path);
        if let Some(entry) = scan_jan_model_yml(&root, snapshot_root) {
            exposures.insert("jan".to_string(), entry);
        }
    }

    exposures
}

fn scan_symlink_exposure(root: &Path, snapshot_path: &Path) -> Option<ExposureEntry> {
    scan_symlink_recursive(root, snapshot_path, 0)
}

fn scan_symlink_recursive(dir: &Path, snapshot_path: &Path, depth: u32) -> Option<ExposureEntry> {
    if depth > 3 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if scan_dir_for_symlink_target(&path, snapshot_path) {
            return Some(ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "symlink".to_string(),
                path: Some(path),
                created_by: Some("detected".to_string()),
            });
        }
        if let Some(found) = scan_symlink_recursive(&path, snapshot_path, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn scan_dir_for_symlink_target(dir: &Path, snapshot_path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let Some(repo_cache) = snapshot_path.parent().and_then(|p| p.parent()) else {
        return false;
    };
    let repo_prefix = format!("{}/", repo_cache.to_string_lossy());

    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            if let Ok(target) = std::fs::read_link(&path) {
                let full = if target.is_absolute() {
                    target.to_string_lossy().to_string()
                } else {
                    path.parent()
                        .unwrap_or(Path::new("."))
                        .join(&target)
                        .to_string_lossy()
                        .to_string()
                };
                if full.starts_with(&repo_prefix) || full == repo_cache.to_string_lossy().as_ref() {
                    return true;
                }
            }
            if let Ok(canonical) = path.canonicalize()
                && canonical.starts_with(repo_cache)
            {
                return true;
            }
        }
    }
    false
}

fn scan_jan_model_yml(root: &Path, snapshot_path: &Path) -> Option<ExposureEntry> {
    let snapshot_str = snapshot_path.to_string_lossy();
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let yml_path = path.join("model.yml");
        if let Ok(content) = std::fs::read_to_string(&yml_path)
            && content.contains(snapshot_str.as_ref())
        {
            return Some(ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "jan-model-yml".to_string(),
                path: Some(path),
                created_by: Some("detected".to_string()),
            });
        }
    }
    None
}

fn select_candidates(
    all: Vec<AdoptionCandidate>,
    input: &AdoptInput,
) -> Result<Vec<AdoptionCandidate>> {
    if !input.names.is_empty() {
        let filtered: Vec<_> = all
            .into_iter()
            .filter(|c| {
                input
                    .names
                    .iter()
                    .any(|name| c.model.name.eq_ignore_ascii_case(name))
            })
            .collect();
        if filtered.is_empty() {
            format::status(
                "\u{229c}",
                &format::dim("no matching adoptable candidates found"),
            );
        }
        return Ok(filtered);
    }

    if input.yes {
        return Ok(all);
    }

    if !tui::can_run() {
        return Err(AppError::InvalidInput(
            "adopt needs model names or -y in non-interactive mode\n  Example: lmm adopt -y  or  lmm adopt Qwen3-8B-4bit-mlx".to_string(),
        ));
    }

    let items: Vec<(String, String, String)> = all
        .iter()
        .map(|c| {
            (
                c.id.clone(),
                format!(
                    "{} · {} · {}",
                    c.model.name,
                    c.model.format,
                    format::bytes(c.model.artifact.size_bytes),
                ),
                c.model.source.repo.clone(),
            )
        })
        .collect();

    let selected_ids = tui::select_many("Adopt models", &items, &[])?;

    Ok(all
        .into_iter()
        .filter(|c| selected_ids.contains(&c.id))
        .collect())
}
