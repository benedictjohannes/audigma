//! Configuration types for logging and filtering.

use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::str::FromStr;
use thiserror::Error;

/// Output renderer format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
    Auto,
}

impl FromStr for LogFormat {
    type Err = LogConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "auto" => Ok(Self::Auto),
            other => Err(LogConfigError::InvalidFormat(other.to_string())),
        }
    }
}

impl LogFormat {
    /// Resolves `Auto` to `Text` if stderr is an interactive terminal, else `Json`.
    pub fn resolve_effective(self) -> Self {
        match self {
            Self::Auto => {
                if std::io::stderr().is_terminal() {
                    Self::Text
                } else {
                    Self::Json
                }
            }
            fixed => fixed,
        }
    }
}

/// Log severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    #[default]
    Warn,
    Info,
    Debug,
    Trace,
}

impl FromStr for LogLevel {
    type Err = LogConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            other => Err(LogConfigError::InvalidLevel(other.to_string())),
        }
    }
}

impl LogLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }
}

/// Errors occurring during logging configuration or initialization.
#[derive(Debug, Error)]
pub enum LogConfigError {
    #[error("invalid log format '{0}': expected 'text', 'json', or 'auto'")]
    InvalidFormat(String),

    #[error("invalid log level '{0}': expected 'error', 'warn', 'info', 'debug', or 'trace'")]
    InvalidLevel(String),

    #[error("failed to parse log filter directive: {0}")]
    FilterParse(String),

    #[error("failed to set global default tracing subscriber: {0}")]
    SetGlobalDefault(String),
}

/// Resolved runtime logging configuration.
#[derive(Debug, Clone)]
pub struct LogConfig {
    pub format: LogFormat,
    pub filter: String,
}

impl LogConfig {
    /// Resolves final logging configuration from CLI, environment, settings, and defaults.
    ///
    /// Precedence (§7.4):
    /// CLI flag > environment variable > Settings > command default.
    pub fn resolve(
        cli_format: Option<LogFormat>,
        cli_level: Option<LogLevel>,
        settings_format: Option<&str>,
        settings_level: Option<&str>,
        default_level: LogLevel,
    ) -> Self {
        // 1. Resolve format
        let format_choice = if let Some(fmt) = cli_format {
            fmt
        } else if let Ok(env_fmt) = std::env::var("AUDIGMA_LOG_FORMAT") {
            LogFormat::from_str(&env_fmt).unwrap_or(LogFormat::Text)
        } else if let Some(sf) = settings_format {
            LogFormat::from_str(sf).unwrap_or(LogFormat::Text)
        } else {
            LogFormat::Text
        };
        let effective_format = format_choice.resolve_effective();

        // 2. Resolve level / filter directive
        let filter = if let Some(lvl) = cli_level {
            lvl.as_str().to_string()
        } else if let Ok(env_filter) = std::env::var("AUDIGMA_LOG") {
            env_filter
        } else if let Ok(env_filter) = std::env::var("RUST_LOG") {
            env_filter
        } else if let Some(sl) = settings_level {
            sl.to_string()
        } else {
            default_level.as_str().to_string()
        };

        Self {
            format: effective_format,
            filter,
        }
    }
}
