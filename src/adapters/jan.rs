//! Jan adapter — create model.yml pointing to HF Cache GGUF.
//! Jan requires a model.yml metadata file to recognize models.
//! The GGUF file itself stays in HF Cache; model.yml references it by path.
use std::fs;
use std::path::PathBuf;

use crate::error::{AppError, Result};
use crate::model::{ExposureEntry, ExposureStatus, FormatKind};
use crate::paths::expand_tilde;
use crate::security::{safe_join, sanitize_alias};

use super::{AdapterClass, AdapterMaturity, CopyBehavior, ExposeRequest, ToolAdapter};

const FORMATS: &[FormatKind] = &[FormatKind::Gguf];

pub struct Jan;

impl ToolAdapter for Jan {
    fn id(&self) -> &'static str {
        "jan"
    }
    fn display_name(&self) -> &'static str {
        "Jan"
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
        config.paths.jan.is_some()
    }

    fn expose(&self, req: &ExposeRequest<'_>) -> Result<ExposureEntry> {
        let root = jan_models_dir(req.config)?;
        let alias = sanitize_alias(req.alias);
        let model_dir = root.join(&alias);

        if model_dir.exists() && !req.replace {
            return Err(AppError::InvalidInput(format!(
                "Jan model directory already exists: {}",
                model_dir.display()
            )));
        }

        let gguf = req
            .files
            .iter()
            .find(|f| f.path.ends_with(".gguf"))
            .ok_or_else(|| AppError::InvalidInput("Jan requires a GGUF file".to_string()))?;

        let gguf_path = safe_join(req.snapshot_root, &gguf.path)?;

        fs::create_dir_all(&model_dir).map_err(|source| AppError::CreateDir {
            path: model_dir.clone(),
            source,
        })?;

        crate::security::validate_path_no_control_chars(&gguf_path)?;
        let safe_alias = yaml_escape(req.alias);
        let safe_path = yaml_escape(&gguf_path.display().to_string());
        let model_yml = format!(
            "model_path: \"{safe_path}\"\nname: \"{safe_alias}\"\nsize_bytes: {}\nembedding: false\n",
            gguf.size_bytes
        );
        let yml_path = model_dir.join("model.yml");
        fs::write(&yml_path, model_yml.as_bytes()).map_err(|source| AppError::Write {
            path: yml_path,
            source,
        })?;

        Ok(ExposureEntry {
            status: ExposureStatus::Ok,
            strategy: "jan-model-yml".to_string(),
            path: Some(model_dir),
            created_by: Some("lmm".to_string()),
        })
    }

    fn remove_exposure(&self, entry: &ExposureEntry, config: &crate::config::Config) -> Result<()> {
        let root = jan_models_dir(config)?;
        if let Some(path) = &entry.path
            && path.exists()
        {
            crate::security::ensure_child_path(&root, path)?;
            if let (Ok(canonical_root), Ok(canonical_path)) =
                (fs::canonicalize(&root), fs::canonicalize(path))
            {
                crate::security::ensure_child_path(&canonical_root, &canonical_path)?;
            }
            fs::remove_dir_all(path).map_err(|source| AppError::Write {
                path: path.clone(),
                source,
            })?;
        }
        Ok(())
    }

    fn incompatibility_reason(&self, format: &FormatKind) -> Option<&'static str> {
        if self.supported_formats().contains(format) {
            None
        } else {
            Some("GGUF required")
        }
    }
}

fn yaml_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn jan_models_dir(config: &crate::config::Config) -> Result<PathBuf> {
    let root = config
        .paths
        .jan
        .as_deref()
        .map(expand_tilde)
        .ok_or_else(|| AppError::InvalidInput("Jan path is not configured".to_string()))?;
    Ok(root)
}
