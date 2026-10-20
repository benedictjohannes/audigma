//! Concrete PipeWire audio backends (`Design.md §1.4`, `§3.3`, `§7`).
//!
//! Provides:
//! - `PipeWireVirtualSink`: Registers a virtual audio sink node (`audigma_sink` with `media.class = "Audio/Sink"`)
//!   in the PipeWire graph, receiving PCM frames from desktop applications and players, and delivering them
//!   into an `audigma-engine` SPSC ring buffer.
//! - `PipeWirePcmOutput`: Plays high-rate PCM (`f32` stereo) directly into the PipeWire graph (`shared-graph` clock),
//!   optionally targeting a specific node (e.g. `alsa_output...`).

use audigma_dsp::PcmRateHz;
use audigma_engine::VolumeUpdate;
use audigma_engine::queue::TaggedChunk;
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use thiserror::Error;

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
        EngineError::Pipeline(err.to_string())
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
    pending_volume: Arc<Mutex<Option<VolumeUpdate>>>,
    running: Arc<AtomicBool>,
}

impl PipeWireVirtualSink {
    /// Creates and binds a new virtual sink node in the PipeWire graph using an `InputConfig`.
    pub fn from_input_config(
        in_cfg: &InputConfig,
        default_rate_hz: u32,
        queue_chunks: usize,
    ) -> Result<Self, PipeWireError> {
        let node_name = in_cfg.node_name.as_deref().unwrap_or("audigma_sink");
        let channels = in_cfg.channels.unwrap_or(2) as u32;
        let positions = in_cfg.resolved_channel_positions();
        let rate_mode = in_cfg.rate_mode();
        let allowed_rates = in_cfg.resolved_allowed_rates();
        let rate_hz = in_cfg.rate_hz.map(|r| r.get()).unwrap_or(default_rate_hz);

        Self::new_internal(
            node_name,
            rate_hz,
            channels,
            positions,
            rate_mode,
            allowed_rates,
            queue_chunks,
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
            backend: audigma_settings::InputBackend::Pipewire,
            node_name: Some(node_name.to_string()),
            device: None,
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

    fn new_internal(
        node_name: &str,
        rate_hz: u32,
        channels: u32,
        channel_positions: Vec<String>,
        rate_mode: RateMode,
        allowed_rates: Vec<u32>,
        queue_chunks: usize,
    ) -> Result<Self, PipeWireError> {
        let desc = format!("Audigma Virtual Sink ({node_name})");
        let node_name_owned = node_name.to_string();

        let (mut producer, consumer) = RingBuffer::new(queue_chunks);
        let generation = Arc::new(AtomicU64::new(1));
        let active_rate = Arc::new(AtomicU32::new(rate_hz));
        let pending_rate = Arc::new(AtomicU32::new(0));
        let pending_volume = Arc::new(Mutex::new(None));
        let running = Arc::new(AtomicBool::new(true));

        let gen_clone = generation.clone();
        let active_rate_cb = active_rate.clone();
        let pending_rate_cb = pending_rate.clone();
        let pending_vol_cb = pending_volume.clone();
        let running_clone = running.clone();

        let pos_str = channel_positions.join(",");
        let allowed_rates_str = allowed_rates
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        let channel_positions_clone = channel_positions.clone();
        let allowed_rates_clone = allowed_rates.clone();

        let worker = std::thread::Builder::new()
            .name("audigma-pw-in".to_string())
            .spawn(move || {
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

                // Create the virtual sink adapter natively in PipeWire
                let mut adapter_props = properties! {
                    "factory.name" => "support.null-audio-sink",
                    "node.name" => node_name_owned.as_str(),
                    "node.description" => desc.as_str(),
                    "media.class" => "Audio/Sink",
                    "audio.channels" => format!("{channels}"),
                    "audio.position" => pos_str.as_str(),
                    "object.linger" => "0",
                };
                if rate_mode == RateMode::Dynamic {
                    adapter_props.insert("audio.allowed-rates", format!("[ {allowed_rates_str} ]"));
                }

                let sink_proxy =
                    core.create_object::<pipewire::node::Node>("adapter", &adapter_props);
                let sink_node = match sink_proxy {
                    Ok(n) => n,
                    Err(_) => return,
                };

                let pending_vol_sink = pending_vol_cb.clone();
                let _sink_listener = sink_node
                    .add_listener_local()
                    .param(move |_seq, param_type, _index, _next, param| {
                        let Some(param) = param else {
                            return;
                        };
                        if param_type == libspa::param::ParamType::Props {
                            if let Some(vol_update) = parse_props_pod(param.as_bytes()) {
                                eprintln!(
                                    "[audigma-pw-in] sink volume event: pct={:?}%, vol={:?} dB, mute={:?}",
                                    vol_update.volume_percent, vol_update.volume_db, vol_update.is_muted
                                );
                                let mut pending = pending_vol_sink.lock().unwrap();
                                let prev: VolumeUpdate = pending.unwrap_or_default();
                                *pending = Some(VolumeUpdate {
                                    volume_db: vol_update.volume_db.or(prev.volume_db),
                                    volume_percent: vol_update.volume_percent.or(prev.volume_percent),
                                    is_muted: vol_update.is_muted.or(prev.is_muted),
                                });
                            }
                        }
                    })
                    .register();

                sink_node.subscribe_params(&[libspa::param::ParamType::Props]);
                sink_node.enum_params(0, Some(libspa::param::ParamType::Props), 0, 1);
                let _sink_node = sink_node;

                let stream_name = format!("audigma-capture-{node_name_owned}");
                let mut stream_props = properties! {
                    *pipewire::keys::NODE_NAME => stream_name.as_str(),
                    "target.object" => node_name_owned.as_str(),
                    *pipewire::keys::STREAM_CAPTURE_SINK => "true",
                    *pipewire::keys::NODE_PASSIVE => "true",
                    *pipewire::keys::MEDIA_TYPE => "Audio",
                    *pipewire::keys::MEDIA_CATEGORY => "Capture",
                    *pipewire::keys::MEDIA_ROLE => "Music",
                    *pipewire::keys::AUDIO_CHANNELS => format!("{channels}"),
                    "audio.position" => pos_str.as_str(),
                };
                if rate_mode == RateMode::Dynamic {
                    stream_props.insert("audio.allowed-rates", format!("[ {allowed_rates_str} ]"));
                }

                let stream = match StreamBox::new(&core, &stream_name, stream_props) {
                    Ok(s) => s,
                    Err(_) => return,
                };

                let run_cb = running_clone.clone();
                let _listener = match stream
                    .add_local_listener_with_user_data(())
                    .param_changed(move |_, _, id, param| {
                        let Some(param) = param else {
                            return;
                        };
                        if id == libspa::param::ParamType::Format.as_raw() {
                            let mut info = AudioInfoRaw::new();
                            if info.parse(param).is_ok() {
                                let negotiated_rate = info.rate();
                                let negotiated_channels = info.channels();
                                eprintln!(
                                    "[audigma-pw-in] format negotiated: rate={}, channels={}, pos={:?}",
                                    negotiated_rate,
                                    negotiated_channels,
                                    &info.position()[..negotiated_channels as usize]
                                );
                                if negotiated_rate > 0 {
                                    let cur_active = active_rate_cb.load(Ordering::Relaxed);
                                    if cur_active != negotiated_rate {
                                        pending_rate_cb.store(negotiated_rate, Ordering::SeqCst);
                                    }
                                }
                            }
                        } else if id == libspa::param::ParamType::Props.as_raw() {
                            let bytes = param.as_bytes();
                            if let Some(vol_update) = parse_props_pod(bytes) {
                                eprintln!(
                                    "[audigma-pw-in] stream volume event: pct={:?}%, vol={:?} dB, mute={:?}",
                                    vol_update.volume_percent, vol_update.volume_db, vol_update.is_muted
                                );
                                let mut pending = pending_vol_cb.lock().unwrap();
                                let prev: VolumeUpdate = pending.unwrap_or_default();
                                *pending = Some(VolumeUpdate {
                                    volume_db: vol_update.volume_db.or(prev.volume_db),
                                    volume_percent: vol_update.volume_percent.or(prev.volume_percent),
                                    is_muted: vol_update.is_muted.or(prev.is_muted),
                                });
                            }
                        }
                    })
                    .process(move |stream, _| {
                        if !run_cb.load(Ordering::Relaxed) {
                            return;
                        }

                        if let Some(mut buffer) = stream.dequeue_buffer() {
                            let datas = buffer.datas_mut();
                            if datas.is_empty() {
                                return;
                            }

                            let cur_gen = gen_clone.load(Ordering::Relaxed);
                            let sample_size = std::mem::size_of::<f32>();
                            let n_ch = channels as usize;

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
                                    let _ = producer.push(chunk);
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
                                            let mut float_buf = Vec::with_capacity(total_floats);
                                            for chunk_bytes in valid_bytes.chunks_exact(sample_size)
                                            {
                                                let f = f32::from_le_bytes(
                                                    chunk_bytes.try_into().unwrap(),
                                                );
                                                float_buf.push(f as f64);
                                            }

                                            chunk.set(cur_gen, n_ch, &float_buf);
                                            let _ = producer.push(chunk);
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

                let pod_bytes = match build_format_pod(
                    rate_hz,
                    channels,
                    &channel_positions_clone,
                    rate_mode,
                    &allowed_rates_clone,
                ) {
                    Ok(b) => b,
                    Err(_) => return,
                };
                let mut params = [Pod::from_bytes(&pod_bytes).unwrap()];

                if stream
                    .connect(
                        Direction::Input,
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
                        std::time::Duration::from_millis(10),
                    ));
                }
            })
            .map_err(|_e| PipeWireError::Native(pipewire::Error::NoMemory))?;

        let info = AudioStreamInfo {
            sample_rate: PcmRateHz::new(rate_hz).unwrap_or(PcmRateHz::new(48000).unwrap()),
            original_channels: channels as u8,
            total_frames: None,
            bits_per_sample: Some(32),
        };

        Ok(Self {
            info,
            _worker: Some(worker),
            consumer,
            current_chunk: None,
            read_offset: 0,
            generation,
            active_rate,
            pending_rate,
            pending_volume,
            running,
        })
    }

    /// Sets the active generation tag for upcoming buffers.
    #[inline]
    pub fn set_generation(&self, r#gen: u64) {
        self.generation.store(r#gen, Ordering::SeqCst);
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

        let mut frames_read = 0;
        let cur_gen = self.generation.load(Ordering::Relaxed);

        while frames_read < max_frames {
            // First consume any remaining frames from the currently held chunk
            if let Some(ref chunk) = self.current_chunk {
                if chunk.generation != cur_gen {
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
                    if chunk.generation != cur_gen {
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
    }

    fn set_active_rate(&mut self, rate: PcmRateHz) {
        self.active_rate.store(rate.get(), Ordering::SeqCst);
        self.pending_rate.store(0, Ordering::SeqCst);
        self.info.sample_rate = rate;
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
}

/// PipeWire high-rate PCM output node (`Design.md §1.4`, `§3.3`).
///
/// Drives high-rate PCM back into the PipeWire graph (`shared-graph` clock),
/// optionally binding to a target hardware node (`alsa_output...`).
pub struct PipeWirePcmOutput {
    _worker: Option<JoinHandle<()>>,
    producer: Producer<Vec<u8>>,
    running: Arc<AtomicBool>,
}

impl PipeWirePcmOutput {
    /// Creates and connects a new output stream into the PipeWire graph.
    pub fn new(
        stream_name: &str,
        target_node: Option<&str>,
        rate_hz: u32,
        channels: u32,
        queue_depth: usize,
    ) -> Result<Self, PipeWireError> {
        let (producer, mut consumer) = RingBuffer::<Vec<u8>>::new(queue_depth);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let stream_name_owned = stream_name.to_string();
        let target_node_owned = target_node.map(|s| s.to_string());

        let worker = std::thread::Builder::new()
            .name("audigma-pw-out".to_string())
            .spawn(move || {
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
                    *pipewire::keys::MEDIA_TYPE => "Audio",
                    *pipewire::keys::MEDIA_CATEGORY => "Playback",
                    *pipewire::keys::MEDIA_ROLE => "Music",
                    *pipewire::keys::AUDIO_CHANNELS => format!("{channels}"),
                };

                if let Some(target) = &target_node_owned {
                    props.insert("target.object", target.as_str());
                }

                let stream = match StreamBox::new(&core, &stream_name_owned, props) {
                    Ok(s) => s,
                    Err(_) => return,
                };

                let run_cb = running_clone.clone();
                let _listener = match stream
                    .add_local_listener_with_user_data(())
                    .process(move |stream, _| {
                        if !run_cb.load(Ordering::Relaxed) {
                            return;
                        }

                        if let Some(mut buffer) = stream.dequeue_buffer() {
                            let datas = buffer.datas_mut();
                            if datas.is_empty() {
                                return;
                            }

                            let data = &mut datas[0];
                            let stride = (channels as usize) * std::mem::size_of::<f32>();

                            let bytes_written = if let Ok(buf) = consumer.pop() {
                                if let Some(slice) = data.data() {
                                    let count = buf.len().min(slice.len());
                                    slice[..count].copy_from_slice(&buf[..count]);
                                    count
                                } else {
                                    0
                                }
                            } else {
                                if let Some(slice) = data.data() {
                                    slice.fill(0);
                                    slice.len()
                                } else {
                                    0
                                }
                            };

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
                        std::time::Duration::from_millis(10),
                    ));
                }
            })
            .map_err(|_e| PipeWireError::Native(pipewire::Error::NoMemory))?;

        Ok(Self {
            _worker: Some(worker),
            producer,
            running,
        })
    }
}

impl Drop for PipeWirePcmOutput {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(worker) = self._worker.take() {
            let _ = worker.join();
        }
    }
}

impl Write for PipeWirePcmOutput {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.producer.is_full() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "PipeWire output queue full",
            ));
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

impl AudioOutput for PipeWirePcmOutput {}
