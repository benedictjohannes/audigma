//! Address-wait primitive for the worker pool's barrier (`Design.md §3.4`).
//!
//! The pool spins first and only blocks through [`Waiter`] once the spin budget is spent, so
//! the wait/wake pair sits next to an OS call. It is supplied by the platform layer
//! (`audigma_io::runtime::worker_setup`) as plain `fn` pointers: the engine stays free of OS
//! code, and the indirect call is negligible next to the syscall it fronts. A trait would
//! either make the pool generic all the way up or put a `dyn` behind an `Arc`; neither buys
//! anything for two functions with no state.

use std::fmt;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

/// Blocks while `*atomic == expected`, for at most `timeout`. May return spuriously.
pub type WaitFn = fn(atomic: &AtomicU32, expected: u32, timeout: Duration);
/// Wakes one thread blocked in [`WaitFn`] on `atomic`.
pub type WakeFn = fn(atomic: &AtomicU32);

/// Upper bound on one blocking wait. Callers re-check their condition after every return, so
/// the bound only matters when no wake arrives: it lets the coordinator notice a worker
/// thread that died (it never wakes anyone) via `JoinHandle::is_finished`.
pub(crate) const WAIT_TIMEOUT: Duration = Duration::from_millis(20);

/// The wait/wake pair the worker pool blocks with.
///
/// Implementations must not allocate or take locks: they run on real-time threads.
#[derive(Clone, Copy)]
pub struct Waiter {
    wait: WaitFn,
    wake_one: WakeFn,
}

impl Waiter {
    /// Builds a waiter from a platform's address-wait primitive (futex, `WaitOnAddress`, ...).
    pub const fn new(wait: WaitFn, wake_one: WakeFn) -> Self {
        Self { wait, wake_one }
    }

    /// Portable fallback: waiting yields the thread and waking does nothing, so a blocked
    /// thread busy-polls its condition. Correct but burns a core per waiting thread; it does
    /// not meet the real-time requirement and is meant for tests and platforms without a port.
    pub const PORTABLE: Self = Self::new(|_, _, _| std::thread::yield_now(), |_| {});

    #[inline]
    pub(crate) fn wait(&self, atomic: &AtomicU32, expected: u32) {
        (self.wait)(atomic, expected, WAIT_TIMEOUT);
    }

    #[inline]
    pub(crate) fn wake_one(&self, atomic: &AtomicU32) {
        (self.wake_one)(atomic);
    }
}

/// [`Waiter::PORTABLE`]. Production callers pass the platform waiter through
/// `audigma_io::runtime::worker_setup`; a hand-built `WorkerSetup` that leaves this default
/// still runs correctly, only without blocking waits.
impl Default for Waiter {
    fn default() -> Self {
        Self::PORTABLE
    }
}

impl fmt::Debug for Waiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Waiter")
    }
}
