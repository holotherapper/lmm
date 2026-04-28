//! Ollama adapter — display only. Ollama copies models to its blob store,
//! doubling disk usage, so lmm does not expose models to Ollama.
//! Models installed via `ollama pull` are shown in `lmm list` through
//! manifest scanning in commands/mod.rs.
use crate::error::{AppError, Result};
use crate::model::{ExposureEntry, FormatKind};

use super::{AdapterClass, CopyBehavior, ExposeRequest, ToolAdapter};

const FORMATS: &[FormatKind] = &[FormatKind::Gguf];

pub struct Ollama;

impl ToolAdapter for Ollama {
    fn id(&self) -> &'static str {
        "ollama"
    }
    fn display_name(&self) -> &'static str {
        "Ollama"
    }
    fn class(&self) -> AdapterClass {
        AdapterClass::ImportingRegistry
    }
    fn supported_formats(&self) -> &'static [FormatKind] {
        FORMATS
    }
    fn creates_physical_copy(&self) -> CopyBehavior {
        CopyBehavior::UnknownCopy
    }

    fn expose(&self, _req: &ExposeRequest<'_>) -> Result<ExposureEntry> {
        Err(AppError::InvalidInput(
            "Ollama copies models to its blob store, doubling disk usage.\n  \
             Use \"ollama pull <model>\" directly instead.\n  \
             lmm shows Ollama models in \"lmm list\" for visibility."
                .to_string(),
        ))
    }

    fn remove_exposure(
        &self,
        _entry: &ExposureEntry,
        _config: &crate::config::Config,
    ) -> Result<()> {
        Ok(())
    }
}
