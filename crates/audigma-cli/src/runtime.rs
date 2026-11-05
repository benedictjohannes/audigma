//! `[runtime]` wiring: topology discovery, placement resolution and OS-side application.

use audigma_engine::{Placement, ThreadPlacement, WorkerSetup, resolve_placement};
use audigma_settings::{RealtimeMode, Settings};
use std::sync::Arc;

use crate::error::CliError;

/// Reads the machine topology (best-effort) and resolves the placement for `channels` DSP workers.
///
/// Must run before anything narrows the affinity mask (it records the process's `allowed` set).
/// `offline` strips all real-time requests.
pub fn resolve(settings: &Settings, channels: usize, offline: bool) -> Result<Placement, CliError> {
    let cfg = settings.runtime_config();
    let topology = match audigma_io::read_cpu_topology() {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!(
                target: "audigma::daemon",
                error = %e,
                "CPU topology discovery failed; running unpinned"
            );
            None
        }
    };
    let mut placement = resolve_placement(topology.as_ref(), &cfg, channels)
        .map_err(|e| CliError::Config(e.to_string()))?;
    for w in &placement.warnings {
        tracing::warn!(target: "audigma::daemon", "placement: {w}");
    }
    if offline {
        if placement.realtime != RealtimeMode::Off {
            tracing::debug!(
                target: "audigma::daemon",
                "realtime scheduling is never requested for offline commands"
            );
        }
        placement = placement.without_realtime();
    }
    tracing::info!(
        target: "audigma::daemon",
        channels,
        workers = placement.dsp.len(),
        dsp_cpus = ?placement.dsp.iter().map(|d| d.cpus.clone()).collect::<Vec<_>>(),
        spin = ?placement.dsp.iter().map(|d| d.spin).collect::<Vec<_>>(),
        housekeeping = ?placement.housekeeping,
        loop_cpus = ?placement.loop_.cpus,
        realtime = ?placement.realtime,
        realtime_priority = placement.realtime_priority,
        "runtime placement resolved"
    );
    Ok(placement)
}

/// Restricts the calling thread (supervisor / offline dispatcher) to the housekeeping set so every
/// helper thread it spawns inherits it. No-op when the set is empty.
pub fn apply_housekeeping(placement: &Placement) {
    if placement.housekeeping.is_empty() {
        return;
    }
    let tp = ThreadPlacement {
        cpus: Some(placement.housekeeping.clone()),
        rt_priority: None,
        spin: false,
    };
    if let Err(e) = audigma_io::apply(&tp) {
        tracing::warn!(target: "audigma::daemon", error = %e, "housekeeping affinity not applied");
    }
}

/// Applies `placement` to the calling thread and logs the outcome as an `RT_PRIORITY` event.
/// `fatal_on_failure` only selects the wording/level of the failure log; the caller aborts.
pub fn apply_logged(
    thread: &str,
    worker: Option<usize>,
    placement: &ThreadPlacement,
    fatal_on_failure: bool,
) -> Result<(), String> {
    match audigma_io::apply(placement) {
        Ok(applied) => {
            tracing::info!(
                target: "audigma::daemon",
                event = audigma_log::RT_PRIORITY,
                thread,
                worker,
                acquired = applied.realtime.is_some(),
                priority = placement.rt_priority,
                cpus = ?placement.cpus,
                spin = placement.spin,
                method = applied.realtime.map(|m| m.to_string()).unwrap_or_default(),
                "thread placement applied"
            );
            Ok(())
        }
        Err(e) => {
            if fatal_on_failure {
                tracing::error!(
                    target: "audigma::daemon",
                    event = audigma_log::RT_PRIORITY,
                    thread,
                    worker,
                    acquired = false,
                    priority = placement.rt_priority,
                    cpus = ?placement.cpus,
                    error = %e,
                    "real-time scheduling required but unavailable; aborting startup"
                );
            } else {
                tracing::warn!(
                    target: "audigma::daemon",
                    event = audigma_log::RT_PRIORITY,
                    thread,
                    worker,
                    acquired = false,
                    priority = placement.rt_priority,
                    cpus = ?placement.cpus,
                    error = %e,
                    "thread placement failed; continuing unpinned / normal priority"
                );
            }
            Err(e.to_string())
        }
    }
}

/// Whether a failed `apply` of `tp` must abort startup (`realtime = "required"` and the thread
/// asked for RT).
pub fn rt_failure_is_fatal(placement: &Placement, tp: &ThreadPlacement) -> bool {
    placement.realtime == RealtimeMode::Required && tp.rt_priority.is_some()
}

/// DSP worker setup: per-worker placements plus the hook that applies them on the worker thread.
pub fn worker_setup(placement: &Placement) -> WorkerSetup {
    let require = placement.realtime == RealtimeMode::Required;
    WorkerSetup {
        workers: Some(placement.dsp.len()),
        placements: placement.dsp.clone(),
        hook: Some(Arc::new(move |ch, p| {
            apply_logged(
                "audigma-dsp",
                Some(ch),
                p,
                require && p.rt_priority.is_some(),
            )
        })),
        // Startup handshake: with `realtime = "required"` a failed worker setup aborts startup.
        require_hook_ok: require,
    }
}
