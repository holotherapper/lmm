//! `lmm remove` — uninstall model exposures and reclaim cache bytes.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::adapters::{self, remove_exposure};
use crate::error::{AppError, Result};
use crate::format;
use crate::lock::{LockFile, StateLock};
use crate::model::ModelEntry;
use crate::paths::AppPaths;
use crate::tui;

use super::{
    ExternalExposure, FormatSelection, RemoveInput, confirm, discover_external_exposures,
    remove_all_tools, repo_cache_from_snapshot, tool_list, validate_tools,
};

#[derive(Debug)]
struct UntrackedTarget {
    repo: String,
    cache_dir: PathBuf,
    size_bytes: u64,
}

#[derive(Debug)]
struct ExternalTarget {
    name: String,
    tool: String,
    path: PathBuf,
    size_bytes: u64,
}

pub fn remove(paths: &AppPaths, input: &RemoveInput) -> Result<()> {
    validate_tools(&input.tools, FormatSelection::Auto)?;
    let _state_lock = StateLock::acquire(&paths.lock_path)?;
    let config = crate::config::Config::load(&paths.config_path)?;
    let hf_cache = crate::paths::configured_hf_cache_dir(&config);
    let mut lock = LockFile::load(&paths.lock_path)?;
    let (targets, untracked, externals) = remove_targets(&lock, &hf_cache, &config, input)?;
    if targets.is_empty() && untracked.is_empty() && externals.is_empty() {
        format::status("⊘", &format::dim("no matching models found"));
        return Ok(());
    }
    print_remove_plan(input, &lock, &targets, &untracked, &externals);
    if input.dry_run {
        format::status("⊘", &format::dim("plan-only; --dry-run prevented removal"));
        return Ok(());
    }
    if !confirm(input.yes, "Remove selected models?")? {
        return Ok(());
    }

    let mut actual_freed_bytes = 0_u64;
    for id in &targets {
        let Some(mut model) = lock.models.remove(id) else {
            continue;
        };
        let tools = if remove_all_tools(input) {
            model.exposures.keys().cloned().collect::<Vec<_>>()
        } else {
            input.tools.clone()
        };

        for tool in tools {
            if let Some(entry) = model.exposures.remove(&tool) {
                remove_exposure(&tool, &entry, &config)?;
            }
        }

        if input.keep_cache || !model.exposures.is_empty() {
            lock.models.insert(id.clone(), model);
            continue;
        }

        if model.source.ownership.deletes_canonical_bytes_by_default() || input.purge_cache {
            actual_freed_bytes += remove_canonical_files(&model)?;
        }
    }

    for target in &untracked {
        actual_freed_bytes += remove_cache_dir(&target.cache_dir)?;
    }

    for target in &externals {
        actual_freed_bytes += remove_external(&target.tool, &target.name, &target.path)?;
    }

    lock.save(&paths.lock_path)?;
    format::outro(&format!(
        "Removed (freed {})",
        format::bytes(actual_freed_bytes)
    ));
    Ok(())
}

fn print_remove_plan(
    input: &RemoveInput,
    lock: &LockFile,
    targets: &[String],
    untracked: &[UntrackedTarget],
    externals: &[ExternalTarget],
) {
    format::heading("Remove plan");
    format::kv("target", &remove_target_label(input));
    format::kv("tools", &tool_list(&input.tools));
    format::kv("keep cache", &input.keep_cache.to_string());
    format::kv("purge adopted cache", &input.purge_cache.to_string());
    format::kv(
        "models",
        &(targets.len() + untracked.len() + externals.len()).to_string(),
    );
    let tracked_freed = expected_remove_freed_bytes(input, lock, targets);
    let untracked_freed: u64 = untracked.iter().map(|t| t.size_bytes).sum();
    let external_freed: u64 = externals.iter().map(|t| t.size_bytes).sum();
    format::kv(
        "expected freed",
        &format::bytes(tracked_freed + untracked_freed + external_freed),
    );
    for target in untracked {
        format::kv(
            "untracked",
            &format!("{} ({})", target.repo, format::bytes(target.size_bytes)),
        );
    }
    for target in externals {
        format::kv(
            &format!("external ({})", target.tool),
            &format!("{} ({})", target.name, format::bytes(target.size_bytes)),
        );
    }
}

fn expected_remove_freed_bytes(input: &RemoveInput, lock: &LockFile, targets: &[String]) -> u64 {
    targets
        .iter()
        .filter_map(|id| lock.models.get(id))
        .filter(|model| {
            !input.keep_cache
                && (model.source.ownership.deletes_canonical_bytes_by_default()
                    || input.purge_cache)
                && removes_all_exposures(input, model)
        })
        .map(|model| model.artifact.size_bytes)
        .sum()
}

fn removes_all_exposures(input: &RemoveInput, model: &ModelEntry) -> bool {
    remove_all_tools(input)
        || model.exposures.keys().all(|tool| {
            input
                .tools
                .iter()
                .any(|selected_tool| selected_tool == tool)
        })
}

fn remove_targets(
    lock: &LockFile,
    hf_cache: &Path,
    config: &crate::config::Config,
    input: &RemoveInput,
) -> Result<(Vec<String>, Vec<UntrackedTarget>, Vec<ExternalTarget>)> {
    if input.all {
        return Ok((
            lock.models.keys().cloned().collect(),
            Vec::new(),
            Vec::new(),
        ));
    }
    if input.names.is_empty() {
        if tui::can_run() {
            return select_remove_targets(lock, hf_cache, config, input);
        }
        return Err(AppError::InvalidInput(
            "remove needs a model name or --all".to_string(),
        ));
    }

    let externals = discover_external_exposures(config, lock)?;
    let mut targets = Vec::new();
    let mut untracked = Vec::new();
    let mut external_targets = Vec::new();
    let mut missing = Vec::new();
    for name in &input.names {
        let found: Vec<String> = lock
            .models
            .iter()
            .filter(|(_, model)| {
                super::model_name_matches(&model.name, name)
                    || (name.contains('/') && model.source.repo.eq_ignore_ascii_case(name))
            })
            .filter(|(_, model)| tracked_matches_tools(model, input))
            .map(|(id, _)| id.clone())
            .collect();
        if !found.is_empty() {
            targets.extend(found);
            continue;
        }

        if let Some(ext) = externals
            .iter()
            .find(|e| super::model_name_matches(&e.name, name))
        {
            external_targets.push(external_target_from(ext));
            continue;
        }

        if name.contains('/') {
            if let Some(target) = find_untracked_target(hf_cache, name) {
                untracked.push(target);
                continue;
            }
        }

        missing.push(name.clone());
    }

    let targets = dedupe(targets);
    if missing.is_empty() {
        Ok((targets, untracked, external_targets))
    } else {
        Err(AppError::InvalidInput(format!(
            "model not found: {}",
            missing.join(", ")
        )))
    }
}

fn select_remove_targets(
    lock: &LockFile,
    hf_cache: &Path,
    config: &crate::config::Config,
    input: &RemoveInput,
) -> Result<(Vec<String>, Vec<UntrackedTarget>, Vec<ExternalTarget>)> {
    let externals = discover_external_exposures(config, lock)?;
    let hf_entries =
        super::scan::hf_cache_entries(&super::scan::hf_cache_repo_dirs(hf_cache)?, lock);

    let mut items: Vec<(String, String, String)> = lock
        .models
        .iter()
        .filter(|(_, model)| tracked_matches_tools(model, input))
        .map(|(id, model)| {
            (
                id.clone(),
                format!(
                    "{} · {} · {}",
                    model.name,
                    model.format,
                    format::bytes(model.artifact.size_bytes)
                ),
                "tracked".to_string(),
            )
        })
        .collect();

    for ext in &externals {
        items.push((
            format!("external:{}", ext.id),
            format!(
                "{} · external ({}) · {}",
                ext.name,
                ext.tool,
                format::bytes(ext.size_hint)
            ),
            format!("external ({})", ext.tool),
        ));
    }

    for entry in &hf_entries {
        items.push((
            format!("untracked:{}", entry.repo),
            format!(
                "{} · {} · untracked",
                entry.repo,
                format::bytes(entry.size_bytes)
            ),
            "untracked HF cache".to_string(),
        ));
    }

    if items.is_empty() {
        return Ok((Vec::new(), Vec::new(), Vec::new()));
    }

    let selected_ids = tui::select_many("Remove models", &items, &[])?;

    let mut tracked_ids = Vec::new();
    let mut untracked_targets = Vec::new();
    let mut external_targets = Vec::new();
    for id in &selected_ids {
        if let Some(stripped) = id.strip_prefix("external:") {
            if let Some(ext) = externals.iter().find(|e| e.id == stripped) {
                external_targets.push(external_target_from(ext));
            }
        } else if let Some(repo) = id.strip_prefix("untracked:") {
            if let Some(target) = find_untracked_target(hf_cache, repo) {
                untracked_targets.push(target);
            }
        } else {
            tracked_ids.push(id.clone());
        }
    }

    Ok((tracked_ids, untracked_targets, external_targets))
}

fn external_target_from(ext: &ExternalExposure) -> ExternalTarget {
    let size = if ext.size_hint > 0 {
        ext.size_hint
    } else {
        dir_total_size(&ext.path)
    };
    ExternalTarget {
        name: ext.name.clone(),
        tool: ext.tool.clone(),
        path: ext.path.clone(),
        size_bytes: size,
    }
}

fn remove_external(tool: &str, name: &str, path: &Path) -> Result<u64> {
    let adapter = adapters::find_adapter(tool)?;
    match adapter.class() {
        adapters::AdapterClass::ImportingRegistry => {
            if tool == "ollama" {
                let output = std::process::Command::new("ollama")
                    .args(["rm", name])
                    .output()
                    .map_err(|source| AppError::Read {
                        path: PathBuf::from("ollama"),
                        source,
                    })?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(AppError::InvalidInput(format!(
                        "ollama rm failed: {}",
                        stderr.trim()
                    )));
                }
            }
            Ok(0)
        }
        _ => {
            let size = dir_total_size(path);
            if path.is_dir() {
                fs::remove_dir_all(path).map_err(|source| AppError::Write {
                    path: path.to_path_buf(),
                    source,
                })?;
            } else if path.exists() {
                fs::remove_file(path).map_err(|source| AppError::Write {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
            cleanup_empty_parent(path);
            Ok(size)
        }
    }
}

fn cleanup_empty_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
}

fn remove_target_label(input: &RemoveInput) -> String {
    if input.all {
        "all tracked".to_string()
    } else if input.names.is_empty() {
        "interactive".to_string()
    } else {
        input.names.join(", ")
    }
}

fn tracked_matches_tools(model: &ModelEntry, input: &RemoveInput) -> bool {
    remove_all_tools(input)
        || input
            .tools
            .iter()
            .any(|tool| model.exposures.contains_key(tool))
}

fn dedupe(ids: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

pub(crate) fn remove_canonical_files(model: &ModelEntry) -> Result<u64> {
    let Some(location) = &model.locations.hf_cache else {
        return Ok(0);
    };
    let mut freed_bytes = 0_u64;
    for file in &model.artifact.files {
        let path = crate::security::safe_join(&location.snapshot_path, &file.path)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|source| AppError::Write {
                path: path.clone(),
                source,
            })?;
        }
        remove_empty_dirs_up_to(&path, &location.snapshot_path);
        if let Some(blob) = &file.hf_blob {
            if blob.contains('/') || blob.contains("..") {
                continue;
            }
            let Some(repo_cache) = repo_cache_from_snapshot(&location.snapshot_path) else {
                continue;
            };
            let blob_path = repo_cache.join("blobs").join(blob);
            if blob_path.exists() && !blob_is_referenced(&repo_cache, blob, &path)? {
                freed_bytes += fs::metadata(&blob_path)
                    .map_err(|source| AppError::Read {
                        path: blob_path.clone(),
                        source,
                    })?
                    .len();
                fs::remove_file(&blob_path).map_err(|source| AppError::Write {
                    path: blob_path,
                    source,
                })?;
            }
        }
    }
    remove_empty_parents(&location.snapshot_path)?;
    if let Some(repo_cache) = repo_cache_from_snapshot(&location.snapshot_path) {
        cleanup_empty_repo_cache(&repo_cache);
    }
    Ok(freed_bytes)
}

fn remove_empty_parents(path: &Path) -> Result<()> {
    let mut current = path.to_path_buf();
    while current.exists() {
        match fs::remove_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(AppError::Write {
                    path: current,
                    source,
                });
            }
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent.file_name().is_some_and(|name| name == "snapshots") {
            break;
        }
        current = parent.to_path_buf();
    }
    Ok(())
}

fn remove_empty_dirs_up_to(from: &Path, stop_at: &Path) {
    let mut current = from.to_path_buf();
    while let Some(parent) = current.parent() {
        if parent == stop_at || !parent.starts_with(stop_at) {
            break;
        }
        let _ = fs::remove_dir(parent);
        current = parent.to_path_buf();
    }
}

fn cleanup_empty_repo_cache(repo_cache: &Path) {
    let snapshots = repo_cache.join("snapshots");
    let has_remaining_snapshots = snapshots
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some());
    if has_remaining_snapshots {
        return;
    }
    let _ = fs::remove_dir_all(repo_cache);
}

fn find_untracked_target(hf_cache: &Path, repo: &str) -> Option<UntrackedTarget> {
    let dir_name = format!("models--{}", repo.replace('/', "--"));
    let cache_dir = hf_cache.join(&dir_name);
    if !cache_dir.is_dir() {
        return None;
    }
    Some(UntrackedTarget {
        repo: repo.to_string(),
        cache_dir: cache_dir.clone(),
        size_bytes: dir_total_size(&cache_dir),
    })
}

fn remove_cache_dir(cache_dir: &Path) -> Result<u64> {
    let size = dir_total_size(cache_dir);
    fs::remove_dir_all(cache_dir).map_err(|source| AppError::Write {
        path: cache_dir.to_path_buf(),
        source,
    })?;
    Ok(size)
}

fn dir_total_size(path: &Path) -> u64 {
    let mut size = 0_u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if let Ok(meta) = entry_path.metadata() {
                size += meta.len();
            }
        }
    }
    size
}

fn blob_is_referenced(repo_cache: &Path, blob: &str, removed_path: &Path) -> Result<bool> {
    let snapshots = repo_cache.join("snapshots");
    if !snapshots.exists() {
        return Ok(false);
    }
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
            if path == removed_path {
                continue;
            }
            let file_type = entry.file_type().map_err(|source| AppError::Read {
                path: path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if let Ok(target) = fs::read_link(&path)
                && target.file_name().and_then(|n| n.to_str()) == Some(blob)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::lock::LockFile;
    use crate::model::{ArtifactFile, ExposureEntry, ExposureStatus, HfCacheLocation, Ownership};

    use super::*;

    #[test]
    fn expected_remove_freed_bytes_should_count_managed_full_removal() {
        let mut lock = LockFile::default();
        let mut model = crate::commands::tests::test_model("Model A");
        model.artifact.size_bytes = 42;
        model.exposures.insert(
            "lmstudio".to_string(),
            ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "test".to_string(),
                path: None,
                created_by: None,
            },
        );
        lock.models.insert("id-a".to_string(), model);
        let input = remove_input(vec!["Model A".to_string()], Vec::new());

        let bytes = expected_remove_freed_bytes(&input, &lock, &["id-a".to_string()]);

        assert_eq!(bytes, 42);
    }

    #[test]
    fn expected_remove_freed_bytes_should_ignore_partial_tool_removal() {
        let mut lock = LockFile::default();
        let mut model = crate::commands::tests::test_model("Model A");
        model.artifact.size_bytes = 42;
        model.exposures.insert(
            "lmstudio".to_string(),
            ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "test".to_string(),
                path: None,
                created_by: None,
            },
        );
        model.exposures.insert(
            "jan".to_string(),
            ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "test".to_string(),
                path: None,
                created_by: None,
            },
        );
        lock.models.insert("id-a".to_string(), model);
        let input = remove_input(vec!["Model A".to_string()], vec!["lmstudio".to_string()]);

        let bytes = expected_remove_freed_bytes(&input, &lock, &["id-a".to_string()]);

        assert_eq!(bytes, 0);
    }

    #[test]
    fn remove_targets_should_return_all_model_ids_when_all_flag_is_set() {
        let hf_cache = tempfile::tempdir().unwrap();
        let mut lock = LockFile::default();
        lock.models
            .insert("id-b".to_string(), crate::commands::tests::test_model("B"));
        lock.models
            .insert("id-a".to_string(), crate::commands::tests::test_model("A"));
        let mut input = remove_input(Vec::new(), Vec::new());
        input.all = true;

        let (targets, untracked, _externals) =
            remove_targets(&lock, hf_cache.path(), &test_config(), &input).unwrap();

        assert_eq!(targets, vec!["id-a".to_string(), "id-b".to_string()]);
        assert!(untracked.is_empty());
    }

    #[test]
    fn remove_targets_should_match_names_case_insensitively() {
        let hf_cache = tempfile::tempdir().unwrap();
        let mut lock = LockFile::default();
        lock.models.insert(
            "id-a".to_string(),
            crate::commands::tests::test_model("Qwen3-8B-MLX"),
        );
        let input = remove_input(vec!["qwen3-8b-mlx".to_string()], Vec::new());

        let (targets, _, _) =
            remove_targets(&lock, hf_cache.path(), &test_config(), &input).unwrap();

        assert_eq!(targets, vec!["id-a".to_string()]);
    }

    #[test]
    fn remove_targets_should_match_by_repo_when_name_contains_slash() {
        let hf_cache = tempfile::tempdir().unwrap();
        let mut lock = LockFile::default();
        let mut model = crate::commands::tests::test_model("my-model");
        model.source.repo = "hexgrad/Kokoro-82M".to_string();
        lock.models.insert("id-a".to_string(), model);
        let input = remove_input(vec!["hexgrad/Kokoro-82M".to_string()], Vec::new());

        let (targets, _, _) =
            remove_targets(&lock, hf_cache.path(), &test_config(), &input).unwrap();

        assert_eq!(targets, vec!["id-a".to_string()]);
    }

    #[test]
    fn remove_targets_should_find_untracked_cache_dir() {
        let hf_cache = tempfile::tempdir().unwrap();
        let repo_dir = hf_cache.path().join("models--org--model");
        fs::create_dir_all(repo_dir.join("blobs")).unwrap();
        fs::write(repo_dir.join("blobs").join("abc"), b"data").unwrap();
        let lock = LockFile::default();
        let input = remove_input(vec!["org/model".to_string()], Vec::new());

        let (targets, untracked, _externals) =
            remove_targets(&lock, hf_cache.path(), &test_config(), &input).unwrap();

        assert!(targets.is_empty());
        assert_eq!(untracked.len(), 1);
        assert_eq!(untracked[0].repo, "org/model");
        assert_eq!(untracked[0].size_bytes, 4);
    }

    #[test]
    fn remove_targets_should_error_without_names_when_not_interactive() {
        let hf_cache = tempfile::tempdir().unwrap();
        let lock = LockFile::default();
        let input = remove_input(Vec::new(), Vec::new());

        let error = remove_targets(&lock, hf_cache.path(), &test_config(), &input).unwrap_err();

        assert!(
            matches!(error, AppError::InvalidInput(message) if message == "remove needs a model name or --all")
        );
    }

    #[test]
    fn tracked_matches_tools_should_require_selected_tool_when_present() {
        let mut model = crate::commands::tests::test_model("Model A");
        model.exposures.insert(
            "jan".to_string(),
            ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "test".to_string(),
                path: None,
                created_by: None,
            },
        );
        let input = remove_input(vec!["Model A".to_string()], vec!["lmstudio".to_string()]);

        assert!(!tracked_matches_tools(&model, &input));
    }

    #[test]
    fn remove_target_label_should_describe_interactive_selection() {
        let input = remove_input(Vec::new(), Vec::new());

        assert_eq!(remove_target_label(&input), "interactive");
    }

    #[test]
    fn dedupe_should_keep_first_seen_order() {
        let ids = dedupe(vec!["b".to_string(), "a".to_string(), "b".to_string()]);

        assert_eq!(ids, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn remove_canonical_files_should_keep_blob_referenced_by_another_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let repo_cache = dir.path().join("models--org--repo");
        let snapshot_a = repo_cache.join("snapshots/a");
        let snapshot_b = repo_cache.join("snapshots/b");
        let blob = repo_cache.join("blobs/blob-a");
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::create_dir_all(&snapshot_a).unwrap();
        fs::create_dir_all(&snapshot_b).unwrap();
        fs::write(&blob, b"model").unwrap();
        make_symlink(&blob, &snapshot_a.join("model.gguf"));
        make_symlink(&blob, &snapshot_b.join("model.gguf"));
        let mut model = crate::commands::tests::test_model("Model A");
        model.source.ownership = Ownership::Managed;
        model.locations.hf_cache = Some(HfCacheLocation {
            snapshot_path: snapshot_a,
        });
        model.artifact.files = vec![ArtifactFile {
            path: "model.gguf".to_string(),
            size_bytes: 5,
            hf_blob: Some("blob-a".to_string()),
            sha256: None,
            role: None,
        }];

        let freed = remove_canonical_files(&model).unwrap();

        assert_eq!(freed, 0);
    }

    #[test]
    fn remove_canonical_files_should_remove_unreferenced_blob() {
        let dir = tempfile::tempdir().unwrap();
        let repo_cache = dir.path().join("models--org--repo");
        let snapshot = repo_cache.join("snapshots/a");
        let blob = repo_cache.join("blobs/blob-a");
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::create_dir_all(&snapshot).unwrap();
        fs::write(&blob, b"model").unwrap();
        make_symlink(&blob, &snapshot.join("model.gguf"));
        let mut model = crate::commands::tests::test_model("Model A");
        model.locations.hf_cache = Some(HfCacheLocation {
            snapshot_path: snapshot,
        });
        model.artifact.files = vec![ArtifactFile {
            path: "model.gguf".to_string(),
            size_bytes: 5,
            hf_blob: Some("blob-a".to_string()),
            sha256: None,
            role: None,
        }];

        let freed = remove_canonical_files(&model).unwrap();

        assert_eq!(freed, 5);
    }

    #[test]
    fn cleanup_empty_repo_cache_should_remove_entire_tree_when_no_snapshots_remain() {
        let dir = tempfile::tempdir().unwrap();
        let repo_cache = dir.path().join("models--org--repo");
        fs::create_dir_all(repo_cache.join("snapshots")).unwrap();
        fs::create_dir_all(repo_cache.join("blobs")).unwrap();
        fs::create_dir_all(repo_cache.join("refs")).unwrap();
        fs::write(repo_cache.join("refs").join("main"), b"abc123").unwrap();

        cleanup_empty_repo_cache(&repo_cache);

        assert!(!repo_cache.exists());
    }

    #[test]
    fn cleanup_empty_repo_cache_should_keep_dir_when_other_snapshots_exist() {
        let dir = tempfile::tempdir().unwrap();
        let repo_cache = dir.path().join("models--org--repo");
        fs::create_dir_all(repo_cache.join("snapshots").join("other_commit")).unwrap();
        fs::create_dir_all(repo_cache.join("refs")).unwrap();

        cleanup_empty_repo_cache(&repo_cache);

        assert!(repo_cache.exists());
        assert!(repo_cache.join("snapshots").join("other_commit").exists());
    }

    #[test]
    fn remove_cache_dir_should_delete_dir_and_return_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("models--org--repo");
        fs::create_dir_all(cache.join("blobs")).unwrap();
        fs::write(cache.join("blobs").join("a"), b"12345").unwrap();
        fs::write(cache.join("blobs").join("b"), b"67").unwrap();

        let freed = remove_cache_dir(&cache).unwrap();

        assert_eq!(freed, 7);
        assert!(!cache.exists());
    }

    fn test_config() -> crate::config::Config {
        let mut config = crate::config::Config::default();
        config.paths.lmstudio = None;
        config.paths.jan = None;
        config
    }

    fn remove_input(names: Vec<String>, tools: Vec<String>) -> RemoveInput {
        RemoveInput {
            names,
            tools,
            all: false,
            keep_cache: false,
            purge_cache: false,
            dry_run: false,
            yes: true,
        }
    }

    fn make_symlink(source: &std::path::Path, target: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(source, target).unwrap();

        #[cfg(not(unix))]
        fs::write(target, fs::read(source).unwrap()).unwrap();
    }
}
