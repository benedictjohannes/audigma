//! Engine transition lifecycle state machine and click-free muting/priming (`Design.md §4.5`).
//!
//! Orchestrates the transition lifecycle:
//! `Stopped → Preparing → Priming → Running → Draining/Muting → Reconfiguring`
//! (plus `Recovering` and `DeviceLost`).
//!
//! Guarantees:
//! 1. Pre-SRC PCM fade: Gain is ramped to zero in floating-point PCM before the interpolation cascade.
//! 2. Group delay draining: Runs `filter_group_delay` zero frames until FIR ring buffers clear completely.
//! 3. Continuous DSD idle emission: Emits balanced `0x69` DSD silence with uninterrupted `0x05`/`0xFA`
//!    DoP marker alternation during stopping, idling, and reconfiguration.
//! 4. Emergency overload recovery: Instantly bypasses output to the verified DSD idle stream at the
//!    modulator boundary, resets integrators to zero, and fades pre-SRC gain back in once safe.

use audigma_dsp::DSD_IDLE_BYTE;
use audigma_dsp::config::{SampleFormat, TargetRateSpec};
use audigma_format::dop::{DopContainerFormat, DopPacker};
use audigma_settings::{OutputTransport, Settings};
use serde::{Deserialize, Serialize};

/// High-level lifecycle state of the audigma engine (`Design.md §4.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LifecycleState {
    /// Quiescent or device closed; emitting no frames or idle bitstream.
    Stopped,
    /// Device handle acquired, stream formats negotiated, buffers allocated.
    Preparing,
    /// Pre-filling hardware ring buffers with balanced idle frames to prevent startup underruns.
    Priming,
    /// Actively processing audio frames with normal or fading gain.
    Running,
    /// Planned stop or pause: ramping pre-SRC PCM to zero and flushing filter group delay.
    DrainingMuting,
    /// Output sample rate or transport format reconfiguration in progress.
    Reconfiguring,
    /// Modulator boundary tripped; emitting DSD idle stream, integrators reset, fading back in.
    Recovering,
    /// ALSA device disconnected or unrecoverable error.
    DeviceLost,
}

/// Output carrier format for idle stream generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleCarrierFormat {
    /// ALSA Native DSD (DSD_U32_BE, DSD_U16_BE, DSD_U8).
    NativeDsd,
    /// DoP v1.1 carrying DSD in PCM frames with alternating markers.
    DoP(DopContainerFormat),
    /// Standard integer/floating-point multi-bit PCM (0x00 silence).
    Pcm,
}

impl IdleCarrierFormat {
    /// Resolves the idle carrier format from the configured output settings and target rate spec.
    pub fn from_settings(settings: &Settings, target_spec: &TargetRateSpec) -> Self {
        match target_spec {
            TargetRateSpec::Dsd { .. } => {
                let transport = settings
                    .output
                    .as_ref()
                    .and_then(|o| o.transport)
                    .unwrap_or(OutputTransport::Native);
                match transport {
                    OutputTransport::Native => Self::NativeDsd,
                    OutputTransport::Dop => {
                        let fmt = settings
                            .output
                            .as_ref()
                            .and_then(|o| o.sample_format)
                            .unwrap_or(SampleFormat::S32Le);
                        let dop_fmt = match fmt {
                            SampleFormat::S32Le => DopContainerFormat::S32Le,
                            SampleFormat::S24Le => DopContainerFormat::S24Le,
                            _ => DopContainerFormat::S24_3Le,
                        };
                        Self::DoP(dop_fmt)
                    }
                    OutputTransport::Pcm => Self::Pcm,
                }
            }
            TargetRateSpec::Pcm { .. } => Self::Pcm,
        }
    }
}

/// Action indicator returned after processing a block of pre-SRC PCM frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlockAction {
    /// Normal processing: all frames were scaled by the active pre-SRC gain.
    Normal,
    /// Fade-in active: gain ramped from starting value to 1.0.
    FadingIn { current_gain: f64 },
    /// Muting drain in progress: pre-SRC PCM ramped to 0.0, group delay draining.
    Draining {
        current_gain: f64,
        drain_frames_remaining: usize,
    },
    /// Filter tail has fully cleared; pipeline is ready to transition to `Stopped` or `Reconfiguring`.
    Drained,
    /// Quiescent / muted state: input samples zeroed.
    Muted,
}

/// Controls click-free transitions, pre-SRC gain ramping, group-delay tail draining,
/// and DoP marker continuity across the entire engine lifecycle.
#[derive(Debug)]
pub struct LifecycleController {
    state: LifecycleState,
    carrier: IdleCarrierFormat,
    dop_packer: Option<DopPacker>,

    // Pre-SRC Gain Staging
    current_gain: f64,
    fade_frames: usize,
    fade_step: usize,
    is_fading_in: bool,
    is_fading_out: bool,

    // FIR Group Delay Tail Draining
    filter_group_delay_input_frames: usize,
    drain_frames_remaining: usize,

    // Overload Recovery Counter
    overload_recovery_frames: usize,
    recovery_timer: usize,
}

impl LifecycleController {
    /// Creates a new `LifecycleController`.
    ///
    /// - `carrier`: Target hardware format (`NativeDsd`, `DoP`, or `Pcm`).
    /// - `filter_group_delay_input_frames`: Group delay of the interpolation cascade in input samples.
    /// - `fade_frames`: Length of the pre-SRC smooth cosine or linear fade ramp (default ~256 samples).
    pub fn new(
        carrier: IdleCarrierFormat,
        filter_group_delay_input_frames: usize,
        fade_frames: usize,
    ) -> Self {
        let dop_packer = match carrier {
            IdleCarrierFormat::DoP(fmt) => Some(DopPacker::new(fmt)),
            _ => None,
        };

        Self {
            state: LifecycleState::Stopped,
            carrier,
            dop_packer,
            current_gain: 0.0,
            fade_frames: fade_frames.max(16),
            fade_step: 0,
            is_fading_in: false,
            is_fading_out: false,
            filter_group_delay_input_frames,
            drain_frames_remaining: 0,
            overload_recovery_frames: fade_frames.max(16) * 2,
            recovery_timer: 0,
        }
    }

    /// Returns the current lifecycle state.
    #[inline]
    pub fn state(&self) -> LifecycleState {
        self.state
    }

    /// Returns the current pre-SRC PCM volume gain factor (`0.0` = muted, `1.0` = full).
    #[inline]
    pub fn current_gain(&self) -> f64 {
        self.current_gain
    }

    /// Returns true if actively running audio.
    #[inline]
    pub fn is_running(&self) -> bool {
        self.state == LifecycleState::Running
    }

    /// Returns true if currently draining filter tail during a planned stop.
    #[inline]
    pub fn is_draining(&self) -> bool {
        self.state == LifecycleState::DrainingMuting
    }

    /// Returns true if stopped or quiescent.
    #[inline]
    pub fn is_stopped(&self) -> bool {
        self.state == LifecycleState::Stopped
    }

    /// Resets the controller back to `Stopped` state.
    pub fn reset(&mut self) {
        self.state = LifecycleState::Stopped;
        self.current_gain = 0.0;
        self.fade_step = 0;
        self.is_fading_in = false;
        self.is_fading_out = false;
        self.drain_frames_remaining = 0;
        self.recovery_timer = 0;
    }

    /// Transitions to `Preparing`.
    pub fn prepare(&mut self) -> Result<(), &'static str> {
        match self.state {
            LifecycleState::Stopped | LifecycleState::Reconfiguring => {
                self.state = LifecycleState::Preparing;
                self.current_gain = 0.0;
                self.fade_step = 0;
                self.is_fading_in = false;
                self.is_fading_out = false;
                Ok(())
            }
            _ => Err("can only prepare from Stopped or Reconfiguring"),
        }
    }

    /// Transitions from `Preparing` to `Priming`.
    pub fn prime(&mut self) -> Result<(), &'static str> {
        if self.state != LifecycleState::Preparing {
            return Err("can only prime from Preparing");
        }
        self.state = LifecycleState::Priming;
        Ok(())
    }

    /// Starts audio playback (`Priming` -> `Running`), arming the pre-SRC fade-in ramp.
    pub fn start(&mut self) -> Result<(), &'static str> {
        match self.state {
            LifecycleState::Priming | LifecycleState::Preparing | LifecycleState::Stopped => {
                self.state = LifecycleState::Running;
                self.current_gain = 0.0;
                self.fade_step = 0;
                self.is_fading_in = true;
                self.is_fading_out = false;
                Ok(())
            }
            LifecycleState::Running => Ok(()),
            _ => Err("cannot start from current state"),
        }
    }

    /// Requests a planned stop or mute: initiates pre-SRC PCM ramp-down and FIR tail draining.
    pub fn request_stop(&mut self) {
        if self.state == LifecycleState::Running || self.state == LifecycleState::Priming {
            self.state = LifecycleState::DrainingMuting;
            self.is_fading_in = false;
            self.is_fading_out = true;
            self.fade_step = 0;
            // Drain both the fade duration and the FIR group delay
            self.drain_frames_remaining = self.fade_frames + self.filter_group_delay_input_frames;
        } else if self.state != LifecycleState::DrainingMuting {
            self.state = LifecycleState::Stopped;
            self.current_gain = 0.0;
        }
    }

    /// Requests reconfiguration (e.g. rate switch): drains first, then enters `Reconfiguring`.
    pub fn request_reconfigure(&mut self) {
        if self.state == LifecycleState::Running {
            self.request_stop();
        } else {
            self.state = LifecycleState::Reconfiguring;
        }
    }

    /// Notifies the controller that an emergency overload trip occurred at the modulator boundary.
    ///
    /// Per `Design.md §4.5`:
    /// 1. Immediately clamp output to verified DSD idle pattern.
    /// 2. Reset and reprime modulator state.
    /// 3. Fade pre-SRC PCM back in once safe.
    pub fn report_overload_trip(&mut self) {
        self.state = LifecycleState::Recovering;
        self.current_gain = 0.0;
        self.is_fading_in = false;
        self.is_fading_out = false;
        self.recovery_timer = self.overload_recovery_frames;
    }

    /// Marks the device as lost (unplugged or ALSA error).
    pub fn report_device_lost(&mut self) {
        self.state = LifecycleState::DeviceLost;
        self.current_gain = 0.0;
    }

    /// Processes a block of multi-channel input PCM frames, applying smooth pre-SRC gain ramping,
    /// muting, or tail flushing across all channels.
    pub fn process_input_channels(&mut self, channels: &mut [&mut [f64]]) -> BlockAction {
        if channels.is_empty() {
            return BlockAction::Normal;
        }
        let num_frames = channels.iter().map(|s| s.len()).min().unwrap_or(0);

        match self.state {
            LifecycleState::Running => {
                if self.is_fading_in {
                    for i in 0..num_frames {
                        if self.fade_step < self.fade_frames {
                            self.fade_step += 1;
                            let t = self.fade_step as f64 / self.fade_frames as f64;
                            self.current_gain = 0.5 * (1.0 - (std::f64::consts::PI * t).cos());
                            if self.fade_step >= self.fade_frames {
                                self.current_gain = 1.0;
                                self.is_fading_in = false;
                            }
                        } else {
                            self.current_gain = 1.0;
                            self.is_fading_in = false;
                        }
                        for ch in channels.iter_mut() {
                            ch[i] *= self.current_gain;
                        }
                    }
                    BlockAction::FadingIn {
                        current_gain: self.current_gain,
                    }
                } else {
                    self.current_gain = 1.0;
                    BlockAction::Normal
                }
            }
            LifecycleState::DrainingMuting => {
                for i in 0..num_frames {
                    if self.is_fading_out {
                        if self.fade_step < self.fade_frames {
                            self.fade_step += 1;
                            let t = self.fade_step as f64 / self.fade_frames as f64;
                            self.current_gain = 0.5 * (1.0 + (std::f64::consts::PI * t).cos());
                            if self.fade_step >= self.fade_frames {
                                self.current_gain = 0.0;
                                self.is_fading_out = false;
                            }
                        } else {
                            self.current_gain = 0.0;
                            self.is_fading_out = false;
                        }
                        for ch in channels.iter_mut() {
                            ch[i] *= self.current_gain;
                        }
                    } else {
                        // After fade-out, feed absolute 0.0 into the cascade to clear the tail
                        for ch in channels.iter_mut() {
                            ch[i] = 0.0;
                        }
                    }

                    if self.drain_frames_remaining > 0 {
                        self.drain_frames_remaining -= 1;
                    }
                }

                if self.drain_frames_remaining == 0 {
                    self.state = LifecycleState::Stopped;
                    BlockAction::Drained
                } else {
                    BlockAction::Draining {
                        current_gain: self.current_gain,
                        drain_frames_remaining: self.drain_frames_remaining,
                    }
                }
            }
            LifecycleState::Recovering => {
                // While recovering, zero the input PCM
                for ch in channels.iter_mut() {
                    ch[..num_frames].fill(0.0);
                }

                if self.recovery_timer > 0 {
                    self.recovery_timer = self.recovery_timer.saturating_sub(num_frames);
                }

                if self.recovery_timer == 0 {
                    // Recovery hold finished: return to Running with fade-in
                    self.state = LifecycleState::Running;
                    self.is_fading_in = true;
                    self.fade_step = 0;
                    self.current_gain = 0.0;
                }

                BlockAction::Muted
            }
            LifecycleState::Stopped
            | LifecycleState::Preparing
            | LifecycleState::Priming
            | LifecycleState::Reconfiguring
            | LifecycleState::DeviceLost => {
                for ch in channels.iter_mut() {
                    ch[..num_frames].fill(0.0);
                }
                self.current_gain = 0.0;
                BlockAction::Muted
            }
        }
    }

    /// Processes a block of input PCM frames, applying smooth pre-SRC gain ramping,
    /// muting, or tail flushing.
    ///
    /// Slices `left` and `right` must have equal length.
    pub fn process_input_block(&mut self, left: &mut [f64], right: &mut [f64]) -> BlockAction {
        assert_eq!(left.len(), right.len());
        self.process_input_channels(&mut [left, right])
    }

    /// Generates idle / silence frames into `out_bytes` matching the active transport carrier format,
    /// ensuring zero DC popping on DACs.
    ///
    /// - For `NativeDsd`: fills with `0x69` (`DSD_IDLE_BYTE`).
    /// - For `DoP`: encodes `0x69` with uninterrupted `0x05`/`0xFA` marker alternation.
    /// - For `Pcm`: fills with `0x00`.
    pub fn emit_idle_frames(&mut self, out_bytes: &mut [u8]) -> usize {
        match self.carrier {
            IdleCarrierFormat::NativeDsd => {
                out_bytes.fill(DSD_IDLE_BYTE);
                out_bytes.len()
            }
            IdleCarrierFormat::DoP(_) => {
                if let Some(ref mut packer) = self.dop_packer {
                    let frame_bytes = packer.format().bytes_per_stereo_frame();
                    let num_frames = out_bytes.len() / frame_bytes;
                    let valid_bytes = num_frames * frame_bytes;
                    packer
                        .pack_stereo_silence_frames(num_frames, &mut out_bytes[..valid_bytes])
                        .unwrap_or(0)
                } else {
                    out_bytes.fill(0);
                    out_bytes.len()
                }
            }
            IdleCarrierFormat::Pcm => {
                out_bytes.fill(0);
                out_bytes.len()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_startup_fade_in() {
        let mut lc = LifecycleController::new(IdleCarrierFormat::NativeDsd, 64, 100);
        assert_eq!(lc.state(), LifecycleState::Stopped);

        lc.prepare().expect("prepare failed");
        assert_eq!(lc.state(), LifecycleState::Preparing);

        lc.prime().expect("prime failed");
        assert_eq!(lc.state(), LifecycleState::Priming);

        lc.start().expect("start failed");
        assert_eq!(lc.state(), LifecycleState::Running);

        let mut left = vec![1.0; 50];
        let mut right = vec![1.0; 50];
        let action = lc.process_input_block(&mut left, &mut right);

        match action {
            BlockAction::FadingIn { current_gain } => {
                assert!(current_gain > 0.0 && current_gain < 1.0);
            }
            _ => panic!("expected FadingIn action"),
        }

        // Advance past fade_frames
        let mut left2 = vec![1.0; 60];
        let mut right2 = vec![1.0; 60];
        let action2 = lc.process_input_block(&mut left2, &mut right2);
        match action2 {
            BlockAction::FadingIn { current_gain } => {
                assert_eq!(current_gain, 1.0);
            }
            _ => panic!("expected FadingIn complete"),
        }

        // Subsequent blocks are normal at unity gain
        let mut left3 = vec![1.0; 10];
        let mut right3 = vec![1.0; 10];
        assert_eq!(
            lc.process_input_block(&mut left3, &mut right3),
            BlockAction::Normal
        );
        assert_eq!(left3[0], 1.0);
    }

    #[test]
    fn test_lifecycle_draining_mute_and_stop() {
        let mut lc = LifecycleController::new(IdleCarrierFormat::NativeDsd, 30, 20);
        lc.start().expect("start failed");

        // Fast forward fade-in
        let mut l = vec![1.0; 25];
        let mut r = vec![1.0; 25];
        lc.process_input_block(&mut l, &mut r);

        // Request stop: initiates drain (20 fade frames + 30 group delay = 50 frames)
        lc.request_stop();
        assert_eq!(lc.state(), LifecycleState::DrainingMuting);

        let mut l_drain = vec![1.0; 30];
        let mut r_drain = vec![1.0; 30];
        let action = lc.process_input_block(&mut l_drain, &mut r_drain);
        match action {
            BlockAction::Draining {
                drain_frames_remaining,
                ..
            } => {
                assert_eq!(drain_frames_remaining, 20); // 50 - 30 = 20 remaining
            }
            _ => panic!("expected Draining action"),
        }

        // Drain the remaining 20 frames
        let mut l_tail = vec![1.0; 20];
        let mut r_tail = vec![1.0; 20];
        let action2 = lc.process_input_block(&mut l_tail, &mut r_tail);
        assert_eq!(action2, BlockAction::Drained);
        assert_eq!(lc.state(), LifecycleState::Stopped);
    }

    #[test]
    fn test_lifecycle_emergency_overload_recovery() {
        let mut lc = LifecycleController::new(IdleCarrierFormat::NativeDsd, 32, 20);
        lc.start().expect("start failed");

        lc.report_overload_trip();
        assert_eq!(lc.state(), LifecycleState::Recovering);

        let mut l = vec![1.0; 50];
        let mut r = vec![1.0; 50];
        let action = lc.process_input_block(&mut l, &mut r);
        assert_eq!(action, BlockAction::Muted);
        assert_eq!(l[0], 0.0);
        assert_eq!(r[0], 0.0);

        // Wait out recovery timer
        let mut l_rec = vec![1.0; 50];
        let mut r_rec = vec![1.0; 50];
        let _ = lc.process_input_block(&mut l_rec, &mut r_rec);
        assert_eq!(lc.state(), LifecycleState::Running);
    }

    #[test]
    fn test_lifecycle_dop_marker_continuity_in_idle_stream() {
        let mut lc =
            LifecycleController::new(IdleCarrierFormat::DoP(DopContainerFormat::S32Le), 64, 64);
        let mut buf = vec![0u8; 8 * 4]; // 4 stereo frames of s32le (8 bytes/frame)
        let written = lc.emit_idle_frames(&mut buf);
        assert_eq!(written, 32);

        // Markers in s32le are at byte index 3 and 7 of each 8-byte frame
        // Frame 0: marker 0x05
        assert_eq!(buf[3], 0x05);
        assert_eq!(buf[7], 0x05);
        // Frame 1: marker 0xFA
        assert_eq!(buf[11], 0xFA);
        assert_eq!(buf[15], 0xFA);
        // Frame 2: marker 0x05
        assert_eq!(buf[19], 0x05);
        assert_eq!(buf[23], 0x05);
        // Frame 3: marker 0xFA
        assert_eq!(buf[27], 0xFA);
        assert_eq!(buf[31], 0xFA);

        // Next call must continue with 0x05
        let mut buf2 = vec![0u8; 8];
        lc.emit_idle_frames(&mut buf2);
        assert_eq!(buf2[3], 0x05);
    }

    #[test]
    fn test_lifecycle_native_dsd_idle_stream_uses_0x69() {
        let mut lc = LifecycleController::new(IdleCarrierFormat::NativeDsd, 64, 64);
        let mut buf = vec![0u8; 16];
        lc.emit_idle_frames(&mut buf);
        assert!(buf.iter().all(|&b| b == 0x69));
    }
}
