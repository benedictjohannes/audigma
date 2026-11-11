//! Concrete PipeWire audio backends (`Design.md §1.4`, `§3.3`, `§7`).
//!
//! Provides:
//! - `PipeWireVirtualSink`: Registers a virtual audio sink node (`audigma_sink` with `media.class = "Audio/Sink"`)
//!   in the PipeWire graph, receiving PCM frames from desktop applications and players, and delivering them
//!   into an `audigma-engine` SPSC ring buffer.
//! - `PipeWirePcmOutput`: Plays high-rate PCM (`f32` stereo) directly into the PipeWire graph (`shared-graph` clock),
//!   optionally targeting a specific node (e.g. `alsa_output...`).

use audigma_dsp::{DsdBitRateHz, PcmRateHz};
use audigma_engine::VolumeUpdate;
use audigma_engine::queue::TaggedChunk;
use audigma_engine::traits::{AudioInput, AudioOutput, DsdDecodeConfig, EngineError};
use audigma_format::AudioStreamInfo;
use audigma_settings::{InputConfig, RateMode};
use libspa::param::audio::{AudioFormat, AudioInfoRaw};
use libspa::pod::deserialize::PodDeserializer;
use libspa::pod::{ChoiceValue, Pod, Value, ValueArray};
use libspa::utils::{ChoiceEnum, Direction};
use pipewire::properties::properties;
use pipewire::stream::{StreamBox, StreamFlags};
use rtrb::{Consumer, Producer, RingBuffer};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use thiserror::Error;

mod capture_pull;
use capture_pull::{PullClock, PullDemand, TriggerDone, advance_driver_clock};

/// Node property that marks audigma's own virtual sinks, so probes can skip them.
pub(crate) const AUDIGMA_NODE_PROP: &str = "audigma.sink";

/// Default `node.name` prefix of audigma's virtual sinks (`audigma_sink`, `audigma_sink_<instance>`).
pub(crate) const AUDIGMA_SINK_PREFIX: &str = "audigma_sink";

/// A pull whose cycle has not completed within this window is presumed lost
/// (e.g. the follower vanished mid-cycle) so capture cannot stall forever.
const PULL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Errors arising from PipeWire I/O backend operations.
#[derive(Debug, Error)]
pub enum PipeWireError {
    #[error("PipeWire native error: {0}")]
    Native(#[from] pipewire::Error),

    #[error("Failed to build SPA format POD: {0}")]
    FormatPod(String),

    #[error("PipeWire stream disconnected or failed")]
    StreamFailed,
}

impl From<PipeWireError> for EngineError {
    fn from(err: PipeWireError) -> Self {
        match err {
            PipeWireError::StreamFailed => {
                EngineError::DeviceDisconnected("PipeWire stream disconnected or failed".into())
            }
            other => EngineError::AudioBackend {
                backend: "pipewire",
                message: other.to_string(),
            },
        }
    }
}

/// Maps standard channel position strings to SPA channel position constants.
pub fn channel_position_to_spa(pos: &str) -> u32 {
    match pos.trim().to_ascii_uppercase().as_str() {
        "MONO" => libspa::sys::SPA_AUDIO_CHANNEL_MONO,
        "FL" => libspa::sys::SPA_AUDIO_CHANNEL_FL,
        "FR" => libspa::sys::SPA_AUDIO_CHANNEL_FR,
        "FC" => libspa::sys::SPA_AUDIO_CHANNEL_FC,
        "LFE" => libspa::sys::SPA_AUDIO_CHANNEL_LFE,
        "SL" => libspa::sys::SPA_AUDIO_CHANNEL_SL,
        "SR" => libspa::sys::SPA_AUDIO_CHANNEL_SR,
        "FLC" => libspa::sys::SPA_AUDIO_CHANNEL_FLC,
        "FRC" => libspa::sys::SPA_AUDIO_CHANNEL_FRC,
        "RC" => libspa::sys::SPA_AUDIO_CHANNEL_RC,
        "RL" => libspa::sys::SPA_AUDIO_CHANNEL_RL,
        "RR" => libspa::sys::SPA_AUDIO_CHANNEL_RR,
        _ => libspa::sys::SPA_AUDIO_CHANNEL_UNKNOWN,
    }
}

/// Helper function to build dynamic SPA EnumFormat POD choice objects for multi-rate and multi-channel audio.
pub fn build_format_pod(
    rate_hz: u32,
    channels: u32,
    channel_positions: &[String],
    rate_mode: RateMode,
    allowed_rates: &[u32],
) -> Result<Vec<u8>, PipeWireError> {
    let mut position_ids = Vec::with_capacity(channels as usize);
    for pos in channel_positions.iter().take(channels as usize) {
        position_ids.push(libspa::utils::Id(channel_position_to_spa(pos)));
    }
    while position_ids.len() < channels as usize {
        position_ids.push(libspa::utils::Id(libspa::sys::SPA_AUDIO_CHANNEL_UNKNOWN));
    }

    let rate_value = if rate_mode == RateMode::Dynamic && !allowed_rates.is_empty() {
        let (def, alternatives): (i32, Vec<i32>) = if allowed_rates.contains(&rate_hz) {
            (
                rate_hz as i32,
                allowed_rates
                    .iter()
                    .copied()
                    .filter(|&r| r != rate_hz)
                    .map(|r| r as i32)
                    .collect(),
            )
        } else {
            (
                allowed_rates[0] as i32,
                allowed_rates[1..].iter().map(|&r| r as i32).collect(),
            )
        };

        if alternatives.is_empty() {
            libspa::pod::Value::Int(def)
        } else {
            libspa::pod::Value::Choice(libspa::pod::ChoiceValue::Int(libspa::utils::Choice(
                libspa::utils::ChoiceFlags::empty(),
                libspa::utils::ChoiceEnum::Enum {
                    default: def,
                    alternatives,
                },
            )))
        }
    } else {
        libspa::pod::Value::Int(rate_hz as i32)
    };

    let props = vec![
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_mediaType,
            libspa::pod::Value::Id(libspa::utils::Id(libspa::sys::SPA_MEDIA_TYPE_audio)),
        ),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_mediaSubtype,
            libspa::pod::Value::Id(libspa::utils::Id(libspa::sys::SPA_MEDIA_SUBTYPE_raw)),
        ),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_AUDIO_format,
            libspa::pod::Value::Id(libspa::utils::Id(AudioFormat::F32LE.as_raw())),
        ),
        libspa::pod::Property::new(libspa::sys::SPA_FORMAT_AUDIO_rate, rate_value),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_AUDIO_channels,
            libspa::pod::Value::Int(channels as i32),
        ),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_AUDIO_position,
            libspa::pod::Value::ValueArray(libspa::pod::ValueArray::Id(position_ids)),
        ),
    ];

    let obj = libspa::pod::Object {
        type_: libspa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: libspa::param::ParamType::EnumFormat.as_raw(),
        properties: props,
    };

    libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &libspa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| PipeWireError::FormatPod(format!("{e:?}")))
}

/// Parsed format description from an SPA Format POD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedFormatInfo {
    pub is_dsd: bool,
    pub is_lsb: bool,
    pub interleave: i32,
    pub rate: u32,
    pub channels: u32,
}

/// Helper function to build dynamic SPA EnumFormat POD choice objects for native DSD audio (`audio/dsd`) (`Design.md §1.1`, `§5.1`).
pub fn build_dsd_format_pod(
    channels: u32,
    channel_positions: &[String],
    default_dsd_byte_rate: u32,
    allowed_dsd_byte_rates: &[u32],
) -> Result<Vec<u8>, PipeWireError> {
    let mut position_ids = Vec::with_capacity(channels as usize);
    for pos in channel_positions.iter().take(channels as usize) {
        position_ids.push(libspa::utils::Id(channel_position_to_spa(pos)));
    }
    while position_ids.len() < channels as usize {
        position_ids.push(libspa::utils::Id(libspa::sys::SPA_AUDIO_CHANNEL_UNKNOWN));
    }

    let rate_value = if !allowed_dsd_byte_rates.is_empty() {
        let (def, alternatives): (i32, Vec<i32>) =
            if allowed_dsd_byte_rates.contains(&default_dsd_byte_rate) {
                (
                    default_dsd_byte_rate as i32,
                    allowed_dsd_byte_rates
                        .iter()
                        .copied()
                        .filter(|&r| r != default_dsd_byte_rate)
                        .map(|r| r as i32)
                        .collect(),
                )
            } else {
                (
                    allowed_dsd_byte_rates[0] as i32,
                    allowed_dsd_byte_rates[1..]
                        .iter()
                        .map(|&r| r as i32)
                        .collect(),
                )
            };

        if alternatives.is_empty() {
            libspa::pod::Value::Int(def)
        } else {
            libspa::pod::Value::Choice(libspa::pod::ChoiceValue::Int(libspa::utils::Choice(
                libspa::utils::ChoiceFlags::empty(),
                libspa::utils::ChoiceEnum::Enum {
                    default: def,
                    alternatives,
                },
            )))
        }
    } else {
        libspa::pod::Value::Int(default_dsd_byte_rate as i32)
    };

    let bitorder_value =
        libspa::pod::Value::Choice(libspa::pod::ChoiceValue::Id(libspa::utils::Choice(
            libspa::utils::ChoiceFlags::empty(),
            libspa::utils::ChoiceEnum::Enum {
                default: libspa::utils::Id(libspa::sys::SPA_PARAM_BITORDER_msb),
                alternatives: vec![
                    libspa::utils::Id(libspa::sys::SPA_PARAM_BITORDER_msb),
                    libspa::utils::Id(libspa::sys::SPA_PARAM_BITORDER_lsb),
                ],
            },
        )));

    let props = vec![
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_mediaType,
            libspa::pod::Value::Id(libspa::utils::Id(libspa::sys::SPA_MEDIA_TYPE_audio)),
        ),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_mediaSubtype,
            libspa::pod::Value::Id(libspa::utils::Id(libspa::sys::SPA_MEDIA_SUBTYPE_dsd)),
        ),
        libspa::pod::Property::new(libspa::sys::SPA_FORMAT_AUDIO_bitorder, bitorder_value),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_AUDIO_interleave,
            libspa::pod::Value::Int(1), // Interleaved (interleave = 1 byte per channel)
        ),
        libspa::pod::Property::new(libspa::sys::SPA_FORMAT_AUDIO_rate, rate_value),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_AUDIO_channels,
            libspa::pod::Value::Int(channels as i32),
        ),
        libspa::pod::Property::new(
            libspa::sys::SPA_FORMAT_AUDIO_position,
            libspa::pod::Value::ValueArray(libspa::pod::ValueArray::Id(position_ids)),
        ),
    ];

    let obj = libspa::pod::Object {
        type_: libspa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: libspa::param::ParamType::EnumFormat.as_raw(),
        properties: props,
    };

    libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &libspa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| PipeWireError::FormatPod(format!("{e:?}")))
}

/// Computes a power-of-two PipeWire quantum (in frames) scaled to the stream rate (~21–23 ms).
pub fn quantum_frames_for_rate(rate_hz: u32) -> u32 {
    let mult = (rate_hz.max(44_100) / 44_100).next_power_of_two();
    1024_u32.saturating_mul(mult).clamp(1024, 65_536)
}

/// Default input cushion requested before the engine resumes consuming after startup
/// or a rate/format switch if not configured from the output buffer duration.
const DEFAULT_INPUT_PREROLL_SECS: f64 = 0.090;

fn input_preroll_ready(
    buffered_secs: f64,
    target_secs: f64,
    is_dsd: bool,
    occupied_slots: usize,
    capacity: usize,
) -> bool {
    buffered_secs >= target_secs
        // Small capture buffers can fill the DSD ring before reaching the time
        // cushion. Start consuming with half the slots still free instead of
        // waiting forever for a duration the ring cannot hold.
        || (is_dsd && occupied_slots >= (capacity / 2).max(1))
}

/// Helper function to build an SPA `ParamBuffers` POD sized for the given quantum and frame stride.
pub fn build_buffers_pod(quantum_frames: u32, stride: u32) -> Result<Vec<u8>, PipeWireError> {
    let stride = stride.max(1) as i32;
    let default_size = (quantum_frames.max(256) as i32).saturating_mul(stride);
    let min_size = 16_i32.saturating_mul(stride);
    let max_size = default_size.saturating_mul(4).max(131_072);

    build_buffers_pod_bounds(stride, default_size, min_size, max_size)
}

fn build_buffers_pod_bounds(
    stride: i32,
    default_size: i32,
    min_size: i32,
    max_size: i32,
) -> Result<Vec<u8>, PipeWireError> {
    let buffers_choice =
        libspa::pod::Value::Choice(libspa::pod::ChoiceValue::Int(libspa::utils::Choice(
            libspa::utils::ChoiceFlags::empty(),
            libspa::utils::ChoiceEnum::Range {
                default: 4,
                min: 2,
                max: 16,
            },
        )));

    let size_choice =
        libspa::pod::Value::Choice(libspa::pod::ChoiceValue::Int(libspa::utils::Choice(
            libspa::utils::ChoiceFlags::empty(),
            libspa::utils::ChoiceEnum::Range {
                default: default_size,
                min: min_size,
                max: max_size,
            },
        )));

    let props = vec![
        libspa::pod::Property::new(libspa::sys::SPA_PARAM_BUFFERS_buffers, buffers_choice),
        libspa::pod::Property::new(
            libspa::sys::SPA_PARAM_BUFFERS_blocks,
            libspa::pod::Value::Int(1),
        ),
        libspa::pod::Property::new(libspa::sys::SPA_PARAM_BUFFERS_size, size_choice),
        libspa::pod::Property::new(
            libspa::sys::SPA_PARAM_BUFFERS_stride,
            libspa::pod::Value::Int(stride),
        ),
    ];

    let obj = libspa::pod::Object {
        type_: libspa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: libspa::param::ParamType::Buffers.as_raw(),
        properties: props,
    };

    libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &libspa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| PipeWireError::FormatPod(format!("{e:?}")))
}

/// Helper function to parse an SPA Format POD into audio format details.
pub fn parse_format_pod(bytes: &[u8]) -> Option<ParsedFormatInfo> {
    if bytes.len() < std::mem::size_of::<libspa::sys::spa_pod>() {
        return None;
    }
    let pod_ptr = bytes.as_ptr() as *const libspa::sys::spa_pod;
    unsafe {
        let mut media_type: u32 = 0;
        let mut media_subtype: u32 = 0;
        let res = libspa::sys::spa_format_parse(pod_ptr, &mut media_type, &mut media_subtype);
        if res < 0 || media_type != libspa::sys::SPA_MEDIA_TYPE_audio {
            return None;
        }

        let is_dsd = media_subtype == libspa::sys::SPA_MEDIA_SUBTYPE_dsd;
        let mut is_lsb = false;
        let mut interleave: i32 = 1; // default to interleaved
        let mut rate: Option<u32> = None;
        let mut channels: Option<u32> = None;

        let obj = pod_ptr as *const libspa::sys::spa_pod_object;
        let mut prop = libspa::sys::spa_pod_prop_first(&raw const (*obj).body);
        while libspa::sys::spa_pod_prop_is_inside(&raw const (*obj).body, (*pod_ptr).size, prop) {
            let key = (*prop).key;
            let val_ptr = &raw const (*prop).value;

            if key == libspa::sys::SPA_FORMAT_AUDIO_rate {
                if (*val_ptr).type_ == libspa::sys::SPA_TYPE_Int {
                    let mut r: i32 = 0;
                    if libspa::sys::spa_pod_get_int(val_ptr, &mut r) >= 0 && r > 0 {
                        rate = Some(r as u32);
                    }
                } else if libspa::sys::spa_pod_is_choice(val_ptr) > 0 {
                    let choice = val_ptr as *const libspa::sys::spa_pod_choice;
                    let body = &raw const (*choice).body;
                    let child_size = (*body).child.size as usize;
                    if child_size >= 4 {
                        let data_ptr = (body as *const u8)
                            .add(std::mem::size_of::<libspa::sys::spa_pod_choice_body>())
                            as *const i32;
                        let r = *data_ptr;
                        if r > 0 {
                            rate = Some(r as u32);
                        }
                    }
                }
            } else if key == libspa::sys::SPA_FORMAT_AUDIO_channels {
                if (*val_ptr).type_ == libspa::sys::SPA_TYPE_Int {
                    let mut c: i32 = 0;
                    if libspa::sys::spa_pod_get_int(val_ptr, &mut c) >= 0 && c > 0 {
                        channels = Some(c as u32);
                    }
                } else if libspa::sys::spa_pod_is_choice(val_ptr) > 0 {
                    let choice = val_ptr as *const libspa::sys::spa_pod_choice;
                    let body = &raw const (*choice).body;
                    let child_size = (*body).child.size as usize;
                    if child_size >= 4 {
                        let data_ptr = (body as *const u8)
                            .add(std::mem::size_of::<libspa::sys::spa_pod_choice_body>())
                            as *const i32;
                        let c = *data_ptr;
                        if c > 0 {
                            channels = Some(c as u32);
                        }
                    }
                }
            } else if key == libspa::sys::SPA_FORMAT_AUDIO_interleave {
                if (*val_ptr).type_ == libspa::sys::SPA_TYPE_Int {
                    let mut iv: i32 = 0;
                    if libspa::sys::spa_pod_get_int(val_ptr, &mut iv) >= 0 {
                        interleave = iv;
                    }
                } else if libspa::sys::spa_pod_is_choice(val_ptr) > 0 {
                    let choice = val_ptr as *const libspa::sys::spa_pod_choice;
                    let body = &raw const (*choice).body;
                    let child_size = (*body).child.size as usize;
                    if child_size >= 4 {
                        let data_ptr = (body as *const u8)
                            .add(std::mem::size_of::<libspa::sys::spa_pod_choice_body>())
                            as *const i32;
                        interleave = *data_ptr;
                    }
                }
            } else if key == libspa::sys::SPA_FORMAT_AUDIO_bitorder {
                if (*val_ptr).type_ == libspa::sys::SPA_TYPE_Id {
                    let mut b: u32 = 0;
                    if libspa::sys::spa_pod_get_id(val_ptr, &mut b) >= 0
                        && b == libspa::sys::SPA_PARAM_BITORDER_lsb
                    {
                        is_lsb = true;
                    }
                } else if libspa::sys::spa_pod_is_choice(val_ptr) > 0 {
                    let choice = val_ptr as *const libspa::sys::spa_pod_choice;
                    let body = &raw const (*choice).body;
                    let child_size = (*body).child.size as usize;
                    if child_size >= 4 {
                        let data_ptr = (body as *const u8)
                            .add(std::mem::size_of::<libspa::sys::spa_pod_choice_body>())
                            as *const u32;
                        if *data_ptr == libspa::sys::SPA_PARAM_BITORDER_lsb {
                            is_lsb = true;
                        }
                    }
                }
            }

            prop = libspa::sys::spa_pod_prop_next(prop);
        }

        if let Some(r) = rate {
            return Some(ParsedFormatInfo {
                is_dsd,
                is_lsb,
                interleave,
                rate: r,
                channels: channels.unwrap_or(2),
            });
        }
    }
    None
}

/// Helper function to parse SPA Props POD bytes into volume and mute updates.
pub fn parse_props_pod(bytes: &[u8]) -> Option<VolumeUpdate> {
    if let Ok((_, Value::Object(obj))) = PodDeserializer::deserialize_any_from(bytes) {
        let mut vol_pct: Option<f64> = None;
        let mut vol_db: Option<f64> = None;
        let mut is_muted: Option<bool> = None;

        let f_to_pct = |f: f32| -> f64 {
            if f <= 1e-6 {
                0.0
            } else {
                ((f as f64).cbrt() * 100.0).clamp(0.0, 150.0)
            }
        };

        for prop in obj.properties {
            if prop.key == libspa::sys::SPA_PROP_volume {
                match prop.value {
                    Value::Float(f) => {
                        let pct = f_to_pct(f);
                        vol_pct = Some(pct);
                        vol_db = Some(audigma_dsp::alsamixer_percentage_to_db(
                            pct,
                            audigma_dsp::DEFAULT_SOFTWARE_MIN_DB,
                        ));
                    }
                    Value::Choice(ChoiceValue::Float(choice)) => {
                        let f = match choice.1 {
                            ChoiceEnum::None(v) => v,
                            ChoiceEnum::Range { default, .. } => default,
                            ChoiceEnum::Step { default, .. } => default,
                            ChoiceEnum::Enum { default, .. } => default,
                            ChoiceEnum::Flags { default, .. } => default,
                        };
                        let pct = f_to_pct(f);
                        vol_pct = Some(pct);
                        vol_db = Some(audigma_dsp::alsamixer_percentage_to_db(
                            pct,
                            audigma_dsp::DEFAULT_SOFTWARE_MIN_DB,
                        ));
                    }
                    _ => {}
                }
            } else if prop.key == libspa::sys::SPA_PROP_mute {
                match prop.value {
                    Value::Bool(b) => is_muted = Some(b),
                    Value::Choice(ChoiceValue::Bool(choice)) => {
                        let b = match choice.1 {
                            ChoiceEnum::None(v) => v,
                            ChoiceEnum::Range { default, .. } => default,
                            ChoiceEnum::Step { default, .. } => default,
                            ChoiceEnum::Enum { default, .. } => default,
                            ChoiceEnum::Flags { default, .. } => default,
                        };
                        is_muted = Some(b);
                    }
                    _ => {}
                }
            } else if prop.key == libspa::sys::SPA_PROP_channelVolumes {
                if let Value::ValueArray(ValueArray::Float(floats)) = prop.value {
                    if !floats.is_empty() {
                        let avg: f32 = floats.iter().sum::<f32>() / floats.len() as f32;
                        let pct = f_to_pct(avg);
                        vol_pct = Some(pct);
                        vol_db = Some(audigma_dsp::alsamixer_percentage_to_db(
                            pct,
                            audigma_dsp::DEFAULT_SOFTWARE_MIN_DB,
                        ));
                    }
                }
            }
        }

        if vol_pct.is_some() || vol_db.is_some() || is_muted.is_some() {
            return Some(VolumeUpdate {
                volume_db: vol_db,
                volume_percent: vol_pct,
                is_muted,
            });
        }
    }
    None
}

/// PipeWire virtual audio sink node (`audigma_sink`).
///
/// Desktop sound and audio players route into this virtual sink (`media.class = "Audio/Sink"`).
/// Incoming audio blocks are pushed into an SPSC ring buffer with monotonic generation tags.
pub struct PipeWireVirtualSink {
    info: AudioStreamInfo,
    _worker: Option<JoinHandle<()>>,
    consumer: Consumer<TaggedChunk>,
    current_chunk: Option<TaggedChunk>,
    read_offset: usize,
    generation: Arc<AtomicU64>,
    active_rate: Arc<AtomicU32>,
    pending_rate: Arc<AtomicU32>,
    is_dsd: Arc<AtomicBool>,
    is_lsb: Arc<AtomicBool>,
    interleave: Arc<AtomicI32>,
    active_dsd_bit_rate: Arc<AtomicU32>,
    pending_volume: Arc<Mutex<Option<VolumeUpdate>>>,
    running: Arc<AtomicBool>,
    dropped_chunks: Arc<AtomicU64>,
    processed_chunks: Arc<AtomicU64>,
    /// Queued input in native units: PCM frames for PCM, bytes-per-channel for DSD.
    /// Producer adds on push; consumer subtracts as native units are consumed.
    buffered_units: Arc<AtomicU64>,
    dsd_gain_db: f64,
    dsd_max_intermediate_rate: Option<PcmRateHz>,
    /// Lazy compatibility decoders for callers of `read_channels`. The live
    /// worker-input path reads raw bytes and never constructs these cascades.
    decimators: Vec<audigma_dsp::fir::HalfBandDecimationCascade>,
    dsd_scratch: Vec<Vec<u8>>,
    dsd_residue: Vec<Vec<u8>>,
    prerolled: bool,
    preroll_secs: f64,
    capture_cushion_ns: Arc<AtomicU64>,
    rt_consumer: Option<audigma_log::RtLogConsumer>,
    dropped_rt_events: Arc<AtomicU64>,
    decimation_recorder: Arc<audigma_log::DecimationErrorRecorder>,
}

/// Applies the thread's placement (housekeeping affinity + RT). Never fatal: a failure
/// (including on reconnect, after startup) logs an `RT_PRIORITY` event and the thread keeps
/// running at normal priority.
fn apply_thread_placement(thread: &str, placement: Option<&audigma_engine::ThreadPlacement>) {
    let Some(p) = placement else { return };
    #[cfg(target_os = "linux")]
    match crate::sched::apply(p) {
        Ok(applied) => tracing::info!(
            target: "audigma::daemon",
            event = audigma_log::RT_PRIORITY,
            thread,
            acquired = applied.realtime.is_some(),
            priority = p.rt_priority,
            cpus = ?p.cpus,
            method = applied.realtime.map(|m| m.to_string()).unwrap_or_default(),
            "thread placement applied"
        ),
        Err(e) => tracing::warn!(
            target: "audigma::daemon",
            event = audigma_log::RT_PRIORITY,
            thread,
            acquired = false,
            priority = p.rt_priority,
            error = %e,
            "thread placement failed; continuing at normal priority"
        ),
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (thread, p);
}

impl PipeWireVirtualSink {
    /// Creates and binds a new virtual sink node in the PipeWire graph using an `InputConfig`.
    pub fn from_input_config(
        in_cfg: &InputConfig,
        default_rate_hz: u32,
        queue_chunks: usize,
    ) -> Result<Self, PipeWireError> {
        Self::from_input_config_with_placement(in_cfg, default_rate_hz, queue_chunks, None)
    }

    /// Creates and binds a new virtual sink node in the PipeWire graph using an `InputConfig`
    /// and an optional thread placement for its PipeWire thread.
    pub fn from_input_config_with_placement(
        in_cfg: &InputConfig,
        default_rate_hz: u32,
        queue_chunks: usize,
        placement: Option<audigma_engine::ThreadPlacement>,
    ) -> Result<Self, PipeWireError> {
        let node_name = in_cfg.node_name.as_deref().unwrap_or("audigma_sink");
        let channels = in_cfg.channels.unwrap_or(2) as u32;
        let positions = in_cfg.resolved_channel_positions();
        let rate_mode = in_cfg.rate_mode();
        let allowed_rates = in_cfg.resolved_allowed_rates();
        let rate_hz = in_cfg.rate_hz.map(|r| r.get()).unwrap_or(default_rate_hz);
        let allow_dsd = in_cfg.allow_dsd();
        let dsd_gain_db = in_cfg.dsd_gain_db();
        let dsd_max_intermediate = in_cfg.dsd_max_intermediate_rate_hz();

        Self::new_internal(
            node_name,
            rate_hz,
            channels,
            positions,
            rate_mode,
            allowed_rates,
            allow_dsd,
            dsd_gain_db,
            dsd_max_intermediate,
            queue_chunks,
            placement,
        )
    }

    /// Creates and binds a new virtual sink node in the PipeWire graph with default stereo settings.
    pub fn new(
        node_name: &str,
        rate_hz: u32,
        channels: u32,
        queue_chunks: usize,
    ) -> Result<Self, PipeWireError> {
        let dummy_cfg = InputConfig {
            backend: Some(audigma_settings::InputBackend::Pipewire),
            node_name: Some(node_name.to_string()),
            sample_format: None,
            rate_hz: PcmRateHz::new(rate_hz),
            channels: Some(channels as u8),
            channel_positions: None,
            rate_mode: None,
            allowed_rates: None,
            allow_dsd: None,
            dsd: None,
        };
        Self::from_input_config(&dummy_cfg, rate_hz, queue_chunks)
    }

    #[allow(clippy::too_many_arguments)]
    fn new_internal(
        node_name: &str,
        rate_hz: u32,
        channels: u32,
        channel_positions: Vec<String>,
        rate_mode: RateMode,
        allowed_rates: Vec<u32>,
        allow_dsd: bool,
        dsd_gain_db: f64,
        dsd_max_intermediate: Option<PcmRateHz>,
        queue_chunks: usize,
        placement: Option<audigma_engine::ThreadPlacement>,
    ) -> Result<Self, PipeWireError> {
        let desc = format!("Audigma Virtual Sink ({node_name})");
        let node_name_owned = node_name.to_string();

        let (mut producer, consumer) = RingBuffer::new(queue_chunks);
        let generation = Arc::new(AtomicU64::new(1));
        let active_rate = Arc::new(AtomicU32::new(rate_hz));
        let pending_rate = Arc::new(AtomicU32::new(0));
        let is_dsd = Arc::new(AtomicBool::new(false));
        let is_lsb = Arc::new(AtomicBool::new(false));
        let interleave = Arc::new(AtomicI32::new(1));
        let active_dsd_bit_rate = Arc::new(AtomicU32::new(0));
        let pending_volume = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));
        let dropped_chunks = Arc::new(AtomicU64::new(0));
        let processed_chunks = Arc::new(AtomicU64::new(0));
        let buffered_units = Arc::new(AtomicU64::new(0));
        let capture_cushion_ns =
            Arc::new(AtomicU64::new((DEFAULT_INPUT_PREROLL_SECS * 1e9) as u64));
        let capture_cushion_worker = capture_cushion_ns.clone();
        let buffered_units_worker = buffered_units.clone();

        let (rt_producer, rt_consumer, dropped_rt_events) =
            audigma_log::rt_log_channel(audigma_log::DEFAULT_RT_QUEUE_CAPACITY);
        let rt_producer = Arc::new(Mutex::new(rt_producer));
        let rt_producer_cb = rt_producer.clone();
        let decimation_recorder = Arc::new(audigma_log::DecimationErrorRecorder::new());

        let gen_clone = generation.clone();
        let active_rate_cb = active_rate.clone();
        let pending_rate_cb = pending_rate.clone();
        let is_dsd_cb = is_dsd.clone();
        let is_lsb_cb = is_lsb.clone();
        let interleave_cb = interleave.clone();
        let active_dsd_rate_cb = active_dsd_bit_rate.clone();
        let pending_vol_cb = pending_volume.clone();
        let running_clone = running.clone();
        let dropped_chunks_cb = dropped_chunks.clone();
        let processed_chunks_cb = processed_chunks.clone();
        let buffered_units_cb = buffered_units.clone();

        let pos_str = channel_positions.join(",");
        let mut all_allowed_rates = allowed_rates.clone();
        if allow_dsd {
            for &dsd_byte_rate in &[352_800u32, 705_600, 1_411_200, 2_822_400] {
                if !all_allowed_rates.contains(&dsd_byte_rate) {
                    all_allowed_rates.push(dsd_byte_rate);
                }
            }
        }
        let allowed_rates_str = all_allowed_rates
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        let channel_positions_clone = channel_positions.clone();
        let allowed_rates_clone = allowed_rates.clone();

        let worker = std::thread::Builder::new()
            .name("audigma-pw-in".to_string())
            .spawn(move || {
                apply_thread_placement("audigma-pw-in", placement.as_ref());
                pipewire::init();
                let mainloop = match pipewire::main_loop::MainLoopBox::new(None) {
                    Ok(m) => m,
                    Err(_) => return,
                };
                let context = match pipewire::context::ContextBox::new(mainloop.loop_(), None) {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let core = match context.connect(None) {
                    Ok(c) => c,
                    Err(_) => return,
                };

                let stream_name = node_name_owned.clone();
                let mut stream_props = properties! {
                    *pipewire::keys::NODE_NAME => stream_name.as_str(),
                    *pipewire::keys::NODE_DESCRIPTION => desc.as_str(),
                    *pipewire::keys::MEDIA_CLASS => "Audio/Sink",
                    AUDIGMA_NODE_PROP => "true",
                    *pipewire::keys::MEDIA_TYPE => "Audio",
                    *pipewire::keys::MEDIA_ROLE => "Music",
                    *pipewire::keys::AUDIO_CHANNELS => format!("{channels}"),
                    "audio.position" => pos_str.as_str(),
                    // The sink always drives its own graph and pulls input as
                    // the engine consumes it. It never joins (or switches
                    // between) hardware drivers, so input timing follows the
                    // output and a source's requested size is in its own units.
                    *pipewire::keys::NODE_DRIVER => "true",
                };
                if rate_mode == RateMode::Dynamic || allow_dsd {
                    stream_props.insert("audio.allowed-rates", format!("[ {allowed_rates_str} ]"));
                }

                let stream = match StreamBox::new(&core, &stream_name, stream_props) {
                    Ok(s) => s,
                    Err(_) => return,
                };

                let run_cb = running_clone.clone();
                let is_dsd_process_cb = is_dsd_cb.clone();
                let is_lsb_process_cb = is_lsb_cb.clone();
                let interleave_process_cb = interleave_cb.clone();
                let pull_pending = Arc::new(AtomicBool::new(false));
                let pull_pending_state = pull_pending.clone();
                let _trigger_done = TriggerDone::new(&stream, pull_pending.clone());
                // Negotiated capture rate in units per second (PCM frames or
                // DSD bytes per channel); zero until a format is negotiated.
                let capture_rate = Arc::new(AtomicU32::new(0));
                let capture_rate_format = capture_rate.clone();
                let capture_units = Arc::new(AtomicU64::new(0));
                let capture_units_cb = capture_units.clone();
                let capture_units_format = capture_units.clone();
                let position = Arc::new(AtomicPtr::<libspa::sys::spa_io_position>::new(
                    std::ptr::null_mut(),
                ));
                let position_io = position.clone();
                let _listener = match stream
                    .add_local_listener_with_user_data(())
                    .io_changed(move |_, _, id, area, size| {
                        if id == libspa::sys::SPA_IO_Position {
                            let ptr = if size as usize
                                >= std::mem::size_of::<libspa::sys::spa_io_position>()
                            {
                                area.cast()
                            } else {
                                std::ptr::null_mut()
                            };
                            position_io.store(ptr, Ordering::Release);
                        }
                    })
                    .state_changed(move |_, _, _, state| {
                        if state != pipewire::stream::StreamState::Streaming {
                            pull_pending_state.store(false, Ordering::Release);
                        }
                    })
                    .param_changed(move |stream, _, id, param| {
                        let Some(param) = param else {
                            if id == libspa::param::ParamType::Format.as_raw() {
                                capture_rate_format.store(0, Ordering::Release);
                            }
                            return;
                        };
                        if id == libspa::param::ParamType::Format.as_raw() {
                            let bytes = param.as_bytes();
                            let parsed = parse_format_pod(bytes);
                            let mut negotiated_stream_rate_and_stride: Option<(u32, u32)> = None;
                            if let Some(parsed) = parsed {
                                is_dsd_cb.store(parsed.is_dsd, Ordering::SeqCst);
                                is_lsb_cb.store(parsed.is_lsb, Ordering::SeqCst);
                                interleave_cb.store(parsed.interleave, Ordering::SeqCst);
                                if parsed.is_dsd {
                                    let bit_rate = parsed.rate * 8;
                                    active_dsd_rate_cb.store(bit_rate, Ordering::SeqCst);
                                    let dsd_rate_opt = DsdBitRateHz::new(bit_rate);
                                    let intermediate_rate = dsd_rate_opt
                                        .map(|r| {
                                            audigma_dsp::rates::select_dsd_intermediate_rate(
                                                r,
                                                None,
                                                dsd_max_intermediate,
                                            )
                                            .get()
                                        })
                                        .unwrap_or(176_400);

                                    if let Ok(mut p) = rt_producer_cb.try_lock() {
                                        let _ =
                                            p.try_push(audigma_log::RtLogEvent::FormatNegotiated {
                                                is_dsd: true,
                                                rate_hz: parsed.rate,
                                                bit_rate_hz: bit_rate,
                                                intermediate_rate_hz: intermediate_rate,
                                                channels: parsed.channels as u8,
                                                interleave: parsed.interleave,
                                            });
                                    }
                                    let stride = parsed.channels.max(1)
                                        * parsed.interleave.unsigned_abs().max(1);
                                    negotiated_stream_rate_and_stride = Some((parsed.rate, stride));
                                    let cur_active = active_rate_cb.load(Ordering::Relaxed);
                                    if cur_active != intermediate_rate {
                                        pending_rate_cb.store(intermediate_rate, Ordering::SeqCst);
                                    }
                                } else {
                                    active_dsd_rate_cb.store(0, Ordering::SeqCst);
                                    if let Ok(mut p) = rt_producer_cb.try_lock() {
                                        let _ =
                                            p.try_push(audigma_log::RtLogEvent::FormatNegotiated {
                                                is_dsd: false,
                                                rate_hz: parsed.rate,
                                                bit_rate_hz: parsed.rate,
                                                intermediate_rate_hz: parsed.rate,
                                                channels: parsed.channels as u8,
                                                interleave: 1,
                                            });
                                    }
                                    let stride = parsed.channels.max(1) * 4;
                                    negotiated_stream_rate_and_stride = Some((parsed.rate, stride));
                                    let cur_active = active_rate_cb.load(Ordering::Relaxed);
                                    if cur_active != parsed.rate {
                                        pending_rate_cb.store(parsed.rate, Ordering::SeqCst);
                                    }
                                }
                            } else {
                                let mut info = AudioInfoRaw::new();
                                if info.parse(param).is_ok() {
                                    is_dsd_cb.store(false, Ordering::SeqCst);
                                    is_lsb_cb.store(false, Ordering::SeqCst);
                                    active_dsd_rate_cb.store(0, Ordering::SeqCst);
                                    let negotiated_rate = info.rate();
                                    let negotiated_channels = info.channels();
                                    if let Ok(mut p) = rt_producer_cb.try_lock() {
                                        let _ =
                                            p.try_push(audigma_log::RtLogEvent::FormatNegotiated {
                                                is_dsd: false,
                                                rate_hz: negotiated_rate,
                                                bit_rate_hz: negotiated_rate,
                                                intermediate_rate_hz: negotiated_rate,
                                                channels: negotiated_channels as u8,
                                                interleave: 1,
                                            });
                                    }
                                    if negotiated_rate > 0 {
                                        let stride = negotiated_channels.max(1) * 4;
                                        negotiated_stream_rate_and_stride =
                                            Some((negotiated_rate, stride));
                                        let cur_active = active_rate_cb.load(Ordering::Relaxed);
                                        if cur_active != negotiated_rate {
                                            pending_rate_cb
                                                .store(negotiated_rate, Ordering::SeqCst);
                                        }
                                    }
                                }
                            }

                            if let Some((stream_rate, stride)) = negotiated_stream_rate_and_stride {
                                let quantum_frames = quantum_frames_for_rate(stream_rate);
                                let native_dsd = is_dsd_cb.load(Ordering::Acquire);
                                capture_units_format
                                    .store(u64::from(quantum_frames), Ordering::Release);
                                capture_rate_format.store(stream_rate, Ordering::Release);
                                let buffers = if native_dsd {
                                    let iv =
                                        interleave_cb.load(Ordering::Relaxed).unsigned_abs().max(1);
                                    let size =
                                        quantum_frames.div_ceil(iv).saturating_mul(stride) as i32;
                                    build_buffers_pod_bounds(stride as i32, size, size, size)
                                } else {
                                    build_buffers_pod(quantum_frames, stride)
                                };
                                if let Ok(buf_bytes) = buffers {
                                    if let Some(buf_pod) = Pod::from_bytes(&buf_bytes) {
                                        let mut params = [buf_pod];
                                        let _ = stream.update_params(&mut params);
                                    }
                                }
                            }
                        } else if id == libspa::param::ParamType::Props.as_raw() {
                            let bytes = param.as_bytes();
                            if let Some(vol_update) = parse_props_pod(bytes) {
                                if let Ok(mut p) = rt_producer_cb.try_lock() {
                                    let _ = p.try_push(audigma_log::RtLogEvent::VolumeEvent {
                                        volume_percent: vol_update.volume_percent.map(|v| v as f32),
                                        volume_db: vol_update.volume_db.map(|v| v as f32),
                                        muted: vol_update.is_muted,
                                    });
                                }
                                let mut pending = pending_vol_cb.lock().unwrap();
                                let prev: VolumeUpdate = pending.unwrap_or_default();
                                *pending = Some(VolumeUpdate {
                                    volume_db: vol_update.volume_db.or(prev.volume_db),
                                    volume_percent: vol_update
                                        .volume_percent
                                        .or(prev.volume_percent),
                                    is_muted: vol_update.is_muted.or(prev.is_muted),
                                });
                            }
                        }
                    })
                    .process(move |stream, _| {
                        if !run_cb.load(Ordering::Relaxed) {
                            return;
                        }

                        if producer.is_full() {
                            // Leave the buffer queued; the next pull retries it.
                            return;
                        }

                        if let Some(mut buffer) = stream.dequeue_buffer() {
                            let datas = buffer.datas_mut();
                            if datas.is_empty() {
                                return;
                            }

                            processed_chunks_cb.fetch_add(1, Ordering::Relaxed);

                            let cur_gen = gen_clone.load(Ordering::Relaxed);
                            let n_ch = channels as usize;

                            if is_dsd_process_cb.load(Ordering::Relaxed) {
                                // Native DSD stream (SPA audio/dsd)
                                let reverse_bits = is_lsb_process_cb.load(Ordering::Relaxed);
                                let iv = interleave_process_cb.load(Ordering::Relaxed);

                                if iv == 0 || (datas.len() >= n_ch && n_ch > 1) {
                                    // Planar audio/dsd: each datas[c] holds channel c
                                    let channel_slices: Vec<&[u8]> = datas
                                        .iter_mut()
                                        .take(n_ch)
                                        .map(|data| {
                                            let chunk = data.chunk();
                                            let off = chunk.offset() as usize;
                                            let size = chunk.size() as usize;
                                            if let Some(bytes) = data.data() {
                                                let valid_len =
                                                    bytes.len().saturating_sub(off).min(size);
                                                &bytes[off..off + valid_len]
                                            } else {
                                                &[]
                                            }
                                        })
                                        .collect();

                                    let mut min_bytes = usize::MAX;
                                    for slice in &channel_slices {
                                        min_bytes = min_bytes.min(slice.len());
                                    }

                                    if min_bytes > 0 && min_bytes != usize::MAX {
                                        let total_bytes = min_bytes * n_ch;
                                        let mut chunk = TaggedChunk::new(total_bytes);
                                        let mut byte_buf = Vec::with_capacity(total_bytes);
                                        for i in 0..min_bytes {
                                            for slice in &channel_slices {
                                                let b = slice[i];
                                                byte_buf.push(if reverse_bits {
                                                    b.reverse_bits()
                                                } else {
                                                    b
                                                });
                                            }
                                        }
                                        chunk.set_dsd(cur_gen, n_ch, &byte_buf);
                                        let units = chunk.frames() as u64;
                                        capture_units_cb.store(units, Ordering::Release);
                                        buffered_units_cb.fetch_add(units, Ordering::Relaxed);
                                        if producer.push(chunk).is_err() {
                                            let _ = buffered_units_cb.fetch_update(
                                                Ordering::Relaxed,
                                                Ordering::Relaxed,
                                                |cur| Some(cur.saturating_sub(units)),
                                            );
                                            dropped_chunks_cb.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                } else if !datas.is_empty() {
                                    // Interleaved audio/dsd: datas[0] contains interleaved DSD bytes
                                    let data = &mut datas[0];
                                    let chunk = data.chunk();
                                    let off = chunk.offset() as usize;
                                    let size = chunk.size() as usize;
                                    if let Some(bytes) = data.data() {
                                        let valid_len = bytes.len().saturating_sub(off).min(size);
                                        if valid_len > 0 {
                                            let valid_bytes = &bytes[off..off + valid_len];
                                            let abs_iv = iv.unsigned_abs() as usize;
                                            let group_size =
                                                (if abs_iv == 0 { 1 } else { abs_iv }) * n_ch;
                                            let complete_groups = valid_len / group_size;

                                            if complete_groups > 0 {
                                                let usable_len = complete_groups * group_size;
                                                let mut chunk = TaggedChunk::new(usable_len);
                                                let mut byte_buf = Vec::with_capacity(usable_len);

                                                if abs_iv <= 1 {
                                                    // Standard 1-byte interleaved: [L, R, L, R...]
                                                    for &b in &valid_bytes[..usable_len] {
                                                        byte_buf.push(if reverse_bits {
                                                            b.reverse_bits()
                                                        } else {
                                                            b
                                                        });
                                                    }
                                                } else {
                                                    // Block interleaved: K bytes per channel [L0..LK, R0..RK, ...]
                                                    // Normalized into per-frame interleaved [L0, R0, L1, R1, ...]
                                                    let k = abs_iv;
                                                    let reverse_group_bytes = iv < 0;
                                                    for g in 0..complete_groups {
                                                        let g_off = g * group_size;
                                                        for byte_idx in 0..k {
                                                            let effective_idx =
                                                                if reverse_group_bytes {
                                                                    k - 1 - byte_idx
                                                                } else {
                                                                    byte_idx
                                                                };
                                                            for c in 0..n_ch {
                                                                let src_idx =
                                                                    g_off + c * k + effective_idx;
                                                                let b = valid_bytes[src_idx];
                                                                byte_buf.push(if reverse_bits {
                                                                    b.reverse_bits()
                                                                } else {
                                                                    b
                                                                });
                                                            }
                                                        }
                                                    }
                                                }
                                                chunk.set_dsd(cur_gen, n_ch, &byte_buf);
                                                let units = chunk.frames() as u64;
                                                capture_units_cb.store(units, Ordering::Release);
                                                buffered_units_cb
                                                    .fetch_add(units, Ordering::Relaxed);
                                                if producer.push(chunk).is_err() {
                                                    let _ = buffered_units_cb.fetch_update(
                                                        Ordering::Relaxed,
                                                        Ordering::Relaxed,
                                                        |cur| Some(cur.saturating_sub(units)),
                                                    );
                                                    dropped_chunks_cb
                                                        .fetch_add(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                // Multi-bit PCM stream
                                let sample_size = std::mem::size_of::<f32>();
                                if datas.len() >= n_ch && n_ch > 1 {
                                    // Planar audio: each datas[c] holds channel c
                                    let channel_slices: Vec<&[u8]> = datas
                                        .iter_mut()
                                        .take(n_ch)
                                        .map(|data| {
                                            let chunk = data.chunk();
                                            let off = chunk.offset() as usize;
                                            let size = chunk.size() as usize;
                                            if let Some(bytes) = data.data() {
                                                let valid_len =
                                                    bytes.len().saturating_sub(off).min(size);
                                                &bytes[off..off + valid_len]
                                            } else {
                                                &[]
                                            }
                                        })
                                        .collect();

                                    let mut min_frames = usize::MAX;
                                    for slice in &channel_slices {
                                        min_frames = min_frames.min(slice.len() / sample_size);
                                    }

                                    if min_frames > 0 && min_frames != usize::MAX {
                                        let total_floats = min_frames * n_ch;
                                        let mut chunk = TaggedChunk::new(total_floats);
                                        let mut float_buf = Vec::with_capacity(total_floats);

                                        for i in 0..min_frames {
                                            let s_off = i * sample_size;
                                            for slice in &channel_slices {
                                                let f = if s_off + 4 <= slice.len() {
                                                    let bytes: [u8; 4] =
                                                        slice[s_off..s_off + 4].try_into().unwrap();
                                                    f32::from_le_bytes(bytes) as f64
                                                } else {
                                                    0.0
                                                };
                                                float_buf.push(f);
                                            }
                                        }

                                        chunk.set(cur_gen, n_ch, &float_buf);
                                        let units = chunk.frames() as u64;
                                        capture_units_cb.store(units, Ordering::Release);
                                        buffered_units_cb.fetch_add(units, Ordering::Relaxed);
                                        if producer.push(chunk).is_err() {
                                            let _ = buffered_units_cb.fetch_update(
                                                Ordering::Relaxed,
                                                Ordering::Relaxed,
                                                |cur| Some(cur.saturating_sub(units)),
                                            );
                                            dropped_chunks_cb.fetch_add(1, Ordering::Relaxed);
                                        }
                                    }
                                } else {
                                    // Interleaved audio: datas[0] contains samples
                                    let data = &mut datas[0];
                                    let chunk_info = data.chunk();
                                    let offset = chunk_info.offset() as usize;
                                    let size = chunk_info.size() as usize;

                                    if let Some(raw_bytes) = data.data() {
                                        if offset + size <= raw_bytes.len() && size > 0 {
                                            let valid_bytes = &raw_bytes[offset..offset + size];
                                            let total_floats = valid_bytes.len() / sample_size;
                                            if total_floats > 0 {
                                                let mut chunk = TaggedChunk::new(total_floats);
                                                let mut float_buf =
                                                    Vec::with_capacity(total_floats);
                                                for chunk_bytes in
                                                    valid_bytes.chunks_exact(sample_size)
                                                {
                                                    let f = f32::from_le_bytes(
                                                        chunk_bytes.try_into().unwrap(),
                                                    );
                                                    float_buf.push(f as f64);
                                                }

                                                chunk.set(cur_gen, n_ch, &float_buf);
                                                let units = chunk.frames() as u64;
                                                capture_units_cb.store(units, Ordering::Release);
                                                buffered_units_cb
                                                    .fetch_add(units, Ordering::Relaxed);
                                                if producer.push(chunk).is_err() {
                                                    let _ = buffered_units_cb.fetch_update(
                                                        Ordering::Relaxed,
                                                        Ordering::Relaxed,
                                                        |cur| Some(cur.saturating_sub(units)),
                                                    );
                                                    dropped_chunks_cb
                                                        .fetch_add(1, Ordering::Relaxed);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    })
                    .register()
                {
                    Ok(l) => l,
                    Err(_) => return,
                };

                let pcm_pod_bytes = match build_format_pod(
                    rate_hz,
                    channels,
                    &channel_positions_clone,
                    rate_mode,
                    &allowed_rates_clone,
                ) {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let dsd_bytes = if allow_dsd {
                    const ALLOWED_DSD_BYTE_RATES: [u32; 4] =
                        [352_800, 705_600, 1_411_200, 2_822_400];
                    build_dsd_format_pod(
                        channels,
                        &channel_positions_clone,
                        352_800,
                        &ALLOWED_DSD_BYTE_RATES,
                    )
                    .ok()
                } else {
                    None
                };

                let init_quantum = quantum_frames_for_rate(rate_hz);
                let init_stride = channels.max(1) * 4;
                let init_buffers_bytes = build_buffers_pod(init_quantum, init_stride).ok();

                let mut param_vec = Vec::new();
                if let Some(pcm_pod) = Pod::from_bytes(&pcm_pod_bytes) {
                    param_vec.push(pcm_pod);
                }
                if let Some(ref dsd_bytes_ref) = dsd_bytes {
                    if let Some(dsd_pod) = Pod::from_bytes(dsd_bytes_ref) {
                        param_vec.push(dsd_pod);
                    }
                }
                if let Some(ref buf_bytes_ref) = init_buffers_bytes {
                    if let Some(buf_pod) = Pod::from_bytes(buf_bytes_ref) {
                        param_vec.push(buf_pod);
                    }
                }

                if stream
                    .connect(
                        Direction::Input,
                        None,
                        StreamFlags::AUTOCONNECT
                            | StreamFlags::MAP_BUFFERS
                            | StreamFlags::RT_PROCESS,
                        &mut param_vec,
                    )
                    .is_err()
                {
                    return;
                }

                let mut pull_clock = PullClock::default();
                let mut pull_started: Option<std::time::Instant> = None;
                while running_clone.load(Ordering::Relaxed) {
                    mainloop.loop_().iterate(pipewire::loop_::Timeout::Finite(
                        std::time::Duration::from_millis(1),
                    ));
                    let now = std::time::Instant::now();
                    if pull_pending.load(Ordering::Acquire) {
                        if pull_started.is_some_and(|t| now.duration_since(t) >= PULL_TIMEOUT) {
                            tracing::debug!(
                                target: "audigma::pipewire",
                                event = "capture_pull_timeout",
                                "capture cycle did not complete; re-arming pull"
                            );
                            pull_pending.store(false, Ordering::Release);
                        } else {
                            continue;
                        }
                    }
                    pull_started = None;

                    let units_per_sec = capture_rate.load(Ordering::Acquire);
                    if units_per_sec == 0
                        || stream.state() != pipewire::stream::StreamState::Streaming
                        || !stream.is_driving()
                    {
                        pull_clock.reset();
                        continue;
                    }
                    if !pull_clock.ready(now)
                        || !PullDemand::needs_buffer(
                            buffered_units_worker.load(Ordering::Acquire),
                            units_per_sec,
                            capture_cushion_worker.load(Ordering::Relaxed),
                        )
                    {
                        continue;
                    }

                    let ptr = position.load(Ordering::Acquire);
                    if !ptr.is_null() {
                        // SAFETY: PipeWire owns this IO area for as long as
                        // io_changed has not replaced it; both run on this
                        // thread. No cycle is in flight (one pull at a time),
                        // so followers are not reading the clock concurrently.
                        let clock = unsafe { &mut (*ptr).clock };
                        let now_ns =
                            unsafe { pipewire::sys::pw_stream_get_nsec(stream.as_raw_ptr()) };
                        advance_driver_clock(clock, now_ns);
                    }

                    pull_pending.store(true, Ordering::Release);
                    pull_started = Some(now);
                    let units = capture_units.load(Ordering::Acquire).max(1);
                    pull_clock.pulled(now, units, units_per_sec);
                    if stream.trigger_process().is_err() {
                        pull_pending.store(false, Ordering::Release);
                        pull_started = None;
                    }
                }
            })
            .map_err(|_e| PipeWireError::Native(pipewire::Error::NoMemory))?;

        let info = AudioStreamInfo::new_pcm(
            PcmRateHz::new(rate_hz).unwrap_or(PcmRateHz::new(48000).unwrap()),
            channels as u8,
            None,
            Some(32),
        );

        Ok(Self {
            info,
            _worker: Some(worker),
            consumer,
            current_chunk: None,
            read_offset: 0,
            generation,
            active_rate,
            pending_rate,
            is_dsd,
            is_lsb,
            interleave,
            active_dsd_bit_rate,
            pending_volume,
            running,
            dropped_chunks,
            processed_chunks,
            buffered_units,
            dsd_gain_db,
            dsd_max_intermediate_rate: dsd_max_intermediate,
            decimators: Vec::new(),
            dsd_scratch: Vec::new(),
            dsd_residue: Vec::new(),
            prerolled: false,
            preroll_secs: DEFAULT_INPUT_PREROLL_SECS,
            capture_cushion_ns,
            rt_consumer: Some(rt_consumer),
            dropped_rt_events,
            decimation_recorder,
        })
    }

    /// Sets the target preroll cushion duration in seconds.
    #[inline]
    pub fn set_preroll_secs(&mut self, secs: f64) {
        self.preroll_secs = secs.clamp(0.040, 1.0);
        self.capture_cushion_ns
            .store((self.preroll_secs * 1e9) as u64, Ordering::Relaxed);
    }

    /// Returns whether the currently negotiated DSD bitstream is LSB bit order.
    #[inline]
    pub fn is_dsd_lsb(&self) -> bool {
        self.is_lsb.load(Ordering::Relaxed)
    }

    /// Returns the currently negotiated DSD interleave parameter (0 = planar, 1 = byte interleaved, >1 = block interleaved).
    #[inline]
    pub fn dsd_interleave(&self) -> i32 {
        self.interleave.load(Ordering::Relaxed)
    }

    /// Sets the active generation tag for upcoming buffers.
    #[inline]
    pub fn set_generation(&self, r#gen: u64) {
        self.generation.store(r#gen, Ordering::SeqCst);
    }

    /// Number of input chunks dropped because the SPSC ring buffer was full.
    pub fn dropped_chunks(&self) -> u64 {
        self.dropped_chunks.load(Ordering::Relaxed)
    }

    /// Number of input buffers processed from PipeWire.
    pub fn processed_chunks(&self) -> u64 {
        self.processed_chunks.load(Ordering::Relaxed)
    }

    /// Buffered input duration in seconds, including the partially consumed chunk.
    fn buffered_duration_secs(&self) -> f64 {
        let units = self.buffered_units.load(Ordering::Relaxed) as f64;
        if self.is_dsd.load(Ordering::Relaxed) {
            let bit_rate = self.active_dsd_bit_rate.load(Ordering::Relaxed);
            if bit_rate > 0 {
                units * 8.0 / bit_rate as f64
            } else {
                0.0
            }
        } else {
            let rate = self.active_rate.load(Ordering::Relaxed).max(1);
            units / rate as f64
        }
    }

    #[inline]
    fn sub_buffered_units(&self, units: u64) {
        if units > 0 {
            let _ = self
                .buffered_units
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                    Some(cur.saturating_sub(units))
                });
        }
    }

    /// Applies the same duration-based preroll to PCM, decoded DSD, and raw DSD.
    fn input_ready(&mut self) -> bool {
        if !self.prerolled {
            if !input_preroll_ready(
                self.buffered_duration_secs(),
                self.preroll_secs,
                self.is_dsd.load(Ordering::Relaxed),
                self.consumer.slots(),
                self.consumer.buffer().capacity(),
            ) {
                return false;
            }
            self.prerolled = true;
        } else if self.buffered_units.load(Ordering::Relaxed) == 0 {
            return false;
        }
        true
    }

    /// Reads raw DSD bytes, optionally accounting them against `buffered_units`.
    ///
    /// The public `AudioInput::read_dsd_bytes` accounts each byte read. The
    /// decimating `read_channels` path passes `account = false` because it
    /// accounts the *PCM-aligned* byte count instead, which keeps any leftover
    /// `dsd_residue` visible in the buffered-duration signal.
    fn read_dsd_bytes_internal(
        &mut self,
        channels: &mut [&mut [u8]],
        account: bool,
    ) -> Result<usize, EngineError> {
        if channels.is_empty() {
            return Ok(0);
        }
        let max_bytes = channels.iter().map(|s| s.len()).min().unwrap_or(0);
        if max_bytes == 0 {
            return Ok(0);
        }
        if !self.input_ready() {
            return Ok(0);
        }

        let mut bytes_read = 0;
        let cur_gen = self.generation.load(Ordering::Relaxed);

        while bytes_read < max_bytes {
            if let Some(ref chunk) = self.current_chunk {
                if chunk.generation != cur_gen || !chunk.is_dsd {
                    let remaining = chunk.frames().saturating_sub(self.read_offset) as u64;
                    self.sub_buffered_units(remaining);
                    self.current_chunk = None;
                    self.read_offset = 0;
                    continue;
                }

                let total_bytes_per_ch = chunk.frames();
                let available = total_bytes_per_ch.saturating_sub(self.read_offset);
                if available > 0 {
                    let to_copy = (max_bytes - bytes_read).min(available);
                    let dsd_bytes = chunk.dsd_slice();
                    let chunk_ch = chunk.channels;

                    for i in 0..to_copy {
                        let base_idx = (self.read_offset + i) * chunk_ch;
                        for (c, ch_slice) in channels.iter_mut().enumerate() {
                            if c < chunk_ch && base_idx + c < dsd_bytes.len() {
                                ch_slice[bytes_read + i] = dsd_bytes[base_idx + c];
                            } else {
                                ch_slice[bytes_read + i] = 0x69;
                            }
                        }
                    }

                    self.read_offset += to_copy;
                    bytes_read += to_copy;

                    if self.read_offset >= total_bytes_per_ch {
                        self.current_chunk = None;
                        self.read_offset = 0;
                    }

                    if bytes_read == max_bytes {
                        break;
                    }
                } else {
                    self.current_chunk = None;
                    self.read_offset = 0;
                }
            }

            match self.consumer.pop() {
                Ok(chunk) => {
                    if chunk.generation != cur_gen || !chunk.is_dsd {
                        self.sub_buffered_units(chunk.frames() as u64);
                        continue;
                    }
                    self.current_chunk = Some(chunk);
                    self.read_offset = 0;
                }
                Err(_) => {
                    break;
                }
            }
        }

        if account {
            self.sub_buffered_units(bytes_read as u64);
        }
        Ok(bytes_read)
    }
}

impl Drop for PipeWireVirtualSink {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(worker) = self._worker.take() {
            let _ = worker.join();
        }
    }
}

impl AudioInput for PipeWireVirtualSink {
    fn info(&self) -> &AudioStreamInfo {
        &self.info
    }

    fn dsd_decode_config(&self) -> Option<DsdDecodeConfig> {
        let stages = self
            .info
            .dsd_bit_rate?
            .power_of_two_decimation_stages_to(self.info.sample_rate)?;
        Some(DsdDecodeConfig {
            decimation_stages: stages as usize,
            gain_db: self.dsd_gain_db,
        })
    }

    fn read_stereo(&mut self, left: &mut [f64], right: &mut [f64]) -> Result<usize, EngineError> {
        self.read_channels(&mut [left, right])
    }

    fn read_channels(&mut self, channels: &mut [&mut [f64]]) -> Result<usize, EngineError> {
        if channels.is_empty() {
            return Ok(0);
        }
        let max_frames = channels.iter().map(|s| s.len()).min().unwrap_or(0);
        if max_frames == 0 {
            return Ok(0);
        }

        if !self.input_ready() {
            return Ok(0);
        }

        if let Some(config) = self.dsd_decode_config() {
            if self.decimators.is_empty() {
                self.decimators = (0..self.info.original_channels)
                    .map(|_| {
                        audigma_dsp::fir::HalfBandDecimationCascade::new(
                            config.decimation_stages,
                            65536,
                        )
                        .with_gain_db(config.gain_db)
                    })
                    .collect();
            }
            let stages = self.decimators[0].num_stages();
            let divider = 1usize << stages;
            let dsd_bytes_per_pcm = divider.div_ceil(8);
            let pcm_to_produce = max_frames.min(1024);
            let needed_dsd_bytes = pcm_to_produce * dsd_bytes_per_pcm;

            while self.dsd_residue.len() < channels.len() {
                self.dsd_residue.push(Vec::new());
            }

            let mut scratch = std::mem::take(&mut self.dsd_scratch);
            while scratch.len() < channels.len() {
                scratch.push(vec![0; 16384]);
            }
            for buf in scratch.iter_mut() {
                if buf.len() < needed_dsd_bytes {
                    buf.resize(needed_dsd_bytes, 0);
                }
            }

            let res_bytes = self.dsd_residue[0].len();
            #[allow(clippy::needless_range_loop)]
            for c in 0..channels.len() {
                if res_bytes > 0 {
                    scratch[c][..res_bytes].copy_from_slice(&self.dsd_residue[c]);
                    self.dsd_residue[c].clear();
                }
            }

            let to_read = needed_dsd_bytes.saturating_sub(res_bytes);
            let actual_read = if to_read > 0 {
                let mut dsd_ptrs: Vec<&mut [u8]> = scratch
                    .iter_mut()
                    .take(channels.len())
                    .map(|b| &mut b[res_bytes..res_bytes + to_read])
                    .collect();

                match self.read_dsd_bytes_internal(&mut dsd_ptrs, false) {
                    Ok(b) => b,
                    Err(e) => {
                        self.dsd_scratch = scratch;
                        return Err(e);
                    }
                }
            } else {
                0
            };

            let total_available = res_bytes + actual_read;
            let pcm_frames_produced = total_available / dsd_bytes_per_pcm;
            let aligned_bytes = pcm_frames_produced * dsd_bytes_per_pcm;
            let leftover = total_available - aligned_bytes;

            // Account the bytes actually converted to PCM. Any `leftover` stays
            // counted in `buffered_units` because it is carried in `dsd_residue`
            // and will be consumed by a later call.
            self.sub_buffered_units(aligned_bytes as u64);

            if leftover > 0 {
                #[allow(clippy::needless_range_loop)]
                for c in 0..channels.len() {
                    self.dsd_residue[c]
                        .extend_from_slice(&scratch[c][aligned_bytes..total_available]);
                }
            }

            if pcm_frames_produced == 0 {
                self.dsd_scratch = scratch;
                return Ok(0);
            }

            for (c, ch_slice) in channels.iter_mut().enumerate() {
                if c < self.decimators.len() {
                    if let Err(e) = self.decimators[c].process_dsd_bytes(
                        &scratch[c][..aligned_bytes],
                        &mut ch_slice[..pcm_frames_produced],
                    ) {
                        let code = match e {
                            audigma_dsp::FirError::OutputBufferTooSmall { .. } => 1,
                            audigma_dsp::FirError::TileSizeExceeded { .. } => 2,
                            audigma_dsp::FirError::UnsupportedRatio { .. } => 3,
                            audigma_dsp::FirError::UnalignedDecimationInput { .. } => 4,
                        };
                        self.decimation_recorder.record(c as u8, code);
                        ch_slice[..pcm_frames_produced].fill(0.0);
                    }
                } else {
                    ch_slice[..pcm_frames_produced].fill(0.0);
                }
            }

            self.dsd_scratch = scratch;
            return Ok(pcm_frames_produced);
        }

        let mut frames_read = 0;
        let cur_gen = self.generation.load(Ordering::Relaxed);

        while frames_read < max_frames {
            // First consume any remaining frames from the currently held chunk
            if let Some(ref chunk) = self.current_chunk {
                if chunk.generation != cur_gen || chunk.is_dsd {
                    let remaining = chunk.frames().saturating_sub(self.read_offset) as u64;
                    self.sub_buffered_units(remaining);
                    self.current_chunk = None;
                    self.read_offset = 0;
                    continue;
                }

                let total_frames = chunk.frames();
                let available = total_frames.saturating_sub(self.read_offset);
                if available > 0 {
                    let to_copy = (max_frames - frames_read).min(available);
                    let samples = chunk.as_slice();
                    let chunk_ch = chunk.channels;

                    for i in 0..to_copy {
                        let base_idx = (self.read_offset + i) * chunk_ch;
                        if chunk_ch == 1 && channels.len() > 1 {
                            // Mono source into multi-channel (e.g. stereo)
                            let s = samples[self.read_offset + i];
                            for ch_slice in channels.iter_mut() {
                                ch_slice[frames_read + i] = s;
                            }
                        } else {
                            for (c, ch_slice) in channels.iter_mut().enumerate() {
                                if c < chunk_ch {
                                    ch_slice[frames_read + i] = samples[base_idx + c];
                                } else {
                                    ch_slice[frames_read + i] = 0.0;
                                }
                            }
                        }
                    }

                    self.read_offset += to_copy;
                    frames_read += to_copy;
                    self.sub_buffered_units(to_copy as u64);

                    if self.read_offset >= total_frames {
                        self.current_chunk = None;
                        self.read_offset = 0;
                    }

                    if frames_read == max_frames {
                        break;
                    }
                } else {
                    self.current_chunk = None;
                    self.read_offset = 0;
                }
            }

            // If we still need more frames, pop the next chunk from the SPSC ring buffer
            match self.consumer.pop() {
                Ok(chunk) => {
                    if chunk.generation != cur_gen || chunk.is_dsd {
                        self.sub_buffered_units(chunk.frames() as u64);
                        continue;
                    }
                    self.current_chunk = Some(chunk);
                    self.read_offset = 0;
                }
                Err(_) => {
                    // Queue empty, return what we have accumulated
                    break;
                }
            }
        }

        Ok(frames_read)
    }

    fn read_dsd_bytes(&mut self, channels: &mut [&mut [u8]]) -> Result<usize, EngineError> {
        self.read_dsd_bytes_internal(channels, true)
    }

    fn check_rate_change(&self) -> Option<PcmRateHz> {
        let pending = self.pending_rate.load(Ordering::SeqCst);
        let active = self.active_rate.load(Ordering::SeqCst);
        if pending != 0 && pending != active {
            PcmRateHz::new(pending)
        } else {
            None
        }
    }

    fn check_volume_change(&self) -> Option<VolumeUpdate> {
        self.pending_volume.lock().unwrap().take()
    }

    fn set_generation(&mut self, generation_tag: u64) {
        self.generation.store(generation_tag, Ordering::SeqCst);
        while self.consumer.pop().is_ok() {}
        self.buffered_units.store(0, Ordering::Relaxed);
        // Re-arm the duration-based preroll. The previous stream's buffered
        // audio is no longer valid after a rate/format switch, and starting
        // from an empty ring guarantees chronic underruns.
        self.prerolled = false;
        self.current_chunk = None;
        self.read_offset = 0;
        for r in self.dsd_residue.iter_mut() {
            r.clear();
        }
    }

    fn set_active_rate(&mut self, rate: PcmRateHz) {
        self.active_rate.store(rate.get(), Ordering::SeqCst);
        self.pending_rate.store(0, Ordering::SeqCst);
        while self.consumer.pop().is_ok() {}
        self.buffered_units.store(0, Ordering::Relaxed);
        self.prerolled = false;
        self.current_chunk = None;
        self.read_offset = 0;
        for r in self.dsd_residue.iter_mut() {
            r.clear();
        }
        self.decimators.clear();
        if self.is_dsd.load(Ordering::SeqCst) {
            let dsd_rate_raw = self.active_dsd_bit_rate.load(Ordering::SeqCst);
            if let Some(dsd_rate) = DsdBitRateHz::new(dsd_rate_raw) {
                let intermediate_rate = audigma_dsp::rates::select_dsd_intermediate_rate(
                    dsd_rate,
                    None,
                    self.dsd_max_intermediate_rate,
                );
                self.info = AudioStreamInfo::new_dsd_with_intermediate(
                    dsd_rate,
                    self.info.original_channels,
                    None,
                    intermediate_rate,
                );
                return;
            }
        }
        self.info = AudioStreamInfo::new_pcm(rate, self.info.original_channels, None, Some(32));
    }

    fn can_rewind(&self) -> bool {
        false
    }

    fn rewind(&mut self) -> Result<(), EngineError> {
        Ok(())
    }

    fn buffer_occupancy(&self) -> Option<(usize, usize)> {
        let occupied = self.consumer.slots() + if self.current_chunk.is_some() { 1 } else { 0 };
        let capacity = self.consumer.buffer().capacity();
        Some((occupied, capacity))
    }

    fn buffer_fill_secs(&self) -> Option<f64> {
        Some(self.buffered_duration_secs())
    }

    fn dropped_chunks(&self) -> u64 {
        self.dropped_chunks.load(Ordering::Relaxed)
    }

    fn processed_chunks(&self) -> u64 {
        self.processed_chunks.load(Ordering::Relaxed)
    }

    fn input_clock(&self) -> audigma_engine::traits::InputClock {
        // Always its own driver, pulled by the demand gate against the preroll cushion.
        audigma_engine::traits::InputClock::DemandDriven
    }

    fn set_preroll_secs(&mut self, secs: f64) {
        PipeWireVirtualSink::set_preroll_secs(self, secs);
    }

    fn take_rt_consumer(&mut self) -> Option<audigma_log::RtLogConsumer> {
        self.rt_consumer.take()
    }

    fn dropped_rt_events(&self) -> Option<Arc<AtomicU64>> {
        Some(Arc::clone(&self.dropped_rt_events))
    }

    fn decimation_recorder(&self) -> Option<Arc<audigma_log::DecimationErrorRecorder>> {
        Some(Arc::clone(&self.decimation_recorder))
    }
}

/// PipeWire high-rate PCM output node (`Design.md §1.4`, `§3.3`).
///
/// Drives high-rate PCM back into the PipeWire graph (`shared-graph` clock),
/// optionally binding to a target hardware node (`alsa_output...`).
pub struct PipeWirePcmOutput {
    _worker: Option<JoinHandle<()>>,
    producer: Producer<Vec<u8>>,
    running: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
    overruns: Arc<AtomicU64>,
    writer_thread: Arc<AtomicPtr<std::thread::Thread>>,
    /// `true` while the pinned target sink exists; a blocked `write` gives up early when it
    /// turns `false` (the sink was removed) instead of waiting out its timeout.
    target_present: Option<Arc<AtomicBool>>,
}

/// Buffer capacity is not the graph's demand: only consume the requested frames.
/// A zero request means PipeWire supplied no suggestion, so use the aligned capacity.
fn output_buffer_bytes(capacity: usize, requested_frames: u64, stride: usize) -> usize {
    if stride == 0 {
        return 0;
    }
    let capacity_frames = capacity / stride;
    let frames = if requested_frames == 0 {
        capacity_frames
    } else {
        usize::try_from(requested_frames)
            .unwrap_or(usize::MAX)
            .min(capacity_frames)
    };
    frames * stride
}

impl PipeWirePcmOutput {
    /// Creates and connects a new output stream into the PipeWire graph.
    pub fn new(
        stream_name: &str,
        target_node: Option<&str>,
        rate_hz: u32,
        channels: u32,
        queue_depth: usize,
        placement: Option<audigma_engine::ThreadPlacement>,
    ) -> Result<Self, PipeWireError> {
        let (producer, mut consumer) = RingBuffer::<Vec<u8>>::new(queue_depth);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let underruns = Arc::new(AtomicU64::new(0));
        let overruns = Arc::new(AtomicU64::new(0));
        let underruns_cb = underruns.clone();
        let writer_thread: Arc<AtomicPtr<std::thread::Thread>> =
            Arc::new(AtomicPtr::new(std::ptr::null_mut()));
        let writer_thread_cb = writer_thread.clone();

        let stream_name_owned = stream_name.to_string();
        let target_node_owned = target_node.map(|s| s.to_string());

        let worker = std::thread::Builder::new()
            .name("audigma-pw-out".to_string())
            .spawn(move || {
                apply_thread_placement("audigma-pw-out", placement.as_ref());
                pipewire::init();
                let mainloop = match pipewire::main_loop::MainLoopBox::new(None) {
                    Ok(m) => m,
                    Err(_) => return,
                };
                let context = match pipewire::context::ContextBox::new(mainloop.loop_(), None) {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let core = match context.connect(None) {
                    Ok(c) => c,
                    Err(_) => return,
                };

                let mut props = properties! {
                    // A distinct node name lets the registry (and the output-loop guard) tell
                    // this stream apart from other audigma processes.
                    *pipewire::keys::NODE_NAME => stream_name_owned.as_str(),
                    *pipewire::keys::MEDIA_TYPE => "Audio",
                    *pipewire::keys::MEDIA_CATEGORY => "Playback",
                    *pipewire::keys::MEDIA_ROLE => "Music",
                    *pipewire::keys::AUDIO_CHANNELS => format!("{channels}"),
                    // Match the input sink's ~23 ms buffer duration rather than
                    // pulling the shared graph down to a 256-frame quantum.
                    // Native DSD passthrough sources such as MPD can fill the
                    // entire negotiated buffer on each graph cycle.
                    *pipewire::keys::NODE_LATENCY => format!("{}/{rate_hz}", quantum_frames_for_rate(rate_hz)),
                };

                if let Some(target) = &target_node_owned {
                    props.insert("target.object", target.as_str());
                    // Keep the pin: do not fall back to the default sink when the target is
                    // missing, and do not follow later default-sink changes. The default can be
                    // audigma's own input sink, which would loop the output back into itself.
                    props.insert("node.dont-fallback", "true");
                    props.insert("node.dont-move", "true");
                }

                let stream = match StreamBox::new(&core, &stream_name_owned, props) {
                    Ok(s) => s,
                    Err(_) => return,
                };

                let run_cb = running_clone.clone();
                // Retain any partially consumed queued chunk across PipeWire buffer callbacks.
                // The previous implementation popped one whole queued chunk per callback and
                // discarded the bytes that did not fit in the current PipeWire buffer, which
                // caused audible stuttering / rapid play-pause artefacts.
                let mut pending_buf: Option<Vec<u8>> = None;
                let mut pending_offset: usize = 0;
                let _listener = match stream
                    .add_local_listener_with_user_data(())
                    .process(move |stream, _| {
                        if !run_cb.load(Ordering::Relaxed) {
                            return;
                        }

                        if let Some(mut buffer) = stream.dequeue_buffer() {
                            let requested_frames = buffer.requested();
                            let datas = buffer.datas_mut();
                            if datas.is_empty() {
                                return;
                            }

                            let data = &mut datas[0];
                            let stride = (channels as usize) * std::mem::size_of::<f32>();

                            let mut bytes_written = 0usize;
                            if let Some(slice) = data.data() {
                                let total =
                                    output_buffer_bytes(slice.len(), requested_frames, stride);
                                while bytes_written < total {
                                    if pending_buf.is_none() {
                                        match consumer.pop() {
                                            Ok(buf) => {
                                                pending_buf = Some(buf);
                                                pending_offset = 0;
                                                let ptr = writer_thread_cb.load(Ordering::Acquire);
                                                if !ptr.is_null() {
                                                    unsafe {
                                                        (*ptr).unpark();
                                                    }
                                                }
                                            }
                                            Err(_) => {
                                                underruns_cb.fetch_add(1, Ordering::Relaxed);
                                                break;
                                            }
                                        }
                                    }

                                    let to_copy = {
                                        let buf = pending_buf.as_ref().unwrap();
                                        buf.len()
                                            .saturating_sub(pending_offset)
                                            .min(total - bytes_written)
                                    };

                                    if to_copy > 0 {
                                        let buf = pending_buf.as_ref().unwrap();
                                        slice[bytes_written..bytes_written + to_copy]
                                            .copy_from_slice(
                                                &buf[pending_offset..pending_offset + to_copy],
                                            );
                                        pending_offset += to_copy;
                                        bytes_written += to_copy;
                                    }

                                    let done = {
                                        let buf = pending_buf.as_ref().unwrap();
                                        pending_offset >= buf.len()
                                    };
                                    if done {
                                        pending_buf = None;
                                        pending_offset = 0;
                                    }
                                    if to_copy == 0 {
                                        // Defensive: never spin forever on an empty queued chunk.
                                        pending_buf = None;
                                        pending_offset = 0;
                                    }
                                }

                                if bytes_written < total {
                                    slice[bytes_written..total].fill(0);
                                    bytes_written = total;
                                }
                            }

                            let chunk = data.chunk_mut();
                            *chunk.offset_mut() = 0;
                            *chunk.stride_mut() = stride as _;
                            *chunk.size_mut() = bytes_written as _;
                        }
                    })
                    .register()
                {
                    Ok(l) => l,
                    Err(_) => return,
                };

                let pod_bytes = match build_format_pod(rate_hz, channels, &[], RateMode::Fixed, &[])
                {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let mut params = [Pod::from_bytes(&pod_bytes).unwrap()];

                if stream
                    .connect(
                        Direction::Output,
                        None,
                        StreamFlags::AUTOCONNECT
                            | StreamFlags::MAP_BUFFERS
                            | StreamFlags::RT_PROCESS,
                        &mut params,
                    )
                    .is_err()
                {
                    return;
                }

                while running_clone.load(Ordering::Relaxed) {
                    mainloop.loop_().iterate(pipewire::loop_::Timeout::Finite(
                        std::time::Duration::from_millis(1),
                    ));
                }
            })
            .map_err(|_e| PipeWireError::Native(pipewire::Error::NoMemory))?;

        Ok(Self {
            _worker: Some(worker),
            producer,
            running,
            underruns,
            overruns,
            writer_thread,
            target_present: None,
        })
    }
}

impl PipeWirePcmOutput {
    /// Gives the output a flag that is `true` while its pinned target sink exists. When the
    /// flag turns `false` while `write` is blocked on a full queue, `write` fails at once with
    /// `NotConnected` rather than after the 2 s queue timeout.
    pub fn set_target_presence(&mut self, flag: Arc<AtomicBool>) {
        self.target_present = Some(flag);
    }

    /// Blocks until the graph has consumed everything queued by `write`, so dropping the
    /// stream afterwards does not cut off the tail of a finite track.
    ///
    /// Gives up after `timeout` (the graph may be stalled or the node paused).
    pub fn drain(&mut self, timeout: std::time::Duration) {
        let start = std::time::Instant::now();
        let capacity = self.producer.buffer().capacity();
        while self.producer.slots() < capacity
            && self.running.load(Ordering::Relaxed)
            && start.elapsed() < timeout
        {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // The last chunk may still be partially held by the RT callback or the graph's
        // own buffer; one extra quantum covers it.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    /// Number of output callbacks where the queued byte stream was empty.
    pub fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Number of writes rejected because the output queue was full.
    pub fn overrun_count(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }
}

impl Drop for PipeWirePcmOutput {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Disconnect and finish any in-flight RT callback before freeing the
        // thread handle it may have loaded for notification.
        if let Some(worker) = self._worker.take() {
            let _ = worker.join();
        }
        let ptr = self
            .writer_thread
            .swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !ptr.is_null() {
            unsafe {
                drop(Box::from_raw(ptr));
            }
        }
    }
}

impl Write for PipeWirePcmOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.producer.is_full() {
            // Register current thread handle so PipeWire consumer can unpark us
            if self.writer_thread.load(Ordering::Relaxed).is_null() {
                let current = Box::into_raw(Box::new(std::thread::current()));
                if self
                    .writer_thread
                    .compare_exchange(
                        std::ptr::null_mut(),
                        current,
                        Ordering::SeqCst,
                        Ordering::Relaxed,
                    )
                    .is_err()
                {
                    unsafe {
                        drop(Box::from_raw(current));
                    }
                }
            }

            // Wait with backpressure for consumer to pop a slot
            let start = std::time::Instant::now();
            while self.producer.is_full() {
                if !self.running.load(Ordering::Relaxed) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "PipeWire output stopped",
                    ));
                }
                if self
                    .target_present
                    .as_ref()
                    .is_some_and(|p| !p.load(Ordering::Relaxed))
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "PipeWire target sink removed",
                    ));
                }
                if start.elapsed() >= std::time::Duration::from_millis(2000) {
                    self.overruns.fetch_add(1, Ordering::Relaxed);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "PipeWire output queue blocked for > 2000ms",
                    ));
                }
                std::thread::park_timeout(std::time::Duration::from_micros(500));
            }
        }

        self.producer
            .push(buf.to_vec())
            .map(|_| buf.len())
            .map_err(|_| std::io::Error::other("Failed to push to PipeWire queue"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AudioOutput for PipeWirePcmOutput {
    fn xrun_count(&self) -> u64 {
        self.underrun_count() + self.overrun_count()
    }
}

#[cfg(test)]
mod output_tests {
    use super::{output_buffer_bytes, quantum_frames_for_rate};

    #[test]
    fn requested_output_quantum_matches_native_dsd_buffer_duration() {
        for pcm_rate in [44_100u32, 88_200, 176_400, 352_800] {
            for dsd_byte_rate in [352_800u32, 705_600, 1_411_200, 2_822_400] {
                assert_eq!(
                    u64::from(quantum_frames_for_rate(pcm_rate)) * u64::from(dsd_byte_rate),
                    u64::from(quantum_frames_for_rate(dsd_byte_rate)) * u64::from(pcm_rate),
                );
            }
        }
    }

    #[test]
    fn consumes_requested_frames_not_allocation_capacity() {
        // Large allocations must not drain multiple graph cycles per callback.
        assert_eq!(output_buffer_bytes(131_072, 256, 8), 2048);
        assert_eq!(output_buffer_bytes(131_072, 1024, 8), 8192);
    }

    #[test]
    fn request_is_bounded_by_whole_frame_capacity() {
        assert_eq!(output_buffer_bytes(2051, 1024, 8), 2048);
        assert_eq!(output_buffer_bytes(2051, u64::MAX, 8), 2048);
    }

    #[test]
    fn absent_request_uses_aligned_capacity() {
        assert_eq!(output_buffer_bytes(2051, 0, 8), 2048);
        assert_eq!(output_buffer_bytes(0, 256, 8), 0);
        assert_eq!(output_buffer_bytes(7, 256, 8), 0);
        assert_eq!(output_buffer_bytes(2048, 256, 0), 0);
    }
}

#[cfg(test)]
mod input_clock_tests {
    use super::*;

    fn dsd_chunk(bytes: usize) -> TaggedChunk {
        TaggedChunk {
            generation: 0,
            channels: 2,
            is_dsd: true,
            len: bytes * 2,
            samples: Vec::new(),
            dsd_bytes: (0..bytes).flat_map(|_| [0x69, 0x96]).collect(),
        }
    }

    // No PipeWire connection or device access: exercise the actual raw/decoded
    // consumers using only their in-memory capture queue and native-unit counters.
    fn dsd_sink(bytes: usize) -> (PipeWireVirtualSink, Producer<TaggedChunk>) {
        let (producer, consumer) = RingBuffer::new(128);
        (
            PipeWireVirtualSink {
                info: AudioStreamInfo::new_dsd(DsdBitRateHz::DSD64, 2, None),
                _worker: None,
                consumer,
                current_chunk: Some(dsd_chunk(bytes)),
                read_offset: 0,
                generation: Arc::new(AtomicU64::new(0)),
                active_rate: Arc::new(AtomicU32::new(176400)),
                pending_rate: Arc::new(AtomicU32::new(0)),
                is_dsd: Arc::new(AtomicBool::new(true)),
                is_lsb: Arc::new(AtomicBool::new(false)),
                interleave: Arc::new(AtomicI32::new(1)),
                active_dsd_bit_rate: Arc::new(AtomicU32::new(DsdBitRateHz::DSD64.get())),
                pending_volume: Arc::new(Mutex::new(None)),
                running: Arc::new(AtomicBool::new(true)),
                dropped_chunks: Arc::new(AtomicU64::new(0)),
                processed_chunks: Arc::new(AtomicU64::new(1)),
                buffered_units: Arc::new(AtomicU64::new(bytes as u64)),
                dsd_gain_db: 3.0,
                dsd_max_intermediate_rate: None,
                decimators: Vec::new(),
                dsd_scratch: Vec::new(),
                dsd_residue: Vec::new(),
                prerolled: false,
                preroll_secs: 0.090,
                capture_cushion_ns: Arc::new(AtomicU64::new(90_000_000)),
                rt_consumer: None,
                dropped_rt_events: Arc::new(AtomicU64::new(0)),
                decimation_recorder: Arc::new(audigma_log::DecimationErrorRecorder::new()),
            },
            producer,
        )
    }

    #[test]
    fn fixed_dsd_buffer_bounds_are_exact() {
        let bytes = build_buffers_pod_bounds(2, 512, 512, 512).unwrap();
        let (_, Value::Object(object)) = PodDeserializer::deserialize_any_from(&bytes).unwrap()
        else {
            panic!("expected buffer object");
        };
        let size = object
            .properties
            .iter()
            .find(|p| p.key == libspa::sys::SPA_PARAM_BUFFERS_size)
            .unwrap();
        assert_eq!(
            size.value,
            Value::Choice(ChoiceValue::Int(libspa::utils::Choice(
                libspa::utils::ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: 512,
                    min: 512,
                    max: 512
                },
            )))
        );
    }

    #[test]
    fn tiny_dsd_quanta_cannot_deadlock_preroll() {
        assert!(!input_preroll_ready(0.005, 0.090, true, 63, 128));
        assert!(input_preroll_ready(0.006, 0.090, true, 64, 128));
        assert!(input_preroll_ready(0.090, 0.090, true, 4, 128));
        assert!(!input_preroll_ready(0.006, 0.090, false, 64, 128));
    }

    #[test]
    fn raw_dsd_reads_preserve_preroll_accounting_and_do_not_construct_decoders() {
        let (mut sink, mut producer) = dsd_sink(512);
        let mut left = [0; 2];
        let mut right = [0; 2];
        assert_eq!(
            sink.read_dsd_bytes(&mut [&mut left, &mut right]).unwrap(),
            0
        );
        assert!(!sink.prerolled);
        assert_eq!(sink.read_offset, 0);

        producer.push(dsd_chunk(40000)).unwrap();
        sink.buffered_units.fetch_add(40000, Ordering::Relaxed);
        assert_eq!(
            sink.read_dsd_bytes(&mut [&mut left, &mut right]).unwrap(),
            2
        );
        assert_eq!(left, [0x69; 2]);
        assert_eq!(right, [0x96; 2]);
        assert_eq!(sink.buffered_units.load(Ordering::Relaxed), 40510);
        assert!(sink.decimators.is_empty());
        assert!(sink.dsd_scratch.is_empty());
        let config = sink.dsd_decode_config().unwrap();
        assert_eq!(config.decimation_stages, 4);
        assert_eq!(config.gain_db, 3.0);

        AudioInput::set_active_rate(&mut sink, PcmRateHz::PCM_176400);
        assert!(!sink.prerolled);
        assert_eq!(
            sink.read_dsd_bytes(&mut [&mut left, &mut right]).unwrap(),
            0
        );
        assert!(sink.decimators.is_empty());
    }

    #[test]
    fn decoded_dsd_read_compatibility_retains_filter_gain_and_sample_bits() {
        let (mut sink, _producer) = dsd_sink(40000);
        let mut left = [0.0; 256];
        let mut right = [0.0; 256];
        assert_eq!(sink.read_stereo(&mut left, &mut right).unwrap(), 256);
        assert_eq!(sink.decimators.len(), 2);
        assert_eq!(sink.buffered_units.load(Ordering::Relaxed), 40000 - 512);
        for (actual, byte) in [(&left, 0x69), (&right, 0x96)] {
            let mut reference =
                audigma_dsp::fir::HalfBandDecimationCascade::new(4, 65536).with_gain_db(3.0);
            let mut expected = [0.0; 256];
            reference
                .process_dsd_bytes(&[byte; 512], &mut expected)
                .unwrap();
            for (actual, expected) in actual.iter().zip(expected) {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }
    }
}
