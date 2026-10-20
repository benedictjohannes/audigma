//! `audigma-settings`: Schema v1 TOML configuration (`Settings`), `[[rules]]` evaluation,
//! preset resolution, and architectural invariant validation (`Design.md §6`).

use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, DsdMultiplier, DspConfiguration, FilterPhase,
    FilterPreset, OverloadPolicy, PcmRateHz, PresetDefinition, RateFamily, SampleFormat,
    StreamSpec, TargetFormat, TargetRateSpec, presets,
};
use audigma_format::DsfPadMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Supported schema version (`schema_version = 1`).
pub const SCHEMA_VERSION_V1: u32 = 1;

/// Errors returned during TOML parsing, preset resolution, or cross-field validation.
#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("unsupported schema_version {got} (expected {SCHEMA_VERSION_V1})")]
    UnsupportedSchemaVersion { got: u32 },

    #[error("invalid configuration invariant: {0}")]
    Validation(&'static str),

    #[error("unable to resolve target rate for input rate {0}")]
    UnresolvedTargetRate(PcmRateHz),
}

/// `[input] backend` options (`Design.md §6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputBackend {
    Pipewire,
    Alsa,
    File,
    Stdio,
}

/// `[output] backend` options (`Design.md §6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputBackend {
    Pipewire,
    Alsa,
    File,
    Stdio,
}

/// `[output] transport` options (`Design.md §6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputTransport {
    Pcm,
    Native,
    Dop,
}

/// `[output] container` options (`Design.md §5.2–§5.3`, `§6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

    /// Returns the canonical target format associated with this container, if fixed.
    pub const fn target_format(self) -> Option<TargetFormat> {
        match self {
            Self::Dsf => Some(TargetFormat::Dsd),
            Self::Wav | Self::Flac => Some(TargetFormat::Pcm),
            Self::Raw => None,
        }
    }
}

/// `[target] rate_policy` options (`Design.md §3.2`, `§6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RatePolicy {
    #[default]
    #[serde(rename = "fixed")]
    Fixed,
    #[serde(rename = "source-family")]
    SourceFamily,
}

/// `[gain] headroom` options (`Design.md §6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadroomMode {
    #[default]
    Profile,
    Manual,
}

/// `[gain] peak_analysis` options (`Design.md §4.4`, `§6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PeakAnalysisMode {
    #[default]
    #[serde(rename = "single-pass")]
    SinglePass,
    #[serde(rename = "two-pass")]
    TwoPass,
}

/// `[clock] mode` options (`Design.md §1.5`, `§6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClockMode {
    #[serde(rename = "shared-graph")]
    SharedGraph,
    #[serde(rename = "adaptive-input")]
    AdaptiveInput,
}

/// `[runtime] on_device_loss` options (`Design.md §6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceLossPolicy {
    #[default]
    Wait,
    Exit,
}

/// `[offline] tail_policy` options (`Design.md §6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TailPolicy {
    #[default]
    #[serde(rename = "trim-to-duration")]
    TrimToDuration,
    #[serde(rename = "preserve-tail")]
    PreserveTail,
}

/// `[input] rate_mode` options (`Design.md §3.2`, `§6.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RateMode {
    #[default]
    Dynamic,
    Fixed,
}

/// `[output] volume_control` options (`Design.md §1.8`, `§6.2`).
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DsdInputConfig {
    pub max_intermediate_rate_hz: Option<PcmRateHz>,
    pub decimation_preset: Option<FilterPreset>,
}

/// `[input]` section (`Design.md §6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputConfig {
    pub backend: InputBackend,
    pub node_name: Option<String>,
    pub device: Option<String>,
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
}

/// `[output]` section (`Design.md §6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    pub backend: OutputBackend,
    pub target_node: Option<String>,
    pub device: Option<String>,
    pub transport: Option<OutputTransport>,
    pub container: Option<OutputContainer>,
    pub sample_format: Option<SampleFormat>,
    pub reserve_device: Option<bool>,
    pub buffer_ms: Option<u32>,
    pub period_ms: Option<u32>,
    pub volume_control: Option<VolumeControlMode>,
    pub device_wait_ms: Option<i64>,
    pub keep_audio_sink_ms: Option<i64>,
}

impl OutputConfig {
    /// Returns whether D-Bus `org.freedesktop.ReserveDevice1` reservation is enabled (`true` by default for ALSA).
    pub fn reserve_device_enabled(&self) -> bool {
        self.reserve_device
            .unwrap_or(self.backend == OutputBackend::Alsa)
    }

    /// Returns the active volume control strategy (`VolumeControlMode::HardwareSoftwareFallback` by default).
    pub fn volume_control(&self) -> VolumeControlMode {
        self.volume_control.unwrap_or_default()
    }

    /// Returns device wait timeout in milliseconds (`-1` = wait indefinitely by default).
    pub fn device_wait_ms(&self) -> i64 {
        self.device_wait_ms.unwrap_or(-1)
    }

    /// Returns audio sink keep-alive duration in milliseconds (`5000` ms by default).
    pub fn keep_audio_sink_ms(&self) -> i64 {
        self.keep_audio_sink_ms.unwrap_or(5000)
    }
}

/// `[target]` section (`Design.md §6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfig {
    pub format: TargetFormat,
    #[serde(default)]
    pub rate_policy: RatePolicy,
    pub pcm_rate_hz: Option<PcmRateHz>,
    pub pcm_rate_44100_hz: Option<PcmRateHz>,
    pub pcm_rate_48000_hz: Option<PcmRateHz>,
    pub dsd_rate: Option<DsdMultiplier>,
    #[serde(default)]
    pub dsd_bit_rate_hz: Option<DsdBitRateHz>,
    pub dsd_bit_rate_44100_hz: Option<DsdBitRateHz>,
    pub dsd_bit_rate_48000_hz: Option<DsdBitRateHz>,
    pub sample_format: Option<SampleFormat>,
    pub valid_bits: Option<u8>,
    pub dither: Option<DitherType>,
}

/// `[gain]` section (`Design.md §6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GainConfig {
    #[serde(default)]
    pub preamp_db: f64,
    #[serde(default)]
    pub headroom: HeadroomMode,
    #[serde(default)]
    pub peak_analysis: PeakAnalysisMode,
    pub overload_policy: OverloadPolicy,
}

impl Default for GainConfig {
    fn default() -> Self {
        Self {
            preamp_db: 0.0,
            headroom: HeadroomMode::Profile,
            peak_analysis: PeakAnalysisMode::SinglePass,
            overload_policy: OverloadPolicy::MuteRecover,
        }
    }
}

/// `[resampler]` section (`Design.md §4.2`, `§6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResamplerConfig {
    pub preset: FilterPreset,
    #[serde(default = "default_filter_phase")]
    pub phase: FilterPhase,
    #[serde(default = "default_apodization")]
    pub apodization: Apodization,
}

const fn default_filter_phase() -> FilterPhase {
    FilterPhase::Linear
}

const fn default_apodization() -> Apodization {
    Apodization::Off
}

impl Default for ResamplerConfig {
    fn default() -> Self {
        Self {
            preset: FilterPreset::Balanced,
            phase: FilterPhase::Linear,
            apodization: Apodization::Off,
        }
    }
}

/// `[modulator]` section (`Design.md §4.3`, `§6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulatorConfig {
    pub order: u8,
}

impl Default for ModulatorConfig {
    fn default() -> Self {
        Self { order: 5 }
    }
}

/// `[clock]` section (`Design.md §1.5`, `§6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockConfig {
    pub mode: ClockMode,
    pub max_correction_ppm: Option<u32>,
}

/// `[runtime]` section (`Design.md §6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub latency_target_ms: Option<u32>,
    pub settle_ms: Option<u32>,
    pub on_device_loss: Option<DeviceLossPolicy>,
    pub fallback: Option<String>,
}

/// `[offline]` section (`Design.md §6.2`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineConfig {
    pub seed: Option<u64>,
    pub tail_policy: Option<TailPolicy>,
    pub write_report: Option<bool>,
    #[serde(default)]
    pub dsf_pad_mode: DsfPadMode,
}

/// Target override block inside a `[[rules]]` entry (`Design.md §6.1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleTargetOverride {
    pub format: TargetFormat,
    pub dsd_rate: Option<DsdMultiplier>,
    #[serde(default)]
    pub dsd_bit_rate_hz: Option<DsdBitRateHz>,
    pub pcm_rate_hz: Option<PcmRateHz>,
    pub sample_format: Option<SampleFormat>,
    pub valid_bits: Option<u8>,
}

/// Rule entry in `[[rules]]` (`Design.md §6.1`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleConfig {
    pub when_input_rate_family: RateFamily,
    pub apply_preset: FilterPreset,
    pub set_target: RuleTargetOverride,
}

/// Top-level `Schema v1` configuration (`Design.md §6`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub input: Option<InputConfig>,
    pub output: Option<OutputConfig>,
    pub target: Option<TargetConfig>,
    pub gain: Option<GainConfig>,
    pub resampler: Option<ResamplerConfig>,
    pub modulator: Option<ModulatorConfig>,
    pub clock: Option<ClockConfig>,
    pub runtime: Option<RuntimeConfig>,
    pub offline: Option<OfflineConfig>,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

/// Concrete resolution produced by `Settings::explain_for_input_rate` (`audigma config explain`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPipelinePlan {
    pub preset_def: &'static PresetDefinition,
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

    /// Returns default offline conversion settings tailored for the given output container.
    pub fn default_offline(container: OutputContainer) -> Self {
        match container {
            OutputContainer::Dsf => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: InputBackend::File,
                    node_name: None,
                    device: None,
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
                    backend: OutputBackend::File,
                    target_node: None,
                    device: None,
                    transport: None,
                    container: Some(OutputContainer::Dsf),
                    sample_format: None,
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    keep_audio_sink_ms: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Dsd,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: None,
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
                    dsd_rate: Some(DsdMultiplier::Dsd256),
                    dsd_bit_rate_hz: None,
                    dsd_bit_rate_44100_hz: None,
                    dsd_bit_rate_48000_hz: None,
                    sample_format: None,
                    valid_bits: None,
                    dither: None,
                }),
                gain: Some(GainConfig {
                    preamp_db: 0.0,
                    headroom: HeadroomMode::Profile,
                    peak_analysis: PeakAnalysisMode::TwoPass,
                    overload_policy: OverloadPolicy::Error,
                }),
                resampler: Some(ResamplerConfig {
                    preset: FilterPreset::Reference,
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                }),
                modulator: Some(ModulatorConfig { order: 5 }),
                clock: None,
                runtime: None,
                offline: Some(OfflineConfig {
                    seed: Some(12345),
                    tail_policy: Some(TailPolicy::TrimToDuration),
                    write_report: Some(false),
                    dsf_pad_mode: DsfPadMode::SpecZero,
                }),
                rules: Vec::new(),
            },
            OutputContainer::Wav => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: InputBackend::File,
                    node_name: None,
                    device: None,
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
                    backend: OutputBackend::File,
                    target_node: None,
                    device: None,
                    transport: None,
                    container: Some(OutputContainer::Wav),
                    sample_format: Some(SampleFormat::S32Le),
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    keep_audio_sink_ms: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Pcm,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: Some(PcmRateHz::PCM_192000),
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
                    dsd_rate: None,
                    dsd_bit_rate_hz: None,
                    dsd_bit_rate_44100_hz: None,
                    dsd_bit_rate_48000_hz: None,
                    sample_format: Some(SampleFormat::S32Le),
                    valid_bits: Some(24),
                    dither: Some(DitherType::Tpdf),
                }),
                gain: Some(GainConfig {
                    preamp_db: -3.0,
                    headroom: HeadroomMode::Profile,
                    peak_analysis: PeakAnalysisMode::SinglePass,
                    overload_policy: OverloadPolicy::Error,
                }),
                resampler: Some(ResamplerConfig {
                    preset: FilterPreset::Reference,
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                }),
                modulator: None,
                clock: None,
                runtime: None,
                offline: Some(OfflineConfig {
                    seed: None,
                    tail_policy: Some(TailPolicy::TrimToDuration),
                    write_report: Some(false),
                    dsf_pad_mode: DsfPadMode::SpecZero,
                }),
                rules: Vec::new(),
            },
            OutputContainer::Flac => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: InputBackend::File,
                    node_name: None,
                    device: None,
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
                    backend: OutputBackend::File,
                    target_node: None,
                    device: None,
                    transport: None,
                    container: Some(OutputContainer::Flac),
                    sample_format: Some(SampleFormat::S24Le),
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    keep_audio_sink_ms: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Pcm,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: Some(PcmRateHz::PCM_96000),
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
                    dsd_rate: None,
                    dsd_bit_rate_hz: None,
                    dsd_bit_rate_44100_hz: None,
                    dsd_bit_rate_48000_hz: None,
                    sample_format: Some(SampleFormat::S24Le),
                    valid_bits: Some(24),
                    dither: Some(DitherType::Tpdf),
                }),
                gain: Some(GainConfig {
                    preamp_db: -3.0,
                    headroom: HeadroomMode::Profile,
                    peak_analysis: PeakAnalysisMode::SinglePass,
                    overload_policy: OverloadPolicy::Error,
                }),
                resampler: Some(ResamplerConfig {
                    preset: FilterPreset::Reference,
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                }),
                modulator: None,
                clock: None,
                runtime: None,
                offline: Some(OfflineConfig {
                    seed: None,
                    tail_policy: Some(TailPolicy::TrimToDuration),
                    write_report: Some(false),
                    dsf_pad_mode: DsfPadMode::SpecZero,
                }),
                rules: Vec::new(),
            },
            OutputContainer::Raw => Self {
                schema_version: SCHEMA_VERSION_V1,
                input: Some(InputConfig {
                    backend: InputBackend::Stdio,
                    node_name: None,
                    device: None,
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
                    backend: OutputBackend::Stdio,
                    target_node: None,
                    device: None,
                    transport: Some(OutputTransport::Dop),
                    container: Some(OutputContainer::Raw),
                    sample_format: Some(SampleFormat::S24_3Le),
                    reserve_device: None,
                    buffer_ms: None,
                    period_ms: None,
                    volume_control: None,
                    device_wait_ms: None,
                    keep_audio_sink_ms: None,
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Dsd,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: None,
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
                    dsd_rate: Some(DsdMultiplier::Dsd128),
                    dsd_bit_rate_hz: None,
                    dsd_bit_rate_44100_hz: None,
                    dsd_bit_rate_48000_hz: None,
                    sample_format: None,
                    valid_bits: None,
                    dither: None,
                }),
                gain: Some(GainConfig {
                    preamp_db: 0.0,
                    headroom: HeadroomMode::Profile,
                    peak_analysis: PeakAnalysisMode::SinglePass,
                    overload_policy: OverloadPolicy::Error,
                }),
                resampler: Some(ResamplerConfig {
                    preset: FilterPreset::Balanced,
                    phase: FilterPhase::Linear,
                    apodization: Apodization::Off,
                }),
                modulator: Some(ModulatorConfig { order: 5 }),
                clock: None,
                runtime: None,
                offline: None,
                rules: Vec::new(),
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

        if self.target.is_none() && self.rules.is_empty() {
            return Err(SettingsError::Validation(
                "configuration must specify either [target] or non-empty [[rules]]",
            ));
        }

        if let Some(modulator) = &self.modulator {
            if modulator.order != 5 && modulator.order != 7 {
                return Err(SettingsError::Validation(
                    "[modulator] order must be 5 or 7",
                ));
            }
        }

        // Validate [input] channel positions and stdio constraints (Design.md §3.4 & §5.3)
        if let Some(input) = &self.input {
            if let Some(positions) = &input.channel_positions {
                if let Some(ch) = input.channels {
                    if positions.len() != ch as usize {
                        return Err(SettingsError::Validation(
                            "[input] channel_positions length must strictly match channels",
                        ));
                    }
                }
                for pos in positions {
                    if !is_valid_channel_position(pos) {
                        return Err(SettingsError::Validation(
                            "[input] channel_positions contains unrecognized channel position token",
                        ));
                    }
                }
            }

            if input.backend == InputBackend::Stdio
                && (input.sample_format.is_none()
                    || input.rate_hz.is_none()
                    || input.channels.is_none())
            {
                return Err(SettingsError::Validation(
                    "[input] backend = \"stdio\" requires sample_format, rate_hz, and channels",
                ));
            }
        }

        // Validate [output] & [target] cross-field constraints (Design.md §1.4 & §5.3)
        if let Some(output) = &self.output {
            if let Some(wait) = output.device_wait_ms {
                if wait < -1 {
                    return Err(SettingsError::Validation(
                        "[output] device_wait_ms must be >= -1",
                    ));
                }
            }
            if let Some(keep) = output.keep_audio_sink_ms {
                if keep < -1 {
                    return Err(SettingsError::Validation(
                        "[output] keep_audio_sink_ms must be >= -1",
                    ));
                }
            }

            // Non-seekable stdout cannot stream header-finalized DSF (Design.md §5.3)
            if output.backend == OutputBackend::Stdio
                && output.container == Some(OutputContainer::Dsf)
            {
                return Err(SettingsError::Validation(
                    "non-seekable [output] backend = \"stdio\" cannot write container = \"dsf\"",
                ));
            }

            if output.backend == OutputBackend::Stdio
                && (output.container != Some(OutputContainer::Raw)
                    || output.sample_format.is_none())
            {
                return Err(SettingsError::Validation(
                    "[output] backend = \"stdio\" requires container = \"raw\" and explicit sample_format",
                ));
            }

            // DSD output must never return to PipeWire normal PCM graph (Design.md §1.4)
            if let Some(target) = &self.target {
                if target.format == TargetFormat::Dsd && output.backend == OutputBackend::Pipewire {
                    return Err(SettingsError::Validation(
                        "[target] format = \"dsd\" requires exclusive ALSA, file, or stdio backend (cannot use output.backend = \"pipewire\")",
                    ));
                }
            }

            // Container vs Target format consistency
            if let (Some(container), Some(target)) = (output.container, &self.target) {
                match container {
                    OutputContainer::Dsf if target.format != TargetFormat::Dsd => {
                        return Err(SettingsError::Validation(
                            "output container = \"dsf\" requires [target] format = \"dsd\"",
                        ));
                    }
                    OutputContainer::Flac if target.format != TargetFormat::Pcm => {
                        return Err(SettingsError::Validation(
                            "output container = \"flac\" requires [target] format = \"pcm\"",
                        ));
                    }
                    _ => {}
                }
            }
        }

        // Validate [target] rate fields
        if let Some(target) = &self.target {
            match (target.format, target.rate_policy) {
                (TargetFormat::Pcm, RatePolicy::Fixed) => {
                    if target.pcm_rate_hz.is_none() {
                        return Err(SettingsError::Validation(
                            "[target] format = \"pcm\" with rate_policy = \"fixed\" requires pcm_rate_hz",
                        ));
                    }
                }
                (TargetFormat::Pcm, RatePolicy::SourceFamily) => {
                    if target.pcm_rate_44100_hz.is_none() || target.pcm_rate_48000_hz.is_none() {
                        return Err(SettingsError::Validation(
                            "[target] format = \"pcm\" with rate_policy = \"source-family\" requires pcm_rate_44100_hz and pcm_rate_48000_hz",
                        ));
                    }
                }
                (TargetFormat::Dsd, RatePolicy::Fixed) => {
                    if target.dsd_rate.is_none() && target.dsd_bit_rate_hz.is_none() {
                        return Err(SettingsError::Validation(
                            "[target] format = \"dsd\" with rate_policy = \"fixed\" requires dsd_rate (or dsd_bit_rate_hz)",
                        ));
                    }
                }
                (TargetFormat::Dsd, RatePolicy::SourceFamily) => {
                    if target.dsd_rate.is_none()
                        && target.dsd_bit_rate_44100_hz.is_none()
                        && target.dsd_bit_rate_48000_hz.is_none()
                        && target.dsd_bit_rate_hz.is_none()
                    {
                        return Err(SettingsError::Validation(
                            "[target] format = \"dsd\" with rate_policy = \"source-family\" requires dsd_rate or DSD bit rate specifications",
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns any non-fatal configuration warnings (e.g. timeout inconsistencies).
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(output) = &self.output {
            let wait = output.device_wait_ms();
            let keep = output.keep_audio_sink_ms();
            if wait > 0 && keep > wait {
                warnings.push(format!(
                    "[output] keep_audio_sink_ms ({keep} ms) is greater than device_wait_ms ({wait} ms); daemon will exit before sink grace period expires"
                ));
            }
        }
        warnings
    }

    /// Evaluates `[[rules]]` in first-match order (or falls back to `[target]` + `rate_policy`)
    /// to resolve the concrete `PresetDefinition`, `StreamSpec`, and `DspConfiguration` (`Design.md §6.1`).
    pub fn explain_for_input_rate(
        &self,
        input_rate: PcmRateHz,
    ) -> Result<ResolvedPipelinePlan, SettingsError> {
        let input_family = input_rate.rate_family();
        let channels = self.input.as_ref().and_then(|i| i.channels).unwrap_or(2);
        let resampler = self.resampler.clone().unwrap_or_default();
        let gain = self.gain.clone().unwrap_or_default();

        // 1. Check first-match [[rules]]
        if let Some(rule) = self
            .rules
            .iter()
            .find(|r| r.when_input_rate_family == input_family)
        {
            let preset_def = presets::by_preset(rule.apply_preset);
            let target_spec = match rule.set_target.format {
                TargetFormat::Dsd => {
                    let bit_rate = rule
                        .set_target
                        .dsd_rate
                        .and_then(|m| m.to_bit_rate(input_family))
                        .or(rule.set_target.dsd_bit_rate_hz)
                        .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                    TargetRateSpec::Dsd { bit_rate }
                }
                TargetFormat::Pcm => {
                    let rate = rule
                        .set_target
                        .pcm_rate_hz
                        .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                    TargetRateSpec::Pcm {
                        rate,
                        sample_format: rule.set_target.sample_format.unwrap_or(SampleFormat::F32),
                        valid_bits: rule.set_target.valid_bits,
                    }
                }
            };

            let mut dsp_config = preset_def.to_default_dsp_config(
                resampler.phase,
                resampler.apodization,
                gain.preamp_db,
                0.0,
                gain.overload_policy,
            );
            if gain.headroom == HeadroomMode::Manual {
                dsp_config.profile_headroom_db = 0.0;
            }

            return Ok(ResolvedPipelinePlan {
                preset_def,
                stream_spec: StreamSpec {
                    input_rate,
                    channels,
                    target: target_spec,
                },
                dsp_config,
            });
        }

        // 2. Fallback to [target] + rate_policy
        let target = self
            .target
            .as_ref()
            .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
        let preset_def = presets::by_preset(resampler.preset);

        let target_spec = match (target.format, target.rate_policy) {
            (TargetFormat::Pcm, RatePolicy::Fixed) => TargetRateSpec::Pcm {
                rate: target
                    .pcm_rate_hz
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?,
                sample_format: target.sample_format.unwrap_or(SampleFormat::F32),
                valid_bits: target.valid_bits,
            },
            (TargetFormat::Pcm, RatePolicy::SourceFamily) => {
                let rate = match input_family {
                    RateFamily::Hz44100 => target.pcm_rate_44100_hz,
                    RateFamily::Hz48000 | RateFamily::Other => target.pcm_rate_48000_hz,
                }
                .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                TargetRateSpec::Pcm {
                    rate,
                    sample_format: target.sample_format.unwrap_or(SampleFormat::S32Le),
                    valid_bits: target.valid_bits,
                }
            }
            (TargetFormat::Dsd, RatePolicy::Fixed) => TargetRateSpec::Dsd {
                bit_rate: target
                    .dsd_rate
                    .and_then(|m| m.to_bit_rate(RateFamily::Hz44100))
                    .or(target.dsd_bit_rate_hz)
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?,
            },
            (TargetFormat::Dsd, RatePolicy::SourceFamily) => {
                let bit_rate = target
                    .dsd_rate
                    .and_then(|m| m.to_bit_rate(input_family))
                    .or_else(|| match input_family {
                        RateFamily::Hz44100 => {
                            target.dsd_bit_rate_44100_hz.or(target.dsd_bit_rate_hz)
                        }
                        RateFamily::Hz48000 | RateFamily::Other => {
                            target.dsd_bit_rate_48000_hz.or(target.dsd_bit_rate_hz)
                        }
                    })
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?;
                TargetRateSpec::Dsd { bit_rate }
            }
        };

        let mut dsp_config = preset_def.to_default_dsp_config(
            resampler.phase,
            resampler.apodization,
            gain.preamp_db,
            0.0,
            gain.overload_policy,
        );
        if let Some(dither) = target.dither {
            dsp_config.dither = dither;
        }
        if let Some(modulator) = &self.modulator {
            dsp_config.modulator_order = modulator.order;
        }
        if gain.headroom == HeadroomMode::Manual && target.format == TargetFormat::Pcm {
            dsp_config.profile_headroom_db = 0.0;
        }

        Ok(ResolvedPipelinePlan {
            preset_def,
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

    #[test]
    fn test_design_md_section_6_1_rules_fixture() {
        let toml = r#"
schema_version = 1

[[rules]]
when_input_rate_family = "44100"   # 44.1k, 88.2k, 176.4k, 352.8k
apply_preset = "reference"
set_target = { format = "dsd", dsd_rate = "dsd256" }

[[rules]]
when_input_rate_family = "48000"   # 48k, 96k, 192k, 384k
apply_preset = "reference"
set_target = { format = "pcm", pcm_rate_hz = 768000 }
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let plan_44k = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(plan_44k.preset_def, &presets::PRESET_REFERENCE);
        assert_eq!(
            plan_44k.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );

        let plan_48k = settings
            .explain_for_input_rate(PcmRateHz::PCM_48000)
            .unwrap();
        assert_eq!(plan_48k.preset_def, &presets::PRESET_REFERENCE);
        assert_eq!(
            plan_48k.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_768000,
                sample_format: SampleFormat::F32,
                valid_bits: None
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
rate_policy = "fixed"
pcm_rate_hz = 192000
sample_format = "f32"

[gain]
preamp_db = -3.0
overload_policy = "mute-recover"

[resampler]
preset = "balanced"
phase = "linear"
apodization = "off"

[clock]
mode = "shared-graph"

[runtime]
latency_target_ms = 40
on_device_loss = "wait"
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
        assert_eq!(plan.preset_def, &presets::PRESET_BALANCED);
        assert_eq!(
            plan.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_192000,
                sample_format: SampleFormat::F32,
                valid_bits: None,
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
rate_policy = "fixed"
dsd_rate = "dsd256"

[gain]
preamp_db = 0.0
headroom = "profile"
overload_policy = "mute-recover"

[resampler]
preset = "balanced"
phase = "minimum"
apodization = "off"

[modulator]
order = 5

[clock]
mode = "adaptive-input"
max_correction_ppm = 500

[runtime]
settle_ms = 50
on_device_loss = "wait"
fallback = "none"
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        assert!(settings.output.as_ref().unwrap().reserve_device_enabled());
        assert!(settings.input.as_ref().unwrap().allow_dsd());
        let plan = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(plan.preset_def, &presets::PRESET_BALANCED);
        assert_eq!(plan.dsp_config.phase, FilterPhase::Minimum);
        assert_eq!(
            plan.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256,
            }
        );
    }

    #[test]
    fn test_design_md_fixture_c_loopback_alsa_pcm() {
        let toml = r#"
schema_version = 1

[input]
backend = "alsa"
device = "hw:Loopback,1,0"
channels = 2
channel_positions = [ "FL", "FR" ]

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "pcm"
reserve_device = true
volume_control = "hardware-software-fallback"
device_wait_ms = -1
keep_audio_sink_ms = 5000
buffer_ms = 40
period_ms = 5

[target]
format = "pcm"
rate_policy = "source-family"
pcm_rate_44100_hz = 705600
pcm_rate_48000_hz = 768000
sample_format = "s32le"
valid_bits = 24
dither = "tpdf"

[gain]
preamp_db = -4.0
overload_policy = "mute-recover"

[resampler]
preset = "reference"
phase = "linear"
apodization = "off"

[clock]
mode = "adaptive-input"
max_correction_ppm = 500
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let plan_44 = settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan_44.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_705600,
                sample_format: SampleFormat::S32Le,
                valid_bits: Some(24),
            }
        );

        let plan_48 = settings
            .explain_for_input_rate(PcmRateHz::PCM_96000)
            .unwrap();
        assert_eq!(
            plan_48.stream_spec.target,
            TargetRateSpec::Pcm {
                rate: PcmRateHz::PCM_768000,
                sample_format: SampleFormat::S32Le,
                valid_bits: Some(24),
            }
        );
    }

    #[test]
    fn test_design_md_fixture_d_offline_dsf() {
        let toml = r#"
schema_version = 1

[input]
backend = "file"

[output]
backend = "file"
container = "dsf"

[target]
format = "dsd"
rate_policy = "fixed"
dsd_rate = "dsd256"

[gain]
preamp_db = 0.0
headroom = "profile"
peak_analysis = "two-pass"
overload_policy = "error"

[resampler]
preset = "reference"
phase = "linear"
apodization = "off"

[modulator]
order = 5

[offline]
seed = 12345
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
    fn test_design_md_fixture_e_stdio_dop() {
        let toml = r#"
schema_version = 1

[input]
backend = "stdio"
sample_format = "s16le"
rate_hz = 44100
channels = 2

[output]
backend = "stdio"
container = "raw"
transport = "dop"
sample_format = "s24_3le"

[target]
format = "dsd"
rate_policy = "fixed"
dsd_rate = "dsd128"

[gain]
headroom = "profile"
overload_policy = "error"

[resampler]
preset = "balanced"
phase = "linear"
apodization = "off"

[modulator]
order = 5
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
    fn test_rejects_dsd_over_pipewire_and_nonseekable_stdout_dsf() {
        let dsd_over_pw = r#"
schema_version = 1
[output]
backend = "pipewire"
[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 2822400
"#;
        assert!(Settings::from_toml_str(dsd_over_pw).is_err());

        let stdout_dsf = r#"
schema_version = 1
[output]
backend = "stdio"
container = "dsf"
[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 2822400
"#;
        assert!(Settings::from_toml_str(stdout_dsf).is_err());
    }

    #[test]
    fn test_output_container_flac_and_default_offline() {
        let flac_toml = r#"
schema_version = 1

[input]
backend = "file"

[output]
backend = "file"
container = "flac"
sample_format = "s24le"

[target]
format = "pcm"
rate_policy = "fixed"
pcm_rate_hz = 192000
sample_format = "s24le"
valid_bits = 24
dither = "tpdf"

[gain]
preamp_db = -3.0
overload_policy = "error"

[resampler]
preset = "reference"
"#;
        let settings = Settings::from_toml_str(flac_toml).unwrap();
        assert_eq!(
            settings.output.as_ref().unwrap().container,
            Some(OutputContainer::Flac)
        );

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
    fn test_rejects_container_target_mismatch() {
        let flac_dsd = r#"
schema_version = 1
[output]
backend = "file"
container = "flac"
[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 11289600
"#;
        assert!(Settings::from_toml_str(flac_dsd).is_err());

        let dsf_pcm = r#"
schema_version = 1
[output]
backend = "file"
container = "dsf"
[target]
format = "pcm"
rate_policy = "fixed"
pcm_rate_hz = 192000
"#;
        assert!(Settings::from_toml_str(dsf_pcm).is_err());
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
            backend: InputBackend::Pipewire,
            node_name: None,
            device: None,
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
    fn test_device_wait_and_keep_sink_timeouts_and_warnings() {
        let valid_toml = r#"
schema_version = 1
[output]
backend = "alsa"
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
device_wait_ms = 1000
keep_audio_sink_ms = 5000
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let warning_settings = Settings::from_toml_str(warning_toml).unwrap();
        let warnings = warning_settings.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("is greater than device_wait_ms"));

        // Reject values < -1
        let invalid_wait = r#"
schema_version = 1
[output]
backend = "alsa"
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
        // Test fixed policy with dsd_rate = "dsd256"
        let fixed_dsd = r#"
schema_version = 1
[target]
format = "dsd"
rate_policy = "fixed"
dsd_rate = "dsd256"
"#;
        let fixed_settings = Settings::from_toml_str(fixed_dsd).unwrap();
        let plan_44 = fixed_settings
            .explain_for_input_rate(PcmRateHz::PCM_44100)
            .unwrap();
        assert_eq!(
            plan_44.stream_spec.target,
            TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD256
            }
        );

        // Test source-family policy with dsd_rate = "dsd256"
        let family_dsd = r#"
schema_version = 1
[target]
format = "dsd"
rate_policy = "source-family"
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
rate_policy = "source-family"
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
    fn test_allow_dsd_and_dsd_input_config() {
        let toml = r#"
schema_version = 1
[input]
backend = "pipewire"
allow_dsd = true
[input.dsd]
max_intermediate_rate_hz = 352800
decimation_preset = "reference"
[target]
format = "pcm"
pcm_rate_hz = 96000
"#;
        let settings = Settings::from_toml_str(toml).unwrap();
        let input = settings.input.as_ref().unwrap();
        assert!(input.allow_dsd());
        let dsd = input.dsd.as_ref().unwrap();
        assert_eq!(dsd.max_intermediate_rate_hz, Some(PcmRateHz::PCM_352800));
        assert_eq!(dsd.decimation_preset, Some(FilterPreset::Reference));
    }
}
