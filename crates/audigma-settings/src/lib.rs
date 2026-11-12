//! `audigma-settings`: Schema v1 TOML configuration (`Settings`),
//! pipeline resolution, and architectural invariant validation (`Design.md §6`).

use audigma_dsp::{
    Apodization, DitherType, DsdMultiplier, DspConfiguration, FilterPhase, OverloadPolicy,
    PcmRateHz, RateFamily, SampleFormat, StreamSpec, TargetFormat, TargetRateSpec,
};
use audigma_format::{DopContainerFormat, DsfPadMode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "meta")]
pub mod meta;

/// Supported schema version (`schema_version = 1`).
pub const SCHEMA_VERSION_V1: u32 = 1;

/// Errors returned during TOML parsing, pipeline resolution, or cross-field validation.
#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("unsupported schema_version {got} (expected {SCHEMA_VERSION_V1})")]
    UnsupportedSchemaVersion { got: u32 },

    /// A rule on one key (or one key's relation to others). `path` is the absolute dotted config
    /// path the message is about, e.g. `output.device`, so a form can show it next to the field.
    #[error("invalid configuration at `{path}`: {message}")]
    Validation {
        path: &'static str,
        message: &'static str,
    },

    #[error("unable to resolve target rate for input rate {0}")]
    UnresolvedTargetRate(PcmRateHz),
}

impl SettingsError {
    /// A validation failure attributed to the config key `path`.
    pub const fn invalid(path: &'static str, message: &'static str) -> Self {
        Self::Validation { path, message }
    }

    /// The config key this error is about, when it is attributed to one.
    pub fn path(&self) -> Option<&'static str> {
        match self {
            Self::Validation { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// A non-fatal finding attributed to a config key (`Settings::warnings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIssue {
    /// Absolute dotted config path, e.g. `output.keep_audio_sink_ms`.
    pub path: &'static str,
    pub message: String,
}

impl std::fmt::Display for FieldIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}`: {}", self.path, self.message)
    }
}

/// `[input] backend` options (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputBackend {
    Pipewire,
}

/// `[output] backend` options (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputBackend {
    Pipewire,
    Alsa,
}

/// `[output] transport` options (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputTransport {
    Pcm,
    Native,
    Dop,
}

/// `[output] dop_sample_format` options (`Design.md §5.2`): the PCM word that carries DoP.
///
/// `Auto` is a device negotiation rule, not a byte layout: the ALSA output tries
/// [`Self::candidates`] in order and reports the word it opened, which the DoP packer then
/// uses. Without a device (offline, `-o -`) only [`Self::fixed`] applies.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DopSampleFormat {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "s32le")]
    S32Le,
    #[serde(rename = "s24le")]
    S24Le,
    #[serde(rename = "s24_3le")]
    S24_3Le,
}

impl DopSampleFormat {
    /// Order `auto` tries the DoP words in: the 24-bit words first, `s32le` last.
    ///
    /// DoP v1.1 is defined on 24-bit samples. A DAC that also offers `S32_LE` may treat it as
    /// true 32-bit PCM and never look for markers there (JadeAudio JA11: DoP over `S24_3LE`
    /// plays, over `S32_LE` it does not), so `s32le` is only the fallback for DACs that have
    /// no 24-bit format (FiiO K17: DoP over `S32_LE` plays). MPD uses the same rule.
    pub const AUTO_ORDER: [DopContainerFormat; 3] = [
        DopContainerFormat::S24_3Le,
        DopContainerFormat::S24Le,
        DopContainerFormat::S32Le,
    ];

    /// The pinned DoP word, or `None` for `auto`.
    pub const fn fixed(self) -> Option<DopContainerFormat> {
        match self {
            Self::Auto => None,
            Self::S32Le => Some(DopContainerFormat::S32Le),
            Self::S24Le => Some(DopContainerFormat::S24Le),
            Self::S24_3Le => Some(DopContainerFormat::S24_3Le),
        }
    }

    /// The DoP words to try, in order, when opening a device: all of [`Self::AUTO_ORDER`]
    /// for `auto`, else only the pinned one (no fallback).
    pub fn candidates(self) -> &'static [DopContainerFormat] {
        match self {
            Self::Auto => &Self::AUTO_ORDER,
            Self::S32Le => &[DopContainerFormat::S32Le],
            Self::S24Le => &[DopContainerFormat::S24Le],
            Self::S24_3Le => &[DopContainerFormat::S24_3Le],
        }
    }

    /// The DoP word to write when there is no device to negotiate with: the pinned word, or
    /// `s32le` for `auto`.
    pub fn without_device(self) -> DopContainerFormat {
        self.fixed().unwrap_or_default()
    }
}

/// Offline output container (`Design.md §5.2–§5.3`). Not a config key: `audigma convert`
/// derives it from the output file extension (`-o -` is raw).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputContainer {
    Dsf,
    Wav,
    Flac,
    Raw,
}

impl OutputContainer {
    /// Infers output container from a file extension or path (case-insensitive).
    pub fn from_extension(ext: &str) -> Option<Self> {
        let clean = ext.trim_start_matches('.').to_ascii_lowercase();
        match clean.as_str() {
            "dsf" => Some(Self::Dsf),
            "wav" | "wave" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "raw" => Some(Self::Raw),
            _ => None,
        }
    }

    /// Infers output container from a file path.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }

    /// Lower-case name, as used in messages and file extensions.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dsf => "dsf",
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Raw => "raw",
        }
    }

    /// Checks that `target` can be written to this container.
    pub fn check_target(self, target: &TargetConfig) -> Result<(), SettingsError> {
        match self {
            Self::Dsf if target.format != TargetFormat::Dsd => Err(SettingsError::invalid(
                "target.format",
                "a .dsf output requires format = \"dsd\"",
            )),
            Self::Wav if target.format != TargetFormat::Pcm => Err(SettingsError::invalid(
                "target.format",
                "a .wav output requires format = \"pcm\"",
            )),
            Self::Flac if target.format != TargetFormat::Pcm => Err(SettingsError::invalid(
                "target.format",
                "a .flac output requires format = \"pcm\"",
            )),
            Self::Flac
                if matches!(
                    target.sample_format,
                    Some(SampleFormat::F32 | SampleFormat::F64)
                ) =>
            {
                Err(SettingsError::invalid(
                    "target.sample_format",
                    "FLAC carries integer PCM only; use s16le, s24le, s24_3le or s32le (or leave it unset for 32 bit)",
                ))
            }
            _ => Ok(()),
        }
    }

    /// Returns the canonical target format associated with this container, if fixed.
    pub const fn target_format(self) -> Option<TargetFormat> {
        match self {
            Self::Dsf => Some(TargetFormat::Dsd),
            Self::Wav | Self::Flac => Some(TargetFormat::Pcm),
            Self::Raw => None,
        }
    }
}

/// `[target] rate_family` options (`Design.md §1.1`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TargetRateFamily {
    #[default]
    #[serde(rename = "source", alias = "source-family")]
    Source,
    #[serde(rename = "44100")]
    Hz44100,
    #[serde(rename = "48000")]
    Hz48000,
}

impl TargetRateFamily {
    /// Resolves the concrete clock family for a given input clock family.
    #[inline]
    pub const fn resolve_family(self, input_family: RateFamily) -> RateFamily {
        match self {
            Self::Source => match input_family {
                RateFamily::Hz44100 => RateFamily::Hz44100,
                RateFamily::Hz48000 | RateFamily::Other => RateFamily::Hz48000,
            },
            Self::Hz44100 => RateFamily::Hz44100,
            Self::Hz48000 => RateFamily::Hz48000,
        }
    }
}

impl std::fmt::Display for TargetRateFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Source => write!(f, "source"),
            Self::Hz44100 => write!(f, "44100"),
            Self::Hz48000 => write!(f, "48000"),
        }
    }
}

/// `[gain] peak_analysis` options (`Design.md §4.4`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PeakAnalysisMode {
    #[default]
    #[serde(rename = "single-pass")]
    SinglePass,
    #[serde(rename = "two-pass")]
    TwoPass,
}

/// `[resampler] drift_correction` options (`Design.md §1.5`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriftCorrection {
    /// Correct a free-running input on ALSA output (the DAC clock is master); otherwise off.
    #[default]
    Auto,
    /// Correct a free-running input; observe (estimate only) a demand-driven one.
    On,
    Off,
}

/// `[offline] tail_policy` options (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TailPolicy {
    #[default]
    #[serde(rename = "trim-to-duration")]
    TrimToDuration,
    #[serde(rename = "preserve-tail")]
    PreserveTail,
}

/// `[input] rate_mode` options (`Design.md §3.2`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateMode {
    #[default]
    Dynamic,
    Fixed,
}

/// `[output] device_wait_expiry` options: what happens when `device_wait_ms` runs out.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceWaitExpiry {
    /// Exit the process.
    #[default]
    Exit,
    /// Release everything and wait in the paused state for `audigma resume`.
    Pause,
}

/// `[output] volume_control` options (`Design.md §1.8`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VolumeControlMode {
    #[default]
    #[serde(rename = "hardware-software-fallback")]
    HardwareSoftwareFallback,
    #[serde(rename = "software")]
    Software,
    #[serde(rename = "hardware")]
    Hardware,
    #[serde(rename = "none")]
    None,
}

/// Recognized standard SPA audio channel position tokens (`Design.md §3.4`).
pub const RECOGNIZED_CHANNEL_POSITIONS: &[&str] = &[
    "FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR", "MONO", "RC", "FLC", "FRC", "UNKNOWN",
];

/// Checks whether a channel position token is recognized by PipeWire / SPA.
pub fn is_valid_channel_position(token: &str) -> bool {
    let upper = token.to_ascii_uppercase();
    RECOGNIZED_CHANNEL_POSITIONS
        .iter()
        .any(|&p| p == upper || upper.starts_with("UNKNOWN"))
}

/// `[input] dsd` options (`Design.md §1.6`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DsdInputConfig {
    pub max_intermediate_rate_hz: Option<PcmRateHz>,
    pub gain_db: Option<f64>,
}

impl DsdInputConfig {
    /// Resolved decimation normalization gain in decibels (default: +6.0 dB,
    /// restoring Scarlet Book 50% modulation / 0 dB SACD to 0 dBFS PCM).
    pub fn resolved_gain_db(&self) -> f64 {
        self.gain_db.unwrap_or(6.0)
    }

    /// Resolved linear gain multiplier corresponding to `resolved_gain_db`.
    pub fn resolved_linear_gain(&self) -> f64 {
        10.0_f64.powf(self.resolved_gain_db() / 20.0)
    }
}

/// `[input]` section (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    pub backend: Option<InputBackend>,
    pub node_name: Option<String>,
    pub sample_format: Option<SampleFormat>,
    pub rate_hz: Option<PcmRateHz>,
    pub channels: Option<u8>,
    pub channel_positions: Option<Vec<String>>,
    pub rate_mode: Option<RateMode>,
    pub allowed_rates: Option<Vec<u32>>,
    pub allow_dsd: Option<bool>,
    pub dsd: Option<DsdInputConfig>,
}

impl InputConfig {
    /// Returns the active input backend (defaults to `pipewire`, the only live backend).
    pub fn backend(&self) -> InputBackend {
        self.backend.unwrap_or(InputBackend::Pipewire)
    }

    /// Returns the resolved channel positions for this input, strictly preserving
    /// user-declared order if specified, or synthesizing standard defaults from `channels`.
    pub fn resolved_channel_positions(&self) -> Vec<String> {
        if let Some(pos) = &self.channel_positions {
            return pos.clone();
        }
        let ch = self.channels.unwrap_or(2);
        match ch {
            1 => vec!["MONO".to_string()],
            2 => vec!["FL".to_string(), "FR".to_string()],
            4 => vec![
                "FL".to_string(),
                "FR".to_string(),
                "RL".to_string(),
                "RR".to_string(),
            ],
            6 => vec![
                "FL".to_string(),
                "FR".to_string(),
                "FC".to_string(),
                "LFE".to_string(),
                "RL".to_string(),
                "RR".to_string(),
            ],
            8 => vec![
                "FL".to_string(),
                "FR".to_string(),
                "FC".to_string(),
                "LFE".to_string(),
                "RL".to_string(),
                "RR".to_string(),
                "SL".to_string(),
                "SR".to_string(),
            ],
            n => (1..=n).map(|i| format!("UNKNOWN_{i}")).collect(),
        }
    }

    /// Returns the default list of allowed rates for dynamic multi-rate sinks.
    pub fn default_allowed_rates() -> Vec<u32> {
        vec![
            44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000,
        ]
    }

    /// Returns resolved allowed rates for dynamic multi-rate negotiation.
    pub fn resolved_allowed_rates(&self) -> Vec<u32> {
        self.allowed_rates
            .clone()
            .unwrap_or_else(Self::default_allowed_rates)
    }

    /// Returns the active rate mode (defaults to `RateMode::Dynamic`).
    pub fn rate_mode(&self) -> RateMode {
        self.rate_mode.unwrap_or_default()
    }

    /// Returns whether DSD format ingestion is allowed (defaults to false).
    pub fn allow_dsd(&self) -> bool {
        self.allow_dsd.unwrap_or(false)
    }

    /// Returns the configured maximum intermediate PCM rate for DSD decimation, if set.
    pub fn dsd_max_intermediate_rate_hz(&self) -> Option<PcmRateHz> {
        self.dsd.as_ref().and_then(|d| d.max_intermediate_rate_hz)
    }

    /// Returns the resolved DSD decimation gain in dB (defaults to +6.0 dB).
    pub fn dsd_gain_db(&self) -> f64 {
        self.dsd
            .as_ref()
            .map(|d| d.resolved_gain_db())
            .unwrap_or(6.0)
    }

    /// Returns the resolved DSD decimation linear gain multiplier (defaults to 10^(6/20) ~= 1.99526).
    pub fn dsd_linear_gain(&self) -> f64 {
        10.0_f64.powf(self.dsd_gain_db() / 20.0)
    }
}

/// `[output]` section (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub backend: Option<OutputBackend>,
    pub target_node: Option<String>,
    pub device: Option<String>,
    pub transport: Option<OutputTransport>,
    pub dop_sample_format: Option<DopSampleFormat>,
    pub reserve_device: Option<bool>,
    pub buffer_ms: Option<u32>,
    pub period_ms: Option<u32>,
    pub volume_control: Option<VolumeControlMode>,
    pub device_wait_ms: Option<i64>,
    pub device_wait_expiry: Option<DeviceWaitExpiry>,
    pub keep_audio_sink_ms: Option<i64>,
    pub mixer_control: Option<String>,
}

impl OutputConfig {
    /// Returns whether D-Bus `org.freedesktop.ReserveDevice1` reservation is enabled (`true` by default for ALSA).
    pub fn reserve_device_enabled(&self) -> bool {
        self.reserve_device
            .unwrap_or(self.backend == Some(OutputBackend::Alsa))
    }

    /// Returns the requested DoP word (`auto` by default).
    pub fn dop_sample_format(&self) -> DopSampleFormat {
        self.dop_sample_format.unwrap_or_default()
    }

    /// Returns the active volume control strategy (`VolumeControlMode::HardwareSoftwareFallback` by default).
    pub fn volume_control(&self) -> VolumeControlMode {
        self.volume_control.unwrap_or_default()
    }

    /// Returns device wait timeout in milliseconds (`-1` = wait indefinitely by default).
    pub fn device_wait_ms(&self) -> i64 {
        self.device_wait_ms.unwrap_or(-1)
    }

    /// Returns what happens when the device wait runs out (`exit` by default).
    pub fn device_wait_expiry(&self) -> DeviceWaitExpiry {
        self.device_wait_expiry.unwrap_or_default()
    }

    /// Returns audio sink keep-alive duration in milliseconds (`5000` ms by default).
    pub fn keep_audio_sink_ms(&self) -> i64 {
        self.keep_audio_sink_ms.unwrap_or(5000)
    }

    /// Returns optional user-configured mixer control element name (e.g. `"Master"`, `"PCM"`).
    pub fn mixer_control(&self) -> Option<&str> {
        self.mixer_control.as_deref()
    }
}

/// `[target]` section (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    pub format: TargetFormat,
    #[serde(default)]
    pub rate_family: TargetRateFamily,
    pub pcm_rate_hz: Option<PcmRateHz>,
    pub dsd_rate: Option<DsdMultiplier>,
    pub sample_format: Option<SampleFormat>,
    pub dither: Option<DitherType>,
    pub dither_tpdf_seed: Option<u64>,
}

/// `[gain]` section (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GainConfig {
    #[serde(default)]
    pub preamp_db: f64,
    #[serde(default)]
    pub peak_analysis: PeakAnalysisMode,
    #[serde(default)]
    pub overload_policy: OverloadPolicy,
    /// Modulator safety margin `M` in dB: 0, 0.25 or 0.5. Applies to DSD targets only; PCM targets use `M = 0` (`Design.md §4.4`).
    #[serde(default = "default_dsd_safety_margin_db")]
    pub dsd_safety_margin_db: f64,
}

const fn default_dsd_safety_margin_db() -> f64 {
    audigma_dsp::DEFAULT_SAFETY_MARGIN_DB
}

impl Default for GainConfig {
    fn default() -> Self {
        Self {
            preamp_db: 0.0,
            peak_analysis: PeakAnalysisMode::SinglePass,
            overload_policy: OverloadPolicy::MuteRecover,
            dsd_safety_margin_db: default_dsd_safety_margin_db(),
        }
    }
}

/// `[resampler]` section (`Design.md §4.2`, `§6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResamplerConfig {
    #[serde(default = "default_filter_phase")]
    pub phase: FilterPhase,
    #[serde(default = "default_apodization")]
    pub apodization: Apodization,
    #[serde(default = "default_modulator_order")]
    pub modulator_order: u8,
    /// Input-side drift correction against the output clock (`Design.md §1.5`).
    #[serde(default)]
    pub drift_correction: DriftCorrection,
    /// Largest drift correction in ppm (default 500); only used while drift correction is active.
    pub max_clock_drift_ppm: Option<u32>,
}

const fn default_filter_phase() -> FilterPhase {
    FilterPhase::Linear
}

const fn default_apodization() -> Apodization {
    Apodization::Off
}

const fn default_modulator_order() -> u8 {
    5
}

impl Default for ResamplerConfig {
    fn default() -> Self {
        Self {
            phase: FilterPhase::Linear,
            apodization: Apodization::Off,
            modulator_order: 5,
            drift_correction: DriftCorrection::Auto,
            max_clock_drift_ppm: None,
        }
    }
}

/// `[offline]` section (`Design.md §6.2`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineConfig {
    pub tail_policy: Option<TailPolicy>,
    pub write_report: Option<bool>,
    #[serde(default)]
    pub dsf_pad_mode: DsfPadMode,
}

/// Default `[resampler] max_clock_drift_ppm`.
pub const DEFAULT_MAX_CLOCK_DRIFT_PPM: u32 = 500;

/// Default aggregation interval for DSP deadline-miss records.
pub const DEFAULT_LATE_TILE_REPORT_INTERVAL_MS: u64 = 10_000;

/// Lower bound for the deadline-miss aggregation interval.
///
/// A zero interval would turn the aggregator into a busy loop, so values below
/// this bound are clamped when the pipeline reads the setting.
pub const MIN_LATE_TILE_REPORT_INTERVAL_MS: u64 = 100;

/// Lowest accepted `[runtime] realtime_priority` (loop = p, pw = p-1, DSP = p-2 must stay >= 1).
pub const MIN_REALTIME_PRIORITY: u8 = 3;
/// Highest accepted `[runtime] realtime_priority`.
pub const MAX_REALTIME_PRIORITY: u8 = 99;
/// Default `[runtime] realtime_priority` (matches RTKit's default maximum).
pub const DEFAULT_REALTIME_PRIORITY: u8 = 20;

/// `[runtime] dsp_threads`: `"auto"`, `"inline"` or an integer >= 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DspThreads {
    #[default]
    Auto,
    /// No worker threads: all channels are processed sequentially on the loop thread.
    Inline,
    Count(u32),
}

/// `"auto" | "inline" | integer >= 1`; mirrors the hand-written `Deserialize` impl.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for DspThreads {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DspThreads".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [
                { "enum": ["auto", "inline"] },
                { "type": "integer", "minimum": 1 }
            ]
        })
    }
}

impl Serialize for DspThreads {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Inline => serializer.serialize_str("inline"),
            Self::Count(n) => serializer.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for DspThreads {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DspThreads;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"auto\", \"inline\" or an integer >= 1")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<DspThreads, E> {
                match v {
                    "auto" => Ok(DspThreads::Auto),
                    "inline" => Ok(DspThreads::Inline),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(v), &self)),
                }
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<DspThreads, E> {
                match u32::try_from(v) {
                    Ok(n) if n >= 1 => Ok(DspThreads::Count(n)),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Unsigned(v), &self)),
                }
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<DspThreads, E> {
                u64::try_from(v)
                    .map_err(|_| E::invalid_value(serde::de::Unexpected::Signed(v), &self))
                    .and_then(|u| self.visit_u64(u))
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// `[runtime] realtime`: RT scheduling policy.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeMode {
    /// Try RT scheduling; warn and continue at normal priority on failure.
    #[default]
    Auto,
    /// Startup fails when RT scheduling cannot be acquired.
    Required,
    /// Never request RT scheduling.
    Off,
}

/// `[runtime] systray`: whether the daemon shows a system tray icon.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystrayMode {
    /// No tray icon.
    #[default]
    Off,
    /// Show a tray icon; warn and continue without one when no tray host is available.
    Auto,
    /// Startup fails when the tray icon cannot be created.
    Required,
}

/// `[runtime] core_selection`: which cores DSP workers prefer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreSelection {
    /// Fastest cores first.
    #[default]
    Performance,
    /// Slowest cores first.
    Efficient,
    /// No automatic pinning; the OS scheduler decides.
    Os,
}

/// `[runtime] smt_siblings`: tolerance for placing DSP workers on SMT siblings.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtSiblings {
    /// Use siblings only once all allowed physical cores are taken.
    #[default]
    Avoid,
    /// Never share a physical core between DSP workers; fail at startup if impossible.
    Never,
}

/// `[runtime]` section: threading, core placement and RT scheduling.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RuntimeConfig {
    pub dsp_threads: DspThreads,
    pub realtime: RealtimeMode,
    pub realtime_priority: u8,
    pub core_selection: CoreSelection,
    pub smt_siblings: SmtSiblings,
    /// Explicit CPU for the `audigma-loop` thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_cpu: Option<u32>,
    /// Explicit CPUs for DSP workers (one per worker).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsp_cpus: Option<Vec<u32>>,
    /// System tray icon for `run`; fixed at process start.
    pub systray: SystrayMode,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            dsp_threads: DspThreads::Auto,
            realtime: RealtimeMode::Auto,
            realtime_priority: DEFAULT_REALTIME_PRIORITY,
            core_selection: CoreSelection::Performance,
            smt_siblings: SmtSiblings::Avoid,
            loop_cpu: None,
            dsp_cpus: None,
            systray: SystrayMode::Off,
        }
    }
}

impl RuntimeConfig {
    /// Cross-field checks that do not need the machine.
    fn validate(&self) -> Result<(), SettingsError> {
        if !(MIN_REALTIME_PRIORITY..=MAX_REALTIME_PRIORITY).contains(&self.realtime_priority) {
            return Err(SettingsError::invalid(
                "runtime.realtime_priority",
                "must be in 3..=99",
            ));
        }
        if let DspThreads::Count(0) = self.dsp_threads {
            return Err(SettingsError::invalid(
                "runtime.dsp_threads",
                "must be \"auto\", \"inline\" or >= 1",
            ));
        }
        if self.dsp_threads == DspThreads::Inline && self.dsp_cpus.is_some() {
            return Err(SettingsError::invalid(
                "runtime.dsp_cpus",
                "cannot be combined with dsp_threads = \"inline\"",
            ));
        }
        if let Some(cpus) = &self.dsp_cpus {
            if cpus.is_empty() {
                return Err(SettingsError::invalid(
                    "runtime.dsp_cpus",
                    "must not be empty",
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            if !cpus.iter().all(|c| seen.insert(*c)) {
                return Err(SettingsError::invalid(
                    "runtime.dsp_cpus",
                    "must not contain duplicates",
                ));
            }
            if let DspThreads::Count(n) = self.dsp_threads
                && usize::try_from(n).ok() != Some(cpus.len())
            {
                return Err(SettingsError::invalid(
                    "runtime.dsp_cpus",
                    "length must equal dsp_threads",
                ));
            }
            if let Some(loop_cpu) = self.loop_cpu
                && cpus.contains(&loop_cpu)
            {
                return Err(SettingsError::invalid(
                    "runtime.loop_cpu",
                    "must not be in dsp_cpus",
                ));
            }
        }
        Ok(())
    }

    /// Worker count implied by explicit settings, if any: `0` for inline, `dsp_threads` when
    /// numeric, else the `dsp_cpus` length capped at `channels`. `None` means fully automatic.
    pub fn explicit_worker_count(&self, channels: u8) -> Option<usize> {
        match (self.dsp_threads, &self.dsp_cpus) {
            (DspThreads::Inline, _) => Some(0),
            (DspThreads::Count(n), _) => Some(n as usize),
            (DspThreads::Auto, Some(cpus)) => Some(cpus.len().min(usize::from(channels))),
            (DspThreads::Auto, None) => None,
        }
    }
}

/// `[logging]` options.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    pub format: Option<String>,
    pub level: Option<String>,
    /// Aggregation interval for `audigma::late` episode slices, in milliseconds.
    pub rt_report_interval_ms: Option<u64>,
}

impl LoggingConfig {
    /// Returns the configured late-tile report interval in milliseconds, clamped to a
    /// sane lower bound and defaulting to 10 seconds.
    pub fn rt_report_interval_ms(&self) -> u64 {
        self.rt_report_interval_ms
            .unwrap_or(DEFAULT_LATE_TILE_REPORT_INTERVAL_MS)
            .max(MIN_LATE_TILE_REPORT_INTERVAL_MS)
    }
}

/// Top-level `Schema v1` configuration (`Design.md §6`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub input: Option<InputConfig>,
    pub output: Option<OutputConfig>,
    pub target: Option<TargetConfig>,
    pub gain: Option<GainConfig>,
    pub resampler: Option<ResamplerConfig>,
    pub offline: Option<OfflineConfig>,
    pub logging: Option<LoggingConfig>,
    pub runtime: Option<RuntimeConfig>,
}

/// Concrete resolution produced by `Settings::explain_for_input_rate` (`audigma config explain`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPipelinePlan {
    pub stream_spec: StreamSpec,
    pub dsp_config: DspConfiguration,
}

impl Settings {
    /// Parses a TOML configuration string and validates all `Schema v1` invariants.
    pub fn from_toml_str(toml_str: &str) -> Result<Self, SettingsError> {
        let settings: Self = toml::from_str(toml_str)?;
        settings.validate()?;
        Ok(settings)
    }

    /// Like [`Self::from_toml_str`] but checks only the shape (known keys, value types, enum
    /// values), not the cross-field rules. For editors that pass through states `validate()`
    /// rejects (`backend = "alsa"` before `device` is set); call [`Self::validate`] before use.
    pub fn from_toml_str_unvalidated(toml_str: &str) -> Result<Self, SettingsError> {
        Ok(toml::from_str(toml_str)?)
    }

    /// Builds and validates settings from an already parsed (and possibly layered) TOML table.
    pub fn from_toml_table(table: toml::Table) -> Result<Self, SettingsError> {
        let settings = Self::from_toml_table_unvalidated(table)?;
        settings.validate()?;
        Ok(settings)
    }

    /// Shape-only counterpart of [`Self::from_toml_table`]; see [`Self::from_toml_str_unvalidated`].
    pub fn from_toml_table_unvalidated(table: toml::Table) -> Result<Self, SettingsError> {
        Ok(toml::Value::Table(table).try_into()?)
    }

    /// Returns default offline conversion settings tailored for the given output container.
    pub fn default_offline(container: OutputContainer) -> Self {
        match container {
            OutputContainer::Dsf => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: None,
                    node_name: None,
                    sample_format: None,
                    rate_hz: None,
                    channels: None,
                    channel_positions: None,
                    rate_mode: None,
                    allowed_rates: None,
                    allow_dsd: None,
                    dsd: None,
                }),
                output: Some(OutputConfig {
                    backend: None,
                    target_node: None,
                    device: None,
                    transport: None,
                    dop_sample_format: None,
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    device_wait_expiry: None,
                    keep_audio_sink_ms: None,
                    mixer_control: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Dsd,
                    rate_family: TargetRateFamily::Hz44100,
                    pcm_rate_hz: None,
                    dsd_rate: Some(DsdMultiplier::Dsd256),
                    sample_format: None,
                    dither: None,
                    dither_tpdf_seed: Some(12345),
                }),
                gain: Some(GainConfig {
                    preamp_db: 0.0,
                    peak_analysis: PeakAnalysisMode::TwoPass,
                    overload_policy: OverloadPolicy::Error,
                    dsd_safety_margin_db: 0.25,
                }),
                resampler: Some(ResamplerConfig {
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                    modulator_order: 5,
                    ..Default::default()
                }),
                offline: Some(OfflineConfig {
                    tail_policy: Some(TailPolicy::TrimToDuration),
                    write_report: Some(false),
                    dsf_pad_mode: DsfPadMode::SpecZero,
                }),
                logging: None,
                runtime: None,
            },
            OutputContainer::Wav => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: None,
                    node_name: None,
                    sample_format: None,
                    rate_hz: None,
                    channels: None,
                    channel_positions: None,
                    rate_mode: None,
                    allowed_rates: None,
                    allow_dsd: None,
                    dsd: None,
                }),
                output: Some(OutputConfig {
                    backend: None,
                    target_node: None,
                    device: None,
                    transport: None,
                    dop_sample_format: None,
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    device_wait_expiry: None,
                    keep_audio_sink_ms: None,
                    mixer_control: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Pcm,
                    rate_family: TargetRateFamily::Hz48000,
                    pcm_rate_hz: Some(PcmRateHz::PCM_192000),
                    dsd_rate: None,
                    sample_format: Some(SampleFormat::S32Le),
                    dither: Some(DitherType::Tpdf),
                    dither_tpdf_seed: None,
                }),
                gain: Some(GainConfig {
                    preamp_db: -3.0,
                    peak_analysis: PeakAnalysisMode::SinglePass,
                    overload_policy: OverloadPolicy::Error,
                    dsd_safety_margin_db: 0.25,
                }),
                resampler: Some(ResamplerConfig {
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                    modulator_order: 5,
                    ..Default::default()
                }),
                offline: Some(OfflineConfig {
                    tail_policy: Some(TailPolicy::TrimToDuration),
                    write_report: Some(false),
                    dsf_pad_mode: DsfPadMode::SpecZero,
                }),
                logging: None,
                runtime: None,
            },
            OutputContainer::Flac => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: None,
                    node_name: None,
                    sample_format: None,
                    rate_hz: None,
                    channels: None,
                    channel_positions: None,
                    rate_mode: None,
                    allowed_rates: None,
                    allow_dsd: None,
                    dsd: None,
                }),
                output: Some(OutputConfig {
                    backend: None,
                    target_node: None,
                    device: None,
                    transport: None,
                    dop_sample_format: None,
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    device_wait_expiry: None,
                    keep_audio_sink_ms: None,
                    mixer_control: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Pcm,
                    rate_family: TargetRateFamily::Hz48000,
                    pcm_rate_hz: Some(PcmRateHz::PCM_96000),
                    dsd_rate: None,
                    sample_format: Some(SampleFormat::S24Le),
                    dither: Some(DitherType::Tpdf),
                    dither_tpdf_seed: None,
                }),
                gain: Some(GainConfig {
                    preamp_db: -3.0,
                    peak_analysis: PeakAnalysisMode::SinglePass,
                    overload_policy: OverloadPolicy::Error,
                    dsd_safety_margin_db: 0.25,
                }),
                resampler: Some(ResamplerConfig {
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                    modulator_order: 5,
                    ..Default::default()
                }),
                offline: Some(OfflineConfig {
                    tail_policy: Some(TailPolicy::TrimToDuration),
                    write_report: Some(false),
                    dsf_pad_mode: DsfPadMode::SpecZero,
                }),
                logging: None,
                runtime: None,
            },
            OutputContainer::Raw => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: None,
                    node_name: None,
                    sample_format: Some(SampleFormat::S16Le),
                    rate_hz: Some(PcmRateHz::PCM_44100),
                    channels: Some(2),
                    channel_positions: None,
                    rate_mode: None,
                    allowed_rates: None,
                    allow_dsd: None,
                    dsd: None,
                }),
                output: Some(OutputConfig {
                    backend: None,
                    target_node: None,
                    device: None,
                    transport: Some(OutputTransport::Dop),
                    dop_sample_format: Some(DopSampleFormat::S24_3Le),
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    device_wait_expiry: None,
                    keep_audio_sink_ms: None,
                    mixer_control: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Dsd,
                    rate_family: TargetRateFamily::Hz44100,
                    pcm_rate_hz: None,
                    dsd_rate: Some(DsdMultiplier::Dsd128),
                    sample_format: None,
                    dither: None,
                    dither_tpdf_seed: None,
                }),
                gain: Some(GainConfig {
                    preamp_db: 0.0,
                    peak_analysis: PeakAnalysisMode::SinglePass,
                    overload_policy: OverloadPolicy::Error,
                    dsd_safety_margin_db: 0.25,
                }),
                resampler: Some(ResamplerConfig {
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                    modulator_order: 5,
                    ..Default::default()
                }),
                offline: None,
                logging: None,
                runtime: None,
            },
        }
    }

    /// Validates cross-field architectural invariants from `Design.md §1`, `§5.3`, and `§6`.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.schema_version != SCHEMA_VERSION_V1 {
            return Err(SettingsError::UnsupportedSchemaVersion {
                got: self.schema_version,
            });
        }

        if let Some(runtime) = &self.runtime {
            runtime.validate()?;
        }

        if self.target.is_none() {
            return Err(SettingsError::invalid(
                "target",
                "the configuration must specify [target]",
            ));
        }

        if let Some(gain) = &self.gain {
            if ![0.0, 0.25, 0.5].contains(&gain.dsd_safety_margin_db) {
                return Err(SettingsError::invalid(
                    "gain.dsd_safety_margin_db",
                    "must be 0, 0.25 or 0.5",
                ));
            }
        }

        if let Some(resampler) = &self.resampler {
            if resampler.modulator_order != 5 && resampler.modulator_order != 7 {
                return Err(SettingsError::invalid(
                    "resampler.modulator_order",
                    "must be 5 or 7",
                ));
            }
        }

        // Validate [input] channel positions and stdio constraints (Design.md §3.4 & §5.3)
        if let Some(input) = &self.input {
            if let Some(positions) = &input.channel_positions {
                if let Some(ch) = input.channels {
                    if positions.len() != ch as usize {
                        return Err(SettingsError::invalid(
                            "input.channel_positions",
                            "length must strictly match channels",
                        ));
                    }
                }
                for pos in positions {
                    if !is_valid_channel_position(pos) {
                        return Err(SettingsError::invalid(
                            "input.channel_positions",
                            "contains an unrecognized channel position token",
                        ));
                    }
                }
            }

            if let Some(ref dsd) = input.dsd {
                if let Some(gain_db) = dsd.gain_db {
                    if !(-30.0..=30.0).contains(&gain_db) || gain_db.is_nan() {
                        return Err(SettingsError::invalid(
                            "input.dsd.gain_db",
                            "must be between -30.0 and +30.0 dB",
                        ));
                    }
                }
            }
        }

        // Validate [gain] ranges against the representable volume span.
        // This mirrors `audigma set volume`, so a config file cannot request gain
        // the volume curve cannot represent (and `play -v 80` fails loudly too).
        if let Some(gain) = &self.gain {
            if !gain.preamp_db.is_finite()
                || !(audigma_dsp::MIN_VOLUME_DB..=audigma_dsp::MAX_VOLUME_DB)
                    .contains(&gain.preamp_db)
            {
                return Err(SettingsError::invalid(
                    "gain.preamp_db",
                    "must be between -120.0 and +12.0 dB",
                ));
            }
        }

        // Validate [output] & [target] cross-field constraints (Design.md §1.4 & §5.3)
        if let Some(output) = &self.output {
            if let Some(wait) = output.device_wait_ms {
                if wait < -1 {
                    return Err(SettingsError::invalid(
                        "output.device_wait_ms",
                        "must be >= -1",
                    ));
                }
            }
            if let Some(keep) = output.keep_audio_sink_ms {
                if keep < -1 {
                    return Err(SettingsError::invalid(
                        "output.keep_audio_sink_ms",
                        "must be >= -1",
                    ));
                }
            }

            // Exclusive ALSA needs an explicit hardware device (no implicit "default").
            if output.backend == Some(OutputBackend::Alsa)
                && output.device.as_deref().is_none_or(|d| d.trim().is_empty())
            {
                return Err(SettingsError::invalid(
                    "output.device",
                    "is required when backend = \"alsa\"",
                ));
            }

            // DSD output must never return to PipeWire normal PCM graph (Design.md §1.4)
            if let Some(target) = &self.target {
                if target.format == TargetFormat::Dsd
                    && output.backend == Some(OutputBackend::Pipewire)
                {
                    return Err(SettingsError::invalid(
                        "output.backend",
                        "cannot be \"pipewire\" when [target] format = \"dsd\"; DSD output needs exclusive ALSA",
                    ));
                }
            }
        }

        // Validate [target] rate fields
        if let Some(target) = &self.target {
            match target.format {
                TargetFormat::Pcm => {
                    if target.pcm_rate_hz.is_none() {
                        return Err(SettingsError::invalid(
                            "target.pcm_rate_hz",
                            "is required when format = \"pcm\"",
                        ));
                    }
                }
                TargetFormat::Dsd => {
                    if target.dsd_rate.is_none() {
                        return Err(SettingsError::invalid(
                            "target.dsd_rate",
                            "is required when format = \"dsd\"",
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns `[input].channels` (default 2).
    pub fn input_channels(&self) -> u8 {
        self.input.as_ref().and_then(|i| i.channels).unwrap_or(2)
    }

    /// Returns the `[runtime]` section, or defaults when absent.
    pub fn runtime_config(&self) -> RuntimeConfig {
        self.runtime.clone().unwrap_or_default()
    }

    /// Returns the configured DSP deadline-miss aggregation interval.
    pub fn rt_report_interval(&self) -> std::time::Duration {
        let ms = self
            .logging
            .as_ref()
            .map(|l| l.rt_report_interval_ms())
            .unwrap_or(DEFAULT_LATE_TILE_REPORT_INTERVAL_MS);
        std::time::Duration::from_millis(ms)
    }

    /// Returns any non-fatal configuration warnings (e.g. timeout inconsistencies).
    pub fn warnings(&self) -> Vec<FieldIssue> {
        let mut warnings = Vec::new();
        if let Some(output) = &self.output {
            let wait = output.device_wait_ms();
            let keep = output.keep_audio_sink_ms();
            if wait > 0 && keep > wait {
                warnings.push(FieldIssue {
                    path: "output.keep_audio_sink_ms",
                    message: format!(
                        "{keep} ms is greater than device_wait_ms ({wait} ms); daemon will {} before sink grace period expires",
                        match output.device_wait_expiry() {
                            DeviceWaitExpiry::Exit => "exit",
                            DeviceWaitExpiry::Pause => "pause",
                        }
                    ),
                });
            }
        }
        if self.drift_correction() == DriftCorrection::On
            && self.output_backend() == Some(OutputBackend::Pipewire)
        {
            warnings.push(FieldIssue {
                path: "resampler.drift_correction",
                message: "\"on\" with a PipeWire output adds drift handling to a rate-matched stream; no benefit expected".to_string(),
            });
        }
        if self.output_backend() == Some(OutputBackend::Pipewire)
            && self
                .target
                .as_ref()
                .is_some_and(|t| t.format == TargetFormat::Pcm && t.sample_format.is_some())
        {
            warnings.push(FieldIssue {
                path: "target.sample_format",
                message: "ignored with a PipeWire output, which always carries unquantized f32"
                    .to_string(),
            });
        }
        warnings
    }

    /// The configured `[output] backend`, if any.
    pub fn output_backend(&self) -> Option<OutputBackend> {
        self.output.as_ref().and_then(|o| o.backend)
    }

    /// `[resampler] drift_correction` (default `auto`). The engine resolves it per stream against
    /// the output backend and the input clock (`DriftMode::resolve`, `Design.md §1.5`).
    pub fn drift_correction(&self) -> DriftCorrection {
        self.resampler
            .as_ref()
            .map_or(DriftCorrection::Auto, |r| r.drift_correction)
    }

    /// Drift correction limit in ppm (default 500); only used while drift correction is active.
    pub fn resolved_max_clock_drift_ppm(&self) -> u32 {
        self.resampler
            .as_ref()
            .and_then(|r| r.max_clock_drift_ppm)
            .unwrap_or(DEFAULT_MAX_CLOCK_DRIFT_PPM)
    }

    /// Returns the configured maximum intermediate PCM rate for DSD decimation, if set.
    pub fn resolved_dsd_max_intermediate_rate_hz(&self) -> Option<PcmRateHz> {
        self.input
            .as_ref()
            .and_then(|i| i.dsd_max_intermediate_rate_hz())
    }

    /// Resolves the concrete `StreamSpec` and `DspConfiguration`
    /// from `[target]` + `rate_family` (`Design.md §6.1`).
    pub fn explain_for_input_rate(
        &self,
        input_rate: PcmRateHz,
    ) -> Result<ResolvedPipelinePlan, SettingsError> {
        let input_family = input_rate.rate_family();
        let channels = self.input.as_ref().and_then(|i| i.channels).unwrap_or(2);
        let resampler = self.resampler.clone().unwrap_or_default();
        let gain = self.gain.clone().unwrap_or_default();

        let target = self
            .target
            .as_ref()
            .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;

        let target_spec = match target.format {
            TargetFormat::Pcm => {
                let base_rate = target
                    .pcm_rate_hz
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                let family = target.rate_family.resolve_family(input_family);
                let rate = if base_rate.rate_family() == family {
                    base_rate
                } else {
                    base_rate
                        .corresponding_rate_in_family(family)
                        .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?
                };
                TargetRateSpec::Pcm {
                    rate,
                    sample_format: target.sample_format.unwrap_or(SampleFormat::S32Le),
                }
            }
            TargetFormat::Dsd => {
                let multiplier = target
                    .dsd_rate
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                let family = target.rate_family.resolve_family(input_family);
                let bit_rate = multiplier
                    .to_bit_rate(family)
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                TargetRateSpec::Dsd { bit_rate }
            }
        };

        let mut dsp_config = DspConfiguration::new(
            resampler.phase,
            resampler.apodization,
            gain.preamp_db,
            0.0,
            gain.overload_policy,
        );
        if let Some(dither) = target.dither {
            dsp_config.dither = dither;
        }
        dsp_config.dither_tpdf_seed = target.dither_tpdf_seed;
        dsp_config.modulator_order = resampler.modulator_order;
        // The input limit and headroom belong to the selected modulator (order and bit rate). PCM output
        // has no modulator, so only full scale limits it (the default: no headroom, L = 0 dBFS).
        if let TargetRateSpec::Dsd { bit_rate } = target_spec {
            dsp_config
                .apply_modulator_limits(bit_rate, gain.dsd_safety_margin_db)
                .map_err(|_| {
                    SettingsError::invalid(
                        "resampler.modulator_order",
                        "no modulator for this order and DSD rate",
                    )
                })?;
        }

        Ok(ResolvedPipelinePlan {
            stream_spec: StreamSpec {
                input_rate,
                channels,
                target: target_spec,
            },
            dsp_config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime_toml(runtime: &str) -> String {
        format!(
            "schema_version = 1\n[target]\nformat = \"dsd\"\nrate_family = \"44100\"\ndsd_rate = \"dsd128\"\n{runtime}"
        )
    }

    #[test]
    fn test_drift_correction_setting_and_warnings() {
        let base = |out: &str, resampler: &str| {
            Settings::from_toml_str(&format!(
                "schema_version = 1\n[output]\n{out}\n[target]\nformat = \"pcm\"\npcm_rate_hz = 192000\n{resampler}"
            ))
        };
        let alsa = "backend = \"alsa\"\ndevice = \"hw:0,0\"";
        let pw = "backend = \"pipewire\"";
        assert_eq!(
            base(alsa, "").unwrap().drift_correction(),
            DriftCorrection::Auto
        );
        assert_eq!(
            base(alsa, "[resampler]\nmax_clock_drift_ppm = 200")
                .unwrap()
                .resolved_max_clock_drift_ppm(),
            200
        );
        assert_eq!(base(pw, "").unwrap().resolved_max_clock_drift_ppm(), 500);
        // `off` is the normal case with the demand-driven sink: no warning.
        let off = base(alsa, "[resampler]\ndrift_correction = \"off\"").unwrap();
        assert_eq!(off.drift_correction(), DriftCorrection::Off);
        assert!(off.warnings().is_empty());
        // `on` with a rate-matched PipeWire output still warns.
        let on = base(pw, "[resampler]\ndrift_correction = \"on\"").unwrap();
        assert_eq!(on.drift_correction(), DriftCorrection::On);
        assert_eq!(on.warnings().len(), 1);
        assert!(
            base(alsa, "[resampler]\ndrift_correction = \"on\"")
                .unwrap()
                .warnings()
                .is_empty()
        );
    }

    #[test]
    fn test_runtime_defaults_when_absent() {
        let s = Settings::from_toml_str(&runtime_toml("")).unwrap();
        assert!(s.runtime.is_none());
        let r = s.runtime_config();
        assert_eq!(r.dsp_threads, DspThreads::Auto);
        assert_eq!(r.realtime, RealtimeMode::Auto);
        assert_eq!(r.realtime_priority, 20);
        assert_eq!(r.core_selection, CoreSelection::Performance);
        assert_eq!(r.smt_siblings, SmtSiblings::Avoid);
        assert_eq!(r.systray, SystrayMode::Off);
        assert_eq!(r.explicit_worker_count(2), None);
    }

    #[test]
    fn test_runtime_full_section_parses() {
        let s = Settings::from_toml_str(&runtime_toml(
            "[runtime]\ndsp_threads = 2\nrealtime = \"required\"\nrealtime_priority = 30\ncore_selection = \"efficient\"\nsmt_siblings = \"never\"\nloop_cpu = 4\ndsp_cpus = [0, 2]\nsystray = \"auto\"\n",
        ))
        .unwrap();
        let r = s.runtime_config();
        assert_eq!(r.systray, SystrayMode::Auto);
        assert_eq!(r.dsp_threads, DspThreads::Count(2));
        assert_eq!(r.realtime, RealtimeMode::Required);
        assert_eq!(r.realtime_priority, 30);
        assert_eq!(r.core_selection, CoreSelection::Efficient);
        assert_eq!(r.smt_siblings, SmtSiblings::Never);
        assert_eq!(r.loop_cpu, Some(4));
        assert_eq!(r.dsp_cpus, Some(vec![0, 2]));
    }

    #[test]
    fn test_runtime_inline_parses_and_rejects_pins() {
        let s = Settings::from_toml_str(&runtime_toml("[runtime]\ndsp_threads = \"inline\"\n"))
            .unwrap();
        let r = s.runtime_config();
        assert_eq!(r.dsp_threads, DspThreads::Inline);
        assert_eq!(r.explicit_worker_count(2), Some(0));
        assert!(
            Settings::from_toml_str(&runtime_toml(
                "[runtime]\ndsp_threads = \"inline\"\ndsp_cpus = [0]\n"
            ))
            .is_err()
        );
    }

    #[test]
    fn test_runtime_auto_parses() {
        let s =
            Settings::from_toml_str(&runtime_toml("[runtime]\ndsp_threads = \"auto\"\n")).unwrap();
        assert_eq!(s.runtime_config(), RuntimeConfig::default());
    }

    #[test]
    fn test_runtime_rejects_bad_values() {
        for bad in [
            "[runtime]\ndsp_threads = 0\n",
            "[runtime]\ndsp_threads = -1\n",
            "[runtime]\ndsp_threads = \"many\"\n",
            "[runtime]\nrealtime = \"yes\"\n",
            "[runtime]\ncore_selection = \"fast\"\n",
            "[runtime]\nsmt_siblings = \"off\"\n",
            "[runtime]\nrealtime_priority = 2\n",
            "[runtime]\nrealtime_priority = 100\n",
            "[runtime]\nsystray = \"on\"\n",
            "[runtime]\nsystray = true\n",
            "[runtime]\nbogus = 1\n",
            "[runtime]\ndsp_cpus = []\n",
            "[runtime]\ndsp_cpus = [0, 0]\n",
            "[runtime]\ndsp_threads = 2\ndsp_cpus = [0]\n",
            "[runtime]\nloop_cpu = 2\ndsp_cpus = [0, 2]\n",
        ] {
            assert!(
                Settings::from_toml_str(&runtime_toml(bad)).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn test_runtime_auto_threads_with_pins_caps_at_channels() {
        let s =
            Settings::from_toml_str(&runtime_toml("[runtime]\ndsp_cpus = [0, 2, 4]\n")).unwrap();
        assert_eq!(s.runtime_config().explicit_worker_count(2), Some(2));
        assert_eq!(s.runtime_config().explicit_worker_count(8), Some(3));
    }

    #[test]
    fn test_runtime_roundtrips_through_toml() {
        let s = Settings::from_toml_str(&runtime_toml(
            "[runtime]\ndsp_threads = 2\ndsp_cpus = [0, 2]\n",
        ))
        .unwrap();
        let text = toml::to_string(&s).unwrap();
        assert_eq!(Settings::from_toml_str(&text).unwrap(), s);
    }

    #[test]
    fn test_logging_config_section() {
        let toml_str = r#"
schema_version = 1

[logging]
format = "json"
level = "debug"

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd128"
"#;
        let settings = Settings::from_toml_str(toml_str).unwrap();
        let log_cfg = settings.logging.as_ref().unwrap();
        assert_eq!(log_cfg.format.as_deref(), Some("json"));
        assert_eq!(log_cfg.level.as_deref(), Some("debug"));
    }

    #[test]
    fn test_rt_report_interval_default_and_override() {
        let base = r#"
schema_version = 1

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd128"
"#;
        let settings = Settings::from_toml_str(base).unwrap();
        assert_eq!(
            settings.rt_report_interval(),
            std::time::Duration::from_millis(DEFAULT_LATE_TILE_REPORT_INTERVAL_MS)
        );

        let configured = r#"
schema_version = 1

[logging]
rt_report_interval_ms = 2500

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd128"
"#;
        let settings = Settings::from_toml_str(configured).unwrap();
        assert_eq!(
            settings.rt_report_interval(),
            std::time::Duration::from_millis(2_500)
        );

        let clamped = r#"
schema_version = 1

[logging]
rt_report_interval_ms = 0

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd128"
"#;
        let settings = Settings::from_toml_str(clamped).unwrap();
        assert_eq!(
            settings.rt_report_interval(),
            std::time::Duration::from_millis(MIN_LATE_TILE_REPORT_INTERVAL_MS)
        );
    }
    use audigma_dsp::DsdBitRateHz;

    #[test]
    fn test_explain_source_family_dsd_target() {
        let toml = r#"
schema_version = 1

[target]
format = "dsd"
rate_family = "source"
dsd_rate = "dsd256"

[resampler]
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let plan_44k = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan_44k.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );

        let plan_48k = settings
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(
            plan_48k.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256_48
            }
        );
    }

    #[test]
    fn test_design_md_fixture_a_pipewire_pcm() {
        let toml = r#"
schema_version = 1

[input]
backend = "pipewire"
node_name = "audigma_sink"
channels = 2
channel_positions = [ "FL", "FR" ]
rate_mode = "dynamic"
allowed_rates = [ 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000 ]

[output]
backend = "pipewire"
target_node = "alsa_output.usb-FiiO_K17-00.analog-stereo"
transport = "pcm"
volume_control = "hardware-software-fallback"
device_wait_ms = -1
keep_audio_sink_ms = 5000

[target]
format = "pcm"
rate_family = "48000"
pcm_rate_hz = 192000
sample_format = "f32"

[gain]
preamp_db = -3.0
overload_policy = "mute-recover"

[resampler]
phase = "linear"
apodization = "off"
drift_correction = "off"
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let input = settings.input.as_ref().unwrap();
        assert_eq!(input.rate_mode(), RateMode::Dynamic);
        assert_eq!(input.resolved_channel_positions(), vec!["FL", "FR"]);
        let output = settings.output.as_ref().unwrap();
        assert_eq!(
            output.volume_control(),
            VolumeControlMode::HardwareSoftwareFallback
        );
        assert_eq!(output.device_wait_ms(), -1);
        assert_eq!(output.keep_audio_sink_ms(), 5000);

        let plan = settings
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(
            plan.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_192000,
                sample_format: SampleFormat::F32,
            }
        );
    }

    #[test]
    fn test_design_md_fixture_b_pipewire_alsa_dsd() {
        let toml = r#"
schema_version = 1

[input]
backend = "pipewire"
node_name = "audigma_sink"
channels = 2
channel_positions = [ "FL", "FR" ]
rate_mode = "dynamic"
allowed_rates = [ 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000 ]
allow_dsd = true

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "native"
reserve_device = true
volume_control = "hardware-software-fallback"
device_wait_ms = -1
keep_audio_sink_ms = 5000
buffer_ms = 40
period_ms = 5

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"

[gain]
preamp_db = 0.0
overload_policy = "mute-recover"

[resampler]
phase = "minimum"
apodization = "off"
modulator_order = 5
drift_correction = "on"
max_clock_drift_ppm = 500
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        assert!(settings.output.as_ref().unwrap().reserve_device_enabled());
        assert!(settings.input.as_ref().unwrap().allow_dsd());
        let plan = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(plan.dsp_config.phase, FilterPhase::Minimum);
        assert_eq!(plan.dsp_config.modulator_order, 5);
        assert_eq!(
            plan.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256,
            }
        );
    }

    #[test]
    fn test_modulator_order_7_toml_propagation() {
        let toml = r#"
schema_version = 1

[input]
backend = "pipewire"

[output]
backend = "alsa"
device = "hw:0,0"

[target]
format = "dsd"
dsd_rate = "dsd256"

[resampler]
modulator_order = 7
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        assert_eq!(settings.resampler.as_ref().unwrap().modulator_order, 7);
        let plan = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(plan.dsp_config.modulator_order, 7);
    }

    fn margin_toml(extra: &str) -> String {
        format!(
            "schema_version = 1\n\n[output]\nbackend = \"alsa\"\ndevice = \"hw:0,0\"\n\n[target]\nformat = \"dsd\"\ndsd_rate = \"dsd256\"\n\n[gain]\n{extra}\n"
        )
    }

    #[test]
    fn test_safety_margin_default_values_and_rejection() {
        let plan = |t: &str| {
            Settings::from_toml_str(t).and_then(|s| s.explain_for_input_rate(PcmRateHz::PCM_44100))
        };
        let default = plan(&margin_toml("")).unwrap().dsp_config;
        assert_eq!(default.safety_margin_db, 0.25);
        let m0 = plan(&margin_toml("dsd_safety_margin_db = 0"))
            .unwrap()
            .dsp_config;
        assert_eq!(m0.safety_margin_db, 0.0);
        assert_eq!(m0.profile_headroom_db, m0.max_stable_input_dbfs);
        let m5 = plan(&margin_toml("dsd_safety_margin_db = 0.5"))
            .unwrap()
            .dsp_config;
        assert!((m5.profile_headroom_db - (m5.max_stable_input_dbfs - 0.5)).abs() < 1e-12);
        let pcm = margin_toml("dsd_safety_margin_db = 0.5").replace(
            "format = \"dsd\"\ndsd_rate = \"dsd256\"",
            "format = \"pcm\"\npcm_rate_hz = 192000",
        );
        assert_eq!(plan(&pcm).unwrap().dsp_config.safety_margin_db, 0.0);
        let err = plan(&margin_toml("dsd_safety_margin_db = 0.3")).unwrap_err();
        assert!(err.to_string().contains("dsd_safety_margin_db"));
    }

    #[test]
    fn test_modulator_order_invalid_rejection() {
        let toml = r#"
schema_version = 1

[target]
format = "dsd"
dsd_rate = "dsd256"

[resampler]
modulator_order = 6
"#;
        let err = Settings::from_toml_str(toml).unwrap_err();
        assert!(err.to_string().contains("modulator_order"));
        assert_eq!(err.path(), Some("resampler.modulator_order"));
    }

    #[test]
    fn test_design_md_fixture_c_offline_dsf() {
        let toml = r#"
schema_version = 1

[input]

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"
dither_tpdf_seed = 12345

[gain]
preamp_db = 0.0
peak_analysis = "two-pass"
overload_policy = "error"

[resampler]
phase = "linear"
apodization = "off"
modulator_order = 5

[offline]
tail_policy = "trim-to-duration"
write_report = true
dsf_pad_mode = "spec-zero"
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        assert_eq!(
            settings.gain.as_ref().unwrap().peak_analysis,
            PeakAnalysisMode::TwoPass
        );
        assert_eq!(
            settings.offline.as_ref().unwrap().dsf_pad_mode,
            DsfPadMode::SpecZero
        );
    }

    #[test]
    fn test_design_md_fixture_d_stdio_dop() {
        let toml = r#"
schema_version = 1

[input]
sample_format = "s16le"
rate_hz = 44100
channels = 2

[output]
transport = "dop"
dop_sample_format = "s24_3le"

[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd128"

[gain]
overload_policy = "error"

[resampler]
phase = "linear"
apodization = "off"
modulator_order = 5
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let plan = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD128,
            }
        );
    }

    #[test]
    fn test_rejects_dsd_over_pipewire() {
        let dsd_over_pw = r#"
schema_version = 1
[output]
backend = "pipewire"
[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd64"
"#;
        assert!(Settings::from_toml_str(dsd_over_pw).is_err());
    }

    #[test]
    fn test_dop_sample_format_accepts_only_dop_carriers() {
        let with = |fmt: &str| {
            format!(
                "schema_version = 1\n[output]\ntransport = \"dop\"\ndop_sample_format = \"{fmt}\"\n\
                 [target]\nformat = \"dsd\"\nrate_family = \"44100\"\ndsd_rate = \"dsd64\"\n"
            )
        };
        for (fmt, want) in [
            ("auto", DopSampleFormat::Auto),
            ("s32le", DopSampleFormat::S32Le),
            ("s24le", DopSampleFormat::S24Le),
            ("s24_3le", DopSampleFormat::S24_3Le),
        ] {
            let s = Settings::from_toml_str(&with(fmt)).unwrap();
            assert_eq!(s.output.unwrap().dop_sample_format(), want);
        }
        // DoP needs a 24-bit word: 16-bit, float and native DSD words cannot carry it.
        for fmt in ["s16le", "f32", "f64", "dsd_u32_be", "dsd_u32_le"] {
            assert!(
                matches!(
                    Settings::from_toml_str(&with(fmt)),
                    Err(SettingsError::TomlParse(_))
                ),
                "{fmt} must be rejected"
            );
        }
        let unset =
            Settings::from_toml_str(&with("auto").replace("dop_sample_format = \"auto\"\n", ""))
                .unwrap();
        assert_eq!(
            unset.output.unwrap().dop_sample_format(),
            DopSampleFormat::Auto
        );
    }

    #[test]
    fn test_dop_sample_format_resolution() {
        use DopContainerFormat as C;
        // `auto` tries the 24-bit words first, `s32le` last; a pinned word allows no fallback.
        assert_eq!(
            DopSampleFormat::Auto.candidates(),
            &[C::S24_3Le, C::S24Le, C::S32Le]
        );
        assert_eq!(DopSampleFormat::S24Le.candidates(), &[C::S24Le]);
        assert_eq!(DopSampleFormat::S24_3Le.candidates(), &[C::S24_3Le]);
        assert_eq!(DopSampleFormat::S32Le.candidates(), &[C::S32Le]);
        // Without a device, `auto` writes `s32le`.
        assert_eq!(DopSampleFormat::Auto.fixed(), None);
        assert_eq!(DopSampleFormat::Auto.without_device(), C::S32Le);
        assert_eq!(DopSampleFormat::S24_3Le.without_device(), C::S24_3Le);
    }

    #[test]
    fn test_output_container_flac_and_default_offline() {
        let flac_toml = r#"
schema_version = 1

[input]

[target]
format = "pcm"
rate_family = "48000"
pcm_rate_hz = 192000
sample_format = "s24le"
dither = "tpdf"

[gain]
preamp_db = -3.0
overload_policy = "error"

[resampler]
"#;
        let settings = Settings::from_toml_str(flac_toml).unwrap();
        OutputContainer::Flac
            .check_target(settings.target.as_ref().unwrap())
            .unwrap();

        // Test extension & path inference
        assert_eq!(
            OutputContainer::from_extension("flac"),
            Some(OutputContainer::Flac)
        );
        assert_eq!(
            OutputContainer::from_extension(".DSF"),
            Some(OutputContainer::Dsf)
        );
        assert_eq!(
            OutputContainer::from_extension("wav"),
            Some(OutputContainer::Wav)
        );
        assert_eq!(
            OutputContainer::from_path(std::path::Path::new("music/test.flac")),
            Some(OutputContainer::Flac)
        );

        // Test default_offline
        let dsf_def = Settings::default_offline(OutputContainer::Dsf);
        assert!(dsf_def.validate().is_ok());
        assert_eq!(dsf_def.target.as_ref().unwrap().format, TargetFormat::Dsd);

        let flac_def = Settings::default_offline(OutputContainer::Flac);
        assert!(flac_def.validate().is_ok());
        assert_eq!(flac_def.target.as_ref().unwrap().format, TargetFormat::Pcm);

        let wav_def = Settings::default_offline(OutputContainer::Wav);
        assert!(wav_def.validate().is_ok());
        assert_eq!(wav_def.target.as_ref().unwrap().format, TargetFormat::Pcm);
    }

    #[test]
    fn test_container_target_mismatch() {
        let target = |body: &str| {
            Settings::from_toml_str(&format!("schema_version = 1\n[target]\n{body}"))
                .unwrap()
                .target
                .unwrap()
        };
        let dsd = target("format = \"dsd\"\nrate_family = \"44100\"\ndsd_rate = \"dsd256\"\n");
        let pcm = target("format = \"pcm\"\nrate_family = \"48000\"\npcm_rate_hz = 192000\n");
        for c in [OutputContainer::Flac, OutputContainer::Wav] {
            assert_eq!(
                c.check_target(&dsd).unwrap_err().path(),
                Some("target.format")
            );
            c.check_target(&pcm).unwrap();
        }
        assert_eq!(
            OutputContainer::Dsf.check_target(&pcm).unwrap_err().path(),
            Some("target.format")
        );
        OutputContainer::Dsf.check_target(&dsd).unwrap();
        OutputContainer::Raw.check_target(&dsd).unwrap();
        OutputContainer::Raw.check_target(&pcm).unwrap();
    }

    #[test]
    fn test_output_container_key_is_gone() {
        let err = Settings::from_toml_str("schema_version = 1\n[output]\ncontainer = \"flac\"\n")
            .unwrap_err();
        assert!(err.to_string().contains("container"), "{err}");
    }

    #[test]
    fn test_rejects_unknown_field_dsd_bit_rate_hz() {
        let legacy_toml = r#"
schema_version = 1
[target]
format = "dsd"
rate_family = "44100"
dsd_bit_rate_hz = 11289600
"#;
        let err = Settings::from_toml_str(legacy_toml).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("unknown field `dsd_bit_rate_hz`"),
            "Expected unknown field error for dsd_bit_rate_hz, got: {err_msg}"
        );
    }

    #[test]
    fn test_rejects_unknown_field_rate_policy() {
        let legacy_toml = r#"
schema_version = 1
[target]
format = "dsd"
rate_policy = "source-family"
dsd_rate = "dsd256"
"#;
        let err = Settings::from_toml_str(legacy_toml).unwrap_err();
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("unknown field `rate_policy`"),
            "Expected unknown field error for rate_policy, got: {err_msg}"
        );
    }

    #[test]
    fn test_channel_positions_order_preservation_and_synthesis() {
        // Strict user order preservation (e.g. reversed stereo)
        let reversed_toml = r#"
schema_version = 1
[input]
backend = "pipewire"
channels = 2
channel_positions = [ "FR", "FL" ]
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let settings = Settings::from_toml_str(reversed_toml).unwrap();
        let input = settings.input.as_ref().unwrap();
        assert_eq!(input.resolved_channel_positions(), vec!["FR", "FL"]);

        // Synthesize standard defaults when channel_positions omitted
        let mono_input = InputConfig {
            backend: Some(InputBackend::Pipewire),
            node_name: None,
            sample_format: None,
            rate_hz: None,
            channels: Some(1),
            channel_positions: None,
            rate_mode: None,
            allowed_rates: None,
            allow_dsd: None,
            dsd: None,
        };
        assert_eq!(mono_input.resolved_channel_positions(), vec!["MONO"]);

        let stereo_input = InputConfig {
            channels: Some(2),
            ..mono_input.clone()
        };
        assert_eq!(stereo_input.resolved_channel_positions(), vec!["FL", "FR"]);

        let quad_input = InputConfig {
            channels: Some(4),
            ..mono_input.clone()
        };
        assert_eq!(
            quad_input.resolved_channel_positions(),
            vec!["FL", "FR", "RL", "RR"]
        );

        let surround_51 = InputConfig {
            channels: Some(6),
            ..mono_input.clone()
        };
        assert_eq!(
            surround_51.resolved_channel_positions(),
            vec!["FL", "FR", "FC", "LFE", "RL", "RR"]
        );

        let surround_71 = InputConfig {
            channels: Some(8),
            ..mono_input.clone()
        };
        assert_eq!(
            surround_71.resolved_channel_positions(),
            vec!["FL", "FR", "FC", "LFE", "RL", "RR", "SL", "SR"]
        );

        let custom_3 = InputConfig {
            channels: Some(3),
            ..mono_input.clone()
        };
        assert_eq!(
            custom_3.resolved_channel_positions(),
            vec!["UNKNOWN_1", "UNKNOWN_2", "UNKNOWN_3"]
        );
    }

    #[test]
    fn test_channel_positions_validation_errors() {
        // Length mismatch
        let mismatch_toml = r#"
schema_version = 1
[input]
backend = "pipewire"
channels = 2
channel_positions = [ "FL" ]
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        assert!(Settings::from_toml_str(mismatch_toml).is_err());

        // Unrecognized token
        let invalid_pos_toml = r#"
schema_version = 1
[input]
backend = "pipewire"
channels = 2
channel_positions = [ "FL", "BOGUS_CHANNEL" ]
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        assert!(Settings::from_toml_str(invalid_pos_toml).is_err());
    }

    #[test]
    fn test_volume_control_modes() {
        for (mode_str, expected) in [
            (
                "hardware-software-fallback",
                VolumeControlMode::HardwareSoftwareFallback,
            ),
            ("software", VolumeControlMode::Software),
            ("hardware", VolumeControlMode::Hardware),
            ("none", VolumeControlMode::None),
        ] {
            let toml = format!(
                r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
volume_control = "{mode_str}"
[target]
format = "pcm"
pcm_rate_hz = 96000
"#
            );
            let settings = Settings::from_toml_str(&toml).unwrap();
            assert_eq!(settings.output.as_ref().unwrap().volume_control(), expected);
        }

        // Default when omitted
        let default_toml = r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let settings = Settings::from_toml_str(default_toml).unwrap();
        assert_eq!(
            settings.output.as_ref().unwrap().volume_control(),
            VolumeControlMode::HardwareSoftwareFallback
        );
    }

    #[test]
    fn test_gain_preamp_db_range_validation() {
        let with_preamp = |db: &str| {
            format!(
                r#"
schema_version = 1
[target]
format = "pcm"
pcm_rate_hz = 96000
[gain]
preamp_db = {db}
overload_policy = "mute-recover"
"#
            )
        };

        // Bounds are inclusive, and integers widen to f64.
        for ok in ["-120.0", "0", "12.0"] {
            Settings::from_toml_str(&with_preamp(ok))
                .unwrap_or_else(|e| panic!("preamp_db = {ok} should be valid: {e}"));
        }

        for bad in ["-120.1", "12.1", "nan", "inf"] {
            let err = Settings::from_toml_str(&with_preamp(bad)).unwrap_err();
            assert!(
                err.to_string().contains("preamp_db"),
                "preamp_db = {bad} should be rejected: {err}"
            );
        }
    }

    #[test]
    fn test_device_wait_expiry_parse_default_and_warning() {
        let base = r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
device_wait_ms = 1000
keep_audio_sink_ms = 5000
{EXTRA}
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let parse = |extra: &str| Settings::from_toml_str(&base.replace("{EXTRA}", extra)).unwrap();

        let default = parse("");
        assert_eq!(
            default.output.as_ref().unwrap().device_wait_expiry(),
            DeviceWaitExpiry::Exit
        );
        assert!(default.warnings()[0].message.contains("daemon will exit"));

        let pause = parse("device_wait_expiry = \"pause\"");
        assert_eq!(
            pause.output.as_ref().unwrap().device_wait_expiry(),
            DeviceWaitExpiry::Pause
        );
        assert!(pause.warnings()[0].message.contains("daemon will pause"));

        let bad = base.replace("{EXTRA}", "device_wait_expiry = \"later\"");
        assert!(Settings::from_toml_str(&bad).is_err());
    }

    #[test]
    fn test_device_wait_and_keep_sink_timeouts_and_warnings() {
        let valid_toml = r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
device_wait_ms = 10000
keep_audio_sink_ms = 2000
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let settings = Settings::from_toml_str(valid_toml).unwrap();
        assert_eq!(settings.output.as_ref().unwrap().device_wait_ms(), 10000);
        assert_eq!(settings.output.as_ref().unwrap().keep_audio_sink_ms(), 2000);
        assert!(settings.warnings().is_empty());

        // Warning when keep_audio_sink_ms > device_wait_ms with device_wait_ms > 0
        let warning_toml = r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
device_wait_ms = 1000
keep_audio_sink_ms = 5000
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let warning_settings = Settings::from_toml_str(warning_toml).unwrap();
        let warnings = warning_settings.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .message
                .contains("is greater than device_wait_ms")
        );
        assert_eq!(warnings[0].path, "output.keep_audio_sink_ms");

        // Reject values < -1
        let invalid_wait = r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
device_wait_ms = -2
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        assert!(Settings::from_toml_str(invalid_wait).is_err());

        let invalid_keep = r#"
schema_version = 1
[output]
backend = "alsa"
device = "hw:CARD=Test,DEV=0"
keep_audio_sink_ms = -5
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        assert!(Settings::from_toml_str(invalid_keep).is_err());
    }

    #[test]
    fn test_rate_mode_and_allowed_rates() {
        let dynamic_toml = r#"
schema_version = 1
[input]
backend = "pipewire"
rate_mode = "dynamic"
allowed_rates = [ 44100, 48000, 96000, 192000 ]
[target]
format = "pcm"
pcm_rate_hz = 192000
"#;
        let settings = Settings::from_toml_str(dynamic_toml).unwrap();
        let input = settings.input.as_ref().unwrap();
        assert_eq!(input.rate_mode(), RateMode::Dynamic);
        assert_eq!(
            input.resolved_allowed_rates(),
            vec![44100, 48000, 96000, 192000]
        );

        let fixed_toml = r#"
schema_version = 1
[input]
backend = "pipewire"
rate_mode = "fixed"
[target]
format = "pcm"
pcm_rate_hz = 192000
"#;
        let fixed_settings = Settings::from_toml_str(fixed_toml).unwrap();
        assert_eq!(
            fixed_settings.input.as_ref().unwrap().rate_mode(),
            RateMode::Fixed
        );
        assert_eq!(
            fixed_settings
                .input
                .as_ref()
                .unwrap()
                .resolved_allowed_rates(),
            InputConfig::default_allowed_rates()
        );
    }

    #[test]
    fn test_dsd_rate_symbolic_multipliers_and_source_family() {
        // Test fixed 44100 family with dsd_rate = "dsd256"
        let fixed_dsd_44 = r#"
schema_version = 1
[target]
format = "dsd"
rate_family = "44100"
dsd_rate = "dsd256"
"#;
        let settings_44 = Settings::from_toml_str(fixed_dsd_44).unwrap();
        let plan_44 = settings_44
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan_44.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );
        let plan_44_from_48 = settings_44
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(
            plan_44_from_48.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );

        // Test fixed 48000 family with dsd_rate = "dsd256"
        let fixed_dsd_48 = r#"
schema_version = 1
[target]
format = "dsd"
rate_family = "48000"
dsd_rate = "dsd256"
"#;
        let settings_48 = Settings::from_toml_str(fixed_dsd_48).unwrap();
        let plan_48 = settings_48
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(
            plan_48.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256_48
            }
        );
        let plan_48_from_44 = settings_48
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan_48_from_44.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256_48
            }
        );

        // Test explicit rate_family = "source" with dsd_rate = "dsd256"
        let family_dsd = r#"
schema_version = 1
[target]
format = "dsd"
rate_family = "source"
dsd_rate = "dsd256"
"#;
        let family_settings = Settings::from_toml_str(family_dsd).unwrap();
        let plan_fam_44 = family_settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan_fam_44.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );
        let plan_fam_48 = family_settings
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(
            plan_fam_48.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256_48
            }
        );

        // Test default rate_family (omitted) defaults to "source"
        let default_dsd = r#"
schema_version = 1
[target]
format = "dsd"
dsd_rate = "dsd256"
"#;
        let def_settings = Settings::from_toml_str(default_dsd).unwrap();
        assert_eq!(
            def_settings.target.as_ref().unwrap().rate_family,
            TargetRateFamily::Source
        );
        assert_eq!(
            def_settings
                .explain_for_input_rate(PcmRateHz::PCM_44100)
                .unwrap()
                .stream_spec
                .target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );
        assert_eq!(
            def_settings
                .explain_for_input_rate(PcmRateHz::PCM_48000)
                .unwrap()
                .stream_spec
                .target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256_48
            }
        );

        // Test other multipliers
        for (rate_str, expected_44, expected_48) in [
            ("dsd64", DsdBitRateHz::DSD64, DsdBitRateHz::DSD64_48),
            ("dsd128", DsdBitRateHz::DSD128, DsdBitRateHz::DSD128_48),
            ("dsd512", DsdBitRateHz::DSD512, DsdBitRateHz::DSD512_48),
        ] {
            let toml = format!(
                r#"
schema_version = 1
[target]
format = "dsd"
rate_family = "source"
dsd_rate = "{rate_str}"
"#
            );
            let s = Settings::from_toml_str(&toml).unwrap();
            let p44 = s.explain_for_input_rate(PcmRateHz::PCM_44100).unwrap();
            assert_eq!(
                p44.stream_spec.target,
                TargetRateSpec::Dsd {
                    bit_rate: expected_44
                }
            );
            let p48 = s.explain_for_input_rate(PcmRateHz::PCM_48000).unwrap();
            assert_eq!(
                p48.stream_spec.target,
                TargetRateSpec::Dsd {
                    bit_rate: expected_48
                }
            );
        }
    }

    #[test]
    fn test_pcm_rate_tier_matching_with_rate_family() {
        // Given pcm_rate_hz = 768000 (16x 48k) with rate_family = "source":
        let toml_source = r#"
schema_version = 1
[target]
format = "pcm"
rate_family = "source"
pcm_rate_hz = 768000
"#;
        let s_source = Settings::from_toml_str(toml_source).unwrap();
        // 44.1k input automatically resolves to 16x 44.1k = 705.6k
        let p44 = s_source
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            p44.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_705600,
                sample_format: SampleFormat::S32Le,
            }
        );
        // 48k input automatically resolves to 16x 48k = 768k
        let p48 = s_source
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(
            p48.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_768000,
                sample_format: SampleFormat::S32Le,
            }
        );

        // Fixed 44100 family with pcm_rate_hz = 192000 (4x):
        // Automatically maps 4x to 176.4k and locks regardless of input rate
        let toml_fixed_44 = r#"
schema_version = 1
[target]
format = "pcm"
rate_family = "44100"
pcm_rate_hz = 192000
"#;
        let s_fixed = Settings::from_toml_str(toml_fixed_44).unwrap();
        assert_eq!(
            s_fixed
                .explain_for_input_rate(PcmRateHz::PCM_48000)
                .unwrap()
                .stream_spec
                .target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_176400,
                sample_format: SampleFormat::S32Le,
            }
        );
        assert_eq!(
            s_fixed
                .explain_for_input_rate(PcmRateHz::PCM_44100)
                .unwrap()
                .stream_spec
                .target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_176400,
                sample_format: SampleFormat::S32Le,
            }
        );
    }

    #[test]
    fn test_allow_dsd_and_dsd_input_config() {
        let toml = r#"
schema_version = 1
[input]
backend = "pipewire"
allow_dsd = true
[input.dsd]
max_intermediate_rate_hz = 352800
gain_db = 6.0
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let input = settings.input.as_ref().unwrap();
        assert!(input.allow_dsd());
        let dsd = input.dsd.as_ref().unwrap();
        assert_eq!(dsd.max_intermediate_rate_hz, Some(PcmRateHz::PCM_352800));
        assert_eq!(dsd.gain_db, Some(6.0));
        assert_eq!(input.dsd_gain_db(), 6.0);
        assert!((input.dsd_linear_gain() - 10.0_f64.powf(6.0 / 20.0)).abs() < 1e-6);

        // Invalid gain_db rejection
        let invalid_toml = r#"
schema_version = 1
[input]
backend = "pipewire"
[input.dsd]
gain_db = 45.0
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let err = Settings::from_toml_str(invalid_toml).unwrap_err();
        match err {
            SettingsError::Validation { path, message: msg } => {
                assert_eq!(path, "input.dsd.gain_db");
                assert!(msg.contains("between -30.0"));
            }
            other => panic!("expected Validation error, got: {other:?}"),
        }
    }

    #[test]
    fn test_output_mixer_control() {
        let toml = r#"
schema_version = 1
[input]
backend = "pipewire"
[output]
backend = "alsa"
device = "hw:CARD=JA11,DEV=0"
mixer_control = "PCM"
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let output = settings.output.as_ref().unwrap();
        assert_eq!(output.mixer_control(), Some("PCM"));
    }

    #[test]
    fn test_gain_and_resampler_sections_accept_omitted_keys() {
        let settings = Settings::from_toml_str(
            r#"
schema_version = 1

[target]
format = "dsd"
dsd_rate = "dsd64"

[gain]
preamp_db = -2.0

[resampler]
phase = "minimum"
"#,
        )
        .unwrap();
        let gain = settings.gain.unwrap();
        assert_eq!(gain.overload_policy, OverloadPolicy::MuteRecover);
        let resampler = settings.resampler.unwrap();
        assert_eq!(resampler.phase, FilterPhase::Minimum);
        assert_eq!(resampler.apodization, Apodization::Off);
        assert_eq!(resampler.modulator_order, 5);
    }

    #[test]
    fn test_resampler_apodization_values_and_removed_preset() {
        let parse = |resampler: &str| {
            Settings::from_toml_str(&format!(
                "schema_version = 1\n[target]\nformat = \"dsd\"\ndsd_rate = \"dsd64\"\n[resampler]\n{resampler}\n"
            ))
        };
        let apodization = |value: &str| {
            parse(&format!("apodization = {value}")).map(|s| s.resampler.unwrap().apodization)
        };
        assert_eq!(apodization(r#""off""#).unwrap(), Apodization::Off);
        assert_eq!(
            apodization(r#""gentle""#).unwrap(),
            Apodization::PassEdge19000
        );
        assert_eq!(apodization("18000").unwrap(), Apodization::PassEdge18000);
        assert_eq!(apodization("18500").unwrap(), Apodization::PassEdge18500);
        assert_eq!(apodization("19000").unwrap(), Apodization::PassEdge19000);
        assert_eq!(apodization("20000").unwrap(), Apodization::PassEdge20000);
        for bad in ["17500", "21000", r#""on""#, r#""18000""#] {
            assert!(apodization(bad).is_err(), "apodization = {bad} must fail");
        }
        // The filter presets are gone: the old keys are unknown fields now.
        assert!(parse(r#"preset = "balanced""#).is_err());
        assert!(
            Settings::from_toml_str(
                "schema_version = 1\n[input]\nbackend = \"pipewire\"\n[input.dsd]\ndecimation_preset = \"reference\"\n[target]\nformat = \"pcm\"\npcm_rate_hz = 96000\n"
            )
            .is_err()
        );
    }

    #[test]
    fn test_modulator_headroom_applies_to_dsd_but_not_pcm() {
        let mut settings = Settings::default_offline(OutputContainer::Dsf);
        let dsd = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap()
            .dsp_config;
        assert!(dsd.profile_headroom_db < 0.0);

        settings = Settings::default_offline(OutputContainer::Wav);
        let pcm = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap()
            .dsp_config;
        assert_eq!(pcm.profile_headroom_db, 0.0);
        assert_eq!(pcm.max_stable_input_dbfs, 0.0);
    }
    #[test]
    fn flac_rejects_float_sample_formats() {
        let toml = |fmt: &str| {
            format!("schema_version = 1\n[target]\nformat = \"pcm\"\npcm_rate_hz = 96000\n{fmt}")
        };
        let check = |fmt: &str| {
            let settings = Settings::from_toml_str(&toml(fmt))?;
            OutputContainer::Flac.check_target(settings.target.as_ref().unwrap())
        };
        for bad in ["f32", "f64"] {
            let err = check(&format!("sample_format = \"{bad}\"")).unwrap_err();
            assert_eq!(err.path(), Some("target.sample_format"), "{bad}");
        }
        for ok in [
            "",
            "sample_format = \"s16le\"",
            "sample_format = \"s24le\"",
            "sample_format = \"s24_3le\"",
            "sample_format = \"s32le\"",
        ] {
            check(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
    }

    #[test]
    fn pipewire_output_warns_that_sample_format_is_ignored() {
        let base = |extra: &str| {
            Settings::from_toml_str(&format!(
                "schema_version = 1\n[output]\nbackend = \"pipewire\"\n[target]\nformat = \"pcm\"\npcm_rate_hz = 96000\n{extra}"
            ))
            .unwrap()
        };
        assert!(base("").warnings().is_empty());
        let w = base("sample_format = \"s24le\"").warnings();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].path, "target.sample_format");
    }

    #[test]
    fn validation_errors_carry_the_field_path() {
        let err = Settings::from_toml_str(
            "schema_version = 1\n[output]\nbackend = \"alsa\"\n[target]\nformat = \"pcm\"\npcm_rate_hz = 96000\n",
        )
        .unwrap_err();
        assert_eq!(err.path(), Some("output.device"));
        assert!(err.to_string().contains("`output.device`"));
    }
    #[test]
    fn from_toml_table_matches_from_toml_str() {
        let text = "schema_version = 1\n[gain]\npreamp_db = -3\n[target]\nformat = \"pcm\"\npcm_rate_hz = 96000\n";
        let table: toml::Table = toml::from_str(text).unwrap();
        assert_eq!(
            Settings::from_toml_table(table).unwrap(),
            Settings::from_toml_str(text).unwrap()
        );
        let bad: toml::Table = toml::from_str("schema_version = 1").unwrap();
        assert_eq!(
            Settings::from_toml_table(bad).unwrap_err().path(),
            Some("target")
        );
    }
}
