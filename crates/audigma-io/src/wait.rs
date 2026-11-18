//! Platform [`Waiter`] for the engine's worker barrier (`Design.md §3.4`).
//!
//! Linux uses a process-private futex: no allocation, no lock, and the kernel compares the
//! word atomically with going to sleep, so a wake between the caller's check and the wait is
//! never lost. Other platforms get [`Waiter::PORTABLE`] until their port supplies an
//! address-wait primitive (`WaitOnAddress` on Windows, `os_sync_wait_on_address` on macOS).

use audigma_engine::Waiter;

/// The waiter for this platform.
pub fn waiter() -> Waiter {
    #[cfg(target_os = "linux")]
    {
        Waiter::new(futex::wait, futex::wake_one)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Waiter::PORTABLE
    }
}

#[cfg(target_os = "linux")]
mod futex {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    const FUTEX_WAIT_PRIVATE: libc::c_int = libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG;
    const FUTEX_WAKE_PRIVATE: libc::c_int = libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG;

    pub(super) fn wait(atomic: &AtomicU32, expected: u32, timeout: Duration) {
        let ts = libc::timespec {
            tv_sec: timeout.as_secs() as libc::time_t,
            tv_nsec: timeout.subsec_nanos() as libc::c_long,
        };
        // Return value ignored: EAGAIN (value already changed), EINTR and ETIMEDOUT all mean
        // "re-check the condition", which every caller does.
        unsafe {
            libc::syscall(
                libc::SYS_futex,
                atomic.as_ptr(),
                FUTEX_WAIT_PRIVATE,
                expected,
                &ts as *const libc::timespec,
            );
        }
    }

    pub(super) fn wake_one(atomic: &AtomicU32) {
        unsafe {
            libc::syscall(libc::SYS_futex, atomic.as_ptr(), FUTEX_WAKE_PRIVATE, 1);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::futex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn wait_returns_at_once_when_value_differs() {
        let a = AtomicU32::new(1);
        let start = Instant::now();
        futex::wait(&a, 0, Duration::from_secs(1));
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn wait_is_bounded_by_timeout() {
        let a = AtomicU32::new(0);
        let start = Instant::now();
        futex::wait(&a, 0, Duration::from_millis(10));
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(5), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    }

    #[test]
    fn wake_releases_a_blocked_waiter() {
        let a = Arc::new(AtomicU32::new(0));
        let waiter = {
            let a = Arc::clone(&a);
            std::thread::spawn(move || {
                let start = Instant::now();
                while a.load(Ordering::Acquire) == 0 {
                    futex::wait(&a, 0, Duration::from_secs(5));
                }
                start.elapsed()
            })
        };
        std::thread::sleep(Duration::from_millis(20));
        a.store(1, Ordering::Release);
        futex::wake_one(&a);
        // Woken, not timed out.
        assert!(waiter.join().unwrap() < Duration::from_secs(4));
    }
}
