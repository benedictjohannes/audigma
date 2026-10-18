//! `audigma-settings`: Schema v1 TOML configuration (`Settings`), `[[rules]]` evaluation,
//! preset resolution, and architectural invariant validation (`Design.md §6`).

use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, DspConfiguration, FilterPhase, FilterPreset,
    OverloadPolicy, PcmRateHz, PresetDefinition, RateFamily, SampleFormat, StreamSpec,
    TargetFormat, TargetRateSpec, presets,
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
}

impl OutputConfig {
    /// Returns whether D-Bus `org.freedesktop.ReserveDevice1` reservation is enabled (`true` by default for ALSA).
    pub fn reserve_device_enabled(&self) -> bool {
        self.reserve_device
            .unwrap_or(self.backend == OutputBackend::Alsa)
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
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Dsd,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: None,
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
                    dsd_bit_rate_hz: Some(DsdBitRateHz::DSD256),
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
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Pcm,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: Some(PcmRateHz::PCM_192000),
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
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
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Pcm,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: Some(PcmRateHz::PCM_96000),
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
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
                }),
                target: Some(TargetConfig {
                    format: TargetFormat::Dsd,
                    rate_policy: RatePolicy::Fixed,
                    pcm_rate_hz: None,
                    pcm_rate_44100_hz: None,
                    pcm_rate_48000_hz: None,
                    dsd_bit_rate_hz: Some(DsdBitRateHz::DSD128),
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

        // Validate [input] stdio constraints (Design.md §5.3)
        if let Some(input) = &self.input {
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
                    if target.dsd_bit_rate_hz.is_none() {
                        return Err(SettingsError::Validation(
                            "[target] format = \"dsd\" with rate_policy = \"fixed\" requires dsd_bit_rate_hz",
                        ));
                    }
                }
                (TargetFormat::Dsd, RatePolicy::SourceFamily) => {
                    if target.dsd_bit_rate_44100_hz.is_none()
                        && target.dsd_bit_rate_48000_hz.is_none()
                        && target.dsd_bit_rate_hz.is_none()
                    {
                        return Err(SettingsError::Validation(
                            "[target] format = \"dsd\" with rate_policy = \"source-family\" requires DSD bit rate specifications",
                        ));
                    }
                }
            }
        }

        Ok(())
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
                        .dsd_bit_rate_hz
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
                    .dsd_bit_rate_hz
                    .ok_or(SettingsError::UnresolvedTargetRate(input_rate))?,
            },
            (TargetFormat::Dsd, RatePolicy::SourceFamily) => {
                let bit_rate = match input_family {
                    RateFamily::Hz44100 => target.dsd_bit_rate_44100_hz.or(target.dsd_bit_rate_hz),
                    RateFamily::Hz48000 | RateFamily::Other => {
                        target.dsd_bit_rate_48000_hz.or(target.dsd_bit_rate_hz)
                    }
                }
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
set_target = { format = "dsd", dsd_bit_rate_hz = 11289600 }

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

[output]
backend = "pipewire"
target_node = "alsa_output.usb-FiiO_K17-00.analog-stereo"
transport = "pcm"

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

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "native"
reserve_device = true
buffer_ms = 40
period_ms = 5

[target]
format = "dsd"
rate_policy = "fixed"
dsd_bit_rate_hz = 11289600

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

[output]
backend = "alsa"
device = "hw:CARD=K17,DEV=0"
transport = "pcm"
reserve_device = true
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
dsd_bit_rate_hz = 11289600

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
dsd_bit_rate_hz = 5644800

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
}
