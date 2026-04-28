//! CLI command implementations and shared helpers.
mod add;
mod adopt;
mod config;
pub(crate) mod consolidate;
mod doctor;
mod external_scan;
mod format_resolution;
mod gc;
mod list;
mod model_helpers;
mod remove;
mod scan;
mod search;
mod tool_resolution;
mod update;

use std::io;

use clap::CommandFactory;
use clap_complete::{Shell, generate};

use crate::error::Result;
use crate::format;
use crate::tui;

pub use add::add;
pub use adopt::adopt;
pub use config::{config_get, config_path, config_set};
pub use doctor::doctor;
pub use gc::gc;
pub use list::{info, list};
pub use remove::remove;
pub use search::search;
pub use update::update;

pub(crate) use external_scan::{ExternalExposure, discover_external_exposures};
pub(crate) use format_resolution::{
    effective_format_selection, parse_format_id, resolve_format_interactive,
};
pub(crate) use model_helpers::{
    exposure_is_stale, model_matches_format, model_name_matches, model_status, model_status_styled,
    model_where, reclaimable_bytes, remove_all_tools, repo_cache_from_snapshot, short_commit,
    verified_saved_bytes,
};
pub(crate) use tool_resolution::{
    auto_available_tools, resolve_tools, resolve_tools_interactive, tool_list, validate_tools,
};

use crate::model::FormatKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatSelection {
    Auto,
    Format(FormatKind),
}

pub struct AddInput {
    pub repo: Option<String>,
    pub revision: String,
    pub file: Option<String>,
    pub name: Option<String>,
    pub tools: Vec<String>,
    pub format: FormatSelection,
    pub list: bool,
    pub all: bool,
    pub dry_run: bool,
    pub replace: bool,
    pub take_ownership: bool,
    pub ollama_name: Option<String>,
    pub yes: bool,
}

pub struct RemoveInput {
    pub names: Vec<String>,
    pub tools: Vec<String>,
    pub all: bool,
    pub keep_cache: bool,
    pub purge_cache: bool,
    pub dry_run: bool,
    pub yes: bool,
}

pub struct ListInput {
    pub wide: bool,
    pub paths: bool,
    pub json: bool,
    pub format: FormatSelection,
    pub tool: Option<String>,
}

pub struct AdoptInput {
    pub names: Vec<String>,
    pub tools: Vec<String>,
    pub take_ownership: bool,
    pub yes: bool,
}

pub struct SearchInput {
    pub query: Option<String>,
    pub format: FormatSelection,
    pub author: Option<String>,
    pub limit: usize,
    pub sort: String,
    pub yes: bool,
}

pub struct UpdateInput {
    pub names: Vec<String>,
    pub all: bool,
    pub dry_run: bool,
    pub yes: bool,
}

pub fn completions<C>(shell: Shell)
where
    C: CommandFactory,
{
    let mut command = C::command();
    let bin_name = command.get_name().to_string();
    generate(shell, &mut command, bin_name, &mut io::stdout());
}

fn confirm(auto_confirm: bool, prompt: &str) -> Result<bool> {
    if auto_confirm {
        return Ok(true);
    }

    if !tui::can_run() {
        format::status("⊘", &format::dim("plan-only; re-run with --yes to execute"));
        return Ok(false);
    }

    tui::confirm(prompt)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    use crate::config::Config;
    use crate::error::AppError;
    use crate::lock::LockFile;
    use crate::model::{
        Artifact, ArtifactFile, ExposureEntry, ExposureStatus, FormatKind, HfCacheLocation,
        Locations, ModelEntry, Ownership, Source,
    };

    use super::external_scan::{
        external_exposure_id, ollama_manifest_size, scan_direct_tool_exposures,
        scan_lmstudio_exposures,
    };
    use super::format_resolution::{format_choice_items, resolve_format};
    use super::tool_resolution::{adapter_detail, expand_tools};
    use super::update::update_target_ids;
    use super::{
        FormatSelection, RemoveInput, UpdateInput, effective_format_selection,
        model_matches_format, model_name_matches, model_status, model_where,
        repo_cache_from_snapshot, resolve_tools, validate_tools, verified_saved_bytes,
    };

    #[test]
    fn tool_all_should_not_auto_run_importing_registry_adapters() {
        let tools = expand_tools(&["all".to_string()], FormatKind::Gguf);

        assert!(tools.iter().any(|tool| tool == "lmstudio"));
        assert!(tools.iter().any(|tool| tool == "llama-cpp"));
        assert!(tools.iter().any(|tool| tool == "jan"));
        assert!(!tools.iter().any(|tool| tool == "ollama"));
    }

    #[test]
    fn remove_name_matching_should_allow_case_only_differences() {
        assert!(model_name_matches("Qwen3-8B-MLX", "qwen3-8b-mlx"));
    }

    #[test]
    fn update_with_yes_and_no_names_targets_all_models() {
        let mut lock = LockFile::default();
        lock.models
            .insert("id-a".to_string(), test_model("Model A"));
        lock.models
            .insert("id-b".to_string(), test_model("Model B"));

        let ids = update_target_ids(
            &lock,
            &UpdateInput {
                names: Vec::new(),
                all: true,
                dry_run: false,
                yes: false,
            },
        )
        .unwrap();

        assert_eq!(ids, vec!["id-a".to_string(), "id-b".to_string()]);
    }

    #[test]
    fn update_targets_match_names_case_insensitively() {
        let mut lock = LockFile::default();
        lock.models
            .insert("id-qwen".to_string(), test_model("Qwen3-8B-MLX"));

        let ids = update_target_ids(
            &lock,
            &UpdateInput {
                names: vec!["qwen3-8b-mlx".to_string()],
                all: false,
                dry_run: false,
                yes: false,
            },
        )
        .unwrap();

        assert_eq!(ids, vec!["id-qwen".to_string()]);
    }

    #[test]
    fn validate_tools_should_reject_unknown_adapter() {
        let error = validate_tools(&["unknown".to_string()], FormatSelection::Auto).unwrap_err();

        assert!(matches!(error, AppError::UnknownAdapter(tool) if tool == "unknown"));
    }

    #[test]
    fn validate_tools_should_reject_incompatible_format() {
        let error = validate_tools(
            &["ollama".to_string()],
            FormatSelection::Format(FormatKind::Safetensors),
        )
        .unwrap_err();

        assert!(matches!(error, AppError::InvalidInput(ref msg) if msg.contains("ollama")));
    }

    #[test]
    fn effective_format_selection_should_use_config_default() {
        let mut config = Config::default();
        config.defaults.format = "gguf".to_string();

        let selection = effective_format_selection(FormatSelection::Auto, &config).unwrap();

        assert_eq!(selection, FormatSelection::Format(FormatKind::Gguf));
    }

    #[test]
    fn effective_format_selection_should_reject_unknown_config_default() {
        let mut config = Config::default();
        config.defaults.format = "onnx".to_string();

        let error = effective_format_selection(FormatSelection::Auto, &config).unwrap_err();

        assert!(
            matches!(error, AppError::InvalidInput(message) if message == "unknown format: onnx")
        );
    }

    #[test]
    fn resolve_tools_should_prefer_config_defaults_when_no_tools_requested() {
        let mut config = Config::default();
        config.defaults.default_tools = vec!["transformers".to_string()];

        let tools = resolve_tools(&[], FormatKind::Safetensors, &config);

        assert_eq!(tools, vec!["transformers".to_string()]);
    }

    #[test]
    fn resolve_tools_should_expand_all_to_compatible_non_registry_tools() {
        let tools = resolve_tools(&["all".to_string()], FormatKind::Gguf, &Config::default());

        assert!(!tools.iter().any(|tool| tool == "ollama"));
    }

    #[test]
    fn resolve_format_should_return_explicit_selection_without_detection() {
        let format = resolve_format(FormatSelection::Format(FormatKind::Mlx), &[], None).unwrap();

        assert_eq!(format, FormatKind::Mlx);
    }

    #[test]
    fn format_choices_should_offer_snapshot_formats_for_safetensors_files() {
        let items = format_choice_items(&[
            candidate("config.json"),
            candidate("model.safetensors"),
            candidate("tokenizer.json"),
        ]);

        assert_eq!(
            items
                .iter()
                .map(|(id, _, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["safetensors", "mlx"]
        );
    }

    #[test]
    fn adapter_detail_should_describe_importing_registry() {
        let detail = adapter_detail(crate::adapters::AdapterClass::ImportingRegistry);

        assert_eq!(detail, "imports through tool registry");
    }

    #[test]
    fn model_where_should_join_present_exposure_tools() {
        let mut model = test_model("Model A");
        model.exposures.insert(
            "lmstudio".to_string(),
            exposure_entry("symlink", Path::new("/tmp/model-a")),
        );
        assert_eq!(model_where(&model), "lmstudio");
    }

    #[test]
    fn model_status_should_mark_missing_snapshot_files_partial() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = test_model("Model A");
        model.locations.hf_cache = Some(HfCacheLocation {
            snapshot_path: dir.path().join("snapshot"),
        });
        model.artifact.files = vec![artifact_file("missing.gguf", 1, None)];

        let status = model_status(&model).unwrap();

        assert_eq!(status, "partial");
    }

    #[test]
    fn model_status_should_mark_missing_exposure_path_stale() {
        let mut model = test_model("Model A");
        model.exposures.insert(
            "lmstudio".to_string(),
            exposure_entry("symlink", Path::new("/definitely/missing")),
        );

        let status = model_status(&model).unwrap();

        assert_eq!(status, "stale");
    }

    #[cfg(unix)]
    #[test]
    fn verified_saved_bytes_should_count_symlink_exposure_to_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = dir.path().join("snapshot");
        let exposure = dir.path().join("exposure");
        fs::create_dir_all(&snapshot).unwrap();
        fs::create_dir_all(&exposure).unwrap();
        fs::write(snapshot.join("model.gguf"), b"model").unwrap();
        std::os::unix::fs::symlink(snapshot.join("model.gguf"), exposure.join("model.gguf"))
            .unwrap();
        let mut model = test_model("Model A");
        model.artifact.size_bytes = 5;
        model.artifact.files = vec![artifact_file("model.gguf", 5, None)];
        model.locations.hf_cache = Some(HfCacheLocation {
            snapshot_path: snapshot,
        });
        model
            .exposures
            .insert("lmstudio".to_string(), exposure_entry("symlink", &exposure));

        let saved_bytes = verified_saved_bytes(&model).unwrap();

        assert_eq!(saved_bytes, 5);
    }

    #[test]
    fn model_matches_format_should_accept_auto_filter() {
        let model = test_model("Model A");

        assert!(model_matches_format(&model, FormatSelection::Auto));
    }

    #[test]
    fn external_exposure_id_should_include_tool_and_path() {
        let id = external_exposure_id("jan", Path::new("/models/foo"));

        assert_eq!(id, "external:jan:/models/foo");
    }

    #[test]
    fn scan_direct_tool_exposures_should_skip_tracked_paths() {
        let dir = tempfile::tempdir().unwrap();
        let tracked = dir.path().join("tracked");
        let untracked = dir.path().join("untracked");
        fs::create_dir_all(&tracked).unwrap();
        fs::create_dir_all(&untracked).unwrap();
        let tracked_paths = [tracked].into_iter().collect();

        let exposures = scan_direct_tool_exposures("jan", dir.path(), &tracked_paths).unwrap();

        assert_eq!(exposures[0].name, "untracked");
    }

    #[test]
    fn scan_lmstudio_exposures_should_read_author_model_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("author").join("model")).unwrap();
        let tracked_paths = Default::default();

        let exposures = scan_lmstudio_exposures(dir.path(), &tracked_paths).unwrap();

        assert_eq!(exposures[0].name, "model");
    }

    #[test]
    fn ollama_manifest_size_should_sum_layer_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("latest");
        fs::write(
            &manifest,
            r#"{"layers":[{"size":2},{"size":3},{"digest":"x"}]}"#,
        )
        .unwrap();

        assert_eq!(ollama_manifest_size(&manifest), 5);
    }

    #[test]
    fn repo_cache_from_snapshot_should_return_repository_cache_root() {
        let snapshot = Path::new("/cache/models--org--repo/snapshots/commit");

        assert_eq!(
            repo_cache_from_snapshot(snapshot).unwrap(),
            Path::new("/cache/models--org--repo")
        );
    }

    #[test]
    fn remove_all_tools_should_treat_explicit_all_as_full_removal() {
        let input = RemoveInput {
            names: vec!["model".to_string()],
            tools: vec!["all".to_string()],
            all: false,
            keep_cache: false,
            purge_cache: false,
            dry_run: false,
            yes: true,
        };

        assert!(super::remove_all_tools(&input));
    }

    pub(crate) fn test_model(name: &str) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            source: Source {
                kind: "hf".to_string(),
                repo: "org/repo".to_string(),
                revision: "main".to_string(),
                commit: "old".to_string(),
                ownership: Ownership::Managed,
            },
            format: FormatKind::Safetensors,
            artifact: Artifact {
                signature: "signature".to_string(),
                size_bytes: 1,
                full_snapshot: false,
                files: Vec::new(),
            },
            locations: Locations { hf_cache: None },
            exposures: BTreeMap::new(),
        }
    }

    fn candidate(path: &str) -> crate::artifacts::CandidateFile {
        crate::artifacts::CandidateFile {
            path: path.to_string(),
            size_bytes: 1,
        }
    }

    fn artifact_file(path: &str, size_bytes: u64, hf_blob: Option<&str>) -> ArtifactFile {
        ArtifactFile {
            path: path.to_string(),
            size_bytes,
            hf_blob: hf_blob.map(str::to_string),
            sha256: None,
            role: None,
        }
    }

    fn exposure_entry(strategy: &str, path: &Path) -> ExposureEntry {
        ExposureEntry {
            status: ExposureStatus::Ok,
            strategy: strategy.to_string(),
            path: Some(path.to_path_buf()),
            created_by: Some("lmm".to_string()),
        }
    }
}
