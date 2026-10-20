//! Linux ALSA soundcard event watcher using inotify on `/dev/snd` (`Design.md §1.6`, `Plan.md Step 5.2`).
//!
//! Provides zero-polling, push-driven event detection for hotplugged sound cards.
//! Monitors `/dev/snd` node creation (`IN_CREATE`, `IN_ATTRIB`) and matches against
//! configured ALSA device strings (e.g. `hw:CARD=JA11,DEV=0` or card names).

use std::ffi::CString;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// ALSA device watcher monitoring `/dev/snd` for card arrivals and changes.
pub struct AlsaDeviceWatcher {
    inotify_fd: RawFd,
    watch_wd: i32,
    running: Arc<AtomicBool>,
}

impl AlsaDeviceWatcher {
    /// Initializes an inotify watcher on `/dev/snd`.
    /// Returns `None` if `/dev/snd` cannot be watched (e.g. non-Linux or permission error).
    pub fn new() -> Option<Self> {
        Self::with_path(Path::new("/dev/snd"))
    }

    /// Initializes an inotify watcher on a custom path (useful for testing).
    pub fn with_path(path: &Path) -> Option<Self> {
        let inotify_fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if inotify_fd < 0 {
            return None;
        }

        let c_path = CString::new(path.to_str()?).ok()?;
        let mask = libc::IN_CREATE | libc::IN_ATTRIB | libc::IN_DELETE;
        let watch_wd = unsafe { libc::inotify_add_watch(inotify_fd, c_path.as_ptr(), mask) };
        if watch_wd < 0 {
            unsafe { libc::close(inotify_fd) };
            return None;
        }

        Some(Self {
            inotify_fd,
            watch_wd,
            running: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Drains any pending inotify events from the descriptor without blocking.
    pub fn drain_events(&self) {
        let mut buf = [0u8; 1024];
        loop {
            let res = unsafe {
                libc::read(
                    self.inotify_fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            if res <= 0 {
                break;
            }
        }
    }

    /// Waits for an inotify event on the watched directory with an optional timeout.
    /// Returns `Ok(true)` if an event was received, `Ok(false)` on timeout, or `Err` on failure.
    pub fn wait_for_event(&self, timeout: Option<Duration>) -> std::io::Result<bool> {
        let mut pfd = libc::pollfd {
            fd: self.inotify_fd,
            events: libc::POLLIN,
            revents: 0,
        };

        let timeout_ms = match timeout {
            Some(t) => t.as_millis().min(i32::MAX as u128) as i32,
            None => -1,
        };

        let res = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if res < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                return Ok(false);
            }
            return Err(err);
        }

        if res == 0 {
            return Ok(false);
        }

        // An event occurred; drain the event buffer so subsequent waits are fresh
        self.drain_events();
        Ok(true)
    }

    /// Signals the watcher to abort any ongoing waits.
    pub fn cancel(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl Drop for AlsaDeviceWatcher {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if self.watch_wd >= 0 {
            unsafe { libc::inotify_rm_watch(self.inotify_fd, self.watch_wd) };
        }
        if self.inotify_fd >= 0 {
            unsafe { libc::close(self.inotify_fd) };
        }
    }
}

/// Resolves an ALSA device string (e.g. `hw:CARD=JA11,DEV=0` or `hw:1,0`) to a card index `u32`.
pub fn resolve_card_index(device_str: &str) -> Option<u32> {
    if let Some(card_part) = device_str.strip_prefix("hw:CARD=") {
        let card_name = card_part.split(',').next().unwrap_or("").trim();
        // Check /proc/asound/<name> symlink if present
        if let Ok(target) = std::fs::read_link(format!("/proc/asound/{card_name}")) {
            if let Some(file_name) = target.file_name().and_then(|f| f.to_str()) {
                if let Some(idx_str) = file_name.strip_prefix("card") {
                    if let Ok(idx) = idx_str.parse::<u32>() {
                        return Some(idx);
                    }
                }
            }
        }
        #[cfg(feature = "alsa")]
        {
            if let Ok(c_str) = CString::new(card_name) {
                if let Ok(card) = alsa::Card::from_str(&c_str) {
                    return Some(card.get_index() as u32);
                }
            }
            for c in alsa::card::Iter::new().flatten() {
                if let Ok(name) = c.get_name() {
                    if name.eq_ignore_ascii_case(card_name) {
                        return Some(c.get_index() as u32);
                    }
                }
                if let Ok(longname) = c.get_longname() {
                    if longname.eq_ignore_ascii_case(card_name) {
                        return Some(c.get_index() as u32);
                    }
                }
            }
        }
        None
    } else if let Some(stripped) = device_str.strip_prefix("hw:") {
        stripped
            .split(',')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
    } else {
        None
    }
}

///
/// Supports device formats such as:
/// - `hw:CARD=JA11,DEV=0` (or `hw:CARD=JA11`)
/// - `hw:0,0` (or `hw:0`)
/// - `default`
pub fn is_alsa_device_present(device: &str) -> bool {
    #[cfg(feature = "alsa")]
    {
        // Try to probe for the card
        if device == "default" {
            return true;
        }

        // If format is hw:CARD=<name>,... check card by name
        if let Some(card_part) = device.strip_prefix("hw:CARD=") {
            let target_name = card_part.split(',').next().unwrap_or("").trim();
            if let Ok(c_str) = CString::new(target_name) {
                if alsa::Card::from_str(&c_str).is_ok() {
                    return true;
                }
            }

            for c in alsa::card::Iter::new().flatten() {
                if let Ok(name) = c.get_name() {
                    if name.eq_ignore_ascii_case(target_name) {
                        return true;
                    }
                }
                if let Ok(longname) = c.get_longname() {
                    if longname.eq_ignore_ascii_case(target_name) {
                        return true;
                    }
                }
            }

            return false;
        }

        // If format is hw:<index>,... check card index
        if let Some(stripped) = device.strip_prefix("hw:") {
            if let Some(idx_str) = stripped.split(',').next() {
                if let Ok(idx) = idx_str.parse::<i32>() {
                    for c in alsa::card::Iter::new().flatten() {
                        if c.get_index() == idx {
                            return true;
                        }
                    }

                    return false;
                }
            }
        }

        // Fallback: check if we can open the card/device ctl
        if let Ok(c_dev) = CString::new(device) {
            alsa::ctl::Ctl::open(&c_dev, false).is_ok()
        } else {
            false
        }
    }
    #[cfg(not(feature = "alsa"))]
    {
        let _ = device;
        true
    }
}

/// Waits for an ALSA device to become available, using inotify push notifications.
///
/// - `device_wait_ms == 0`: exits immediately if not present.
/// - `device_wait_ms == -1`: waits indefinitely.
/// - `device_wait_ms > 0`: waits up to `device_wait_ms` milliseconds.
///
/// Returns `Ok(true)` if device is present, `Ok(false)` if timeout expired,
/// or `Err` on failure.
pub fn wait_for_alsa_device(
    device: &str,
    device_wait_ms: i64,
    cancel_flag: &AtomicBool,
) -> std::io::Result<bool> {
    if is_alsa_device_present(device) {
        return Ok(true);
    }

    if device_wait_ms == 0 {
        return Ok(false);
    }

    let watcher = match AlsaDeviceWatcher::new() {
        Some(w) => w,
        None => {
            // If inotify fails (e.g. testing in sandbox without /dev/snd), sleep briefly
            let start = Instant::now();
            let limit = if device_wait_ms > 0 {
                Some(Duration::from_millis(device_wait_ms as u64))
            } else {
                None
            };
            while !cancel_flag.load(Ordering::Relaxed) {
                if let Some(lim) = limit {
                    if start.elapsed() >= lim {
                        return Ok(false);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
                if is_alsa_device_present(device) {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
    };

    let start = Instant::now();
    let limit = if device_wait_ms > 0 {
        Some(Duration::from_millis(device_wait_ms as u64))
    } else {
        None
    };

    while !cancel_flag.load(Ordering::Relaxed) {
        let remaining = match limit {
            Some(lim) => {
                let elapsed = start.elapsed();
                if elapsed >= lim {
                    return Ok(false);
                }
                Some(lim - elapsed)
            }
            None => None,
        };

        // Poll in slices of up to 500ms to allow cancel_flag checks
        let slice_timeout = match remaining {
            Some(r) => r.min(Duration::from_millis(500)),
            None => Duration::from_millis(500),
        };

        let _ = watcher.wait_for_event(Some(slice_timeout));
        if is_alsa_device_present(device) {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_watcher_tempdir() {
        let temp = tempfile::tempdir().unwrap();
        let watcher = AlsaDeviceWatcher::with_path(temp.path()).expect("watcher init");

        // Initially no events
        assert!(
            !watcher
                .wait_for_event(Some(Duration::from_millis(50)))
                .unwrap()
        );

        // Create a file in tempdir
        let file_path = temp.path().join("controlC99");
        std::fs::write(&file_path, b"test").unwrap();

        // Watcher should notify event
        assert!(
            watcher
                .wait_for_event(Some(Duration::from_millis(500)))
                .unwrap()
        );
    }

    #[test]
    fn test_is_alsa_device_present_default() {
        assert!(is_alsa_device_present("default"));
    }
}
