//! Shared model inspection, status, and matching helpers.
use std::path::Path;

use crate::error::{AppError, Result};
use crate::format;
use crate::model::{ExposureEntry, ModelEntry};

use super::{FormatSelection, RemoveInput};

pub(crate) fn model_name_matches(existing: &str, requested: &str) -> bool {
    existing == requested || existing.eq_ignore_ascii_case(requested)
}

pub(crate) fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

pub(crate) fn model_where(model: &ModelEntry) -> String {
    let tools: Vec<&str> = model.exposures.keys().map(|tool| tool.as_str()).collect();
    if tools.is_empty() {
        "\u{2014}".to_string()
    } else {
        tools.join(", ")
    }
}

pub(crate) fn model_status(model: &ModelEntry) -> Result<&'static str> {
    if model_has_missing_snapshot_files(model)? {
        return Ok("partial");
    }
    if model.exposures.values().any(exposure_is_stale) {
        return Ok("stale");
    }
    Ok("tracked")
}

fn model_has_missing_snapshot_files(model: &ModelEntry) -> Result<bool> {
    let Some(location) = &model.locations.hf_cache else {
        return Ok(!model.artifact.files.is_empty());
    };
    for file in &model.artifact.files {
        let path = crate::security::safe_join(&location.snapshot_path, &file.path)?;
        if !path.exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn exposure_is_stale(entry: &ExposureEntry) -> bool {
    if entry.strategy == "ollama-create" {
        return false;
    }
    entry.path.as_ref().is_some_and(|path| !path.exists())
}

pub(crate) fn verified_saved_bytes(model: &ModelEntry) -> Result<u64> {
    let mut saved_bytes = 0_u64;
    for entry in model.exposures.values() {
        if entry.strategy == "symlink" && exposure_links_to_snapshot(model, entry)? {
            saved_bytes += model.artifact.size_bytes;
        }
    }
    Ok(saved_bytes)
}

fn exposure_links_to_snapshot(model: &ModelEntry, entry: &ExposureEntry) -> Result<bool> {
    let Some(target_root) = &entry.path else {
        return Ok(false);
    };
    let Some(location) = &model.locations.hf_cache else {
        return Ok(false);
    };

    for file in &model.artifact.files {
        let source = crate::security::safe_join(&location.snapshot_path, &file.path)?;
        let target = crate::security::safe_join(target_root, &file.path)?;
        if !target.exists() {
            return Ok(false);
        }
        let source = std::fs::canonicalize(&source).map_err(|source_error| AppError::Read {
            path: source,
            source: source_error,
        })?;
        let target = std::fs::canonicalize(&target).map_err(|source_error| AppError::Read {
            path: target,
            source: source_error,
        })?;
        if source != target {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn reclaimable_bytes(model: &ModelEntry) -> u64 {
    if model.source.ownership.deletes_canonical_bytes_by_default() {
        model.artifact.size_bytes
    } else {
        0
    }
}

pub(crate) fn model_status_styled(status: &str) -> String {
    match status {
        "tracked" => format::green("tracked"),
        "partial" => format::yellow("partial"),
        "stale" => format::yellow("stale"),
        "incomplete" => format::dim("incomplete"),
        "untracked" => format::dim("untracked"),
        s if s.starts_with("external") => format::dim(s),
        other => other.to_string(),
    }
}

pub(crate) fn repo_cache_from_snapshot(snapshot_path: &Path) -> Option<std::path::PathBuf> {
    snapshot_path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

pub(crate) fn model_matches_format(model: &ModelEntry, format: FormatSelection) -> bool {
    match format {
        FormatSelection::Auto => true,
        FormatSelection::Format(format) => model.format == format,
    }
}

pub(crate) fn remove_all_tools(input: &RemoveInput) -> bool {
    input.tools.is_empty() || input.tools.iter().any(|tool| tool == "all")
}
