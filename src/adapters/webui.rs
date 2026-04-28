//! Adapters for WebUI-style tools that store models in their own directories.
//! All use symlink trees from HF Cache.
use std::path::PathBuf;

use crate::error::{AppError, Result};
use crate::model::{ExposureEntry, FormatKind};
use crate::paths::expand_tilde;

use super::{
    AdapterClass, AdapterMaturity, CopyBehavior, ExposeRequest, ToolAdapter, expose_symlink_tree,
    remove_directory_exposure,
};

macro_rules! webui_adapter {
    (
        $struct_name:ident,
        $id:literal,
        $display:literal,
        $formats:expr,
        $config_field:ident,
        $config_hint:literal
    ) => {
        pub struct $struct_name;

        impl ToolAdapter for $struct_name {
            fn id(&self) -> &'static str {
                $id
            }
            fn display_name(&self) -> &'static str {
                $display
            }
            fn class(&self) -> AdapterClass {
                AdapterClass::FilesystemExposure
            }
            fn supported_formats(&self) -> &'static [FormatKind] {
                $formats
            }
            fn maturity(&self) -> AdapterMaturity {
                AdapterMaturity::Experimental
            }
            fn creates_physical_copy(&self) -> CopyBehavior {
                CopyBehavior::VerifiedDedupe
            }
            fn is_configured(&self, config: &crate::config::Config) -> bool {
                config.paths.$config_field.is_some()
            }

            fn expose(&self, req: &ExposeRequest<'_>) -> Result<ExposureEntry> {
                let root = resolve_path(&req.config.paths.$config_field, $config_hint)?;
                let target = root.join(req.alias);
                expose_symlink_tree(
                    &root,
                    &target,
                    req.snapshot_root,
                    req.files,
                    req.replace,
                    "symlink",
                )
            }

            fn remove_exposure(
                &self,
                entry: &ExposureEntry,
                config: &crate::config::Config,
            ) -> Result<()> {
                let root = resolve_path(&config.paths.$config_field, $config_hint)?;
                remove_directory_exposure(entry, &root)
            }
        }
    };
}

fn resolve_path(configured: &Option<String>, hint: &str) -> Result<PathBuf> {
    match configured {
        Some(path) => Ok(expand_tilde(path)),
        None => Err(AppError::InvalidInput(format!(
            "path not configured. Set it with: lmm config set {hint}"
        ))),
    }
}

const SD_FORMATS: &[FormatKind] = &[FormatKind::Safetensors, FormatKind::Gguf];
const LLM_FORMATS: &[FormatKind] = &[FormatKind::Safetensors, FormatKind::Mlx, FormatKind::Gguf];

webui_adapter!(
    A1111,
    "a1111",
    "AUTOMATIC1111",
    SD_FORMATS,
    a1111,
    "paths.a1111 /path/to/stable-diffusion-webui/models/Stable-diffusion"
);
webui_adapter!(
    InvokeAi,
    "invokeai",
    "InvokeAI",
    SD_FORMATS,
    invokeai,
    "paths.invokeai ~/invokeai/models"
);
webui_adapter!(
    Fooocus,
    "fooocus",
    "Fooocus",
    SD_FORMATS,
    fooocus,
    "paths.fooocus /path/to/Fooocus/models/checkpoints"
);
webui_adapter!(
    TextGenWebui,
    "text-gen-webui",
    "text-generation-webui",
    LLM_FORMATS,
    text_gen_webui,
    "paths.text_gen_webui /path/to/text-generation-webui/user_data/models"
);
webui_adapter!(
    Gpt4All,
    "gpt4all",
    "GPT4All",
    &[FormatKind::Gguf],
    gpt4all,
    "paths.gpt4all ~/Library/Application\\ Support/nomic.ai/GPT4All"
);
