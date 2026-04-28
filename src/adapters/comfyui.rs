//! ComfyUI adapter — symlink model files into the ComfyUI models directory.
use std::path::PathBuf;

use crate::config::Config;
use crate::error::{AppError, Result};

use crate::model::{ExposureEntry, FormatKind};
use crate::paths::expand_tilde;

use super::{
    AdapterClass, AdapterMaturity, CopyBehavior, ExposeRequest, ToolAdapter, expose_symlink_tree,
    remove_directory_exposure,
};

const FORMATS: &[FormatKind] = &[FormatKind::Safetensors, FormatKind::Mlx, FormatKind::Gguf];

pub struct ComfyUi;

impl ToolAdapter for ComfyUi {
    fn id(&self) -> &'static str {
        "comfyui"
    }
    fn display_name(&self) -> &'static str {
        "ComfyUI"
    }
    fn class(&self) -> AdapterClass {
        AdapterClass::FilesystemExposure
    }
    fn supported_formats(&self) -> &'static [FormatKind] {
        FORMATS
    }
    fn maturity(&self) -> AdapterMaturity {
        AdapterMaturity::Experimental
    }
    fn creates_physical_copy(&self) -> CopyBehavior {
        CopyBehavior::VerifiedDedupe
    }
    fn is_configured(&self, config: &crate::config::Config) -> bool {
        config.paths.comfyui.is_some()
    }

    fn expose(&self, req: &ExposeRequest<'_>) -> Result<ExposureEntry> {
        let root = comfyui_models_dir(req.config)?;
        let subdir = model_subdir(req);
        let target = root.join(subdir).join(req.alias);
        expose_symlink_tree(
            &root,
            &target,
            req.snapshot_root,
            req.files,
            req.replace,
            "symlink",
        )
    }

    fn remove_exposure(&self, entry: &ExposureEntry, config: &crate::config::Config) -> Result<()> {
        let root = comfyui_models_dir(config)?;
        remove_directory_exposure(entry, &root)
    }
}

fn comfyui_models_dir(config: &Config) -> Result<PathBuf> {
    if let Some(path) = &config.paths.comfyui {
        return Ok(expand_tilde(path));
    }
    Err(AppError::InvalidInput(
        "ComfyUI models path not configured. Set it with: lmm config set paths.comfyui /path/to/ComfyUI/models".to_string()
    ))
}

fn model_subdir(req: &ExposeRequest<'_>) -> &'static str {
    let has_lora = req.files.iter().any(|f| is_lora_file(&f.path));
    if has_lora {
        return "loras";
    }
    let has_vae = req
        .files
        .iter()
        .any(|f| f.path.contains("vae") || f.path.contains("VAE"));
    if has_vae {
        return "vae";
    }
    "checkpoints"
}

fn is_lora_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("lora") || lower.contains("loha") || lower.contains("lokr")
}
