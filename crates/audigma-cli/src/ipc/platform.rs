//! Unix transport for the daemon IPC: `AF_UNIX` socket plus an advisory `flock` lockfile.
//!
//! This module is the only place that knows about filesystem paths, `flock` and Unix sockets.
//! Everything else in `ipc` talks to it through a narrow interface so another transport
//! (e.g. Windows named pipes) can be added as a sibling module without touching callers:
//!
//! - [`InstanceLock`] / [`acquire`]: single-instance mutual exclusion
//! - [`bind`] / [`unbind`] / [`accept`] / [`clone_stream`]: server side
//! - [`connect`]: client side
//! - [`list_live`]: instance enumeration
//! - [`endpoint_display`]: human-readable endpoint for logs

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{Instance, IpcError, is_valid_instance_name};

pub(super) type Listener = UnixListener;
pub(super) type Stream = UnixStream;

/// Idle read timeout for an accepted client, so a stalled client cannot block daemon shutdown.
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// RAII advisory lock marking an instance as running. The kernel releases it when the process
/// exits for any reason, including `SIGKILL`.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Base runtime directory: `$XDG_RUNTIME_DIR/audigma`, falling back to `/tmp/audigma-<euid>`.
fn base_dir(override_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = override_dir {
        return dir.to_path_buf();
    }
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("audigma"),
        // SAFETY: geteuid has no preconditions and cannot fail.
        _ => PathBuf::from(format!("/tmp/audigma-{}", unsafe { libc::geteuid() })),
    }
}

fn dir_of(instance: &Instance) -> PathBuf {
    base_dir(instance.base_dir_override())
}

fn socket_path(instance: &Instance) -> PathBuf {
    dir_of(instance).join(format!("{}.sock", instance.name()))
}

fn lock_path(instance: &Instance) -> PathBuf {
    dir_of(instance).join(format!("{}.lock", instance.name()))
}

pub(super) fn endpoint_display(instance: &Instance) -> String {
    socket_path(instance).display().to_string()
}

/// Creates the runtime directory (mode 0700) and verifies it is a real directory owned by us.
/// This matters for the shared `/tmp` fallback, where another user could pre-create the path.
fn ensure_dir(dir: &Path) -> Result<(), IpcError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != euid {
        return Err(IpcError::Server(format!(
            "runtime directory {} is not a directory owned by the current user",
            dir.display()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Acquires the exclusive instance lock, or fails with `InstanceAlreadyRunning`.
/// Never touches the socket file.
pub(super) fn acquire(instance: &Instance) -> Result<InstanceLock, IpcError> {
    ensure_dir(&dir_of(instance))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(lock_path(instance))?;

    // SAFETY: the fd is valid for the lifetime of `file`.
    let res = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if res != 0 {
        let err = io::Error::last_os_error();
        return Err(match err.raw_os_error() {
            Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => {
                IpcError::InstanceAlreadyRunning(instance.name().to_string())
            }
            _ => IpcError::Io(err),
        });
    }
    Ok(InstanceLock { _file: file })
}

/// Binds the instance socket. Holding the lock proves any existing socket file is a dead orphan.
pub(super) fn bind(instance: &Instance, _lock: &InstanceLock) -> io::Result<Listener> {
    let path = socket_path(instance);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub(super) fn unbind(instance: &Instance) {
    let _ = fs::remove_file(socket_path(instance));
}

/// Non-blocking accept; `WouldBlock` means no pending client.
pub(super) fn accept(listener: &Listener) -> io::Result<Stream> {
    let (stream, _) = listener.accept()?;
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(CLIENT_READ_TIMEOUT))?;
    Ok(stream)
}

pub(super) fn clone_stream(stream: &Stream) -> io::Result<Stream> {
    stream.try_clone()
}

/// Connects to a running instance; a missing or stale socket means "not running".
pub(super) fn connect(instance: &Instance) -> Result<Stream, IpcError> {
    match UnixStream::connect(socket_path(instance)) {
        Ok(stream) => Ok(stream),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(IpcError::DaemonNotRunning(instance.name().to_string()))
        }
        Err(e) => Err(IpcError::Io(e)),
    }
}

/// Names of instances that currently accept connections, sorted.
/// Probes by connecting (no locking), so it cannot interfere with a daemon that is starting.
pub(super) fn list_live(base_dir_override: Option<&Path>) -> Vec<String> {
    let dir = base_dir(base_dir_override);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut live = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if is_valid_instance_name(name) && UnixStream::connect(&path).is_ok() {
            live.push(name.to_string());
        }
    }
    live.sort();
    live
}
