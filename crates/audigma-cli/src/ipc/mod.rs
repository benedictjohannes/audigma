//! Daemon IPC protocol, instance identity and control server/client (`Design.md §7`).
//!
//! A daemon is addressed by an *instance name* (default `"default"`), never by a path.
//! Everything transport-specific (socket paths, `flock`, listener/stream types) lives in
//! [`platform`]; this module is transport-agnostic.
//!
//! Wire protocol: newline-delimited JSON.
//! - `GetStatus` -> `Telemetry`
//! - `SetVolume` / `SetVolumePercent` /  `SetMute` / `Stop` -> `Ok`
//!
//! The server runs on an independent I/O thread and talks to audio threads via atomics and mutexes.

use audigma_engine::DaemonControlHandle;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use thiserror::Error;

#[cfg(unix)]
mod platform;

pub use platform::InstanceLock;

/// Instance name used when none is given.
pub const DEFAULT_INSTANCE: &str = "default";

/// How long a `pause`/`resume` request blocks for the supervisor before reporting a timeout.
const PAUSE_RESUME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// IPC request commands sent over the Unix control socket.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload")]
pub enum IpcRequest {
    GetStatus,
    SetVolume {
        volume_db: f64,
    },
    SetVolumePercent {
        percent: f64,
    },
    SetMute {
        muted: bool,
    },
    /// Tear the pipeline down and release the device; the daemon stays up.
    Pause,
    /// Rebuild the pipeline from the (re-read) configuration.
    Resume,
    Stop,
}

/// IPC response sent back over the Unix control socket.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum IpcResponse {
    Ok,
    Telemetry(Box<audigma_engine::DaemonTelemetry>),
    Error(String),
}

/// Errors arising from daemon IPC operations.
#[derive(Debug, Error)]
pub enum IpcError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IPC server error: {0}")]
    Server(String),

    #[error("audigma instance '{0}' is not running")]
    DaemonNotRunning(String),

    #[error("audigma instance '{0}' is already running")]
    InstanceAlreadyRunning(String),

    #[error("invalid instance name '{0}': use 1-32 characters from [A-Za-z0-9_-]")]
    InvalidInstanceName(String),
}

/// Returns true for `[A-Za-z0-9_-]{1,32}`: valid in file names and pipe names alike, and short
/// enough for `sun_path` limits.
pub(crate) fn is_valid_instance_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Identity of a daemon instance. Opaque to callers: no paths are exposed.
#[derive(Debug, Clone)]
pub struct Instance {
    name: String,
    base_dir: Option<PathBuf>,
}

impl Instance {
    pub fn new(name: impl Into<String>) -> Result<Self, IpcError> {
        let name = name.into();
        if !is_valid_instance_name(&name) {
            return Err(IpcError::InvalidInstanceName(name));
        }
        Ok(Self {
            name,
            base_dir: None,
        })
    }

    /// The default instance.
    pub fn default_instance() -> Self {
        Self {
            name: DEFAULT_INSTANCE.to_string(),
            base_dir: None,
        }
    }

    /// Overrides the runtime directory (tests only; avoids touching the real one).
    pub fn with_base_dir(name: impl Into<String>, dir: PathBuf) -> Result<Self, IpcError> {
        let mut inst = Self::new(name)?;
        inst.base_dir = Some(dir);
        Ok(inst)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Human-readable endpoint for logs and diagnostics.
    pub fn display_endpoint(&self) -> String {
        platform::endpoint_display(self)
    }

    pub(crate) fn base_dir_override(&self) -> Option<&Path> {
        self.base_dir.as_deref()
    }

    /// Acquires the exclusive single-instance lock. Fails with `InstanceAlreadyRunning` if
    /// another daemon holds it. Never touches the socket.
    pub fn acquire_lock(&self) -> Result<InstanceLock, IpcError> {
        platform::acquire(self)
    }
}

/// Names of instances that currently accept connections, sorted.
/// `base_dir` overrides the runtime directory (tests only).
pub fn list_live_instances(base_dir: Option<&Path>) -> Vec<String> {
    platform::list_live(base_dir)
}

/// Daemon IPC control server. Owns the instance lock for its whole lifetime.
pub struct ControlServer {
    instance: Instance,
    shutdown: Arc<AtomicBool>,
    listener_thread: Option<JoinHandle<()>>,
    // Declared last so the lock is released only after the socket is removed and the thread joined.
    _lock: InstanceLock,
}

impl ControlServer {
    /// Binds the instance socket and starts serving `control`. `lock` must come from
    /// [`Instance::acquire_lock`] for the same instance.
    pub fn start(
        instance: Instance,
        lock: InstanceLock,
        control: DaemonControlHandle,
    ) -> Result<Self, IpcError> {
        let listener = platform::bind(&instance, &lock)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_thread = Arc::clone(&shutdown);

        let listener_thread = std::thread::Builder::new()
            .name("audigma-ipc".to_string())
            .spawn(move || {
                while control.is_running() && !shutdown_thread.load(Ordering::Acquire) {
                    match platform::accept(&listener) {
                        Ok(stream) => {
                            if let Ok(reader) = platform::clone_stream(&stream) {
                                handle_client(reader, stream, &control);
                            }
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
            })?;

        Ok(Self {
            instance,
            shutdown,
            listener_thread: Some(listener_thread),
            _lock: lock,
        })
    }

    pub fn instance(&self) -> &Instance {
        &self.instance
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // Stop and join first so the thread can never outlive the lock and touch a successor's socket.
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.listener_thread.take() {
            let _ = handle.join();
        }
        platform::unbind(&self.instance);
    }
}

fn handle_client(reader: impl Read, mut writer: impl Write, control: &DaemonControlHandle) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    while let Ok(bytes) = reader.read_line(&mut line) {
        if bytes == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let req: Result<IpcRequest, _> = serde_json::from_str(trimmed);
        let resp = match req {
            Ok(IpcRequest::GetStatus) => {
                let telem = control.get_telemetry();
                IpcResponse::Telemetry(Box::new(telem))
            }
            Ok(IpcRequest::SetVolume { volume_db }) => {
                control.set_volume(volume_db);
                IpcResponse::Ok
            }
            Ok(IpcRequest::SetVolumePercent { percent }) => {
                control.set_volume_percent(percent);
                IpcResponse::Ok
            }
            Ok(IpcRequest::SetMute { muted }) => {
                control.set_muted(muted);
                IpcResponse::Ok
            }
            Ok(IpcRequest::Pause) => match control.pause_and_wait(PAUSE_RESUME_TIMEOUT) {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e),
            },
            Ok(IpcRequest::Resume) => match control.resume_and_wait(PAUSE_RESUME_TIMEOUT) {
                Ok(()) => IpcResponse::Ok,
                Err(e) => IpcResponse::Error(e),
            },
            Ok(IpcRequest::Stop) => {
                control.stop();
                IpcResponse::Ok
            }
            Err(e) => IpcResponse::Error(format!("invalid request JSON: {e}")),
        };

        if let Ok(mut json_str) = serde_json::to_string(&resp) {
            json_str.push('\n');
            let _ = writer.write_all(json_str.as_bytes());
            let _ = writer.flush();
        }

        line.clear();
    }
}

/// Client helper for sending requests to a running audigma daemon.
pub struct ControlClient;

impl ControlClient {
    /// Sends an IPC request to `instance` and returns the response.
    pub fn send(instance: &Instance, request: IpcRequest) -> Result<IpcResponse, IpcError> {
        let mut stream = platform::connect(instance)?;
        let mut msg = serde_json::to_string(&request)?;
        msg.push('\n');
        stream.write_all(msg.as_bytes())?;
        stream.flush()?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;

        let resp: IpcResponse = serde_json::from_str(line.trim())?;
        Ok(resp)
    }
}
