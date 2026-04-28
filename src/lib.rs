//! lmm — Local LLM model manager for Apple Silicon workflows.
//!
//! Treats HF Cache as the canonical model store, then exposes artifacts
//! to local tools (LM Studio, mlx-lm, llama.cpp, Ollama, etc.) via
//! symlinks and tool-specific adapters.
pub mod adapters;
pub mod artifacts;
mod cli;
pub(crate) mod commands;
pub mod config;
pub mod error;
pub mod format;
pub mod hf;
pub mod lock;
pub mod model;
pub mod paths;
pub mod security;
pub(crate) mod tui;

pub use error::{AppError, Result};

pub fn run() -> Result<()> {
    cli::run()
}
