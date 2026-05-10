//! User configuration: tool paths, defaults, network, and UI settings.
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub paths: PathsConfig,
    pub defaults: DefaultsConfig,
    pub network: NetworkConfig,
    pub tools: ToolsConfig,
    pub ui: UiConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PathsConfig {
    pub hf_cache: Option<String>,
    pub lmstudio: Option<String>,
    pub ollama: Option<String>,
    pub jan: Option<String>,
    pub comfyui: Option<String>,
    pub a1111: Option<String>,
    pub invokeai: Option<String>,
    pub fooocus: Option<String>,
    pub text_gen_webui: Option<String>,
    pub gpt4all: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DefaultsConfig {
    pub exposure_strategy: String,
    pub default_tools: Vec<String>,
    pub format: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkConfig {
    pub hf_endpoint: String,
    pub hf_token_source: String,
    pub download_concurrency: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolsConfig {
    pub llama_cpp: LlamaCppConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LlamaCppConfig {
    pub llama_cli: String,
    pub llama_server: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UiConfig {
    pub color: String,
    pub confirm_destructive: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            paths: PathsConfig {
                hf_cache: None,
                lmstudio: Some("~/.lmstudio/models".to_string()),
                ollama: Some("~/.ollama/models".to_string()),
                jan: Some(default_jan_path()),
                comfyui: None,
                a1111: None,
                invokeai: None,
                fooocus: None,
                text_gen_webui: None,
                gpt4all: Some(default_gpt4all_path()),
            },
            defaults: DefaultsConfig {
                exposure_strategy: "auto".to_string(),
                default_tools: Vec::new(),
                format: "auto".to_string(),
            },
            network: NetworkConfig {
                hf_endpoint: "https://huggingface.co".to_string(),
                hf_token_source: "auto".to_string(),
                download_concurrency: 4,
            },
            tools: ToolsConfig {
                llama_cpp: LlamaCppConfig {
                    llama_cli: "llama-cli".to_string(),
                    llama_server: "llama-server".to_string(),
                },
            },
            ui: UiConfig {
                color: "auto".to_string(),
                confirm_destructive: true,
            },
        }
    }
}

fn default_jan_path() -> String {
    if cfg!(target_os = "macos") {
        "~/Library/Application Support/Jan/data/llamacpp/models".to_string()
    } else {
        "~/.config/Jan/data/models".to_string()
    }
}

fn default_gpt4all_path() -> String {
    if cfg!(target_os = "macos") {
        "~/Library/Application Support/nomic.ai/GPT4All".to_string()
    } else {
        "~/.local/share/nomic.ai/GPT4All".to_string()
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let bytes = fs::read(path).map_err(|source| AppError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|source| AppError::ParseJson {
                path: path.to_path_buf(),
                source,
            })?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        let format = self.defaults.format.as_str();
        if !matches!(format, "auto" | "" | "mlx" | "gguf" | "safetensors") {
            return Err(AppError::InvalidInput(format!(
                "invalid defaults.format in config: \"{format}\" (expected auto, mlx, gguf, or safetensors)"
            )));
        }
        let color = self.ui.color.as_str();
        if !matches!(color, "auto" | "always" | "never") {
            return Err(AppError::InvalidInput(format!(
                "invalid ui.color in config: \"{color}\" (expected auto, always, or never)"
            )));
        }
        if self.network.download_concurrency == 0 {
            return Err(AppError::InvalidInput(
                "network.download_concurrency must be at least 1".to_string(),
            ));
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let bytes =
            serde_json::to_vec_pretty(self).map_err(|source| AppError::EncodeJson { source })?;
        let tmp = atomic_tmp_path(path)?;
        fs::write(&tmp, bytes).map_err(|source| AppError::Write {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, path).map_err(|source| AppError::Rename {
            from: tmp,
            to: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

pub fn atomic_tmp_path(target: &Path) -> crate::error::Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let parent = target.parent().unwrap_or(Path::new("."));
    let prefix = target.file_name().and_then(|n| n.to_str()).unwrap_or("tmp");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(".{prefix}.{}.{ts}.tmp", std::process::id()));
    let _file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|source| crate::error::AppError::Write {
            path: tmp_path.clone(),
            source,
        })?;
    Ok(tmp_path)
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn config_default_has_expected_structure() {
        let config = Config::default();
        assert_eq!(config.version, 1);
        assert!(config.paths.lmstudio.is_some());
        assert_eq!(config.defaults.format, "auto");
        assert_eq!(config.ui.color, "auto");
    }

    #[test]
    fn config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = Config::default();
        config.defaults.format = "mlx".to_string();
        config.save(&path).unwrap();

        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.defaults.format, "mlx");
    }

    #[test]
    fn config_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("nope.json")).unwrap();
        assert_eq!(config.version, 1);
    }
}
