//! Platform-neutral tray model: what the tray shows and offers, independent of the host
//! protocol (`Design.md` §6.1.1 `[runtime] systray`).

use audigma_dsp::DsdBitRateHz;
use audigma_engine::{DaemonOutputFormat, DaemonState, DaemonTelemetry, PauseReason};

/// An action the user can trigger from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Pause,
    Resume,
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
    pub muted: bool,
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
                        "{} dB",
                        trim_decimal(f64::from(self.volume_tenth_db) / 10.0)
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

    /// Menu: status line, Pause or Resume (by state), Stop.
    pub fn menu(&self) -> Vec<MenuEntry> {
        let stopped = self.state == DaemonState::Stopped;
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
            muted: false,
        }
    }

    #[test]
    fn running_status_line_shows_rates_and_volume() {
        assert_eq!(
            running().status_line(),
            "k17: Running - 96 kHz -> DSD256 - -12 dB"
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
            "k17: Running - 44.1 kHz -> 352.8 kHz - -12.5 dB"
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
        assert_eq!(m.status_line(), "k17: Running - DSD64 -> DSD256 - -12 dB");
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
                label: "Pause",
                action: TrayAction::Pause,
                enabled: true
            }
        );
        assert_eq!(
            menu[3],
            MenuEntry::Action {
                label: "Stop",
                action: TrayAction::Stop,
                enabled: true
            }
        );

        let mut p = running();
        p.state = DaemonState::Paused;
        assert_eq!(
            p.menu()[2],
            MenuEntry::Action {
                label: "Resume",
                action: TrayAction::Resume,
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
}
