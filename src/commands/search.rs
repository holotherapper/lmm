//! `lmm find` and `lmm search` — search Hugging Face with live results.
use crate::artifacts::{FormatDetection, detect_format};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::format::{Align, Table};
use crate::hf::{HfClient, ModelSummary, candidate_files};
use crate::model::FormatKind;
use crate::paths::{AppPaths, configured_hf_cache_dir};
use crate::tui;

use super::{FormatSelection, SearchInput};

pub fn search(paths: &AppPaths, input: &SearchInput) -> Result<()> {
    let config = Config::load(&paths.config_path)?;
    let hf_cache = configured_hf_cache_dir(&config);
    let client = HfClient::new(config.network.hf_endpoint, hf_cache);

    if input.query.is_none() && tui::can_run() {
        return find_interactive(paths, &client, input);
    }

    let Some(query) = input.query.as_deref().filter(|q| !q.trim().is_empty()) else {
        return Err(AppError::InvalidInput(
            "search needs a query when not running interactively".to_string(),
        ));
    };

    format::logo();
    format::intro("lmm search");
    format::step(&format!("Searching for \"{}\"", format::cyan(query)));
    let models = client.search_models(query, input.author.as_deref(), input.limit, &input.sort)?;
    let filtered = filter_search_results(&client, &models, input.format)?;
    if filtered.is_empty() {
        format::outro_cancel(&format!("No models found for \"{query}\""));
        return Ok(());
    }

    print_model_table(&filtered);
    eprintln!();
    eprintln!("Install with: {} <repo>", format::bold("lmm add"));
    Ok(())
}

fn find_interactive(paths: &AppPaths, client: &HfClient, input: &SearchInput) -> Result<()> {
    format::logo();
    format::intro("lmm search");

    let repo = loop {
        let query: String = tui::input("Search models", "e.g. llama mlx 4bit")?;
        if query.trim().is_empty() {
            format::outro_cancel("cancelled");
            return Ok(());
        }

        format::step(&format!("Searching for \"{}\"…", format::cyan(&query)));
        let models = client.search_models(&query, None, 100, "downloads")?;
        if models.is_empty() {
            format::info(&format!(
                "No models found for \"{query}\", try a different query"
            ));
            continue;
        }

        let count_label = if models.len() >= 100 {
            "100+ models".to_string()
        } else {
            format!("{} models", models.len())
        };
        format::info(&format!(
            "Found {count_label} (↑↓ to scroll, esc to search again)"
        ));

        let items: Vec<(String, String, String)> = models
            .iter()
            .map(|m| {
                let (meta, detail) = format_model_lines(m);
                (m.id.clone(), format!("{} · {}", m.id, meta), detail)
            })
            .collect();

        match tui::select_one("Select model", &items) {
            Ok(repo) => break repo,
            Err(AppError::Cancelled) => continue,
            Err(e) => return Err(e),
        }
    };

    format::success(&format!("Selected {}", format::cyan(&repo)));
    if let Ok(total_size) = client.model_total_size(&repo) {
        format::info(&format!(
            "Total size: {}",
            format::bold(&format::bytes(total_size))
        ));
    }

    super::add::add_repo(paths, &repo, input.format, input.yes)
}

fn format_model_lines(model: &ModelSummary) -> (String, String) {
    let mut meta_parts = Vec::new();

    let formats = model.formats();
    for fmt_name in &formats {
        meta_parts.push(format!("[{fmt_name}]"));
    }

    if let Some(bits) = model.quant_bits {
        meta_parts.push(format!("{bits}bit"));
    } else if let Some(quant) = model.quant() {
        meta_parts.push(quant.to_string());
    }

    let downloads = model
        .downloads
        .map(format::count)
        .unwrap_or_else(|| "—".to_string());
    let likes = model
        .likes
        .map(format::count)
        .unwrap_or_else(|| "—".to_string());
    meta_parts.push(format!("{downloads} ↓"));
    meta_parts.push(format!("{likes} likes"));

    let meta = meta_parts.join("  ");

    let mut detail_parts = Vec::new();

    if let Some(model_type) = &model.model_type {
        detail_parts.push(model_type.clone());
    }

    if let Some(arch) = model.architectures.first() {
        let short = arch
            .trim_end_matches("ForCausalLM")
            .trim_end_matches("ForConditionalGeneration")
            .trim_end_matches("Model");
        if !short.is_empty() && model.model_type.as_deref() != Some(short) {
            detail_parts.push(short.to_string());
        }
    }

    if let Some(pipeline) = &model.pipeline_tag {
        detail_parts.push(pipeline.clone());
    }

    (meta, detail_parts.join("  "))
}

fn print_model_table(models: &[&ModelSummary]) {
    let mut table = Table::new(&[
        ("REPO", Align::Left),
        ("DOWNLOADS", Align::Right),
        ("LIKES", Align::Right),
    ]);
    for model in models {
        table.row(&[
            &model.id,
            &model
                .downloads
                .map(format::count)
                .unwrap_or_else(|| "—".to_string()),
            &model
                .likes
                .map(format::count)
                .unwrap_or_else(|| "—".to_string()),
        ]);
    }
    print!("{}", table.render());
}

fn filter_search_results<'a>(
    client: &HfClient,
    models: &'a [ModelSummary],
    format: FormatSelection,
) -> Result<Vec<&'a ModelSummary>> {
    let FormatSelection::Format(format) = format else {
        return Ok(models.iter().collect());
    };

    let mut filtered = Vec::new();
    for model in models {
        if search_result_matches_format(client, model, format)? {
            filtered.push(model);
        }
    }
    Ok(filtered)
}

fn search_result_matches_format(
    client: &HfClient,
    model: &ModelSummary,
    format: FormatKind,
) -> Result<bool> {
    let commit = if let Some(sha) = &model.sha {
        sha.clone()
    } else {
        client.repo_metadata(&model.id, "main")?.commit
    };
    let repo_files = client.repo_files(&model.id, &commit)?;
    let files = candidate_files(&repo_files);
    let matches = match format {
        FormatKind::Gguf => files.iter().any(|file| file.path.ends_with(".gguf")),
        FormatKind::Mlx | FormatKind::Safetensors => {
            detect_format(&files, None, model.library_name.as_deref())
                == FormatDetection::Detected(format)
        }
        FormatKind::GgufSplit => files
            .iter()
            .any(|file| crate::artifacts::is_split_gguf(&file.path)),
        FormatKind::Unknown => true,
    };
    Ok(matches)
}
