//! Tool adapter trait and registry.
//!
//! Each supported tool implements [`ToolAdapter`], providing `expose` and
//! `remove_exposure` methods. New tools are added by creating a module and
//! registering it in [`all_adapters`].
mod comfyui;
pub(crate) mod hf_direct;
mod jan;
mod llama_cpp;
mod lmstudio;
mod ollama;
mod webui;

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::hf::RepoFile;
use crate::model::{ExposureEntry, ExposureStatus, FormatKind};

// ── Adapter trait ───────────────────────────────────────────────────────

#[allow(dead_code)]
pub trait ToolAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn class(&self) -> AdapterClass;
    fn supported_formats(&self) -> &'static [FormatKind];
    fn maturity(&self) -> AdapterMaturity {
        AdapterMaturity::Stable
    }
    fn creates_physical_copy(&self) -> CopyBehavior {
        CopyBehavior::NoStore
    }
    fn is_configured(&self, _config: &crate::config::Config) -> bool {
        true
    }

    fn expose(&self, request: &ExposeRequest<'_>) -> Result<ExposureEntry>;

    fn remove_exposure(&self, entry: &ExposureEntry, config: &Config) -> Result<()>;

    fn incompatibility_reason(&self, format: &FormatKind) -> Option<&'static str> {
        if self.supported_formats().contains(format) {
            None
        } else {
            Some("unsupported format for this tool")
        }
    }
}

// ── Request types ───────────────────────────────────────────────────────

pub struct ExposeRequest<'a> {
    pub tools: &'a [String],
    pub repo: &'a str,
    pub alias: &'a str,
    pub format: FormatKind,
    pub snapshot_root: &'a Path,
    pub files: &'a [RepoFile],
    pub config: &'a Config,
    pub replace: bool,
    pub ollama_name: Option<&'a str>,
}

// ── Enums ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterClass {
    FilesystemExposure,
    DirectHfReader,
    PathConsumer,
    ImportingRegistry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum CopyBehavior {
    NoStore,
    VerifiedDedupe,
    UnknownCopy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum AdapterMaturity {
    Stable,
    Experimental,
}

// ── Registry ────────────────────────────────────────────────────────────

pub fn all_adapters() -> Vec<Box<dyn ToolAdapter>> {
    vec![
        Box::new(lmstudio::LmStudio),
        Box::new(comfyui::ComfyUi),
        Box::new(llama_cpp::LlamaCpp),
        Box::new(jan::Jan),
        Box::new(ollama::Ollama),
        Box::new(webui::A1111),
        Box::new(webui::InvokeAi),
        Box::new(webui::Fooocus),
        Box::new(webui::TextGenWebui),
        Box::new(webui::Gpt4All),
    ]
}

pub fn find_adapter(id: &str) -> Result<Box<dyn ToolAdapter>> {
    all_adapters()
        .into_iter()
        .find(|a| a.id() == id)
        .ok_or_else(|| {
            let mut known: Vec<_> = all_adapters().iter().map(|a| a.id()).collect();
            for tool in hf_direct::DIRECT_HF_TOOLS {
                known.push(tool.id);
            }
            AppError::UnknownAdapter(format!("{id} (available: {})", known.join(", ")))
        })
}

// ── Bulk operations ─────────────────────────────────────────────────────

pub fn create_exposures(request: &ExposeRequest<'_>) -> Result<BTreeMap<String, ExposureEntry>> {
    let mut exposures = BTreeMap::new();
    let mut created: Vec<(String, ExposureEntry)> = Vec::new();

    for tool_id in request.tools {
        let adapter = find_adapter(tool_id)?;
        match adapter.expose(request) {
            Ok(entry) => {
                created.push((tool_id.clone(), entry.clone()));
                exposures.insert(tool_id.clone(), entry);
            }
            Err(error) => {
                rollback(&created, request.config);
                return Err(error);
            }
        }
    }

    Ok(exposures)
}

pub fn remove_exposure(tool_id: &str, entry: &ExposureEntry, config: &Config) -> Result<()> {
    let adapter = find_adapter(tool_id)?;
    adapter.remove_exposure(entry, config)
}

pub fn compatibility_reason(tool_id: &str, format: &FormatKind) -> Option<&'static str> {
    if let Ok(adapter) = find_adapter(tool_id) {
        return adapter.incompatibility_reason(format);
    }
    if let Some(tool) = hf_direct::DIRECT_HF_TOOLS.iter().find(|t| t.id == tool_id) {
        if tool.formats.contains(format) {
            return None;
        }
        return Some("unsupported format for this tool");
    }
    None
}

fn rollback(created: &[(String, ExposureEntry)], config: &Config) {
    for (tool_id, entry) in created.iter().rev() {
        if let Ok(adapter) = find_adapter(tool_id) {
            let _ = adapter.remove_exposure(entry, config);
        }
    }
}

// ── Shared helpers for filesystem-based adapters ────────────────────────

pub fn expose_symlink_tree(
    root: &Path,
    target_dir: &Path,
    snapshot_root: &Path,
    files: &[RepoFile],
    replace: bool,
    strategy: &str,
) -> Result<ExposureEntry> {
    use crate::security::ensure_child_path;
    use std::fs;

    ensure_child_path(root, target_dir)?;

    if target_dir.exists() && !replace {
        return Err(AppError::InvalidInput(format!(
            "target already exists: {}",
            target_dir.display()
        )));
    }

    let stage = stage_path_for(target_dir)?;
    ensure_child_path(root, &stage)?;
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|source| AppError::Write {
            path: stage.clone(),
            source,
        })?;
    }
    if let Some(parent) = stage.parent() {
        fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    if let Err(error) = create_symlink_tree(&stage, snapshot_root, files) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }

    if target_dir.exists() {
        let canonical_target = fs::canonicalize(target_dir).map_err(|source| AppError::Read {
            path: target_dir.to_path_buf(),
            source,
        })?;
        let canonical_root = fs::canonicalize(root).map_err(|source| AppError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        ensure_child_path(&canonical_root, &canonical_target)?;
        fs::remove_dir_all(target_dir).map_err(|source| AppError::Write {
            path: target_dir.to_path_buf(),
            source,
        })?;
    }
    fs::rename(&stage, target_dir).map_err(|source| AppError::Rename {
        from: stage,
        to: target_dir.to_path_buf(),
        source,
    })?;

    Ok(ExposureEntry {
        status: ExposureStatus::Ok,
        strategy: strategy.to_string(),
        path: Some(target_dir.to_path_buf()),
        created_by: Some("lmm".to_string()),
    })
}

fn stage_path_for(target: &Path) -> Result<std::path::PathBuf> {
    let Some(name) = target.file_name().and_then(|n| n.to_str()) else {
        return Err(AppError::InvalidInput(format!(
            "target path has no file name: {}",
            target.display()
        )));
    };
    Ok(target.with_file_name(format!(".{name}.lmm.{}.tmp", std::process::id())))
}

fn create_symlink_tree(stage: &Path, snapshot_root: &Path, files: &[RepoFile]) -> Result<()> {
    use crate::security::safe_join;
    use std::fs;

    for file in files {
        let source = safe_join(snapshot_root, &file.path)?;
        let destination = safe_join(stage, &file.path)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        create_relative_symlink(&source, &destination)?;
    }
    Ok(())
}

fn create_relative_symlink(source: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let link_target =
            pathdiff::diff_paths(source, target.parent().unwrap_or_else(|| Path::new(".")))
                .unwrap_or_else(|| source.to_path_buf());
        std::os::unix::fs::symlink(link_target, target).map_err(|source_error| AppError::Write {
            path: target.to_path_buf(),
            source: source_error,
        })
    }
    #[cfg(not(unix))]
    {
        Err(AppError::InvalidInput(format!(
            "symlink creation is unsupported on this platform for `{}`",
            target.display()
        )))
    }
}

pub fn remove_directory_exposure(entry: &ExposureEntry, tool_root: &Path) -> Result<()> {
    if let Some(path) = &entry.path
        && path.exists()
    {
        crate::security::ensure_child_path(tool_root, path)?;

        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            std::fs::remove_file(path).map_err(|source| AppError::Write {
                path: path.clone(),
                source,
            })?;
        } else {
            let canonical = std::fs::canonicalize(path).map_err(|source| AppError::Read {
                path: path.clone(),
                source,
            })?;
            let canonical_root =
                std::fs::canonicalize(tool_root).map_err(|source| AppError::Read {
                    path: tool_root.to_path_buf(),
                    source,
                })?;
            crate::security::ensure_child_path(&canonical_root, &canonical)?;
            std::fs::remove_dir_all(path).map_err(|source| AppError::Write {
                path: path.clone(),
                source,
            })?;
        }
    }
    Ok(())
}

// ── Backward-compatible re-exports ──────────────────────────────────────

pub fn builtin_adapters_with_config(config: &crate::config::Config) -> Vec<AdapterInfo> {
    all_adapters()
        .iter()
        .map(|a| AdapterInfo {
            id: a.id(),
            display_name: a.display_name(),
            class: a.class(),
            maturity: a.maturity(),
            formats: a.supported_formats(),
            can_expose: true,
            configured: a.is_configured(config),
        })
        .collect()
}

pub fn builtin_adapters() -> Vec<AdapterInfo> {
    all_adapters()
        .iter()
        .map(|a| AdapterInfo {
            id: a.id(),
            display_name: a.display_name(),
            class: a.class(),
            maturity: a.maturity(),
            formats: a.supported_formats(),
            can_expose: true,
            configured: true,
        })
        .collect()
}

pub struct AdapterInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub class: AdapterClass,
    pub maturity: AdapterMaturity,
    pub formats: &'static [FormatKind],
    pub can_expose: bool,
    pub configured: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FormatKind;

    #[test]
    fn all_adapters_have_unique_ids() {
        let adapters = all_adapters();
        let mut ids: Vec<&str> = adapters.iter().map(|a| a.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate adapter IDs found");
    }

    #[test]
    fn find_adapter_returns_known_tools() {
        assert!(find_adapter("lmstudio").is_ok());
        assert!(find_adapter("ollama").is_ok());
        assert!(find_adapter("llama-cpp").is_ok());
        assert!(find_adapter("nonexistent").is_err());
    }

    #[test]
    fn mlx_cannot_be_exposed_to_ollama() {
        assert_eq!(
            compatibility_reason("ollama", &FormatKind::Mlx),
            Some("unsupported format for this tool")
        );
    }

    #[test]
    fn gguf_is_compatible_with_llama_cpp() {
        assert_eq!(compatibility_reason("llama-cpp", &FormatKind::Gguf), None);
    }

    #[test]
    fn compatibility_safetensors_to_transformers_accepted() {
        assert!(compatibility_reason("transformers", &FormatKind::Safetensors).is_none());
    }

    #[test]
    fn compatibility_gguf_to_mlx_lm_rejected() {
        assert!(compatibility_reason("mlx-lm", &FormatKind::Gguf).is_some());
    }
}
