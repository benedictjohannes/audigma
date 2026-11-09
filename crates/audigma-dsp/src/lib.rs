//! `audigma-dsp`: Pure numerical processing, strongly typed rate domains, filter/modulator tables,
//! and PCM/DSD DSP primitives (`Design.md §1`, `§2`, `§4`).
//!
//! Zero OS, file, or network I/O dependencies.

pub mod config;
pub mod dc_blocker;
pub mod defaults;
pub mod fir;
pub mod ftz;
pub mod modulator;
pub mod quantizer;
pub mod rates;
pub mod tables;
pub mod volume;

pub use tables::{
    CrfbCoefficients, CrfbCoefficients5, CrfbCoefficients7, HALFBAND_MAX_STAGES, HalfBandTaps,
    SRC_ASRC_PHASES, SRC_CUTOFF, SRC_KAISER_BETA, SRC_PASS_EDGE, SRC_STOP_EDGE, SRC_TAPS_UNITY,
    get_crfb_coefficients, get_crfb5_coefficients, get_crfb7_coefficients,
    get_halfband_minimum_taps, get_halfband_odd_taps, get_interpolation_stage_filter,
};

pub use config::{
    Apodization, DitherType, DspConfiguration, FilterPhase, OverloadPolicy, SampleFormat,
    StreamSpec, TargetFormat, TargetRateSpec,
};
pub use dc_blocker::{DC_BLOCKER_CORNER_HZ, DcBlocker};
pub use defaults::{
    DEFAULT_MODULATOR_ORDER, DEFAULT_SAFETY_MARGIN_DB, DSD_IDLE_BYTE, DSD_IDLE_U32_BE,
    FILTER_REJECTION_TARGET_DB,
};
pub use fir::{
    DSD_BYTE_TO_FLOATS, FirError, HalfBandCascade, HalfBandDecimationCascade,
    HalfBandDecimationStage, HalfBandStage, PolyphaseInterpolationStage, PolyphaseMode,
    PolyphaseResampler, SRC_MAX_RATIONAL_PHASES, src_band_limit_scale, src_prototype,
    src_taps_for_scale, unpack_dsd_bytes_to_floats,
};
pub use ftz::ScopedFtzDaz;
pub use modulator::{
    CrfbCore, CrfbCore5, CrfbCore7, CrfbModulator, CrfbModulator5, CrfbModulator7,
    GenericCrfbModulator, Modulator, ModulatorError, STABILITY_THRESHOLD, TpdfDither,
    max_stable_input_dbfs,
};
pub use quantizer::{DEFAULT_PCM_DITHER_SEED, PCM_DITHER_AMPLITUDE_LSB, PcmQuantizer};
pub use rates::{
    DEFAULT_AUDIO_BASEBAND_HZ, DsdBitRateHz, DsdMultiplier, PcmRateHz, RateFamily,
    TransportFrameRateHz, select_dsd_intermediate_rate,
};
pub use volume::{
    DEFAULT_SOFTWARE_MIN_DB, MAX_VOLUME_DB, MIN_VOLUME_DB, alsamixer_db_to_percentage,
    alsamixer_percentage_to_db,
};
