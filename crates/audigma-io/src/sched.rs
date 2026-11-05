//! Thread scheduling and affinity (Linux).
//!
//! `apply` is the single entry point for putting the calling thread into its
//! [`ThreadPlacement`]. Real-time elevation uses raw `sched_setattr` with
//! `SCHED_FLAG_RESET_ON_FORK`, so threads spawned by an RT thread (including
//! those created inside libpipewire) start at `SCHED_OTHER`. If the direct
//! attempt is refused, RealtimeKit is tried (requires the `dbus` feature).

use audigma_engine::ThreadPlacement;
use thiserror::Error;

/// How real-time scheduling was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimePriorityAcquisition {
    /// Direct `sched_setattr(SCHED_FIFO | RESET_ON_FORK)`.
    DirectSchedFifo,
    /// Granted by `org.freedesktop.RealtimeKit1` (lands as `SCHED_RR`).
    RealtimeKit,
}

impl std::fmt::Display for RealtimePriorityAcquisition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectSchedFifo => write!(f, "SCHED_FIFO"),
            Self::RealtimeKit => write!(f, "RealtimeKit"),
        }
    }
}

#[derive(Debug, Error)]
pub enum SchedError {
    #[error("sched_setaffinity failed: {0}")]
    Affinity(std::io::Error),
    #[error("real-time scheduling unavailable: {0}")]
    Realtime(String),
}

/// What `apply` achieved for the calling thread.
#[derive(Debug, Default)]
pub struct AppliedPlacement {
    pub realtime: Option<RealtimePriorityAcquisition>,
}

/// Applies `placement` to the calling thread: affinity first, then scheduling class.
pub fn apply(placement: &ThreadPlacement) -> Result<AppliedPlacement, SchedError> {
    #[cfg(target_os = "linux")]
    {
        if let Some(cpus) = &placement.cpus {
            set_affinity(cpus)?;
        }
        let mut applied = AppliedPlacement::default();
        if let Some(prio) = placement.rt_priority {
            applied.realtime = Some(make_realtime(prio)?);
        }
        Ok(applied)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = placement;
        Ok(AppliedPlacement::default())
    }
}

#[cfg(target_os = "linux")]
fn set_affinity(cpus: &[usize]) -> Result<(), SchedError> {
    if cpus.is_empty() {
        return Ok(());
    }
    unsafe {
        let mut set: libc::cpu_set_t = core::mem::zeroed();
        for &c in cpus {
            if c < libc::CPU_SETSIZE as usize {
                libc::CPU_SET(c, &mut set);
            }
        }
        if libc::sched_setaffinity(0, core::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
            return Err(SchedError::Affinity(std::io::Error::last_os_error()));
        }
    }
    Ok(())
}

/// Kernel `struct sched_attr` (VER0 layout, 48 bytes).
#[cfg(target_os = "linux")]
#[repr(C)]
struct SchedAttr {
    size: u32,
    sched_policy: u32,
    sched_flags: u64,
    sched_nice: i32,
    sched_priority: u32,
    sched_runtime: u64,
    sched_deadline: u64,
    sched_period: u64,
}

#[cfg(target_os = "linux")]
const SCHED_FLAG_RESET_ON_FORK: u64 = 0x01;

#[cfg(target_os = "linux")]
fn make_realtime(priority: u8) -> Result<RealtimePriorityAcquisition, SchedError> {
    let force_rtkit = std::env::var_os("AUDIGMA_FORCE_RTKIT").is_some_and(|v| v == "1");
    let mut direct_err = None;
    if !force_rtkit {
        let attr = SchedAttr {
            size: core::mem::size_of::<SchedAttr>() as u32,
            sched_policy: libc::SCHED_FIFO as u32,
            sched_flags: SCHED_FLAG_RESET_ON_FORK,
            sched_nice: 0,
            sched_priority: priority as u32,
            sched_runtime: 0,
            sched_deadline: 0,
            sched_period: 0,
        };
        // pid 0 = calling thread.
        let rc =
            unsafe { libc::syscall(libc::SYS_sched_setattr, 0, &attr as *const SchedAttr, 0u32) };
        if rc == 0 {
            return Ok(RealtimePriorityAcquisition::DirectSchedFifo);
        }
        direct_err = Some(std::io::Error::last_os_error());
    }

    #[cfg(feature = "dbus")]
    {
        prepare_rttime_limit();
        crate::dbus::rtkit_make_realtime(priority as u32)
            .map(|()| RealtimePriorityAcquisition::RealtimeKit)
            .map_err(|e| match direct_err {
                Some(d) => SchedError::Realtime(format!("sched_setattr: {d}; RealtimeKit: {e}")),
                None => SchedError::Realtime(format!("RealtimeKit: {e}")),
            })
    }
    #[cfg(not(feature = "dbus"))]
    {
        Err(SchedError::Realtime(match direct_err {
            Some(e) => format!("sched_setattr: {e}"),
            None => "AUDIGMA_FORCE_RTKIT=1 but dbus feature is disabled".to_string(),
        }))
    }
}

/// RealtimeKit refuses processes without a finite `RLIMIT_RTTIME` (its default maximum is 200 ms).
#[cfg(all(target_os = "linux", feature = "dbus"))]
const RTKIT_RTTIME_USEC: libc::rlim_t = 200_000;

/// Sets a finite `RLIMIT_RTTIME` once per process, before the first RealtimeKit request. A runaway
/// RT thread then gets SIGXCPU/SIGKILL instead of hanging the machine; our RT threads all block
/// in `futex_wait`/sleep well within the limit.
#[cfg(all(target_os = "linux", feature = "dbus"))]
fn prepare_rttime_limit() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut cur = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut cur) } != 0 {
            return;
        }
        let new = desired_rttime_limit(cur);
        if (new.rlim_cur, new.rlim_max) == (cur.rlim_cur, cur.rlim_max) {
            return;
        }
        if unsafe { libc::setrlimit(libc::RLIMIT_RTTIME, &new) } != 0 {
            tracing::debug!(
                error = %std::io::Error::last_os_error(),
                "could not set RLIMIT_RTTIME before RealtimeKit request"
            );
        }
    });
}

/// Finite limit to apply given the current one: keep an already finite soft limit, never raise the
/// hard limit, otherwise 200 ms.
#[cfg(all(target_os = "linux", feature = "dbus"))]
fn desired_rttime_limit(cur: libc::rlimit) -> libc::rlimit {
    let hard = cur.rlim_max.min(RTKIT_RTTIME_USEC);
    let soft = cur.rlim_cur.min(hard);
    libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn sched_attr_is_ver0_size() {
        assert_eq!(core::mem::size_of::<SchedAttr>(), 48);
    }

    #[cfg(feature = "dbus")]
    #[test]
    fn rttime_limit_becomes_finite_and_never_raises() {
        let inf = libc::RLIM_INFINITY;
        let n = desired_rttime_limit(libc::rlimit {
            rlim_cur: inf,
            rlim_max: inf,
        });
        assert_eq!((n.rlim_cur, n.rlim_max), (200_000, 200_000));
        let n = desired_rttime_limit(libc::rlimit {
            rlim_cur: 50_000,
            rlim_max: inf,
        });
        assert_eq!((n.rlim_cur, n.rlim_max), (50_000, 200_000));
        let n = desired_rttime_limit(libc::rlimit {
            rlim_cur: inf,
            rlim_max: 100_000,
        });
        assert_eq!((n.rlim_cur, n.rlim_max), (100_000, 100_000));
    }

    #[test]
    fn apply_without_rt_or_cpus_is_noop() {
        let p = ThreadPlacement {
            cpus: None,
            rt_priority: None,
            spin: true,
        };
        assert!(apply(&p).unwrap().realtime.is_none());
    }
}
