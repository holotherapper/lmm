//! llama.cpp adapter — canonical GGUF path consumer.
use crate::error::{AppError, Result};
use crate::model::{ExposureEntry, ExposureStatus, FormatKind};
use crate::security::safe_join;

use super::{AdapterClass, ExposeRequest, ToolAdapter};

const FORMATS: &[FormatKind] = &[FormatKind::Gguf];

pub struct LlamaCpp;

impl ToolAdapter for LlamaCpp {
    fn id(&self) -> &'static str {
        "llama-cpp"
    }
    fn display_name(&self) -> &'static str {
        "llama.cpp"
    }
    fn class(&self) -> AdapterClass {
        AdapterClass::PathConsumer
    }
    fn supported_formats(&self) -> &'static [FormatKind] {
        FORMATS
    }

    fn expose(&self, req: &ExposeRequest<'_>) -> Result<ExposureEntry> {
        let gguf = req
            .files
            .iter()
            .find(|f| f.path.ends_with(".gguf"))
            .ok_or_else(|| AppError::InvalidInput("llama.cpp requires a GGUF file".to_string()))?;
        Ok(ExposureEntry {
            status: ExposureStatus::Ok,
            strategy: "path-consumer".to_string(),
            path: Some(safe_join(req.snapshot_root, &gguf.path)?),
            created_by: Some("lmm".to_string()),
        })
    }

    fn remove_exposure(
        &self,
        _entry: &ExposureEntry,
        _config: &crate::config::Config,
    ) -> Result<()> {
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
