//! Platform-neutral tray model: what the tray shows and offers, independent of the host
//! protocol (`Design.md` §6.1.1 `[runtime] systray`).

use audigma_dsp::DsdBitRateHz;
use audigma_engine::{DaemonOutputFormat, DaemonState, DaemonTelemetry, PauseReason};

/// An action the user can trigger from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Pause,
    Resume,
    Mute,
    Unmute,
    Stop,
}

/// One entry of the neutral tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// Read-only, disabled status line.
    Status(String),
    Separator,
    Action {
        label: &'static str,
        action: TrayAction,
        enabled: bool,
    },
}

/// The subset of daemon telemetry the tray renders. Equality drives change detection, so only
/// fields that change what is displayed belong here.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayModel {
    pub instance: String,
    pub state: DaemonState,
    pub pause_reason: Option<PauseReason>,
    pub input_rate_hz: u32,
    /// DSD bit rate of the input stream; `None` for PCM input.
    pub input_dsd_bit_rate_hz: Option<u32>,
    pub output_format: DaemonOutputFormat,
    pub output_rate_hz: u32,
    /// Volume rounded to 0.1 dB so meter jitter does not cause updates.
    pub volume_tenth_db: i32,
    /// Volume in whole percent (perceptual curve).
    pub volume_percent: u32,
    pub muted: bool,
}

/// One scroll-wheel notch in StatusNotifierItem `Scroll` delta units (Qt's angle-delta
/// convention, used by KDE). Hosts that report smaller units accumulate until a notch.
pub const SCROLL_NOTCH: i32 = 120;

/// Volume change per scroll notch, in percent.
pub const SCROLL_STEP_PERCENT: f64 = 2.0;

/// Turns raw scroll deltas into whole notches, carrying the remainder so high-resolution
/// wheels and touchpads step once per notch's worth of travel.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScrollAccumulator {
    remainder: i32,
}

impl ScrollAccumulator {
    /// Adds `delta` and returns the whole notches reached: positive for scroll up (volume
    /// up). A direction change discards the remainder of the previous direction.
    pub fn push(&mut self, delta: i32) -> i32 {
        if delta != 0 && self.remainder != 0 && (delta < 0) != (self.remainder < 0) {
            self.remainder = 0;
        }
        self.remainder = self.remainder.saturating_add(delta);
        let notches = self.remainder / SCROLL_NOTCH;
        self.remainder -= notches * SCROLL_NOTCH;
        notches
    }
}

/// Applies `notches` to `current_percent`, clamped to 0..=100 (100 % is 0 dBFS).
pub fn step_percent(current_percent: f64, notches: i32) -> f64 {
    let base = if current_percent.is_finite() {
        current_percent
    } else {
        0.0
    };
    (base + f64::from(notches) * SCROLL_STEP_PERCENT).clamp(0.0, 100.0)
}

impl TrayModel {
    pub fn from_telemetry(instance: &str, t: &DaemonTelemetry) -> Self {
        Self {
            instance: instance.to_string(),
            state: t.state,
            pause_reason: t.pause_reason,
            input_rate_hz: t.input_rate_hz,
            input_dsd_bit_rate_hz: t.input_dsd_bit_rate_hz,
            output_format: t.output_format,
            output_rate_hz: t.output_rate_hz,
            volume_tenth_db: (t.volume_db * 10.0).round() as i32,
            volume_percent: t.volume_percent.round().clamp(0.0, 100.0) as u32,
            muted: t.is_muted,
        }
    }

    /// Status line, e.g. `k17: Running - 96 kHz -> DSD256 - -12 dB`.
    pub fn status_line(&self) -> String {
        let body = match self.state {
            DaemonState::Paused => match self.pause_reason {
                Some(PauseReason::DeviceWaitExpired) => "Paused (device not found)".to_string(),
                _ => "Paused".to_string(),
            },
            DaemonState::Running => {
                let vol = if self.muted {
                    "muted".to_string()
                } else {
                    format!(
                        "{} dB ({}%)",
                        trim_decimal(f64::from(self.volume_tenth_db) / 10.0),
                        self.volume_percent
                    )
                };
                format!(
                    "Running - {} -> {} - {vol}",
                    self.input_label(),
                    self.output_label(),
                )
            }
            DaemonState::Stopped => "Stopped".to_string(),
            DaemonState::WaitingForDevice => "Waiting for device".to_string(),
            DaemonState::Preparing => "Preparing".to_string(),
            DaemonState::Priming => "Priming".to_string(),
            DaemonState::DrainingMuting => "Draining".to_string(),
            DaemonState::Reconfiguring => "Reconfiguring".to_string(),
            DaemonState::Recovering => "Recovering".to_string(),
            DaemonState::DeviceLost => "Device lost".to_string(),
        };
        format!("{}: {body}", self.instance)
    }

    /// Input side of the status line: `DSD64` for DSD input, else the PCM rate.
    fn input_label(&self) -> String {
        match self.input_dsd_bit_rate_hz.and_then(DsdBitRateHz::new) {
            Some(dsd) => dsd.short_label(),
            None => format_rate(self.input_rate_hz),
        }
    }

    /// Output side of the status line: `DSD256` for DSD, `192 kHz` for PCM.
    fn output_label(&self) -> String {
        match self.output_format {
            DaemonOutputFormat::Pcm => format_rate(self.output_rate_hz),
            DaemonOutputFormat::Dsd => DsdBitRateHz::new(self.output_rate_hz)
                .map_or_else(|| format_rate(self.output_rate_hz), |d| d.short_label()),
        }
    }

    /// Short tooltip / title text.
    pub fn title(&self) -> String {
        format!("Audigma ({})", self.instance)
    }

    /// Freedesktop icon name for the current state.
    pub fn icon_name(&self) -> &'static str {
        match self.state {
            DaemonState::Paused => "media-playback-pause",
            DaemonState::DeviceLost | DaemonState::Recovering => "dialog-warning",
            _ => "audio-card",
        }
    }

    /// Menu: status line, Mute or Unmute, Pause or Resume (by state), Stop.
    pub fn menu(&self) -> Vec<MenuEntry> {
        let stopped = self.state == DaemonState::Stopped;
        let mute = if self.muted {
            MenuEntry::Action {
                label: "Unmute",
                action: TrayAction::Unmute,
                enabled: !stopped,
            }
        } else {
            MenuEntry::Action {
                label: "Mute",
                action: TrayAction::Mute,
                enabled: !stopped,
            }
        };
        let toggle = if self.state == DaemonState::Paused {
            MenuEntry::Action {
                label: "Resume",
                action: TrayAction::Resume,
                enabled: true,
            }
        } else {
            MenuEntry::Action {
                label: "Pause",
                action: TrayAction::Pause,
                enabled: !stopped,
            }
        };
        vec![
            MenuEntry::Status(self.status_line()),
            MenuEntry::Separator,
            mute,
            toggle,
            MenuEntry::Action {
                label: "Stop",
                action: TrayAction::Stop,
                enabled: !stopped,
            },
        ]
    }
}

/// `44100` -> `44.1 kHz`, `96000` -> `96 kHz`.
fn format_rate(hz: u32) -> String {
    format!("{} kHz", trim_decimal(f64::from(hz) / 1000.0))
}

/// One decimal, without a trailing `.0`.
fn trim_decimal(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> TrayModel {
        TrayModel {
            instance: "k17".into(),
            state: DaemonState::Running,
            pause_reason: None,
            input_rate_hz: 96_000,
            input_dsd_bit_rate_hz: None,
            output_format: DaemonOutputFormat::Dsd,
            output_rate_hz: 11_289_600,
            volume_tenth_db: -120,
            volume_percent: 85,
            muted: false,
        }
    }

    #[test]
    fn running_status_line_shows_rates_and_volume() {
        assert_eq!(
            running().status_line(),
            "k17: Running - 96 kHz -> DSD256 - -12 dB (85%)"
        );
    }

    #[test]
    fn pcm_target_and_fractional_values() {
        let mut m = running();
        m.input_rate_hz = 44_100;
        m.output_format = DaemonOutputFormat::Pcm;
        m.output_rate_hz = 352_800;
        m.volume_tenth_db = -125;
        assert_eq!(
            m.status_line(),
            "k17: Running - 44.1 kHz -> 352.8 kHz - -12.5 dB (85%)"
        );
    }

    #[test]
    fn dsd_48k_family_label() {
        let mut m = running();
        m.output_rate_hz = 12_288_000;
        assert!(m.status_line().contains("DSD256"));
    }

    #[test]
    fn dsd_input_shows_dsd_rate_not_intermediate() {
        let mut m = running();
        m.input_rate_hz = 176_400;
        m.input_dsd_bit_rate_hz = Some(2_822_400);
        assert_eq!(
            m.status_line(),
            "k17: Running - DSD64 -> DSD256 - -12 dB (85%)"
        );
    }

    #[test]
    fn muted_replaces_volume() {
        let mut m = running();
        m.muted = true;
        assert_eq!(m.status_line(), "k17: Running - 96 kHz -> DSD256 - muted");
    }

    #[test]
    fn paused_status_reuses_pause_reason() {
        let mut m = running();
        m.state = DaemonState::Paused;
        assert_eq!(m.status_line(), "k17: Paused");
        m.pause_reason = Some(PauseReason::DeviceWaitExpired);
        assert_eq!(m.status_line(), "k17: Paused (device not found)");
    }

    #[test]
    fn menu_offers_pause_while_running_and_resume_while_paused() {
        let m = running();
        let menu = m.menu();
        assert_eq!(menu[0], MenuEntry::Status(m.status_line()));
        assert_eq!(menu[1], MenuEntry::Separator);
        assert_eq!(
            menu[2],
            MenuEntry::Action {
                label: "Mute",
                action: TrayAction::Mute,
                enabled: true
            }
        );
        assert_eq!(
            menu[3],
            MenuEntry::Action {
                label: "Pause",
                action: TrayAction::Pause,
                enabled: true
            }
        );
        assert_eq!(
            menu[4],
            MenuEntry::Action {
                label: "Stop",
                action: TrayAction::Stop,
                enabled: true
            }
        );

        let mut p = running();
        p.state = DaemonState::Paused;
        assert_eq!(
            p.menu()[3],
            MenuEntry::Action {
                label: "Resume",
                action: TrayAction::Resume,
                enabled: true
            }
        );
    }

    #[test]
    fn menu_offers_unmute_while_muted() {
        let mut m = running();
        m.muted = true;
        assert_eq!(
            m.menu()[2],
            MenuEntry::Action {
                label: "Unmute",
                action: TrayAction::Unmute,
                enabled: true
            }
        );
    }

    #[test]
    fn stopped_disables_actions() {
        let mut m = running();
        m.state = DaemonState::Stopped;
        for e in m.menu() {
            if let MenuEntry::Action { enabled, .. } = e {
                assert!(!enabled);
            }
        }
    }

    #[test]
    fn icon_follows_state() {
        let mut m = running();
        assert_eq!(m.icon_name(), "audio-card");
        m.state = DaemonState::Paused;
        assert_eq!(m.icon_name(), "media-playback-pause");
    }

    #[test]
    fn scroll_one_notch_is_one_step() {
        let mut a = ScrollAccumulator::default();
        assert_eq!(a.push(120), 1);
        assert_eq!(a.push(-120), -1);
    }

    #[test]
    fn scroll_partial_deltas_accumulate_and_carry() {
        let mut a = ScrollAccumulator::default();
        assert_eq!(a.push(50), 0);
        assert_eq!(a.push(50), 0);
        assert_eq!(a.push(50), 1); // 150 -> 1 notch, 30 carried
        assert_eq!(a.push(90), 1); // 120 -> 1 notch
        assert_eq!(a, ScrollAccumulator::default());
    }

    #[test]
    fn scroll_fast_flick_gives_multiple_notches() {
        let mut a = ScrollAccumulator::default();
        assert_eq!(a.push(-240), -2);
        assert_eq!(a.push(0), 0);
    }

    #[test]
    fn scroll_direction_change_drops_remainder() {
        let mut a = ScrollAccumulator::default();
        assert_eq!(a.push(100), 0);
        // Without the reset, +100 - 100 - 30 would stay under a notch.
        assert_eq!(a.push(-100), 0);
        assert_eq!(a.push(-30), -1);
    }

    #[test]
    fn step_percent_clamps() {
        assert_eq!(step_percent(50.0, 1), 52.0);
        assert_eq!(step_percent(50.0, -2), 46.0);
        assert_eq!(step_percent(99.0, 1), 100.0);
        assert_eq!(step_percent(1.0, -1), 0.0);
        assert_eq!(step_percent(f64::NAN, 1), 2.0);
    }
}
