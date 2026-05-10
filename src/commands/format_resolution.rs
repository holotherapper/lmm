//! Format detection, selection, and interactive resolution helpers.
use crate::artifacts::{CandidateFile, FormatDetection, detect_format};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::model::FormatKind;
use crate::tui;

use super::FormatSelection;

pub(crate) fn resolve_format(
    selection: FormatSelection,
    files: &[CandidateFile],
    selected_file: Option<&str>,
) -> Result<FormatKind> {
    match selection {
        FormatSelection::Format(format) => Ok(format),
        FormatSelection::Auto => match detect_format(files, selected_file, None) {
            FormatDetection::Detected(format) => Ok(format),
            FormatDetection::Ambiguous(message) | FormatDetection::Unsupported(message) => {
                Err(AppError::InvalidInput(message))
            }
        },
    }
}

pub(crate) fn effective_format_selection(
    selection: FormatSelection,
    config: &Config,
) -> Result<FormatSelection> {
    if selection != FormatSelection::Auto {
        return Ok(selection);
    }

    match config.defaults.format.as_str() {
        "auto" | "" => Ok(FormatSelection::Auto),
        value => parse_format_id(value).map(FormatSelection::Format),
    }
}

pub(crate) fn resolve_format_interactive(
    selection: FormatSelection,
    files: &[CandidateFile],
    selected_file: Option<&str>,
) -> Result<FormatKind> {
    match resolve_format(selection, files, selected_file) {
        Ok(format) => Ok(format),
        Err(error) if tui::can_run() => {
            let items = format_choice_items(files);
            if items.is_empty() {
                return Err(error);
            }
            let selected = tui::select_one("Select model format", &items)?;
            parse_format_id(&selected)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn format_choice_items(files: &[CandidateFile]) -> Vec<(String, String, String)> {
    let mut items = Vec::new();
    if files.iter().any(|file| file.path.ends_with(".gguf")) {
        items.push((
            "gguf".to_string(),
            "GGUF".to_string(),
            "single-file runtime artifact".to_string(),
        ));
    }
    if files.iter().any(|file| file.path.ends_with(".safetensors")) {
        items.push((
            "safetensors".to_string(),
            "safetensors".to_string(),
            "transformers-compatible snapshot".to_string(),
        ));
        items.push((
            "mlx".to_string(),
            "MLX".to_string(),
            "MLX snapshot".to_string(),
        ));
    }
    items
}

pub(crate) fn parse_format_id(id: &str) -> Result<FormatKind> {
    match id {
        "gguf" => Ok(FormatKind::Gguf),
        "mlx" => Ok(FormatKind::Mlx),
        "safetensors" => Ok(FormatKind::Safetensors),
        _ => Err(AppError::InvalidInput(format!("unknown format: {id}"))),
    }
}
