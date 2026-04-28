//! `lmm doctor` — validate tracked models, cache files, and exposures.
use std::fs;

use crate::adapters::builtin_adapters;
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::lock::{LockFile, StateLock};
use crate::model::ExposureStatus;
use crate::paths::{AppPaths, configured_hf_cache_dir};

use super::gc::{hf_cache_tmp_files, state_tmp_files};
use super::{exposure_is_stale, repo_cache_from_snapshot};

pub fn doctor(paths: &AppPaths, fix: bool, deep: bool) -> Result<()> {
    let _state_lock = if fix {
        Some(StateLock::acquire(&paths.lock_path)?)
    } else {
        None
    };
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let mut lock = LockFile::load(&paths.lock_path)?;
    let mut missing_files = 0_u64;
    let mut size_mismatches = 0_u64;
    let mut missing_blobs = 0_u64;
    let mut stale_exposures = 0_u64;
    let partial_temp_files = state_tmp_files(paths)?.len() as u64
        + if deep {
            hf_cache_tmp_files(&hf_cache)?.len() as u64
        } else {
            0
        };
    let orphan_blobs = if deep {
        super::gc::hf_orphan_blob_files(&hf_cache, &lock, false)?.len() as u64
    } else {
        0
    };

    for model in lock.models.values_mut() {
        let Some(location) = &model.locations.hf_cache else {
            missing_files += model.artifact.files.len() as u64;
            continue;
        };

        for file in &model.artifact.files {
            let snapshot_file = crate::security::safe_join(&location.snapshot_path, &file.path)?;
            if !snapshot_file.exists() {
                missing_files += 1;
                continue;
            }

            if deep {
                let metadata = fs::metadata(&snapshot_file).map_err(|source| AppError::Read {
                    path: snapshot_file.clone(),
                    source,
                })?;
                if metadata.len() != file.size_bytes {
                    size_mismatches += 1;
                }

                if let Some(blob) = &file.hf_blob {
                    let blob_path = repo_cache_from_snapshot(&location.snapshot_path)
                        .map(|repo_cache| repo_cache.join("blobs").join(blob))
                        .or_else(|| crate::hf::blob_path(&hf_cache, &model.source.repo, blob).ok());
                    let Some(blob_path) = blob_path else { continue };
                    if !blob_path.exists() {
                        missing_blobs += 1;
                    }
                }
            }
        }

        for entry in model.exposures.values_mut() {
            if exposure_is_stale(entry) {
                stale_exposures += 1;
                if fix {
                    entry.status = ExposureStatus::Stale;
                }
            }
        }
    }

    format::heading("Doctor");
    format::kv("state", &paths.state_dir.display().to_string());
    format::kv("config", &paths.config_path.display().to_string());
    format::kv("lock", &paths.lock_path.display().to_string());
    format::kv("hf cache", &hf_cache.display().to_string());
    format::kv("fix", &fix.to_string());
    format::kv("deep", &deep.to_string());
    eprintln!();

    format::heading("Inventory");
    format::kv("tracked models", &lock.models.len().to_string());
    kv_check("missing snapshot files", missing_files);
    if deep {
        kv_check("size mismatches", size_mismatches);
        kv_check("missing blobs", missing_blobs);
    }
    kv_check("stale exposures", stale_exposures);
    kv_check("partial temp files", partial_temp_files);
    if deep {
        kv_check("orphan blobs", orphan_blobs);
    }
    eprintln!();

    let external_only = count_external_only_models(&config, &lock)?;

    format::heading("Adapters");
    for adapter in builtin_adapters() {
        format::kv(adapter.id, &format::dim(&format!("{:?}", adapter.class)));
    }

    if external_only > 0 {
        eprintln!();
        format::heading("Suggestions");

        let consolidation_candidates = super::consolidate::discover_candidates(&config);
        let untracked: Vec<_> = consolidation_candidates
            .into_iter()
            .filter(|c| !lock.models.values().any(|m| m.source.repo == c.repo_id))
            .collect();

        eprintln!(
            "  {} external model(s) found ({} consolidation candidates).",
            format::yellow(&external_only.to_string()),
            untracked.len(),
        );

        if !untracked.is_empty() && fix && crate::tui::can_run() {
            let client = crate::hf::HfClient::new(config.network.hf_endpoint, hf_cache.clone());
            let _state_lock_for_write = if _state_lock.is_none() {
                Some(StateLock::acquire(&paths.lock_path)?)
            } else {
                None
            };
            match super::consolidate::consolidate_interactive(
                &client, &mut lock, &untracked, &hf_cache,
            ) {
                Err(AppError::Cancelled) => {}
                other => {
                    other?;
                }
            }
            lock.save(&paths.lock_path)?;
        } else if !untracked.is_empty() && !fix {
            eprintln!(
                "  Run {} to consolidate into HF Cache.",
                format::cyan("lmm doctor --fix")
            );
        } else if untracked.is_empty() {
            eprintln!(
                "  These use extra disk space. Run {} to consolidate.",
                format::cyan("lmm add <repo> --tool <tool>")
            );
        }
    }

    if fix && stale_exposures > 0 {
        lock.save(&paths.lock_path)?;
    }

    eprintln!();
    let all_clean = missing_files == 0
        && size_mismatches == 0
        && missing_blobs == 0
        && stale_exposures == 0
        && partial_temp_files == 0
        && orphan_blobs == 0;

    if all_clean && external_only == 0 {
        format::status("\u{2713}", &format::green("all checks passed"));
    } else if all_clean {
        format::status(
            "\u{2713}",
            &format::green("checks passed (external models noted above)"),
        );
    } else if fix {
        format::status(
            "\u{2713}",
            &format::yellow("issues found; fixable exposure statuses updated"),
        );
    } else {
        format::status(
            "!",
            &format::yellow("issues found; re-run with --fix to mark stale exposures"),
        );
    }
    Ok(())
}

fn count_external_only_models(config: &Config, lock: &LockFile) -> Result<u64> {
    let externals = super::discover_external_exposures(config, lock)?;
    Ok(externals.len() as u64)
}

fn kv_check(key: &str, value: u64) {
    if value == 0 {
        format::kv(key, &format::green("0"));
    } else {
        format::kv(key, &format::yellow(&value.to_string()));
    }
}
