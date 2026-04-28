//! `lmm add` — install a model from Hugging Face and expose to tools.
use std::path::PathBuf;

use crate::adapters::{ExposeRequest, create_exposures};
use crate::artifacts::{
    CandidateFile, artifact_signature, default_alias, detect_format, selected_runtime_files,
};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::format::{Align, Table};
use crate::hf::{HfClient, RepoFile, candidate_files, ensure_snapshot_link};
use crate::lock::{LockFile, StateLock};
use crate::model::{
    Artifact, ArtifactFile, FormatKind, HfCacheLocation, Locations, ModelEntry, Ownership, Source,
};
use crate::paths::{AppPaths, configured_hf_cache_dir};
use crate::tui;

use super::{
    AddInput, FormatSelection, confirm, effective_format_selection, resolve_format_interactive,
    resolve_tools_interactive, tool_list, validate_tools,
};

pub fn add_repo(
    paths: &AppPaths,
    repo: &str,
    format: super::FormatSelection,
    yes: bool,
) -> Result<()> {
    let input = AddInput {
        repo: Some(repo.to_string()),
        revision: "main".to_string(),
        file: None,
        name: None,
        tools: Vec::new(),
        format,
        list: false,
        all: false,
        dry_run: false,
        replace: false,
        take_ownership: false,
        ollama_name: None,
        yes,
    };
    validate_tools(&input.tools, input.format)?;
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let client = HfClient::new(config.network.hf_endpoint.clone(), hf_cache.clone());
    add_inner(paths, &input, repo, &config, &hf_cache, &client)
}

pub fn add(paths: &AppPaths, input: &AddInput) -> Result<()> {
    validate_tools(&input.tools, input.format)?;
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let client = HfClient::new(config.network.hf_endpoint.clone(), hf_cache.clone());
    let Some(repo) = input.repo.as_deref() else {
        format::logo();
        eprintln!();
        eprintln!(
            "{} {}",
            format::error_badge(),
            format::red("Missing required argument: repo")
        );
        eprintln!();
        eprintln!("  Usage:");
        eprintln!(
            "    {} {} {}",
            format::cyan("lmm add"),
            format::yellow("<repo>"),
            format::dim("[options]")
        );
        eprintln!();
        eprintln!("  Example:");
        eprintln!(
            "    {} {}",
            format::cyan("lmm add"),
            format::yellow("mlx-community/Qwen3-8B-4bit")
        );
        eprintln!();
        eprintln!("  Or search interactively: {}", format::cyan("lmm search"));
        eprintln!();
        return Err(AppError::InvalidInput(
            "Missing required argument: lmm add <repo>".to_string(),
        ));
    };

    format::logo();
    format::intro("lmm add");
    add_inner(paths, input, repo, &config, &hf_cache, &client)
}

fn add_inner(
    paths: &AppPaths,
    input: &AddInput,
    repo: &str,
    config: &Config,
    hf_cache: &std::path::Path,
    client: &HfClient,
) -> Result<()> {
    format::step(&format!(
        "Source: {}",
        format::cyan(&format!("https://huggingface.co/{repo}"))
    ));

    let metadata = client.repo_metadata(repo, &input.revision)?;
    let repo_files = client.repo_files(repo, &metadata.commit)?;
    let candidates = candidate_files(&repo_files);
    let available = available_artifacts(repo, &candidates, metadata.library_name.as_deref());
    format::step(&format!(
        "Resolved {} ({} files, rev {})",
        repo,
        repo_files.len(),
        super::short_commit(&metadata.commit)
    ));

    if input.list {
        print_available_artifacts(repo, &metadata.commit, &available);
        return Ok(());
    }

    let mut selected_file = input.file.clone();
    let format_selection = effective_format_selection(input.format, config)?;
    let format = resolve_artifact_interactive(
        format_selection,
        &candidates,
        &available,
        &mut selected_file,
    )?;
    let selected_candidates =
        resolve_selected_files_interactive(&candidates, format, &mut selected_file)?;
    let tools = resolve_tools_interactive(&effective_add_tools(input), format, config, input.yes)?;
    if tools.is_empty() {
        format::outro_cancel("cancelled");
        return Ok(());
    }
    format::success(&format!("Tools: {}", tools.join(", ")));
    validate_tools(&tools, FormatSelection::Format(format))?;
    let selected_files =
        crate::artifacts::resolve_selected_repo_files(&repo_files, &selected_candidates)
            .map_err(AppError::InvalidInput)?;
    let raw_alias = input
        .name
        .clone()
        .unwrap_or_else(|| default_alias(repo, format, selected_file.as_deref()));
    let alias = crate::security::sanitize_alias(&raw_alias);
    if alias.is_empty() {
        return Err(AppError::InvalidInput(format!(
            "model alias \"{raw_alias}\" sanitizes to empty; use --name to specify a valid alias"
        )));
    }
    let signature = artifact_signature(repo, &metadata.commit, &format, &selected_candidates);
    let id = format!("hf:{signature}");
    let snapshot_root = client.snapshot_path(repo, &metadata.commit)?;
    let existing_lock = LockFile::load(&paths.lock_path)?;
    let ownership = resolve_ownership(client, &existing_lock, &id, repo, &selected_files, input);

    let plan_view = AddPlanView {
        input,
        repo,
        commit: &metadata.commit,
        format,
        selected_file: selected_file.as_deref(),
        alias: &alias,
        ownership,
        tools: &tools,
        files: &selected_files,
    };
    print_add_plan(&plan_view);

    if input.dry_run {
        format::outro_cancel("plan-only; --dry-run prevented installation");
        return Ok(());
    }
    if !confirm(input.yes, "Install this model?")? {
        return Ok(());
    }

    let _state_lock = StateLock::acquire(&paths.lock_path)?;
    let mut lock = LockFile::load(&paths.lock_path)?;
    let alias_conflict = lock
        .models
        .iter()
        .find(|(existing_id, model)| *existing_id != &id && model.name == alias)
        .map(|(existing_id, model)| (existing_id.clone(), model.name.clone()));
    if let Some((_, conflict_name)) = &alias_conflict
        && !input.replace
    {
        return Err(AppError::InvalidInput(format!(
            "model alias already exists: {conflict_name}; use --replace or --name"
        )));
    }

    let tmp_dir = paths.state_dir.join("tmp").join("downloads");
    ensure_cache_files(
        client,
        hf_cache,
        repo,
        &metadata.commit,
        &selected_files,
        &tmp_dir,
    )?;

    if let Some(existing) = lock.models.get_mut(&id) {
        let tools_to_create = missing_or_replaced_tools(existing, &tools, input.replace);
        if tools_to_create.is_empty() {
            format::outro("Already installed");
            lock.save(&paths.lock_path)?;
            return Ok(());
        }

        let exposures = create_exposures(&ExposeRequest {
            tools: &tools_to_create,
            repo,
            alias: &alias,
            format,
            snapshot_root: &snapshot_root,
            files: &selected_files,
            config,
            replace: input.replace,
            ollama_name: input.ollama_name.as_deref(),
        })?;
        existing.exposures.extend(exposures);
        if input.take_ownership && existing.source.ownership == Ownership::Adopted {
            existing.source.ownership = Ownership::OwnedAdopted;
        }
        lock.save(&paths.lock_path)?;
        format::outro("Updated exposures");
        return Ok(());
    }

    let exposures = create_exposures(&ExposeRequest {
        tools: &tools,
        repo,
        alias: &alias,
        format,
        snapshot_root: &snapshot_root,
        files: &selected_files,
        config,
        replace: input.replace,
        ollama_name: input.ollama_name.as_deref(),
    })?;

    if let Some((conflict_id, _)) = &alias_conflict
        && let Some(mut old_model) = lock.models.remove(conflict_id)
    {
        let new_paths: std::collections::BTreeSet<_> =
            exposures.values().filter_map(|e| e.path.as_ref()).collect();
        let all_old_tools: Vec<String> = old_model.exposures.keys().cloned().collect();
        for tool in &all_old_tools {
            if let Some(entry) = old_model.exposures.remove(tool) {
                let overlaps = entry.path.as_ref().is_some_and(|p| new_paths.contains(p));
                if !overlaps {
                    let _ = crate::adapters::remove_exposure(tool, &entry, config);
                }
            }
        }
        let shares_cache = old_model
            .locations
            .hf_cache
            .as_ref()
            .is_some_and(|loc| loc.snapshot_path == snapshot_root);
        if !shares_cache
            && old_model
                .source
                .ownership
                .deletes_canonical_bytes_by_default()
        {
            let _ = super::remove::remove_canonical_files(&old_model);
        }
    }
    let model = build_model_entry(ModelBuildInput {
        alias,
        repo: repo.to_string(),
        revision: input.revision.clone(),
        commit: metadata.commit,
        ownership,
        format,
        signature,
        snapshot_root,
        selected_files: &selected_files,
        exposures,
    });

    let installed_name = model.name.clone();
    let installed_format = model.format;
    lock.models.insert(id, model);
    lock.save(&paths.lock_path)?;
    let auto_tools = super::auto_available_tools(installed_format);
    if !auto_tools.is_empty() {
        format::info(&format!(
            "{}  {}",
            format::dim("Also available via:"),
            auto_tools.join(", ")
        ));
    }
    format::outro(&format!(
        "Installed {} → {}",
        installed_name,
        tools.join(", ")
    ));
    Ok(())
}

fn resolve_ownership(
    client: &HfClient,
    existing_lock: &LockFile,
    id: &str,
    repo: &str,
    selected_files: &[RepoFile],
    input: &AddInput,
) -> Ownership {
    if let Some(existing) = existing_lock.models.get(id) {
        if input.take_ownership && existing.source.ownership == Ownership::Adopted {
            Ownership::OwnedAdopted
        } else {
            existing.source.ownership
        }
    } else {
        let existed_before = selected_files
            .iter()
            .all(|file| client.blob_path(repo, &file.oid).is_ok_and(|p| p.exists()));
        if existed_before {
            if input.take_ownership {
                Ownership::OwnedAdopted
            } else {
                Ownership::Adopted
            }
        } else {
            Ownership::Managed
        }
    }
}

struct AddPlanView<'a> {
    input: &'a AddInput,
    repo: &'a str,
    commit: &'a str,
    format: FormatKind,
    selected_file: Option<&'a str>,
    alias: &'a str,
    ownership: Ownership,
    tools: &'a [String],
    files: &'a [RepoFile],
}

fn print_add_plan(plan: &AddPlanView<'_>) {
    let total_size = plan.files.iter().map(|file| file.size_bytes).sum::<u64>();
    format::heading("Add plan");
    format::kv("repo", plan.repo);
    format::kv(
        "revision",
        &format!(
            "{} → {}",
            plan.input.revision,
            super::short_commit(plan.commit)
        ),
    );
    format::kv("format", &plan.format.to_string());
    format::kv("file", plan.selected_file.unwrap_or("auto"));
    format::kv("alias", &format::bold(plan.alias));
    format::kv("tools", &tool_list(plan.tools));
    format::kv("ownership", &format!("{:?}", plan.ownership));
    format::kv(
        "files",
        &format!("{} ({})", plan.files.len(), format::bytes(total_size)),
    );
    if let Some(ollama_name) = &plan.input.ollama_name {
        format::kv("ollama name", ollama_name);
    }
    if plan.input.dry_run {
        format::kv("dry run", "true");
    }
}

#[cfg(test)]
fn add_plan_lines(plan: &AddPlanView<'_>) -> Vec<String> {
    let total_size = plan.files.iter().map(|file| file.size_bytes).sum::<u64>();
    let mut lines = vec![
        format!("repo: {}", plan.repo),
        format!(
            "revision: {} → {}",
            plan.input.revision,
            super::short_commit(plan.commit)
        ),
        format!("format: {}", plan.format),
        format!("file: {}", plan.selected_file.unwrap_or("auto")),
        format!("alias: {}", plan.alias),
        format!("tools: {}", plan.tools.join(", ")),
        format!("ownership: {:?}", plan.ownership),
        format!(
            "files: {} ({})",
            plan.files.len(),
            format::bytes(total_size)
        ),
    ];
    if let Some(ollama_name) = &plan.input.ollama_name {
        lines.push(format!("ollama name: {ollama_name}"));
    }
    lines
}

fn ensure_cache_files(
    client: &HfClient,
    hf_cache: &std::path::Path,
    repo: &str,
    commit: &str,
    files: &[RepoFile],
    tmp_dir: &std::path::Path,
) -> Result<()> {
    let mut progress = format::ProgressLine::new("Downloading", files.len());
    for file in files {
        progress.inc(&file.path);
        client.download_file(repo, commit, file, tmp_dir)?;
        ensure_snapshot_link(hf_cache, repo, commit, file)?;
    }
    progress.finish();
    Ok(())
}

pub(crate) fn ensure_cache_files_inner(
    client: &HfClient,
    hf_cache: &std::path::Path,
    repo: &str,
    commit: &str,
    files: &[RepoFile],
    tmp_dir: &std::path::Path,
) -> Result<()> {
    ensure_cache_files(client, hf_cache, repo, commit, files, tmp_dir)
}

fn missing_or_replaced_tools(
    existing: &ModelEntry,
    requested_tools: &[String],
    replace: bool,
) -> Vec<String> {
    if replace {
        return requested_tools.to_vec();
    }

    requested_tools
        .iter()
        .filter(|tool| !existing.exposures.contains_key(*tool))
        .cloned()
        .collect()
}

pub(crate) struct ModelBuildInput<'a> {
    pub alias: String,
    pub repo: String,
    pub revision: String,
    pub commit: String,
    pub ownership: Ownership,
    pub format: FormatKind,
    pub signature: String,
    pub snapshot_root: PathBuf,
    pub selected_files: &'a [RepoFile],
    pub exposures: std::collections::BTreeMap<String, crate::model::ExposureEntry>,
}

pub(crate) fn build_model_entry(input: ModelBuildInput<'_>) -> ModelEntry {
    let artifact_files: Vec<ArtifactFile> = input
        .selected_files
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
    ModelEntry {
        name: input.alias,
        source: Source {
            kind: "hf".to_string(),
            repo: input.repo,
            revision: input.revision,
            commit: input.commit,
            ownership: input.ownership,
        },
        format: input.format,
        artifact: Artifact {
            signature: input.signature,
            size_bytes,
            full_snapshot: false,
            files: artifact_files,
        },
        locations: Locations {
            hf_cache: Some(HfCacheLocation {
                snapshot_path: input.snapshot_root,
            }),
        },
        exposures: input.exposures,
    }
}

fn effective_add_tools(input: &AddInput) -> Vec<String> {
    if input.all {
        return vec!["all".to_string()];
    }
    input.tools.clone()
}

#[derive(Clone, Debug)]
struct AvailableArtifact {
    format: FormatKind,
    selected_file: Option<String>,
    alias: String,
    size_bytes: u64,
    file_count: usize,
    detail: String,
}

fn available_artifacts(
    repo: &str,
    files: &[CandidateFile],
    library_hint: Option<&str>,
) -> Vec<AvailableArtifact> {
    let mut artifacts = Vec::new();

    for file in files.iter().filter(|file| file.path.ends_with(".gguf")) {
        artifacts.push(AvailableArtifact {
            format: FormatKind::Gguf,
            selected_file: Some(file.path.clone()),
            alias: default_alias(repo, FormatKind::Gguf, Some(&file.path)),
            size_bytes: file.size_bytes,
            file_count: 1,
            detail: file.path.clone(),
        });
    }

    if let Ok(runtime_files) = selected_runtime_files(files, FormatKind::Safetensors, None) {
        let size_bytes = runtime_files.iter().map(|file| file.size_bytes).sum();
        artifacts.push(AvailableArtifact {
            format: FormatKind::Safetensors,
            selected_file: None,
            alias: default_alias(repo, FormatKind::Safetensors, None),
            size_bytes,
            file_count: runtime_files.len(),
            detail: "transformers-compatible snapshot".to_string(),
        });
    }

    if library_hint == Some("mlx")
        && let Ok(runtime_files) = selected_runtime_files(files, FormatKind::Mlx, None)
    {
        let size_bytes = runtime_files.iter().map(|file| file.size_bytes).sum();
        artifacts.push(AvailableArtifact {
            format: FormatKind::Mlx,
            selected_file: None,
            alias: default_alias(repo, FormatKind::Mlx, None),
            size_bytes,
            file_count: runtime_files.len(),
            detail: "Apple Silicon MLX snapshot".to_string(),
        });
    }

    artifacts
}

fn print_available_artifacts(repo: &str, commit: &str, artifacts: &[AvailableArtifact]) {
    format::heading("Available artifacts");
    format::kv("repo", repo);
    format::kv("commit", super::short_commit(commit));

    if artifacts.is_empty() {
        format::status("⊘", &format::dim("no supported runtime artifacts found"));
        return;
    }

    let mut table = Table::new(&[
        ("FORMAT", Align::Left),
        ("ALIAS", Align::Left),
        ("FILES", Align::Right),
        ("SIZE", Align::Right),
        ("DETAIL", Align::Left),
    ]);
    for artifact in artifacts {
        table.row(&[
            &artifact.format.to_string(),
            &artifact.alias,
            &artifact.file_count.to_string(),
            &format::bytes(artifact.size_bytes),
            &artifact.detail,
        ]);
    }
    print!("{}", table.render());
}

fn resolve_artifact_interactive(
    selection: FormatSelection,
    files: &[CandidateFile],
    artifacts: &[AvailableArtifact],
    selected_file: &mut Option<String>,
) -> Result<FormatKind> {
    if let Some(pattern) = selected_file.as_deref() {
        let matches: Vec<_> = files
            .iter()
            .filter(|f| crate::artifacts::wildcard_match(pattern, &f.path))
            .collect();
        if !matches.is_empty() {
            if matches.iter().all(|f| f.path.ends_with(".gguf")) {
                return Ok(FormatKind::Gguf);
            }
            if matches.iter().any(|f| f.path.ends_with(".safetensors")) {
                if let FormatSelection::Format(fmt) = selection {
                    return Ok(fmt);
                }
                return Ok(detect_format(files, Some(pattern), None)
                    .into_format()
                    .unwrap_or(FormatKind::Safetensors));
            }
        }
    }

    if selection == FormatSelection::Auto
        && selected_file.is_none()
        && artifacts.len() > 1
        && tui::can_run()
    {
        let items: Vec<(String, String, String)> = artifacts
            .iter()
            .enumerate()
            .map(|(index, artifact)| {
                (
                    index.to_string(),
                    format!(
                        "{} · {} · {}",
                        artifact.alias,
                        artifact.format,
                        format::bytes(artifact.size_bytes),
                    ),
                    format!("{} files · {}", artifact.file_count, artifact.detail),
                )
            })
            .collect();
        let selected = tui::select_one("Select artifact", &items)?;
        let index = selected.parse::<usize>().map_err(|_| {
            AppError::InvalidInput("artifact selection returned an invalid index".to_string())
        })?;
        let Some(artifact) = artifacts.get(index) else {
            return Err(AppError::InvalidInput(
                "artifact selection was out of range".to_string(),
            ));
        };
        *selected_file = artifact.selected_file.clone();
        return Ok(artifact.format);
    }

    resolve_format_interactive(selection, files, selected_file.as_deref())
}

fn resolve_selected_files_interactive(
    candidates: &[CandidateFile],
    format: FormatKind,
    selected_file: &mut Option<String>,
) -> Result<Vec<CandidateFile>> {
    match selected_runtime_files(candidates, format, selected_file.as_deref()) {
        Ok(files) => Ok(files),
        Err(_) if tui::can_run() && format == FormatKind::Gguf => {
            let items: Vec<(String, String, String)> = candidates
                .iter()
                .filter(|file| file.path.ends_with(".gguf"))
                .map(|file| {
                    (
                        file.path.clone(),
                        format!("{} · {}", file.path, format::bytes(file.size_bytes)),
                        String::new(),
                    )
                })
                .collect();
            let file = tui::select_one("Select GGUF file", &items)?;
            *selected_file = Some(file);
            selected_runtime_files(candidates, format, selected_file.as_deref())
                .map_err(AppError::InvalidInput)
        }
        Err(message) => Err(AppError::InvalidInput(message)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::hf::RepoFile;
    use crate::model::{ExposureEntry, ExposureStatus};

    use super::*;

    #[test]
    fn effective_add_tools_should_use_all_marker_when_all_flag_is_set() {
        let input = add_input_with_tools(vec!["lmstudio".to_string()], true);

        assert_eq!(effective_add_tools(&input), vec!["all".to_string()]);
    }

    #[test]
    fn missing_or_replaced_tools_should_skip_existing_exposures() {
        let model = model_with_exposure("lmstudio");

        let missing = missing_or_replaced_tools(
            &model,
            &["lmstudio".to_string(), "mlx-lm".to_string()],
            false,
        );

        assert_eq!(missing, vec!["mlx-lm".to_string()]);
    }

    #[test]
    fn missing_or_replaced_tools_should_return_all_requested_tools_when_replacing() {
        let model = model_with_exposure("lmstudio");

        let missing = missing_or_replaced_tools(&model, &["lmstudio".to_string()], true);

        assert_eq!(missing, vec!["lmstudio".to_string()]);
    }

    #[test]
    fn build_model_entry_should_sum_selected_file_sizes() {
        let files = vec![
            repo_file("config.json", 2, "blob-config"),
            repo_file("model.safetensors", 8, "blob-model"),
        ];

        let model = build_model_entry(ModelBuildInput {
            alias: "tiny-model".to_string(),
            repo: "org/repo".to_string(),
            revision: "main".to_string(),
            commit: "commit-a".to_string(),
            ownership: Ownership::Managed,
            format: FormatKind::Safetensors,
            signature: "signature".to_string(),
            snapshot_root: std::path::PathBuf::from("/cache/snapshot"),
            selected_files: &files,
            exposures: BTreeMap::new(),
        });

        assert_eq!(model.artifact.size_bytes, 10);
    }

    #[test]
    fn available_artifacts_should_list_each_gguf_file() {
        let artifacts = available_artifacts(
            "org/repo",
            &[candidate("a.gguf", 4), candidate("b.gguf", 8)],
            None,
        );

        assert_eq!(artifacts.len(), 2);
    }

    #[test]
    fn available_artifacts_should_include_safetensors_snapshot() {
        let artifacts = available_artifacts(
            "org/repo",
            &[
                candidate("config.json", 1),
                candidate("model.safetensors", 2),
                candidate("tokenizer.json", 1),
            ],
            None,
        );

        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.format == FormatKind::Safetensors)
        );
    }

    #[test]
    fn add_plan_lines_should_include_ollama_name_when_present() {
        let input = AddInput {
            ollama_name: Some("custom:q4".to_string()),
            ..add_input_with_tools(vec!["ollama".to_string()], false)
        };
        let files = vec![repo_file("model.gguf", 4, "blob")];
        let plan = AddPlanView {
            input: &input,
            repo: "org/repo",
            commit: "abcdef1234567890",
            format: FormatKind::Gguf,
            selected_file: Some("model.gguf"),
            alias: "repo-q4",
            ownership: Ownership::Managed,
            tools: &input.tools,
            files: &files,
        };

        assert!(
            add_plan_lines(&plan)
                .iter()
                .any(|line| line == "ollama name: custom:q4")
        );
    }

    fn add_input_with_tools(tools: Vec<String>, all: bool) -> AddInput {
        AddInput {
            repo: Some("org/repo".to_string()),
            revision: "main".to_string(),
            file: None,
            name: None,
            tools,
            format: FormatSelection::Auto,
            list: false,
            all,
            dry_run: false,
            replace: false,
            take_ownership: false,
            ollama_name: None,
            yes: true,
        }
    }

    fn model_with_exposure(tool: &str) -> ModelEntry {
        let mut model = crate::commands::tests::test_model("Model A");
        model.exposures.insert(
            tool.to_string(),
            ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "symlink".to_string(),
                path: Some(std::path::PathBuf::from("/tmp/model-a")),
                created_by: Some("lmm".to_string()),
            },
        );
        model
    }

    fn candidate(path: &str, size_bytes: u64) -> CandidateFile {
        CandidateFile {
            path: path.to_string(),
            size_bytes,
        }
    }

    fn repo_file(path: &str, size_bytes: u64, oid: &str) -> RepoFile {
        RepoFile {
            path: path.to_string(),
            size_bytes,
            oid: oid.to_string(),
        }
    }
}
