//! `audigma-dsp`: Pure numerical processing, strongly typed rate domains, filter/modulator presets,
//! and PCM/DSD DSP primitives (`Design.md §1`, `§2`, `§4`).
//!
//! Zero OS, file, or network I/O dependencies.

pub mod config;
pub mod presets;
pub mod rates;
pub mod tables;

pub use tables::{
    CrfbCoefficients, get_crfb_coefficients, get_halfband_stage0_taps, get_halfband_stage1_taps,
};

pub use config::{
    Apodization, DitherType, DspConfiguration, FilterPhase, FilterPreset, OverloadPolicy,
    SampleFormat, StreamSpec, TargetFormat, TargetRateSpec,
};
pub use presets::{
    ALL_PRESETS, DSD_IDLE_BYTE, DSD_IDLE_U32_BE, ModulatorProfileSpec, PRESET_BALANCED,
    PRESET_LOW_LATENCY, PRESET_REFERENCE, PresetDefinition,
};
pub use rates::{
    DEFAULT_AUDIO_BASEBAND_HZ, DsdBitRateHz, DsdMultiplier, PcmRateHz, RateFamily,
    TransportFrameRateHz,
};
