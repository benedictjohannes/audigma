//! System tray icon (`[runtime] systray`, `Design.md` §6.1.1).
//!
//! `model` is platform-neutral (state -> status line and menu); `linux` renders it as a
//! StatusNotifierItem. Each daemon instance owns its tray.

pub mod model;

#[cfg(target_os = "linux")]
mod linux;

use std::time::Duration;

use audigma_engine::DaemonControlHandle;
use audigma_settings::SystrayMode;

use crate::error::CliError;
use crate::ipc::Instance;

pub use model::{MenuEntry, TrayAction, TrayModel};

/// How long a tray-triggered pause/resume waits for the supervisor (matches the IPC timeout).
const ACTION_TIMEOUT: Duration = Duration::from_secs(15);

/// Minimum spacing between tray refreshes.
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Runs a menu action against the daemon. Called on a worker thread, never on the tray's own.
pub(crate) fn perform(control: &DaemonControlHandle, action: TrayAction) {
    let result = match action {
        TrayAction::Pause => control.pause_and_wait(ACTION_TIMEOUT),
        TrayAction::Resume => control.resume_and_wait(ACTION_TIMEOUT),
        TrayAction::Stop => {
            control.stop();
            Ok(())
        }
    };
    if let Err(e) = result {
        tracing::warn!(
            target: "audigma::daemon",
            event = "tray_action_failed",
            action = ?action,
            error = %e,
            "tray action failed"
        );
    }
}

/// Keeps the tray alive; dropping it removes the icon.
pub struct TrayGuard {
    #[cfg(target_os = "linux")]
    _inner: linux::LinuxTray,
}

/// Starts the tray according to `mode`.
///
/// - `off`: `Ok(None)`.
/// - `auto`: a missing tray host is not an error; the icon appears once a host does. A failure to
///   reach the session bus at all logs a warning and returns `Ok(None)`.
/// - `required`: any failure to register the icon is an error.
pub fn start(
    mode: SystrayMode,
    instance: &Instance,
    control: &DaemonControlHandle,
) -> Result<Option<TrayGuard>, CliError> {
    if mode == SystrayMode::Off {
        return Ok(None);
    }
    start_platform(mode, instance, control)
}

#[cfg(target_os = "linux")]
fn start_platform(
    mode: SystrayMode,
    instance: &Instance,
    control: &DaemonControlHandle,
) -> Result<Option<TrayGuard>, CliError> {
    match linux::LinuxTray::start(mode, instance.name(), control) {
        Ok(inner) => {
            tracing::info!(
                target: "audigma::daemon",
                event = "tray_started",
                instance = instance.name(),
                host_available = inner.host_available(),
                "system tray service started"
            );
            Ok(Some(TrayGuard { _inner: inner }))
        }
        Err(e) if mode == SystrayMode::Required => Err(CliError::Systray(e)),
        Err(e) => {
            tracing::warn!(
                target: "audigma::daemon",
                event = "config_warning",
                error = %e,
                "[runtime] systray = \"auto\": no system tray, continuing without it"
            );
            Ok(None)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn start_platform(
    mode: SystrayMode,
    _instance: &Instance,
    _control: &DaemonControlHandle,
) -> Result<Option<TrayGuard>, CliError> {
    if mode == SystrayMode::Required {
        return Err(CliError::Systray(
            "system tray is not supported on this platform".to_string(),
        ));
    }
    tracing::warn!(
        target: "audigma::daemon",
        event = "config_warning",
        "[runtime] systray is not supported on this platform; continuing without it"
    );
    Ok(None)
}
