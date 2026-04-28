//! `lmm remove` — uninstall model exposures and reclaim cache bytes.
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::adapters::remove_exposure;
use crate::error::{AppError, Result};
use crate::format;
use crate::lock::{LockFile, StateLock};
use crate::model::ModelEntry;
use crate::paths::AppPaths;
use crate::tui;

use super::{
    FormatSelection, RemoveInput, confirm, discover_external_exposures, remove_all_tools,
    repo_cache_from_snapshot, tool_list, validate_tools,
};

pub fn remove(paths: &AppPaths, input: &RemoveInput) -> Result<()> {
    validate_tools(&input.tools, FormatSelection::Auto)?;
    let _state_lock = StateLock::acquire(&paths.lock_path)?;
    let config = crate::config::Config::load(&paths.config_path)?;
    let mut lock = LockFile::load(&paths.lock_path)?;
    let targets = remove_targets(&lock, input)?;
    if targets.is_empty() {
        format::status("⊘", &format::dim("no matching tracked models found"));
        return Ok(());
    }
    print_remove_plan(input, &lock, &targets);
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

    lock.save(&paths.lock_path)?;
    format::outro(&format!(
        "Removed (freed {})",
        format::bytes(actual_freed_bytes)
    ));
    Ok(())
}

fn print_remove_plan(input: &RemoveInput, lock: &LockFile, targets: &[String]) {
    format::heading("Remove plan");
    format::kv("target", &remove_target_label(input));
    format::kv("tools", &tool_list(&input.tools));
    format::kv("keep cache", &input.keep_cache.to_string());
    format::kv("purge adopted cache", &input.purge_cache.to_string());
    format::kv("models", &targets.len().to_string());
    format::kv(
        "expected freed",
        &format::bytes(expected_remove_freed_bytes(input, lock, targets)),
    );
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

fn remove_targets(lock: &LockFile, input: &RemoveInput) -> Result<Vec<String>> {
    if input.all {
        return Ok(lock.models.keys().cloned().collect());
    }
    if input.names.is_empty() {
        if tui::can_run() {
            return select_remove_targets(lock, input);
        }
        return Err(AppError::InvalidInput(
            "remove needs a model name or --all".to_string(),
        ));
    }

    let mut targets = Vec::new();
    let mut missing = Vec::new();
    for name in &input.names {
        let found: Vec<String> = lock
            .models
            .iter()
            .filter(|(_, model)| super::model_name_matches(&model.name, name))
            .filter(|(_, model)| tracked_matches_tools(model, input))
            .map(|(id, _)| id.clone())
            .collect();
        if found.is_empty() {
            missing.push(name.clone());
        } else {
            targets.extend(found);
        }
    }

    let targets = dedupe(targets);
    if missing.is_empty() {
        Ok(targets)
    } else {
        let config = crate::config::Config::load(&crate::paths::AppPaths::resolve()?.config_path)?;
        let externals = discover_external_exposures(&config, lock)?;
        let mut hints = Vec::new();
        for name in &missing {
            if let Some(ext) = externals
                .iter()
                .find(|e| super::model_name_matches(&e.name, name))
            {
                let tool_cmd = match ext.tool.as_str() {
                    "ollama" => format!("ollama rm {}", ext.name),
                    "lmstudio" => {
                        "remove via LM Studio UI or delete from ~/.lmstudio/models/".to_string()
                    }
                    "jan" => "remove via Jan UI".to_string(),
                    _ => format!("remove via {} directly", ext.tool),
                };
                hints.push(format!(
                    "'{}' is managed by {} — use: {}",
                    name, ext.tool, tool_cmd
                ));
            }
        }
        if hints.is_empty() {
            Err(AppError::InvalidInput(format!(
                "model not found: {}",
                missing.join(", ")
            )))
        } else {
            Err(AppError::InvalidInput(hints.join("\n")))
        }
    }
}

fn select_remove_targets(lock: &LockFile, input: &RemoveInput) -> Result<Vec<String>> {
    let config = crate::config::Config::load(&crate::paths::AppPaths::resolve()?.config_path)?;
    let externals = discover_external_exposures(&config, lock)?;

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
            format!("{} · external · {}", ext.name, ext.tool),
            external_remove_guide(&ext.tool, &ext.name),
        ));
    }

    // hf-cache (untracked) entries are handled by `lmm gc`, not remove.

    if items.is_empty() {
        return Ok(Vec::new());
    }

    let selected_ids = tui::select_many("Remove models", &items, &[])?;

    let mut tracked_ids = Vec::new();
    let mut external_hints = Vec::new();
    for id in &selected_ids {
        if let Some(stripped) = id.strip_prefix("external:") {
            if let Some(ext) = externals.iter().find(|e| e.id == stripped) {
                external_hints.push(format!(
                    "  {} \u{2192} {}",
                    ext.name,
                    external_remove_guide(&ext.tool, &ext.name)
                ));
            }
        } else {
            tracked_ids.push(id.clone());
        }
    }

    if !external_hints.is_empty() {
        eprintln!();
        format::heading("External models (not managed by lmm)");
        for hint in &external_hints {
            eprintln!("{hint}");
        }
        eprintln!();
    }

    Ok(tracked_ids)
}

fn external_remove_guide(tool: &str, name: &str) -> String {
    match tool {
        "ollama" => format!("ollama rm '{name}'"),
        "lmstudio" => "delete from ~/.lmstudio/models/".to_string(),
        "jan" => "remove via Jan UI".to_string(),
        _ => format!("remove via {tool} directly"),
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
        let mut lock = LockFile::default();
        lock.models
            .insert("id-b".to_string(), crate::commands::tests::test_model("B"));
        lock.models
            .insert("id-a".to_string(), crate::commands::tests::test_model("A"));
        let mut input = remove_input(Vec::new(), Vec::new());
        input.all = true;

        let targets = remove_targets(&lock, &input).unwrap();

        assert_eq!(targets, vec!["id-a".to_string(), "id-b".to_string()]);
    }

    #[test]
    fn remove_targets_should_match_names_case_insensitively() {
        let mut lock = LockFile::default();
        lock.models.insert(
            "id-a".to_string(),
            crate::commands::tests::test_model("Qwen3-8B-MLX"),
        );
        let input = remove_input(vec!["qwen3-8b-mlx".to_string()], Vec::new());

        let targets = remove_targets(&lock, &input).unwrap();

        assert_eq!(targets, vec!["id-a".to_string()]);
    }

    #[test]
    fn remove_targets_should_error_without_names_when_not_interactive() {
        let lock = LockFile::default();
        let input = remove_input(Vec::new(), Vec::new());

        let error = remove_targets(&lock, &input).unwrap_err();

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
