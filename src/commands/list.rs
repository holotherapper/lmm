//! `lmm list` and `lmm info` — display tracked models and metadata.
use serde::Serialize;

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::format::{Align, Table};
use crate::hf::HfClient;
use crate::lock::LockFile;
use crate::model::ModelEntry;
use crate::paths::{AppPaths, configured_hf_cache_dir};

use super::scan::{HfCacheEntry, hf_cache_entries, hf_cache_repo_dirs};
use super::{
    ExternalExposure, FormatSelection, ListInput, discover_external_exposures,
    model_matches_format, model_status, model_status_styled, model_where, reclaimable_bytes,
    verified_saved_bytes,
};

pub fn list(paths: &AppPaths, input: &ListInput) -> Result<()> {
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let lock = LockFile::load(&paths.lock_path)?;
    let rows = list_rows(&lock, &config, &hf_cache, input)?;

    if input.json {
        let encoded = serde_json::to_string_pretty(&rows)
            .map_err(|source| AppError::EncodeJson { source })?;
        println!("{encoded}");
        return Ok(());
    }

    if rows.is_empty() {
        format::heading("No local models found");
        format::kv("lock", &paths.lock_path.display().to_string());
        return Ok(());
    }

    if input.paths {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("KIND", Align::Left),
            ("FORMAT", Align::Left),
            ("SIZE", Align::Right),
            ("HF_CACHE", Align::Left),
            ("EXPOSURES", Align::Left),
            ("STATUS", Align::Left),
        ]);
        for row in &rows {
            table.row(&[
                &row.name,
                &row.kind,
                &row.format,
                &format::bytes(row.size_bytes),
                &row.hf_cache,
                &row.where_,
                &model_status_styled(&row.status),
            ]);
        }
        print!("{}", table.render());
    } else if input.wide {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("FORMAT", Align::Left),
            ("OWNERSHIP", Align::Left),
            ("SIZE", Align::Right),
            ("TOOLS", Align::Left),
            ("SAVED", Align::Right),
            ("RECLAIMABLE", Align::Right),
            ("REPO", Align::Left),
            ("COMMIT", Align::Left),
            ("STATUS", Align::Left),
        ]);
        for row in &rows {
            table.row(&[
                &row.name,
                &row.format,
                &row.ownership,
                &format::bytes(row.size_bytes),
                &row.where_,
                &format::bytes(row.saved_bytes),
                &format::bytes(row.reclaimable_bytes),
                &row.repo,
                &row.commit,
                &model_status_styled(&row.status),
            ]);
        }
        print!("{}", table.render());
    } else {
        let mut table = Table::new(&[
            ("NAME", Align::Left),
            ("FORMAT", Align::Left),
            ("SIZE", Align::Right),
            ("TOOLS", Align::Left),
            ("STATUS", Align::Left),
        ]);
        for row in &rows {
            table.row(&[
                &row.name,
                &row.format,
                &format::bytes(row.size_bytes),
                &row.where_,
                &model_status_styled(&row.status),
            ]);
        }
        print!("{}", table.render());
    }

    if !input.json {
        let has_untracked = rows.iter().any(|r| r.status == "untracked");
        let has_incomplete = rows.iter().any(|r| r.status == "incomplete");
        if has_untracked || has_incomplete {
            eprintln!();
        }
        if has_untracked {
            eprintln!(
                "  {}",
                format::dim("Untracked models found. Run \"lmm adopt\" to manage them.")
            );
        }
        if has_incomplete {
            eprintln!(
                "  {}",
                format::dim("Incomplete downloads found. Run \"lmm gc --yes\" to clean up.")
            );
        }
    }

    Ok(())
}

pub fn info(paths: &AppPaths, name: &str, json: bool, files: bool) -> Result<()> {
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let lock = LockFile::load(&paths.lock_path)?;
    let Some((_id, model)) = lock
        .models
        .iter()
        .find(|(_, model)| model.name.eq_ignore_ascii_case(name))
    else {
        return Err(AppError::InvalidInput(format!("model not found: {name}")));
    };

    if json {
        let encoded = serde_json::to_string_pretty(model)
            .map_err(|source| AppError::EncodeJson { source })?;
        println!("{encoded}");
        return Ok(());
    }

    let hf_url = format!("https://huggingface.co/{}", model.source.repo);
    format::heading(&model.name);
    format::kv(
        "repo",
        &format!("{} ({})", model.source.repo, format::dim(&hf_url)),
    );
    format::kv("format", &model.format.to_string());
    format::kv("size", &format::bytes(model.artifact.size_bytes));
    format::kv("ownership", &format!("{:?}", model.source.ownership));
    format::kv("where", &model_where(model));
    format::kv("saved", &format::bytes(verified_saved_bytes(model)?));
    format::kv("reclaimable", &format::bytes(reclaimable_bytes(model)));
    format::kv("status", &model_status_styled(model_status(model)?));
    format::kv("commit", &model.source.commit);

    // Fetch HF metadata for richer display
    let client = HfClient::new(config.network.hf_endpoint, hf_cache);
    if let Ok(hf_models) = client.search_models(&model.source.repo, None, 1, "downloads")
        && let Some(hf) = hf_models.first()
    {
        eprintln!();
        format::heading("Model metadata");
        if let Some(model_type) = &hf.model_type {
            format::kv("model type", model_type);
        }
        if let Some(arch) = hf.architectures.first() {
            format::kv("architecture", arch);
        }
        let formats = hf.formats();
        if !formats.is_empty() {
            format::kv("formats", &formats.join(", "));
        }
        if let Some(bits) = hf.quant_bits {
            format::kv("quantization", &format!("{bits}-bit"));
        } else if let Some(quant) = hf.quant() {
            format::kv("quantization", quant);
        }
        if let Some(pipeline) = &hf.pipeline_tag {
            format::kv("pipeline", pipeline);
        }
        if let Some(library) = &hf.library_name {
            format::kv("library", library);
        }
        format::kv(
            "downloads",
            &hf.downloads
                .map(format::count)
                .unwrap_or_else(|| "—".to_string()),
        );
        format::kv(
            "likes",
            &hf.likes
                .map(format::count)
                .unwrap_or_else(|| "—".to_string()),
        );
        if let Ok(total_size) = client.model_total_size(&model.source.repo) {
            format::kv("total repo size", &format::bytes(total_size));
        }
    }

    if let Some(location) = &model.locations.hf_cache {
        format::kv("hf cache", &location.snapshot_path.display().to_string());
    }

    eprintln!();
    if !model.exposures.is_empty() {
        format::heading("Exposures");
        let mut table = Table::new(&[
            ("TOOL", Align::Left),
            ("STRATEGY", Align::Left),
            ("STATUS", Align::Left),
            ("PATH", Align::Left),
        ]);
        for (tool, entry) in &model.exposures {
            let path = entry
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "—".to_string());
            table.row(&[tool, &entry.strategy, &format!("{:?}", entry.status), &path]);
        }
        print!("{}", table.render());
    }

    let hints = usage_hints(model)?;
    if !hints.is_empty() {
        eprintln!();
        format::heading("Usage");
        for hint in hints {
            eprintln!("  {hint}");
        }
    }

    if files {
        eprintln!();
        format::heading("Files");
        let mut table = Table::new(&[("PATH", Align::Left), ("SIZE", Align::Right)]);
        for file in &model.artifact.files {
            table.row(&[&file.path, &format::bytes(file.size_bytes)]);
        }
        print!("{}", table.render());
    }

    Ok(())
}

#[derive(Clone, Debug, Serialize)]
struct ListRow {
    name: String,
    kind: String,
    format: String,
    ownership: String,
    size_bytes: u64,
    #[serde(rename = "where")]
    where_: String,
    saved_bytes: u64,
    reclaimable_bytes: u64,
    repo: String,
    commit: String,
    status: String,
    hf_cache: String,
}

fn list_rows(
    lock: &LockFile,
    config: &Config,
    hf_cache: &std::path::Path,
    input: &ListInput,
) -> Result<Vec<ListRow>> {
    let mut rows = Vec::new();
    for model in filtered_models(lock, input) {
        rows.push(tracked_list_row(model)?);
    }

    for exposure in discover_external_exposures(config, lock)?
        .into_iter()
        .filter(|exposure| external_exposure_matches_list(exposure, input))
    {
        rows.push(external_exposure_list_row(&exposure));
    }

    if include_hf_cache_rows(input) {
        for entry in hf_cache_entries(&hf_cache_repo_dirs(hf_cache)?, lock)
            .into_iter()
            .filter(|entry| hf_entry_matches_format(entry, input.format))
        {
            rows.push(hf_cache_list_row(&entry));
        }
    }

    rows.sort_by(|left, right| {
        status_order(&left.kind, &left.status)
            .cmp(&status_order(&right.kind, &right.status))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
    });
    Ok(rows)
}

fn tracked_list_row(model: &ModelEntry) -> Result<ListRow> {
    let where_ = model_where(model);
    let status = model_status(model)?.to_string();
    Ok(ListRow {
        name: model.name.clone(),
        kind: "tracked".to_string(),
        format: model.format.to_string(),
        ownership: format!("{:?}", model.source.ownership),
        size_bytes: model.artifact.size_bytes,
        where_,
        saved_bytes: verified_saved_bytes(model)?,
        reclaimable_bytes: reclaimable_bytes(model),
        repo: model.source.repo.clone(),
        commit: model.source.commit.clone(),
        status,
        hf_cache: model
            .locations
            .hf_cache
            .as_ref()
            .map(|location| location.snapshot_path.display().to_string())
            .unwrap_or_else(|| "—".to_string()),
    })
}

fn external_exposure_list_row(exposure: &ExternalExposure) -> ListRow {
    let (size_bytes, format) = if exposure.size_hint > 0 {
        (exposure.size_hint, "gguf".to_string())
    } else {
        external_dir_info(&exposure.path)
    };
    ListRow {
        name: exposure.name.clone(),
        kind: "external".to_string(),
        format,
        ownership: "External".to_string(),
        size_bytes,
        where_: exposure.tool.clone(),
        saved_bytes: 0,
        reclaimable_bytes: 0,
        repo: "—".to_string(),
        commit: "—".to_string(),
        status: format!("external ({})", exposure.tool),
        hf_cache: "—".to_string(),
    }
}

fn external_dir_info(path: &std::path::Path) -> (u64, String) {
    let mut size: u64 = 0;
    let mut has_gguf = false;
    let mut has_safetensors = false;
    let mut has_config = false;

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Ok(meta) = p.metadata()
                && meta.is_file()
            {
                size += meta.len();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.ends_with(".gguf") {
                    has_gguf = true;
                }
                if name.ends_with(".safetensors") {
                    has_safetensors = true;
                }
                if name == "config.json" {
                    has_config = true;
                }
            }
        }
    }

    let format = if has_gguf {
        "gguf".to_string()
    } else if has_safetensors && has_config {
        if config_has_quantization(path) {
            "mlx".to_string()
        } else {
            "safetensors".to_string()
        }
    } else if has_safetensors {
        "safetensors".to_string()
    } else {
        "unknown".to_string()
    };
    (size, format)
}

fn config_has_quantization(dir: &std::path::Path) -> bool {
    let config_path = dir.join("config.json");
    let Ok(bytes) = std::fs::read(&config_path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value.get("quantization_config").is_some()
}

fn hf_cache_list_row(entry: &HfCacheEntry) -> ListRow {
    let status = if entry.format == "incomplete" {
        "incomplete".to_string()
    } else {
        "untracked".to_string()
    };
    ListRow {
        name: entry.repo.clone(),
        kind: "hf-cache".to_string(),
        format: entry.format.clone(),
        ownership: "Untracked".to_string(),
        size_bytes: entry.size_bytes,
        where_: "—".to_string(),
        saved_bytes: 0,
        reclaimable_bytes: 0,
        repo: entry.repo.clone(),
        commit: entry.commit.clone(),
        status,
        hf_cache: entry.snapshot_path.display().to_string(),
    }
}

fn status_order(kind: &str, status: &str) -> u8 {
    match kind {
        "tracked" => match status {
            "stale" | "partial" => 0,
            _ => 1,
        },
        "external" => 2,
        "hf-cache" => match status {
            "untracked" => 3,
            "incomplete" => 4,
            _ => 3,
        },
        _ => 3,
    }
}

fn hf_entry_matches_format(entry: &HfCacheEntry, selection: FormatSelection) -> bool {
    match selection {
        FormatSelection::Auto => true,
        FormatSelection::Format(kind) => entry.format == kind.to_string(),
    }
}

fn external_exposure_matches_list(exposure: &ExternalExposure, input: &ListInput) -> bool {
    matches!(input.format, FormatSelection::Auto)
        && input
            .tool
            .as_ref()
            .is_none_or(|tool| tool == &exposure.tool)
}

fn include_hf_cache_rows(input: &ListInput) -> bool {
    input.tool.is_none()
        || input
            .tool
            .as_deref()
            .is_some_and(|tool| matches!(tool, "hf" | "hf-cache"))
}

fn filtered_models<'a>(lock: &'a LockFile, input: &ListInput) -> Vec<&'a ModelEntry> {
    lock.models
        .values()
        .filter(|model| model_matches_format(model, input.format))
        .filter(|model| {
            input
                .tool
                .as_ref()
                .is_none_or(|tool| model.exposures.contains_key(tool))
        })
        .collect()
}

fn usage_hints(model: &ModelEntry) -> Result<Vec<String>> {
    let mut hints = Vec::new();
    let snapshot_path = model
        .locations
        .hf_cache
        .as_ref()
        .map(|location| location.snapshot_path.display().to_string());

    if model.exposures.contains_key("mlx-lm")
        && let Some(path) = &snapshot_path
    {
        hints.push(format!(
            "{}: mlx_lm.load(\"{}\")",
            format::cyan("mlx-lm"),
            path
        ));
    }
    if model.exposures.contains_key("transformers")
        && let Some(path) = &snapshot_path
    {
        hints.push(format!(
            "{}: AutoModelForCausalLM.from_pretrained(\"{}\")",
            format::cyan("transformers"),
            path
        ));
    }
    if model.exposures.contains_key("llama-cpp") {
        let Some(location) = &model.locations.hf_cache else {
            return Ok(hints);
        };
        if let Some(file) = model
            .artifact
            .files
            .iter()
            .find(|file| file.path.ends_with(".gguf"))
        {
            let model_path = crate::security::safe_join(&location.snapshot_path, &file.path)?;
            hints.push(format!(
                "{}: llama-cli -m '{}'",
                format::cyan("llama.cpp"),
                model_path.display()
            ));
        }
    }

    Ok(hints)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::config::Config;
    use crate::lock::LockFile;
    use crate::model::{ArtifactFile, ExposureEntry, ExposureStatus, FormatKind, HfCacheLocation};

    use super::*;

    #[test]
    fn external_dir_info_should_detect_gguf_and_sum_file_sizes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("model.gguf"), b"abcd").unwrap();
        fs::write(dir.path().join("README.md"), b"ef").unwrap();

        let info = external_dir_info(dir.path());

        assert_eq!(info, (6, "gguf".to_string()));
    }

    #[test]
    fn external_dir_info_should_detect_quantized_safetensors_as_mlx() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("model.safetensors"), b"abcd").unwrap();
        fs::write(
            dir.path().join("config.json"),
            br#"{"quantization_config":{"bits":4}}"#,
        )
        .unwrap();

        let info = external_dir_info(dir.path());

        assert_eq!(info.1, "mlx");
    }

    #[test]
    fn external_dir_info_should_report_unknown_without_runtime_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README.md"), b"abcd").unwrap();

        let info = external_dir_info(dir.path());

        assert_eq!(info.1, "unknown");
    }

    #[test]
    fn filtered_models_should_filter_by_format() {
        let mut lock = LockFile::default();
        lock.models
            .insert("a".to_string(), model_with_format("A", FormatKind::Gguf));
        lock.models.insert(
            "b".to_string(),
            model_with_format("B", FormatKind::Safetensors),
        );
        let input = list_input_with_filter(FormatSelection::Format(FormatKind::Gguf), None);

        let models = filtered_models(&lock, &input);

        assert_eq!(models[0].name, "A");
    }

    #[test]
    fn filtered_models_should_filter_by_existing_exposure_tool() {
        let mut lock = LockFile::default();
        let mut model = model_with_format("A", FormatKind::Gguf);
        model
            .exposures
            .insert("lmstudio".to_string(), exposure_entry("/tmp/a"));
        lock.models.insert("a".to_string(), model);
        let input = list_input_with_filter(FormatSelection::Auto, Some("lmstudio".to_string()));

        let models = filtered_models(&lock, &input);

        assert_eq!(models.len(), 1);
    }

    #[test]
    fn external_exposure_matches_list_should_require_auto_format() {
        let exposure = external_exposure("jan");
        let input = list_input_with_filter(FormatSelection::Format(FormatKind::Gguf), None);

        assert!(!external_exposure_matches_list(&exposure, &input));
    }

    #[test]
    fn include_hf_cache_rows_should_accept_hf_cache_tool_filter() {
        let input = list_input_with_filter(FormatSelection::Auto, Some("hf-cache".to_string()));

        assert!(include_hf_cache_rows(&input));
    }

    #[test]
    fn tracked_list_row_should_include_where_and_reclaimable_bytes() {
        let mut model = model_with_format("A", FormatKind::Gguf);
        model.artifact.size_bytes = 42;
        model
            .exposures
            .insert("lmstudio".to_string(), exposure_entry("/tmp/a"));

        let row = tracked_list_row(&model).unwrap();

        assert_eq!(
            (row.where_, row.reclaimable_bytes),
            ("lmstudio".to_string(), 42)
        );
    }

    #[test]
    fn external_exposure_list_row_should_use_size_hint_for_ollama() {
        let mut exposure = external_exposure("ollama");
        exposure.size_hint = 1024;

        let row = external_exposure_list_row(&exposure);

        assert_eq!((row.format, row.size_bytes), ("gguf".to_string(), 1024));
    }

    #[test]
    fn usage_hints_should_include_llama_cpp_model_path_for_gguf() {
        let mut model = model_with_format("A", FormatKind::Gguf);
        model.locations.hf_cache = Some(HfCacheLocation {
            snapshot_path: PathBuf::from("/cache/snapshot"),
        });
        model.artifact.files = vec![ArtifactFile {
            path: "model.gguf".to_string(),
            size_bytes: 1,
            hf_blob: None,
            sha256: None,
            role: None,
        }];
        model
            .exposures
            .insert("llama-cpp".to_string(), exposure_entry("/tmp/llama"));

        let hints = usage_hints(&model).unwrap();

        assert!(hints[0].contains("llama-cli -m '/cache/snapshot/model.gguf'"));
    }

    #[test]
    fn list_rows_should_sort_tracked_rows_case_insensitively() {
        let mut lock = LockFile::default();
        let mut beta = model_with_format("beta", FormatKind::Gguf);
        beta.exposures
            .insert("lmstudio".to_string(), exposure_entry("/tmp/beta"));
        let mut alpha = model_with_format("Alpha", FormatKind::Gguf);
        alpha
            .exposures
            .insert("lmstudio".to_string(), exposure_entry("/tmp/alpha"));
        lock.models.insert("b".to_string(), beta);
        lock.models.insert("a".to_string(), alpha);
        let mut config = Config::default();
        config.paths.lmstudio = None;
        config.paths.jan = None;
        let input = list_input_with_filter(FormatSelection::Auto, Some("lmstudio".to_string()));
        let hf_cache = tempfile::tempdir().unwrap();

        let rows = list_rows(&lock, &config, hf_cache.path(), &input).unwrap();

        assert_eq!(
            rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
            vec!["Alpha", "beta"]
        );
    }

    fn list_input_with_filter(format: FormatSelection, tool: Option<String>) -> ListInput {
        ListInput {
            wide: false,
            paths: false,
            json: false,
            format,
            tool,
        }
    }

    fn model_with_format(name: &str, format: FormatKind) -> ModelEntry {
        let mut model = crate::commands::tests::test_model(name);
        model.format = format;
        model
    }

    fn exposure_entry(path: &str) -> ExposureEntry {
        ExposureEntry {
            status: ExposureStatus::Ok,
            strategy: "symlink".to_string(),
            path: Some(PathBuf::from(path)),
            created_by: Some("lmm".to_string()),
        }
    }

    fn external_exposure(tool: &str) -> ExternalExposure {
        ExternalExposure {
            id: format!("external:{tool}:model"),
            name: "model".to_string(),
            tool: tool.to_string(),
            path: PathBuf::from("/tmp/model"),
            size_hint: 0,
        }
    }
}
