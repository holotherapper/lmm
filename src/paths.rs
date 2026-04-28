//! Application directory resolution (state dir, HF cache, tool paths).
use std::env;
use std::path::PathBuf;

use directories::ProjectDirs;

use crate::config::Config;
use crate::error::{AppError, Result};

const STATE_ENV: &str = "LMM_STATE_DIR";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub state_dir: PathBuf,
    pub config_path: PathBuf,
    pub lock_path: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<Self> {
        let state_dir = state_dir()?;
        Ok(Self {
            config_path: state_dir.join("config.json"),
            lock_path: state_dir.join("lock.json"),
            state_dir,
        })
    }
}

pub fn state_dir() -> Result<PathBuf> {
    if let Some(value) = env::var_os(STATE_ENV) {
        return Ok(PathBuf::from(value));
    }

    let Some(project_dirs) = ProjectDirs::from("dev", "local", "lmm") else {
        return Err(AppError::StateDirectory);
    };

    Ok(project_dirs.data_dir().to_path_buf())
}

pub fn hf_cache_dir() -> PathBuf {
    if let Some(value) = env::var_os("HUGGINGFACE_HUB_CACHE") {
        return PathBuf::from(value);
    }

    if let Some(value) = env::var_os("HF_HOME") {
        return PathBuf::from(value).join("hub");
    }

    if let Some(value) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(value).join("huggingface").join("hub");
    }

    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return PathBuf::from("/tmp/huggingface/hub");
    };
    home.join(".cache").join("huggingface").join("hub")
}

pub fn configured_hf_cache_dir(config: &Config) -> PathBuf {
    config
        .paths
        .hf_cache
        .as_deref()
        .filter(|v| !v.is_empty() && *v != "auto")
        .map(expand_tilde)
        .unwrap_or_else(hf_cache_dir)
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }

    if let Some(stripped) = path.strip_prefix("~/") {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(stripped))
            .unwrap_or_else(|| PathBuf::from(path));
    }

    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{configured_hf_cache_dir, expand_tilde};
    use crate::config::Config;

    #[test]
    fn expand_tilde_leaves_non_tilde_paths_unchanged() {
        let path = expand_tilde("/tmp/hf-cache");
        assert_eq!(path, Path::new("/tmp/hf-cache"));
    }

    #[test]
    fn expand_tilde_home() {
        let expanded = expand_tilde("~/test");
        assert!(!expanded.to_string_lossy().contains('~'));
    }

    #[test]
    fn configured_hf_cache_auto_falls_back() {
        let mut config = Config::default();
        config.paths.hf_cache = Some("auto".to_string());
        let path = configured_hf_cache_dir(&config);
        assert!(!path.to_string_lossy().contains("auto"));
    }

    #[test]
    fn configured_hf_cache_empty_falls_back() {
        let mut config = Config::default();
        config.paths.hf_cache = Some("".to_string());
        let path = configured_hf_cache_dir(&config);
        assert!(
            path.to_string_lossy().contains("huggingface")
                || path.to_string_lossy().contains("hub")
        );
    }
}
