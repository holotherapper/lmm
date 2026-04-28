//! Structured error types for all fallible operations.
use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("failed to read `{path}`")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write `{path}`")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to create directory `{path}`")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to rename `{from}` to `{to}`")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse JSON `{path}`")]
    ParseJson {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("failed to encode JSON")]
    EncodeJson { source: serde_json::Error },
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("failed to resolve application state directory")]
    StateDirectory,
    #[error("state is locked by another process: {0}")]
    LockBusy(PathBuf),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("unknown adapter `{0}`")]
    UnknownAdapter(String),
    #[error("unsupported format `{format}` for tool `{tool}`: {reason}")]
    UnsupportedToolFormat {
        tool: String,
        format: String,
        reason: &'static str,
    },
    #[error("unknown config key `{0}`")]
    UnknownConfigKey(String),
    #[error("cancelled")]
    Cancelled,
}
