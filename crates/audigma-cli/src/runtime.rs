//! `[runtime]` wiring: the implementation lives in `audigma_io::runtime`; this wrapper maps the
//! placement error into [`CliError`].

use audigma_engine::Placement;
use audigma_settings::Settings;

use crate::error::CliError;

pub use audigma_io::runtime::{
    apply_housekeeping, apply_logged, rt_failure_is_fatal, worker_setup,
};

/// See [`audigma_io::runtime::resolve`].
pub fn resolve(settings: &Settings, channels: usize, offline: bool) -> Result<Placement, CliError> {
    audigma_io::runtime::resolve(settings, channels, offline).map_err(CliError::Config)
}
