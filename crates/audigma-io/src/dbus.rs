//! D-Bus interfaces for real-time thread scheduling and ALSA device reservation (`Design.md §1`, `§3.4`).
//!
//! Provides:
//! - `request_realtime_priority`: Acquires real-time priority via `org.freedesktop.RealtimeKit1` over the system D-Bus
//!   with zero C-FFI and graceful fallback to normal priority.
//! - `DeviceReservation`: Implements `org.freedesktop.ReserveDevice1` over the session D-Bus to dynamically
//!   request sound servers (WirePlumber / PipeWire) yield the ALSA hardware DAC, and restores ownership
//!   when dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use zbus::connection::Builder;
use zbus::connection::Connection;
use zbus::interface;

/// Errors arising from D-Bus interactions.
#[derive(Debug, Error)]
pub enum DBusError {
    #[error("D-Bus connection error: {0}")]
    Connection(#[from] zbus::Error),

    #[error("Device reservation error for {device_name}: {reason}")]
    ReservationFailed { device_name: String, reason: String },

    #[error("RealtimeKit request rejected: {0}")]
    RealtimeKitFailed(String),
}

/// The mechanism through which real-time priority was acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimePriorityAcquisition {
    /// Acquired directly via POSIX `SCHED_FIFO` (`pthread_setschedparam`).
    DirectSchedFifo,
    /// Acquired via `org.freedesktop.RealtimeKit1` D-Bus IPC.
    RealtimeKit,
}

impl std::fmt::Display for RealtimePriorityAcquisition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectSchedFifo => write!(f, "POSIX SCHED_FIFO"),
            Self::RealtimeKit => write!(f, "RealtimeKit"),
        }
    }
}

/// Requests real-time priority for the calling thread (`Design.md §1`).
///
/// First attempts direct POSIX real-time scheduling (`SCHED_FIFO` via `pthread_setschedparam`),
/// which succeeds immediately with zero IPC overhead if the user session has real-time privileges (`RLIMIT_RTPRIO` / `ulimit -r`).
/// If unprivileged (`EPERM`), falls back to RealtimeKit (`org.freedesktop.RealtimeKit1`) over the system D-Bus.
///
/// If RTKit is unavailable, permissions are denied, or the system does not run RTKit,
/// returns a descriptive error allowing callers to log and fall back cleanly.
pub fn request_realtime_priority(priority: u32) -> Result<RealtimePriorityAcquisition, DBusError> {
    #[cfg(target_os = "linux")]
    {
        unsafe {
            let mut param: libc::sched_param = core::mem::zeroed();
            param.sched_priority = priority as i32;
            if libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) == 0 {
                return Ok(RealtimePriorityAcquisition::DirectSchedFifo);
            }
        }
    }

    #[cfg(target_os = "linux")]
    let thread_id = unsafe { libc::syscall(libc::SYS_gettid) as u64 };
    #[cfg(not(target_os = "linux"))]
    let thread_id = 0u64;

    zbus::block_on(async {
        let conn = Connection::system().await?;
        let pid = std::process::id() as u64;

        let reply: Result<(), zbus::Error> = conn
            .call_method(
                Some("org.freedesktop.RealtimeKit1"),
                "/org/freedesktop/RealtimeKit1",
                Some("org.freedesktop.RealtimeKit1"),
                "MakeThreadRealtimeWithPID",
                &(pid, thread_id, priority),
            )
            .await
            .map(|_| ());

        match reply {
            Ok(()) => Ok(RealtimePriorityAcquisition::RealtimeKit),
            Err(e) => Err(DBusError::RealtimeKitFailed(format!(
                "Failed to acquire RT priority {priority} for thread {thread_id}: {e}"
            ))),
        }
    })
}

/// D-Bus interface implementation for `org.freedesktop.ReserveDevice1`.
struct ReserveDeviceInterface {
    priority: i32,
    application_name: String,
    release_requested: Arc<AtomicBool>,
}

#[interface(name = "org.freedesktop.ReserveDevice1")]
impl ReserveDeviceInterface {
    /// Priority property of the reservation holder.
    #[zbus(property)]
    fn priority(&self) -> i32 {
        self.priority
    }

    /// Application name property.
    #[zbus(property)]
    fn application_name(&self) -> &str {
        &self.application_name
    }

    /// Application device name property.
    #[zbus(property)]
    fn application_device_name(&self) -> &str {
        &self.application_name
    }

    /// Called by another client requesting us to yield the device.
    fn request_release(&mut self, _priority: i32) -> bool {
        self.release_requested.store(true, Ordering::SeqCst);
        // By default for audigma during active exclusive streaming, do not yield to lower priority
        false
    }
}

/// Manages dynamic ALSA hardware device reservation via `org.freedesktop.ReserveDevice1`.
///
/// When instantiated, acquires `org.freedesktop.ReserveDevice1.Audio<card_index>` on the session bus.
/// WirePlumber monitors this well-known bus name and suspends its ALSA PCM monitor while held.
/// When dropped, the name is automatically released and WirePlumber restores desktop routing.
pub struct DeviceReservation {
    device_name: String,
    well_known_name: String,
    conn: Option<Connection>,
    release_requested: Arc<AtomicBool>,
}

impl DeviceReservation {
    /// Attempts to acquire device reservation for an ALSA card (e.g. `card_index = 0` -> `Audio0`).
    ///
    /// Priority is set high (e.g. `priority = 100`) so sound servers (PipeWire/WirePlumber, default ~0-50)
    /// yield exclusive ownership of the hardware.
    pub fn acquire(card_index: u32, app_name: &str, priority: i32) -> Result<Self, DBusError> {
        let device_name = format!("Audio{card_index}");
        let well_known_name = format!("org.freedesktop.ReserveDevice1.{device_name}");
        let object_path = format!("/org/freedesktop/ReserveDevice1/{device_name}");

        let release_requested = Arc::new(AtomicBool::new(false));
        let iface = ReserveDeviceInterface {
            priority,
            application_name: app_name.to_string(),
            release_requested: release_requested.clone(),
        };

        let conn = zbus::block_on(async {
            Builder::session()
                .name(well_known_name.as_str())
                .serve_at(object_path.as_str(), iface)
                .build()
                .await
        })?;

        Ok(Self {
            device_name,
            well_known_name,
            conn: Some(conn),
            release_requested,
        })
    }

    /// Returns whether another client on the bus has requested release of this device.
    #[inline]
    pub fn is_release_requested(&self) -> bool {
        self.release_requested.load(Ordering::SeqCst)
    }

    /// Returns the reserved device name (e.g. `Audio0`).
    #[inline]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Returns the well-known bus name held.
    #[inline]
    pub fn well_known_name(&self) -> &str {
        &self.well_known_name
    }
}

impl Drop for DeviceReservation {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Releasing the well-known name restores ownership to WirePlumber / sound servers
            let name = self.well_known_name.clone();
            zbus::block_on(async move {
                let _ = conn.release_name(name.as_str()).await;
            });
        }
    }
}
