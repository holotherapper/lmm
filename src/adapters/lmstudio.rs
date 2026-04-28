//! LM Studio adapter — symlink tree into the LM Studio models directory.
use crate::error::{AppError, Result};
use crate::model::{ExposureEntry, FormatKind};
use crate::paths::expand_tilde;
use crate::security::sanitize_alias;

use super::{
    AdapterClass, CopyBehavior, ExposeRequest, ToolAdapter, expose_symlink_tree,
    remove_directory_exposure,
};

const FORMATS: &[FormatKind] = &[FormatKind::Mlx, FormatKind::Gguf, FormatKind::Safetensors];

pub struct LmStudio;

impl ToolAdapter for LmStudio {
    fn id(&self) -> &'static str {
        "lmstudio"
    }
    fn display_name(&self) -> &'static str {
        "LM Studio"
    }
    fn class(&self) -> AdapterClass {
        AdapterClass::FilesystemExposure
    }
    fn supported_formats(&self) -> &'static [FormatKind] {
        FORMATS
    }
    fn creates_physical_copy(&self) -> CopyBehavior {
        CopyBehavior::VerifiedDedupe
    }
    fn is_configured(&self, config: &crate::config::Config) -> bool {
        config.paths.lmstudio.is_some()
    }

    fn expose(&self, req: &ExposeRequest<'_>) -> Result<ExposureEntry> {
        let Some(root_str) = req.config.paths.lmstudio.as_deref() else {
            return Err(AppError::InvalidInput(
                "LM Studio path is not configured".to_string(),
            ));
        };
        let root = expand_tilde(root_str);
        let target = lmstudio_target(&root, req.repo, req.alias);
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
        let Some(root_str) = config.paths.lmstudio.as_deref() else {
            return Err(AppError::InvalidInput(
                "LM Studio path is not configured".to_string(),
            ));
        };
        remove_directory_exposure(entry, &expand_tilde(root_str))
    }
}

fn lmstudio_target(root: &std::path::Path, repo: &str, alias: &str) -> std::path::PathBuf {
    let mut parts = repo.split('/');
    let org = parts
        .next()
        .map(sanitize_alias)
        .unwrap_or_else(|| "unknown".to_string());
    root.join(org).join(sanitize_alias(alias))
}
