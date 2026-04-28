//! `lmm update` — update tracked models to the latest HF revision.
use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::adapters::{ExposeRequest, create_exposures};
use crate::artifacts::{artifact_signature, selected_runtime_files};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::hf::{HfClient, RepoFile, candidate_files};
use crate::lock::{LockFile, StateLock};
use crate::model::{FormatKind, ModelEntry};
use crate::paths::{AppPaths, configured_hf_cache_dir};
use crate::tui;

use super::add::{ModelBuildInput, build_model_entry};
use super::{UpdateInput, confirm, model_name_matches, short_commit};

pub fn update(paths: &AppPaths, input: &UpdateInput) -> Result<()> {
    let _state_lock = StateLock::acquire(&paths.lock_path)?;
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let client = HfClient::new(config.network.hf_endpoint.clone(), hf_cache.clone());
    let mut lock = LockFile::load(&paths.lock_path)?;
    let target_ids = update_target_ids(&lock, input)?;
    if target_ids.is_empty() {
        format::status("⊘", &format::dim("no tracked models to update"));
        return Ok(());
    }

    format::status("⟩", "Checking for updates…");
    let mut plans = Vec::new();
    for id in target_ids {
        let Some(model) = lock.models.get(&id) else {
            continue;
        };
        plans.push(build_update_plan(&client, model, &id)?);
    }

    print_update_plan(input, &plans);
    if input.dry_run {
        format::status("⊘", &format::dim("plan-only; --dry-run prevented update"));
        return Ok(());
    }
    if !confirm(input.yes, "Update selected models?")? {
        return Ok(());
    }

    let tmp_dir = paths.state_dir.join("tmp").join("downloads");
    let mut updated = 0_usize;
    let mut unchanged = 0_usize;
    let mut freed_bytes = 0_u64;
    for plan in plans {
        if !plan.changed {
            unchanged += 1;
            continue;
        }

        super::add::ensure_cache_files_inner(
            &client,
            &hf_cache,
            &plan.repo,
            &plan.new_commit,
            &plan.selected_files,
            &tmp_dir,
        )?;
        if plan.new_id != plan.old_id && lock.models.contains_key(&plan.new_id) {
            return Err(AppError::InvalidInput(format!(
                "updated artifact is already tracked: {}",
                plan.new_id
            )));
        }
        let Some(old_model) = lock.models.get(&plan.old_id).cloned() else {
            continue;
        };

        let tools: Vec<String> = old_model.exposures.keys().cloned().collect();
        let ollama_name = old_model
            .exposures
            .get("ollama")
            .and_then(|entry| entry.path.as_ref())
            .map(|path| path.to_string_lossy().to_string());
        let exposures = match create_exposures(&ExposeRequest {
            tools: &tools,
            repo: &plan.repo,
            alias: &old_model.name,
            format: old_model.format,
            snapshot_root: &plan.snapshot_root,
            files: &plan.selected_files,
            config: &config,
            replace: true,
            ollama_name: ollama_name.as_deref(),
        }) {
            Ok(exposures) => exposures,
            Err(error) => {
                restore_old_exposures(&old_model, &tools, &config, ollama_name.as_deref());
                return Err(error);
            }
        };
        let updated_model = build_model_entry(ModelBuildInput {
            alias: old_model.name.clone(),
            repo: plan.repo,
            revision: old_model.source.revision.clone(),
            commit: plan.new_commit,
            ownership: old_model.source.ownership,
            format: old_model.format,
            signature: plan.signature,
            snapshot_root: plan.snapshot_root,
            selected_files: &plan.selected_files,
            exposures,
        });

        lock.models.remove(&plan.old_id);
        if old_model
            .source
            .ownership
            .deletes_canonical_bytes_by_default()
        {
            freed_bytes += super::remove::remove_canonical_files(&old_model)?;
        }
        lock.models.insert(plan.new_id, updated_model);
        updated += 1;
    }

    lock.save(&paths.lock_path)?;
    format::status(
        "✓",
        &format!(
            "{} ({updated} updated, {unchanged} up-to-date, freed {})",
            format::green("done"),
            format::bytes(freed_bytes)
        ),
    );
    Ok(())
}

pub(crate) fn update_target_ids(lock: &LockFile, input: &UpdateInput) -> Result<Vec<String>> {
    if input.all {
        return Ok(lock.models.keys().cloned().collect());
    }
    if input.names.is_empty() {
        if tui::can_run() {
            return select_update_targets(lock);
        }
        return Err(AppError::InvalidInput(
            "update needs a model name or --all".to_string(),
        ));
    }

    let mut ids = Vec::new();
    let mut missing = Vec::new();
    for name in &input.names {
        let before = ids.len();
        ids.extend(
            lock.models
                .iter()
                .filter(|(_, model)| model_name_matches(&model.name, name))
                .map(|(id, _)| id.clone()),
        );
        if ids.len() == before {
            missing.push(name.clone());
        }
    }

    if !missing.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "model not found: {}",
            missing.join(", ")
        )));
    }

    Ok(dedupe_ids(ids))
}

fn select_update_targets(lock: &LockFile) -> Result<Vec<String>> {
    let items: Vec<(String, String, String)> = lock
        .models
        .iter()
        .map(|(id, model)| {
            (
                id.clone(),
                format!(
                    "{} · {} · {}",
                    model.name,
                    model.format,
                    format::bytes(model.artifact.size_bytes),
                ),
                format!("rev: {}", model.source.revision),
            )
        })
        .collect();
    tui::select_many("Update models", &items, &[])
}

fn dedupe_ids(ids: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

#[derive(Clone, Debug)]
struct UpdatePlan {
    old_id: String,
    new_id: String,
    name: String,
    repo: String,
    revision: String,
    old_commit: String,
    new_commit: String,
    changed: bool,
    old_size_bytes: u64,
    new_size_bytes: u64,
    signature: String,
    snapshot_root: PathBuf,
    selected_files: Vec<RepoFile>,
}

fn build_update_plan(client: &HfClient, model: &ModelEntry, old_id: &str) -> Result<UpdatePlan> {
    let metadata = client.repo_metadata(&model.source.repo, &model.source.revision)?;
    if metadata.commit == model.source.commit {
        return Ok(UpdatePlan {
            old_id: old_id.to_string(),
            new_id: old_id.to_string(),
            name: model.name.clone(),
            repo: model.source.repo.clone(),
            revision: model.source.revision.clone(),
            old_commit: model.source.commit.clone(),
            new_commit: metadata.commit,
            changed: false,
            old_size_bytes: model.artifact.size_bytes,
            new_size_bytes: model.artifact.size_bytes,
            signature: model.artifact.signature.clone(),
            snapshot_root: model
                .locations
                .hf_cache
                .as_ref()
                .map(|location| location.snapshot_path.clone())
                .unwrap_or_default(),
            selected_files: Vec::new(),
        });
    }

    let repo_files = client.repo_files(&model.source.repo, &metadata.commit)?;
    let candidate_files = candidate_files(&repo_files);
    let selected_file = selected_file_for_update(model);
    let selected_candidates =
        selected_runtime_files(&candidate_files, model.format, selected_file.as_deref())
            .map_err(AppError::InvalidInput)?;
    let selected_files =
        crate::artifacts::resolve_selected_repo_files(&repo_files, &selected_candidates)
            .map_err(AppError::InvalidInput)?;
    let signature = artifact_signature(
        &model.source.repo,
        &metadata.commit,
        &model.format,
        &selected_candidates,
    );
    let new_id = format!("hf:{signature}");
    let new_size_bytes = selected_files.iter().map(|file| file.size_bytes).sum();

    Ok(UpdatePlan {
        old_id: old_id.to_string(),
        new_id,
        name: model.name.clone(),
        repo: model.source.repo.clone(),
        revision: model.source.revision.clone(),
        old_commit: model.source.commit.clone(),
        new_commit: metadata.commit.clone(),
        changed: true,
        old_size_bytes: model.artifact.size_bytes,
        new_size_bytes,
        signature,
        snapshot_root: client.snapshot_path(&model.source.repo, &metadata.commit)?,
        selected_files,
    })
}

fn selected_file_for_update(model: &ModelEntry) -> Option<String> {
    if model.format != FormatKind::Gguf {
        return None;
    }
    model
        .artifact
        .files
        .iter()
        .find(|file| file.path.ends_with(".gguf"))
        .map(|file| file.path.clone())
}

fn restore_old_exposures(
    model: &ModelEntry,
    tools: &[String],
    config: &Config,
    ollama_name: Option<&str>,
) {
    let Some(snapshot_root) = model
        .locations
        .hf_cache
        .as_ref()
        .map(|location| location.snapshot_path.as_path())
    else {
        return;
    };
    let Ok(files) = repo_files_from_model(model) else {
        return;
    };
    let _ = create_exposures(&ExposeRequest {
        tools,
        repo: &model.source.repo,
        alias: &model.name,
        format: model.format,
        snapshot_root,
        files: &files,
        config,
        replace: true,
        ollama_name,
    });
}

fn repo_files_from_model(model: &ModelEntry) -> Result<Vec<RepoFile>> {
    model
        .artifact
        .files
        .iter()
        .map(|file| {
            let Some(oid) = &file.hf_blob else {
                return Err(AppError::InvalidInput(format!(
                    "model file has no HF blob recorded: {}",
                    file.path
                )));
            };
            Ok(RepoFile {
                path: file.path.clone(),
                size_bytes: file.size_bytes,
                oid: oid.clone(),
            })
        })
        .collect()
}

fn print_update_plan(input: &UpdateInput, plans: &[UpdatePlan]) {
    format::heading("Update plan");
    format::kv("target", &update_target_label(input));
    format::kv("models", &plans.len().to_string());
    if input.dry_run {
        format::kv("dry run", "true");
    }
    eprintln!();
    for plan in plans {
        let status = if plan.changed {
            format::yellow("update")
        } else {
            format::green("up-to-date")
        };
        eprintln!(
            "  {} {} ({}) {} → {}  {} → {}",
            status,
            format::bold(&plan.name),
            plan.revision,
            short_commit(&plan.old_commit),
            short_commit(&plan.new_commit),
            format::bytes(plan.old_size_bytes),
            format::bytes(plan.new_size_bytes),
        );
    }
}

fn update_target_label(input: &UpdateInput) -> String {
    if input.all {
        "all".to_string()
    } else if input.names.is_empty() {
        "interactive/default".to_string()
    } else {
        input.names.join(", ")
    }
}
