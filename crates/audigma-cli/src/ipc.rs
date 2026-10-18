//! Daemon IPC protocol and Unix domain socket control server/client (`Design.md §7`).
//!
//! Provides:
//! - Local control socket at `$XDG_RUNTIME_DIR/audigma/control.sock`.
//! - Newline-delimited JSON commands:
//!   - `GetStatus` -> `DaemonTelemetry`
//!   - `SetVolume { volume_db }` -> `Ok`
//!   - `SetPreset { preset }` -> `Ok`
//!   - `Stop` -> `Ok`
//! - Real-time safe: runs on an independent I/O thread, communicates with audio threads via atomics and mutexes.

use audigma_engine::DaemonControlHandle;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use thiserror::Error;

/// IPC request commands sent over the Unix control socket.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", content = "payload")]
pub enum IpcRequest {
    GetStatus,
    SetVolume { volume_db: f64 },
    SetPreset { preset: String },
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

    #[error("audigma daemon is not running (socket {0} not found)")]
    DaemonNotRunning(PathBuf),
}

/// Resolves the default control socket path at `$XDG_RUNTIME_DIR/audigma/control.sock`.
pub fn default_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("audigma/control.sock")
    } else {
        PathBuf::from("/tmp/audigma-control.sock")
    }
}

/// Daemon IPC control server.
pub struct ControlServer {
    socket_path: PathBuf,
    _listener_thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Starts the Unix domain socket server at `socket_path` bound to `control`.
    pub fn start(socket_path: PathBuf, control: DaemonControlHandle) -> Result<Self, IpcError> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if socket_path.exists() {
            let _ = fs::remove_file(&socket_path);
        }

        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;

        let sock_clone = socket_path.clone();
        let ctrl_clone = control.clone();

        let listener_thread = std::thread::Builder::new()
            .name("audigma-ipc".to_string())
            .spawn(move || {
                while ctrl_clone.is_running() {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            handle_client(stream, &ctrl_clone);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
                }
                let _ = fs::remove_file(&sock_clone);
            })?;

        Ok(Self {
            socket_path,
            _listener_thread: Some(listener_thread),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn handle_client(mut stream: UnixStream, control: &DaemonControlHandle) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
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
            Ok(IpcRequest::SetPreset { preset }) => {
                control.set_preset(&preset);
                IpcResponse::Ok
            }
            Ok(IpcRequest::Stop) => {
                control.stop();
                IpcResponse::Ok
            }
            Err(e) => IpcResponse::Error(format!("invalid request JSON: {e}")),
        };

        if let Ok(mut json_str) = serde_json::to_string(&resp) {
            json_str.push('\n');
            let _ = stream.write_all(json_str.as_bytes());
            let _ = stream.flush();
        }

        line.clear();
    }
}

/// Client helper for sending requests to a running audigma daemon.
pub struct ControlClient;

impl ControlClient {
    /// Sends an IPC request to the daemon at `socket_path` and returns the response.
    pub fn send(socket_path: &Path, request: IpcRequest) -> Result<IpcResponse, IpcError> {
        if !socket_path.exists() {
            return Err(IpcError::DaemonNotRunning(socket_path.to_path_buf()));
        }

        let mut stream = UnixStream::connect(socket_path)?;
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
