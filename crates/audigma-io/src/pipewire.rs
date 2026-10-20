//! Concrete PipeWire audio backends (`Design.md §1.4`, `§3.3`, `§7`).
//!
//! Provides:
//! - `PipeWireVirtualSink`: Registers a virtual audio sink node (`audigma_sink` with `media.class = "Audio/Sink"`)
//!   in the PipeWire graph, receiving PCM frames from desktop applications and players, and delivering them
//!   into an `audigma-engine` SPSC ring buffer.
//! - `PipeWirePcmOutput`: Plays high-rate PCM (`f32` stereo) directly into the PipeWire graph (`shared-graph` clock),
//!   optionally targeting a specific node (e.g. `alsa_output...`).

use audigma_dsp::PcmRateHz;
use audigma_engine::queue::TaggedChunk;
use audigma_engine::traits::{AudioInput, AudioOutput, EngineError};
use audigma_format::AudioStreamInfo;
use libspa::param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS};
use libspa::pod::Pod;
use libspa::utils::Direction;
use pipewire::properties::properties;
use pipewire::stream::{StreamBox, StreamFlags};
use rtrb::{Consumer, Producer, RingBuffer};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Helper function to build SPA EnumFormat POD for stereo F32LE audio.
fn build_format_pod(rate_hz: u32, channels: u32) -> Result<Vec<u8>, PipeWireError> {
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(rate_hz);
    audio_info.set_channels(channels);

    let mut position = [0u32; MAX_CHANNELS];
    position[0] = 3; // SPA_AUDIO_CHANNEL_FL
    position[1] = 4; // SPA_AUDIO_CHANNEL_FR
    audio_info.set_position(position);

    let obj = libspa::pod::Object {
        type_: libspa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: libspa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };

    libspa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &libspa::pod::Value::Object(obj),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| PipeWireError::FormatPod(format!("{e:?}")))
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
    running: Arc<AtomicBool>,
}

impl PipeWireVirtualSink {
    /// Creates and binds a new virtual sink node in the PipeWire graph.
    pub fn new(
        node_name: &str,
        rate_hz: u32,
        channels: u32,
        queue_chunks: usize,
    ) -> Result<Self, PipeWireError> {
        let desc = format!("Audigma Virtual Sink ({node_name})");
        let node_name_owned = node_name.to_string();

        let (mut producer, consumer) = RingBuffer::new(queue_chunks);
        let generation = Arc::new(AtomicU64::new(1));
        let running = Arc::new(AtomicBool::new(true));

        let gen_clone = generation.clone();
        let running_clone = running.clone();

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
                let sink_proxy = core.create_object::<pipewire::node::Node>(
                    "adapter",
                    &properties! {
                        "factory.name" => "support.null-audio-sink",
                        "node.name" => node_name_owned.as_str(),
                        "node.description" => desc.as_str(),
                        "media.class" => "Audio/Sink",
                        "audio.channels" => format!("{channels}"),
                        "audio.position" => "FL,FR",
                        "object.linger" => "0",
                    },
                );

                let _sink_node = match sink_proxy {
                    Ok(n) => n,
                    Err(_) => return,
                };

                let stream_name = format!("audigma-capture-{node_name_owned}");
                let props = properties! {
                    *pipewire::keys::NODE_NAME => stream_name.as_str(),
                    "target.object" => node_name_owned.as_str(),
                    *pipewire::keys::STREAM_CAPTURE_SINK => "true",
                    *pipewire::keys::MEDIA_TYPE => "Audio",
                    *pipewire::keys::MEDIA_CATEGORY => "Capture",
                    *pipewire::keys::MEDIA_ROLE => "Music",
                    *pipewire::keys::AUDIO_CHANNELS => format!("{channels}"),
                    "audio.position" => "FL,FR",
                };

                let stream = match StreamBox::new(&core, &stream_name, props) {
                    Ok(s) => s,
                    Err(_) => return,
                };

                let run_cb = running_clone.clone();
                let _listener = match stream
                    .add_local_listener_with_user_data(())
                    .param_changed(|_, _, id, param| {
                        let Some(param) = param else {
                            return;
                        };
                        if id != libspa::param::ParamType::Format.as_raw() {
                            return;
                        }

                        if let Ok((media_type, media_subtype)) =
                            libspa::param::format_utils::parse_format(param)
                        {
                            let mut info = AudioInfoRaw::new();
                            if info.parse(param).is_ok() {
                                eprintln!(
                                    "[audigma-pw-in] format negotiated: {:?}/{:?}, format={:?}, rate={}, channels={}, pos={:?}",
                                    media_type,
                                    media_subtype,
                                    info.format(),
                                    info.rate(),
                                    info.channels(),
                                    &info.position()[..info.channels() as usize]
                                );
                            } else {
                                eprintln!(
                                    "[audigma-pw-in] format parsed: {:?}/{:?}",
                                    media_type, media_subtype
                                );
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

                            if datas.len() >= 2 {
                                // Planar audio: datas[0] is Left, datas[1] is Right
                                let (first_slice, second_slice) = datas.split_at_mut(1);
                                let d0 = &mut first_slice[0];
                                let d1 = &mut second_slice[0];

                                let chunk0 = d0.chunk();
                                let chunk1 = d1.chunk();
                                let off0 = chunk0.offset() as usize;
                                let size0 = chunk0.size() as usize;
                                let off1 = chunk1.offset() as usize;
                                let size1 = chunk1.size() as usize;

                                let bytes0 = d0.data().map(|b| &b[off0..off0 + size0]).unwrap_or(&[]);
                                let bytes1 = d1.data().map(|b| &b[off1..off1 + size1]).unwrap_or(&[]);

                                let frames = (bytes0.len() / sample_size).min(bytes1.len() / sample_size);
                                if frames > 0 {
                                    let total_floats = frames * 2;
                                    let mut chunk = TaggedChunk::new(total_floats);
                                    let mut float_buf = Vec::with_capacity(total_floats);

                                    for i in 0..frames {
                                        let l_bytes: [u8; 4] = bytes0[i * 4..(i + 1) * 4].try_into().unwrap();
                                        let r_bytes: [u8; 4] = bytes1[i * 4..(i + 1) * 4].try_into().unwrap();
                                        float_buf.push(f32::from_le_bytes(l_bytes) as f64);
                                        float_buf.push(f32::from_le_bytes(r_bytes) as f64);
                                    }

                                    chunk.set(cur_gen, 2, &float_buf);
                                    let _ = producer.push(chunk);
                                }
                            } else {
                                // Interleaved audio: datas[0] contains all channels
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
                                            for chunk_bytes in valid_bytes.chunks_exact(sample_size) {
                                                let f = f32::from_le_bytes(chunk_bytes.try_into().unwrap());
                                                float_buf.push(f as f64);
                                            }

                                            chunk.set(cur_gen, channels as usize, &float_buf);
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

                let pod_bytes = match build_format_pod(rate_hz, channels) {
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
        let max_frames = left.len().min(right.len());
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

                    for i in 0..to_copy {
                        let sample_idx = (self.read_offset + i) * 2;
                        left[frames_read + i] = samples[sample_idx];
                        right[frames_read + i] = samples[sample_idx + 1];
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

                let pod_bytes = match build_format_pod(rate_hz, channels) {
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
