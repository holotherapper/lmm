//! `lmm config` — read and write configuration values.
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::lock::StateLock;
use crate::paths::AppPaths;

use super::{FormatSelection, parse_format_id, validate_tools};

pub fn config_path(paths: &AppPaths) {
    println!("{}", paths.config_path.display());
}

pub fn config_get(paths: &AppPaths, key: &str) -> Result<()> {
    let config = Config::load(&paths.config_path)?;
    let value = read_config_key(&config, key)?;
    println!("{value}");
    Ok(())
}

pub fn config_set(paths: &AppPaths, key: &str, value: &str) -> Result<()> {
    let _state_lock = StateLock::acquire(&paths.lock_path)?;
    let mut config = Config::load(&paths.config_path)?;
    write_config_key(&mut config, key, value)?;
    config.save(&paths.config_path)?;
    format::status("✓", &format!("{key} = {value}"));
    Ok(())
}

const CONFIG_KEYS: &[&str] = &[
    "paths.hf_cache",
    "paths.lmstudio",
    "paths.ollama",
    "paths.jan",
    "paths.comfyui",
    "paths.a1111",
    "paths.invokeai",
    "paths.fooocus",
    "paths.text_gen_webui",
    "paths.gpt4all",
    "defaults.default_tools",
    "defaults.format",
    "network.hf_endpoint",
    "tools.llama_cpp.llama_cli",
    "tools.llama_cpp.llama_server",
    "ui.color",
];

fn read_config_key(config: &Config, key: &str) -> Result<String> {
    let value = match key {
        "paths.hf_cache" => config
            .paths
            .hf_cache
            .as_deref()
            .unwrap_or("auto")
            .to_string(),
        "paths.lmstudio" => config
            .paths
            .lmstudio
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "paths.ollama" => config
            .paths
            .ollama
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "paths.jan" => config.paths.jan.as_deref().unwrap_or("unset").to_string(),
        "paths.comfyui" => config
            .paths
            .comfyui
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "paths.a1111" => config.paths.a1111.as_deref().unwrap_or("unset").to_string(),
        "paths.invokeai" => config
            .paths
            .invokeai
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "paths.fooocus" => config
            .paths
            .fooocus
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "paths.text_gen_webui" => config
            .paths
            .text_gen_webui
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "paths.gpt4all" => config
            .paths
            .gpt4all
            .as_deref()
            .unwrap_or("unset")
            .to_string(),
        "defaults.default_tools" => config.defaults.default_tools.join(","),
        "defaults.format" => config.defaults.format.clone(),
        "network.hf_endpoint" => config.network.hf_endpoint.clone(),
        "tools.llama_cpp.llama_cli" => config.tools.llama_cpp.llama_cli.clone(),
        "tools.llama_cpp.llama_server" => config.tools.llama_cpp.llama_server.clone(),
        "ui.color" => config.ui.color.clone(),
        _ => {
            return Err(AppError::UnknownConfigKey(format!(
                "{key} (valid keys: {})",
                CONFIG_KEYS.join(", ")
            )));
        }
    };
    Ok(value)
}

fn write_config_key(config: &mut Config, key: &str, value: &str) -> Result<()> {
    match key {
        "paths.hf_cache" => config.paths.hf_cache = Some(value.to_string()),
        "paths.lmstudio" => config.paths.lmstudio = Some(value.to_string()),
        "paths.ollama" => config.paths.ollama = Some(value.to_string()),
        "paths.jan" => config.paths.jan = Some(value.to_string()),
        "paths.comfyui" => config.paths.comfyui = Some(value.to_string()),
        "paths.a1111" => config.paths.a1111 = Some(value.to_string()),
        "paths.invokeai" => config.paths.invokeai = Some(value.to_string()),
        "paths.fooocus" => config.paths.fooocus = Some(value.to_string()),
        "paths.text_gen_webui" => config.paths.text_gen_webui = Some(value.to_string()),
        "paths.gpt4all" => config.paths.gpt4all = Some(value.to_string()),
        "defaults.default_tools" => {
            config.defaults.default_tools = value
                .split(',')
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(ToString::to_string)
                .collect();
            validate_tools(&config.defaults.default_tools, FormatSelection::Auto)?;
        }
        "defaults.format" => {
            if value != "auto" {
                parse_format_id(value)?;
            }
            config.defaults.format = value.to_string();
        }
        "network.hf_endpoint" => {
            if !value.starts_with("http://") && !value.starts_with("https://") {
                return Err(AppError::InvalidInput(
                    "network.hf_endpoint must start with http:// or https://".to_string(),
                ));
            }
            config.network.hf_endpoint = value.to_string();
        }
        "tools.llama_cpp.llama_cli" => config.tools.llama_cpp.llama_cli = value.to_string(),
        "tools.llama_cpp.llama_server" => config.tools.llama_cpp.llama_server = value.to_string(),
        "ui.color" => {
            if !matches!(value, "auto" | "always" | "never") {
                return Err(AppError::InvalidInput(
                    "ui.color must be auto, always, or never".to_string(),
                ));
            }
            config.ui.color = value.to_string();
        }
        _ => {
            return Err(AppError::UnknownConfigKey(format!(
                "{key} (valid keys: {})",
                CONFIG_KEYS.join(", ")
            )));
        }
    }
    Ok(())
}
