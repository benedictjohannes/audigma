//! Two-way volume / mute synchronisation between [`DaemonControlHandle`] and an input's
//! environment (the PipeWire stream's Props).
//!
//! Audigma owns the user volume. [`VolumeSync`] runs once per gain-loop iteration and
//!
//! 1. **handshake**: on a fresh input, pushes the control's state and discards every
//!    environment report until the input confirms that state (or [`HANDSHAKE_TIMEOUT`]
//!    passes). Without this, the stream's default Props (100 %, unmuted) arrive right after
//!    the stream is created and overwrite a restored volume.
//! 2. **push**: later control changes (IPC, tray) are written to the input.
//! 3. **ingest**: a report that is neither the stream's known state nor an in-flight echo of
//!    one of our own writes is a genuine external change and updates the control.
//!
//! Comparison is in percent on the stream's own curve ([`PERCENT_TOLERANCE`]), never in dB, so
//! an echo of a dB-sourced volume is ignored instead of calling `set_volume_percent` (which
//! would flip the unit persisted in the state file).

use crate::live_pipeline::{DaemonControlHandle, DaemonVolumeMode};
use crate::traits::{AudioInput, VolumeUpdate};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Two percentages closer than this are the same volume (float32 round trip through the
/// stream, plus slider quantisation below any audible step).
pub const PERCENT_TOLERANCE: f64 = 0.25;
/// How long a fresh input may take to confirm the seeded state before reports are trusted.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(1);
/// How long an unconfirmed write keeps shadowing reports as a possible echo.
const ECHO_WINDOW: Duration = Duration::from_millis(500);
const MAX_INFLIGHT: usize = 8;
/// The stream's volume range tops out at 150 %.
const MAX_PERCENT: f64 = 150.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct State {
    percent: f64,
    muted: bool,
}

impl State {
    fn differs(self, other: State) -> bool {
        !close(self.percent, other.percent) || self.muted != other.muted
    }

    fn update(self) -> VolumeUpdate {
        VolumeUpdate {
            volume_db: None,
            volume_percent: Some(self.percent),
            is_muted: Some(self.muted),
        }
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= PERCENT_TOLERANCE
}

/// The control's effective volume as a percentage; a dB-sourced volume is mapped through the
/// same curve the input uses for its reports.
fn control_state(control: &DaemonControlHandle) -> State {
    let percent = control.get_volume_percent().unwrap_or_else(|| {
        audigma_dsp::alsamixer_db_to_percentage(
            control.get_volume(),
            audigma_dsp::DEFAULT_SOFTWARE_MIN_DB,
        )
    });
    State {
        percent: percent.clamp(0.0, MAX_PERCENT),
        muted: control.is_muted(),
    }
}

/// The control's state as a push for [`AudioInput::push_volume`], used to seed a stream as
/// soon as it exists.
pub fn seed(control: &DaemonControlHandle) -> VolumeUpdate {
    control_state(control).update()
}

/// Applies a report to the control without any filtering (inputs that cannot be pushed to).
fn apply_unfiltered(control: &DaemonControlHandle, r: VolumeUpdate) {
    if let Some(pct) = r.volume_percent {
        control.set_volume_percent(pct);
    } else if let Some(db) = r.volume_db {
        control.set_volume(db);
    }
    if let Some(muted) = r.is_muted {
        control.set_muted(muted);
    }
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    /// Seeded value not yet confirmed; `None` until the first step starts the clock.
    Handshake {
        deadline: Option<Instant>,
    },
    Synced,
    /// The input cannot be pushed to; reports are applied as they come.
    Unsupported,
}

/// Per-input synchronisation state; create one per input stream (and per streaming run).
pub struct VolumeSync {
    phase: Phase,
    /// What the input's environment is believed to hold.
    known: Option<State>,
    /// Our writes whose echo has not been seen yet, oldest first.
    inflight: VecDeque<(Instant, State)>,
}

impl Default for VolumeSync {
    fn default() -> Self {
        Self::new()
    }
}

impl VolumeSync {
    pub fn new() -> Self {
        Self {
            phase: Phase::Handshake { deadline: None },
            known: None,
            inflight: VecDeque::with_capacity(MAX_INFLIGHT),
        }
    }

    /// Runs one synchronisation step. Call once per gain-loop iteration.
    pub fn step(
        &mut self,
        now: Instant,
        control: &DaemonControlHandle,
        input: &mut dyn AudioInput,
    ) {
        let report = input.check_volume_change();
        if matches!(self.phase, Phase::Unsupported) {
            if let Some(r) = report {
                apply_unfiltered(control, r);
            }
            return;
        }
        // Fixed unity gain: the control is neither pushed nor changed from outside.
        if control.get_volume_mode() == DaemonVolumeMode::None {
            return;
        }

        let want = control_state(control);
        let mut pushed = false;
        if self.known.is_none_or(|k| k.differs(want)) {
            if !input.push_volume(want.update()) {
                self.phase = Phase::Unsupported;
                if let Some(r) = report {
                    apply_unfiltered(control, r);
                }
                return;
            }
            self.known = Some(want);
            if self.inflight.len() == MAX_INFLIGHT {
                self.inflight.pop_front();
            }
            self.inflight.push_back((now, want));
            pushed = true;
        }
        // A report taken before our write describes the stream as it was.
        let report = report.filter(|_| !pushed);

        match self.phase {
            Phase::Handshake { mut deadline } => {
                let deadline = *deadline.get_or_insert(now + HANDSHAKE_TIMEOUT);
                let confirmed = match (report, self.known) {
                    (Some(r), Some(k)) => explains(&r, k),
                    _ => false,
                };
                if confirmed || now >= deadline {
                    self.phase = Phase::Synced;
                    self.inflight.clear();
                    tracing::info!(
                        target: "audigma::daemon",
                        event = audigma_log::VOLUME_SYNC,
                        confirmed,
                        percent = want.percent,
                        muted = want.muted,
                        "volume handshake complete"
                    );
                } else {
                    self.phase = Phase::Handshake {
                        deadline: Some(deadline),
                    };
                }
            }
            Phase::Synced => {
                if let Some(r) = report {
                    self.ingest(now, control, r);
                }
            }
            Phase::Unsupported => {}
        }
    }

    /// Applies the parts of a report that no known state or in-flight write explains.
    fn ingest(&mut self, now: Instant, control: &DaemonControlHandle, r: VolumeUpdate) {
        let Some(known) = self.known else { return };
        self.inflight
            .retain(|(t, _)| now.duration_since(*t) <= ECHO_WINDOW);

        // Report percentage on the stream's curve; a dB-only report is mapped for comparison.
        let reported_pct = r.volume_percent.or_else(|| {
            r.volume_db.map(|db| {
                audigma_dsp::alsamixer_db_to_percentage(db, audigma_dsp::DEFAULT_SOFTWARE_MIN_DB)
            })
        });
        let vol_echo = reported_pct.is_none_or(|p| {
            close(p, known.percent) || self.inflight.iter().any(|(_, s)| close(p, s.percent))
        });
        let mute_echo = r
            .is_muted
            .is_none_or(|m| m == known.muted || self.inflight.iter().any(|(_, s)| s.muted == m));

        if vol_echo && mute_echo {
            // Echoes arrive in order: everything up to the matching write is accounted for.
            if let Some(k) = self.inflight.iter().rposition(|(_, s)| explains(&r, *s)) {
                self.inflight.drain(..=k);
            }
            return;
        }

        if !vol_echo {
            match r.volume_percent {
                Some(pct) => control.set_volume_percent(pct),
                None => {
                    if let Some(db) = r.volume_db {
                        control.set_volume(db);
                    }
                }
            }
        }
        if !mute_echo {
            if let Some(m) = r.is_muted {
                control.set_muted(m);
            }
        }
        // The stream already holds the reported value; nothing to push back.
        let mut state = known;
        if !vol_echo {
            state.percent = reported_pct
                .unwrap_or(known.percent)
                .clamp(0.0, MAX_PERCENT);
        }
        if !mute_echo {
            state.muted = r.is_muted.unwrap_or(known.muted);
        }
        self.known = Some(state);
        self.inflight.clear();
        tracing::debug!(
            target: "audigma::daemon",
            event = audigma_log::VOLUME_SYNC,
            percent = state.percent,
            muted = state.muted,
            "applied external volume change"
        );
    }
}

/// Whether every field present in the report equals the state.
fn explains(r: &VolumeUpdate, s: State) -> bool {
    let pct = r.volume_percent.or_else(|| {
        r.volume_db.map(|db| {
            audigma_dsp::alsamixer_db_to_percentage(db, audigma_dsp::DEFAULT_SOFTWARE_MIN_DB)
        })
    });
    pct.is_none_or(|p| close(p, s.percent)) && r.is_muted.is_none_or(|m| m == s.muted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_pipeline::DaemonOutputFormat;
    use crate::traits::MockAudioInput;
    use audigma_format::AudioStreamInfo;

    fn control() -> DaemonControlHandle {
        DaemonControlHandle::new(48000, DaemonOutputFormat::Pcm, 48000, "test", 0.0)
    }

    fn input() -> (
        MockAudioInput,
        std::sync::Arc<std::sync::Mutex<Vec<VolumeUpdate>>>,
    ) {
        let info = AudioStreamInfo::new_pcm(audigma_dsp::PcmRateHz::PCM_48000, 2, None, Some(32));
        MockAudioInput::new(info, vec![0.0; 4], vec![0.0; 4], false).with_volume_push()
    }

    fn default_report() -> VolumeUpdate {
        VolumeUpdate {
            volume_db: Some(0.0),
            volume_percent: Some(100.0),
            is_muted: Some(false),
        }
    }

    fn desktop(percent: f64, muted: bool) -> VolumeUpdate {
        VolumeUpdate {
            volume_db: None,
            volume_percent: Some(percent),
            is_muted: Some(muted),
        }
    }

    #[test]
    fn handshake_discards_stream_defaults_and_keeps_db_unit() {
        let control = control();
        control.set_volume(-33.5);
        control.set_muted(true);
        let (mut mock, pushed) = input();
        let mut sync = VolumeSync::new();
        let t = Instant::now();

        // Stream defaults land before audigma's seed does.
        mock.simulate_volume_change(default_report());
        sync.step(t, &control, &mut mock);
        assert_eq!(control.get_volume(), -33.5);
        assert_eq!(control.get_volume_percent(), None);
        assert!(control.is_muted());
        let seeded = pushed.lock().unwrap()[0];
        assert_eq!(seeded.is_muted, Some(true));
        let expected = audigma_dsp::alsamixer_db_to_percentage(-33.5, -60.0);
        assert!((seeded.volume_percent.unwrap() - expected).abs() < 1e-9);

        // The echo confirms the handshake; the unit is untouched.
        sync.step(t, &control, &mut mock);
        assert!(matches!(sync.phase, Phase::Synced));
        assert_eq!(control.get_volume_percent(), None);
        assert_eq!(control.get_volume(), -33.5);
        assert_eq!(pushed.lock().unwrap().len(), 1);
    }

    #[test]
    fn handshake_times_out_then_trusts_reports() {
        let control = control();
        control.set_volume_percent(40.0);
        let (mut mock, _) = input();
        let mut sync = VolumeSync::new();
        let t = Instant::now();

        sync.step(t, &control, &mut mock);
        assert!(mock.check_volume_change().is_some()); // the stream never confirms
        mock.simulate_volume_change(desktop(80.0, false));
        sync.step(t + Duration::from_millis(500), &control, &mut mock);
        assert_eq!(control.get_volume_percent(), Some(40.0));

        sync.step(t + HANDSHAKE_TIMEOUT, &control, &mut mock);
        assert!(matches!(sync.phase, Phase::Synced));
        mock.simulate_volume_change(desktop(80.0, false));
        sync.step(t + HANDSHAKE_TIMEOUT, &control, &mut mock);
        assert_eq!(control.get_volume_percent(), Some(80.0));
    }

    #[test]
    fn echoes_are_ignored_and_control_changes_pushed() {
        let control = control();
        control.set_volume(-12.0);
        let (mut mock, pushed) = input();
        let mut sync = VolumeSync::new();
        let t = Instant::now();
        sync.step(t, &control, &mut mock);
        sync.step(t, &control, &mut mock);

        control.set_volume(-20.0);
        control.set_muted(true);
        sync.step(t, &control, &mut mock);
        assert_eq!(pushed.lock().unwrap().len(), 2);
        sync.step(t, &control, &mut mock); // echo
        assert_eq!(control.get_volume(), -20.0);
        assert_eq!(
            control.get_volume_percent(),
            None,
            "echo must not flip the unit"
        );
        assert!(control.is_muted());
        assert_eq!(
            pushed.lock().unwrap().len(),
            2,
            "echo must not be pushed back"
        );
    }

    #[test]
    fn stale_echo_of_superseded_write_is_ignored() {
        let control = control();
        control.set_volume_percent(30.0);
        let (mut mock, _) = input();
        let mut sync = VolumeSync::new();
        let t = Instant::now();
        sync.step(t, &control, &mut mock);
        sync.step(t, &control, &mut mock);

        control.set_volume_percent(35.0);
        sync.step(t, &control, &mut mock); // pushes 35
        control.set_volume_percent(36.0);
        mock.simulate_volume_change(desktop(35.0, false)); // echo of 35 arrives late
        sync.step(t, &control, &mut mock);
        assert_eq!(control.get_volume_percent(), Some(36.0));
    }

    #[test]
    fn external_change_applies_as_percent_and_mute_both_ways() {
        let control = control();
        control.set_volume(-6.0);
        let (mut mock, pushed) = input();
        let mut sync = VolumeSync::new();
        let t = Instant::now();
        sync.step(t, &control, &mut mock);
        sync.step(t, &control, &mut mock);

        mock.simulate_volume_change(desktop(55.0, true));
        sync.step(t, &control, &mut mock);
        assert_eq!(control.get_volume_percent(), Some(55.0));
        assert!(control.is_muted());
        assert_eq!(
            pushed.lock().unwrap().len(),
            1,
            "external change is not echoed back"
        );

        control.set_muted(false);
        sync.step(t, &control, &mut mock);
        assert_eq!(pushed.lock().unwrap().len(), 2);
        assert_eq!(pushed.lock().unwrap()[1].is_muted, Some(false));
    }

    #[test]
    fn input_without_push_support_applies_reports_directly() {
        let control = control();
        let info = AudioStreamInfo::new_pcm(audigma_dsp::PcmRateHz::PCM_48000, 2, None, Some(32));
        let mut mock = MockAudioInput::new(info, vec![0.0; 4], vec![0.0; 4], false);
        let mut sync = VolumeSync::new();
        mock.simulate_volume_change(VolumeUpdate {
            volume_db: Some(-12.5),
            volume_percent: None,
            is_muted: Some(true),
        });
        sync.step(Instant::now(), &control, &mut mock);
        assert_eq!(control.get_volume(), -12.5);
        assert!(control.is_muted());
    }

    #[test]
    fn volume_mode_none_neither_pushes_nor_ingests() {
        let control = control();
        control.set_volume_mode(DaemonVolumeMode::None);
        let (mut mock, pushed) = input();
        let mut sync = VolumeSync::new();
        mock.simulate_volume_change(desktop(10.0, true));
        sync.step(Instant::now(), &control, &mut mock);
        assert_eq!(control.get_volume_percent(), Some(100.0));
        assert!(!control.is_muted());
        assert!(pushed.lock().unwrap().is_empty());
    }
}
