//! `audigma-dsp`: Pure numerical processing, strongly typed rate domains, filter/modulator presets,
//! and PCM/DSD DSP primitives (`Design.md §1`, `§2`, `§4`).
//!
//! Zero OS, file, or network I/O dependencies.

pub mod config;
pub mod fir;
pub mod ftz;
pub mod modulator;
pub mod presets;
pub mod rates;
pub mod tables;
pub mod volume;

pub use tables::{
    CrfbCoefficients, CrfbCoefficients5, CrfbCoefficients7, HalfBandTaps, SRC_ASRC_256_PHASES,
    SRC_RATIONAL_160_PHASES, get_crfb_coefficients, get_crfb5_coefficients, get_crfb7_coefficients,
    get_halfband_stage0_filter, get_halfband_stage0_taps, get_halfband_stage1_filter,
    get_halfband_stage1_taps,
};

pub use config::{
    Apodization, DitherType, DspConfiguration, FilterPhase, FilterPreset, OverloadPolicy,
    SampleFormat, StreamSpec, TargetFormat, TargetRateSpec,
};
pub use fir::{
    DSD_BYTE_TO_FLOATS, FirError, HalfBandCascade, HalfBandDecimationCascade,
    HalfBandDecimationStage, HalfBandStage, PolyphaseInterpolationStage, PolyphaseMode,
    PolyphaseResampler, unpack_dsd_bytes_to_floats,
};
pub use ftz::ScopedFtzDaz;
pub use modulator::{
    CrfbCore, CrfbCore5, CrfbCore7, CrfbModulator, CrfbModulator5, CrfbModulator7,
    GenericCrfbModulator, Modulator, ModulatorError, STABILITY_THRESHOLD, TpdfDither,
};
pub use presets::{
    ALL_PRESETS, DSD_IDLE_BYTE, DSD_IDLE_U32_BE, ModulatorProfileSpec, PRESET_BALANCED,
    PRESET_LOW_LATENCY, PRESET_REFERENCE, PresetDefinition,
};
pub use rates::{
    DEFAULT_AUDIO_BASEBAND_HZ, DsdBitRateHz, DsdMultiplier, PcmRateHz, RateFamily,
    TransportFrameRateHz, select_dsd_intermediate_rate,
};
pub use volume::{
    DEFAULT_SOFTWARE_MIN_DB, MAX_VOLUME_DB, MIN_VOLUME_DB, alsamixer_db_to_percentage,
    alsamixer_percentage_to_db,
};
