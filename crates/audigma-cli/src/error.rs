//! CLI error domain types and conversions (`Design.md §7`, `UseTypedErrors.md §Phase A`).

use std::path::PathBuf;
use thiserror::Error;

/// High-level errors arising from CLI argument parsing, configuration loading,
/// IPC operations, and runtime execution.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("configuration file not found: {path}")]
    ConfigNotFound { path: PathBuf },

    #[error("configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Override(String),

    #[error("volume error: {0}")]
    Volume(String),

    #[error("unknown preset '{name}'")]
    PresetNotFound { name: String },

    #[error("unknown audio backend '{backend}'; supported backends: alsa, pipewire")]
    UnknownBackend { backend: String },

    #[error("IPC communication error: {0}")]
    Ipc(#[from] crate::ipc::IpcError),

    #[error("daemon error: {0}")]
    Daemon(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error(transparent)]
    Format(#[from] audigma_format::FormatError),

    #[error(transparent)]
    Engine(#[from] audigma_engine::traits::EngineError),

    #[error(transparent)]
    Settings(#[from] audigma_settings::SettingsError),

    #[error("{0}")]
    Other(String),
}
