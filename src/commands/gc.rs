//! `lmm gc` — clean temporary files, stale entries, and orphan blobs.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::lock::{LockFile, StateLock};
use crate::model::Ownership;
use crate::paths::{AppPaths, configured_hf_cache_dir};

use super::exposure_is_stale;
use super::scan::hf_cache_repo_dirs;

pub fn gc(paths: &AppPaths, yes: bool, include_adopted: bool) -> Result<()> {
    let _state_lock = if yes {
        Some(StateLock::acquire(&paths.lock_path)?)
    } else {
        None
    };
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let mut lock = LockFile::load(&paths.lock_path)?;

    let mut removable_files = state_tmp_files(paths)?;
    removable_files.extend(hf_cache_tmp_files(&hf_cache)?);
    removable_files.extend(hf_orphan_blob_files(&hf_cache, &lock, include_adopted)?);
    let incomplete_dirs = find_incomplete_snapshots(&hf_cache, &lock)?;
    let removable_bytes = total_size(&removable_files)?;
    let incomplete_bytes: u64 = incomplete_dirs.iter().map(|(_, size)| *size).sum();
    let stale_exposures = count_stale_exposures(&lock);

    format::heading("GC plan");
    format::kv("include adopted", &include_adopted.to_string());
    format::kv("removable files", &removable_files.len().to_string());
    format::kv("removable bytes", &format::bytes(removable_bytes));
    format::kv("incomplete snapshots", &incomplete_dirs.len().to_string());
    format::kv("incomplete bytes", &format::bytes(incomplete_bytes));
    format::kv("stale lock entries", &stale_exposures.to_string());

    if !yes {
        format::status("⊘", &format::dim("dry-run; re-run with --yes to clean"));
        return Ok(());
    }

    let mut progress = format::ProgressLine::new("Cleaning", removable_files.len());
    for file in &removable_files {
        if file.exists() {
            progress.inc(&file.display().to_string());
            fs::remove_file(file).map_err(|source| AppError::Write {
                path: file.clone(),
                source,
            })?;
        }
    }
    if !removable_files.is_empty() {
        progress.finish();
    }

    for (dir, _) in &incomplete_dirs {
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(|source| AppError::Write {
                path: dir.clone(),
                source,
            })?;
        }
    }

    remove_stale_exposure_entries(&mut lock);
    lock.save(&paths.lock_path)?;

    let total_freed = removable_bytes + incomplete_bytes;
    format::status(
        "✓",
        &format!(
            "{} (freed {}, removed {} stale entries, {} incomplete snapshots)",
            format::green("cleaned"),
            format::bytes(total_freed),
            stale_exposures,
            incomplete_dirs.len()
        ),
    );
    Ok(())
}

pub(crate) fn state_tmp_files(paths: &AppPaths) -> Result<Vec<PathBuf>> {
    collect_files_with_suffix(&paths.state_dir.join("tmp"), None, false)
}

pub(crate) fn hf_cache_tmp_files(hf_cache: &Path) -> Result<Vec<PathBuf>> {
    collect_files_with_suffix(hf_cache, Some(".tmp"), true)
}

pub(crate) fn hf_orphan_blob_files(
    hf_cache: &Path,
    lock: &LockFile,
    include_adopted: bool,
) -> Result<Vec<PathBuf>> {
    if !hf_cache.exists() {
        return Ok(Vec::new());
    }

    let lock_blobs = locked_blob_names(lock, include_adopted);
    let mut orphans = Vec::new();
    for repo_dir in hf_cache_repo_dirs(hf_cache)? {
        let referenced = snapshot_referenced_blob_names(&repo_dir)?;
        let blobs_dir = repo_dir.join("blobs");
        if !blobs_dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&blobs_dir).map_err(|source| AppError::Read {
            path: blobs_dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| AppError::Read {
                path: blobs_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(blob_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !referenced.contains(blob_name) && !lock_blobs.contains(blob_name) {
                orphans.push(path);
            }
        }
    }
    Ok(orphans)
}

fn locked_blob_names(lock: &LockFile, include_adopted: bool) -> BTreeSet<String> {
    lock.models
        .values()
        .filter(|model| !include_adopted || model.source.ownership != Ownership::Adopted)
        .flat_map(|model| model.artifact.files.iter())
        .filter_map(|file| file.hf_blob.clone())
        .collect()
}

fn snapshot_referenced_blob_names(repo_dir: &Path) -> Result<BTreeSet<String>> {
    let snapshots = repo_dir.join("snapshots");
    if !snapshots.exists() {
        return Ok(BTreeSet::new());
    }

    let mut referenced = BTreeSet::new();
    let mut stack = vec![snapshots];
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
            if let Ok(target) = fs::read_link(&path)
                && let Some(blob) = target.file_name().and_then(|name| name.to_str())
            {
                referenced.insert(blob.to_string());
            }
        }
    }
    Ok(referenced)
}

fn collect_files_with_suffix(
    root: &Path,
    suffix: Option<&str>,
    require_age: bool,
) -> Result<Vec<PathBuf>> {
    use std::time::{Duration, SystemTime};

    if !root.exists() {
        return Ok(Vec::new());
    }

    let age_threshold = SystemTime::now() - Duration::from_secs(3600);
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
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
            let matches_suffix = suffix.is_none_or(|suffix| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(suffix))
            });
            if !matches_suffix {
                continue;
            }
            if require_age {
                let is_old_enough = fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .map(|mtime| mtime < age_threshold)
                    .unwrap_or(true);
                if !is_old_enough {
                    continue;
                }
            }
            files.push(path);
        }
    }
    Ok(files)
}

fn total_size(paths: &[PathBuf]) -> Result<u64> {
    let mut bytes = 0_u64;
    for path in paths {
        bytes += fs::metadata(path)
            .map_err(|source| AppError::Read {
                path: path.clone(),
                source,
            })?
            .len();
    }
    Ok(bytes)
}

fn count_stale_exposures(lock: &LockFile) -> u64 {
    lock.models
        .values()
        .flat_map(|model| model.exposures.values())
        .filter(|entry| exposure_is_stale(entry))
        .count() as u64
}

fn remove_stale_exposure_entries(lock: &mut LockFile) {
    for model in lock.models.values_mut() {
        model.exposures.retain(|_, entry| !exposure_is_stale(entry));
    }
}

fn find_incomplete_snapshots(hf_cache: &Path, lock: &LockFile) -> Result<Vec<(PathBuf, u64)>> {
    use super::scan::{has_downloaded_weights, hf_cache_repo_dirs};

    let mut results = Vec::new();
    for repo_dir in hf_cache_repo_dirs(hf_cache)? {
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
            let files = match super::scan::snapshot_repo_files_pub(&snapshot_root) {
                Ok(f) => f,
                Err(_) => continue,
            };
            if files.is_empty() || has_downloaded_weights(&snapshot_root, &files) {
                continue;
            }
            let size: u64 = files.iter().map(|f| f.size_bytes).sum();
            results.push((snapshot_root, size));
        }
    }
    Ok(results)
}
