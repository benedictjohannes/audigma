//! ALSA audio output driver (`Design.md §1.4`, `§3.3`, `§4.5`, `§5.1–§5.2`).
//!
//! Provides direct exclusive ALSA hardware egress supporting:
//! - Native DSD (`DSD_U32_BE` / `DSD_U32_LE`)
//! - DoP (`s32le` -> `S32_LE`, `s24le` -> `S24_LE`, `s24_3le` -> `S24_3LE`; `auto` opens the
//!   first of these the device accepts and reports it via `AudioOutput::dop_format`)
//! - High-Rate PCM (`S16_LE`, `S24_3LE`, `S32_LE`, `FLOAT_LE`)
//!
//! Features buffer and period configuration, automatic XRun recovery,
//! partial-frame residue buffering, and clean hardware draining.

#[cfg(feature = "alsa")]
use alsa::pcm::{Access, Format, HwParams, PCM, State};
#[cfg(feature = "alsa")]
use alsa::{Direction, ValueOr};

use audigma_dsp::config::SampleFormat;
use audigma_engine::traits::{AudioOutput, EngineError};
use audigma_format::dop::DopContainerFormat;
use std::io::{ErrorKind, Result as IoResult, Write};
use thiserror::Error;

/// Errors arising during ALSA playback setup or streaming.
#[derive(Debug, Error)]
pub enum AlsaOutputError {
    #[error("ALSA backend feature is disabled")]
    Disabled,

    #[error("ALSA system error: {0}")]
    Alsa(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("unsupported sample format: {0:?}")]
    UnsupportedFormat(SampleFormat),

    #[error("unrecognized ALSA format string: '{0}'")]
    UnrecognizedFormat(String),

    #[error("failed to configure hardware parameters: {0}")]
    HwParamsFailed(String),

    #[error("failed to configure software parameters: {0}")]
    SwParamsFailed(String),

    #[error("ALSA device disconnected: {0}")]
    DeviceDisconnected(String),
}

impl From<AlsaOutputError> for EngineError {
    fn from(err: AlsaOutputError) -> Self {
        match err {
            AlsaOutputError::DeviceDisconnected(reason) => EngineError::DeviceDisconnected(reason),
            AlsaOutputError::Io(e) => EngineError::Io(e),
            AlsaOutputError::Disabled => EngineError::AudioBackend {
                backend: "alsa",
                message: "ALSA backend feature is disabled".to_string(),
            },
            other => EngineError::DeviceUnavailable {
                device: "alsa".to_string(),
                reason: other.to_string(),
            },
        }
    }
}

/// The ALSA sample format to open the device with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlsaFormatRequest {
    /// One ALSA format string, e.g. "DSD_U32_BE", "S32_LE", "S24_3LE", "S16_LE".
    Exact(String),
    /// DoP: the words to try, in order; the first the device accepts for the channel count
    /// and frame rate is opened. A single entry is a pinned word with no fallback.
    Dop(Vec<DopContainerFormat>),
}

impl AlsaFormatRequest {
    /// Human-readable request, e.g. `S32_LE` or `DoP [S24_3LE, S24_LE, S32_LE]`.
    pub fn describe(&self) -> String {
        match self {
            Self::Exact(name) => name.clone(),
            Self::Dop(words) => {
                let names: Vec<&str> = words.iter().map(|w| dop_alsa_format(*w)).collect();
                format!("DoP [{}]", names.join(", "))
            }
        }
    }
}

/// The ALSA format whose bit placement matches a DoP word layout (`Design.md §5.2`):
/// `s24le` is LSB-justified in 32 bits, which is ALSA `S24_LE`, not `S32_LE`.
pub const fn dop_alsa_format(word: DopContainerFormat) -> &'static str {
    match word {
        DopContainerFormat::S32Le => "S32_LE",
        DopContainerFormat::S24Le => "S24_LE",
        DopContainerFormat::S24_3Le => "S24_3LE",
    }
}

/// The first DoP word in `candidates` the device `accepts`, in order.
#[cfg_attr(not(feature = "alsa"), allow(dead_code))]
fn first_accepted_dop_word(
    candidates: &[DopContainerFormat],
    mut accepts: impl FnMut(DopContainerFormat) -> bool,
) -> Option<DopContainerFormat> {
    candidates.iter().copied().find(|&word| accepts(word))
}

/// Parameters for configuring an ALSA output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlsaOutputConfig {
    /// ALSA device identifier (e.g. "hw:CARD=K17,DEV=0" or "default").
    pub device: String,
    /// Channel count (default: 2 for stereo).
    pub channels: u32,
    /// ALSA hardware transport frame rate in Hz.
    pub frame_rate_hz: u32,
    /// Sample format to open; DoP may list several words to negotiate.
    pub format: AlsaFormatRequest,
    /// Target buffer size in milliseconds (e.g. 40 ms).
    pub buffer_ms: u32,
    /// Target period size in milliseconds (e.g. 5 ms).
    pub period_ms: u32,
}

impl Default for AlsaOutputConfig {
    fn default() -> Self {
        Self {
            device: "default".to_string(),
            channels: 2,
            frame_rate_hz: 44100,
            format: AlsaFormatRequest::Exact("S16_LE".to_string()),
            buffer_ms: audigma_settings::OutputConfig::DEFAULT_BUFFER_MS,
            period_ms: 5,
        }
    }
}

impl AlsaOutputConfig {
    /// Resolves an `AlsaOutputConfig` from application settings and resolved target rate specifications.
    pub fn from_settings(
        settings: &audigma_settings::Settings,
        target_spec: &audigma_dsp::config::TargetRateSpec,
    ) -> Result<Self, AlsaOutputError> {
        let device = settings
            .output
            .as_ref()
            .and_then(|o| o.device.clone())
            .unwrap_or_else(|| "default".to_string());

        let buffer_ms = settings.output.as_ref().map_or(
            audigma_settings::OutputConfig::DEFAULT_BUFFER_MS,
            audigma_settings::OutputConfig::buffer_ms,
        );

        let period_ms = settings
            .output
            .as_ref()
            .and_then(|o| o.period_ms)
            .unwrap_or(5);

        let channels = 2;

        match target_spec {
            audigma_dsp::config::TargetRateSpec::Dsd { bit_rate } => {
                let transport = settings
                    .output
                    .as_ref()
                    .and_then(|o| o.transport)
                    .unwrap_or(audigma_settings::OutputTransport::Native);

                match transport {
                    audigma_settings::OutputTransport::Native => {
                        let frame_rate_hz = bit_rate.to_alsa_dsd_u32_frame_rate().get();
                        Ok(Self {
                            device,
                            channels,
                            frame_rate_hz,
                            format: AlsaFormatRequest::Exact("DSD_U32_BE".to_string()),
                            buffer_ms,
                            period_ms,
                        })
                    }
                    audigma_settings::OutputTransport::Dop => {
                        let frame_rate_hz = bit_rate.to_dop_frame_rate().get();
                        let candidates = settings
                            .output
                            .as_ref()
                            .map(|o| o.dop_sample_format())
                            .unwrap_or_default()
                            .candidates()
                            .to_vec();

                        Ok(Self {
                            device,
                            channels,
                            frame_rate_hz,
                            format: AlsaFormatRequest::Dop(candidates),
                            buffer_ms,
                            period_ms,
                        })
                    }
                    audigma_settings::OutputTransport::Pcm => Err(AlsaOutputError::Alsa(
                        "cannot configure PCM transport for DSD target format".into(),
                    )),
                }
            }
            audigma_dsp::config::TargetRateSpec::Pcm {
                rate,
                sample_format,
                ..
            } => {
                let frame_rate_hz = rate.get();
                let alsa_format = match sample_format {
                    SampleFormat::S16Le => "S16_LE".to_string(),
                    SampleFormat::S24_3Le => "S24_3LE".to_string(),
                    SampleFormat::S24Le => "S24_LE".to_string(),
                    SampleFormat::S32Le => "S32_LE".to_string(),
                    SampleFormat::F32 => "FLOAT_LE".to_string(),
                    other => return Err(AlsaOutputError::UnsupportedFormat(*other)),
                };

                Ok(Self {
                    device,
                    channels,
                    frame_rate_hz,
                    format: AlsaFormatRequest::Exact(alsa_format),
                    buffer_ms,
                    period_ms,
                })
            }
        }
    }
}

/// Helper function to parse an ALSA format string into an `alsa::pcm::Format` and sample width in bytes.
#[cfg(feature = "alsa")]
pub fn parse_alsa_format(fmt_str: &str) -> Result<(Format, usize), AlsaOutputError> {
    match fmt_str.to_ascii_uppercase().as_str() {
        "DSD_U32_BE" | "DSDU32BE" => Ok((Format::DSDU32BE, 4)),
        "DSD_U32_LE" | "DSDU32LE" => Ok((Format::DSDU32LE, 4)),
        "DSD_U16_BE" | "DSDU16BE" => Ok((Format::DSDU16BE, 2)),
        "DSD_U16_LE" | "DSDU16LE" => Ok((Format::DSDU16LE, 2)),
        "DSD_U8" | "DSDU8" => Ok((Format::DSDU8, 1)),
        "S32_LE" | "S32LE" => Ok((Format::S32LE, 4)),
        "S24_LE" | "S24LE" => Ok((Format::S24LE, 4)),
        "S24_3LE" | "S243LE" => Ok((Format::S243LE, 3)),
        "S16_LE" | "S16LE" => Ok((Format::S16LE, 2)),
        "FLOAT_LE" | "F32" | "F32LE" => Ok((Format::FloatLE, 4)),
        other => Err(AlsaOutputError::UnrecognizedFormat(other.to_string())),
    }
}

/// Direct exclusive ALSA hardware output audio sink.
pub struct AlsaAudioOutput {
    #[cfg(feature = "alsa")]
    pcm: PCM,
    config: AlsaOutputConfig,
    /// ALSA format the device was opened with.
    alsa_format: String,
    /// DoP word the device was opened with (`None` for PCM and native DSD).
    dop_format: Option<DopContainerFormat>,
    frame_bytes: usize,
    residue: Vec<u8>,
    frames_written: u64,
    bytes_written: u64,
    xrun_count: u64,
    period_frames: u64,
    buffer_frames: u64,
}

impl AlsaAudioOutput {
    /// Opens and configures an ALSA playback device according to the given configuration.
    #[cfg(feature = "alsa")]
    pub fn open(config: AlsaOutputConfig) -> Result<Self, AlsaOutputError> {
        // Fail on a bad format string before touching the device.
        if let AlsaFormatRequest::Exact(name) = &config.format {
            parse_alsa_format(name)?;
        }

        let pcm = PCM::new(&config.device, Direction::Playback, false).map_err(|e| {
            AlsaOutputError::Alsa(format!(
                "failed to open PCM device '{}': {e}",
                config.device
            ))
        })?;

        // Negotiate on the handle being opened, so the word cannot change between the
        // check and the open (`Design.md §5.2`).
        let (alsa_format, dop_format) = match &config.format {
            AlsaFormatRequest::Exact(name) => (name.clone(), None),
            AlsaFormatRequest::Dop(candidates) => {
                let word = Self::negotiate_dop_word(&pcm, &config, candidates)?;
                (dop_alsa_format(word).to_string(), Some(word))
            }
        };
        let (format, bytes_per_sample) = parse_alsa_format(&alsa_format)?;
        let frame_bytes = bytes_per_sample * (config.channels as usize);
        if frame_bytes == 0 {
            return Err(AlsaOutputError::Alsa("invalid frame byte size 0".into()));
        }

        let hwp = HwParams::any(&pcm).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!("failed to query hw_params: {e}"))
        })?;

        hwp.set_access(Access::RWInterleaved).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!("failed to set RWInterleaved access: {e}"))
        })?;

        hwp.set_format(format).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!(
                "device does not support format '{alsa_format}': {e}"
            ))
        })?;

        hwp.set_channels(config.channels).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!(
                "device does not support {} channels: {e}",
                config.channels
            ))
        })?;

        hwp.set_rate_near(config.frame_rate_hz, ValueOr::Nearest)
            .map_err(|e| {
                AlsaOutputError::HwParamsFailed(format!(
                    "failed to set rate {}: {e}",
                    config.frame_rate_hz
                ))
            })?;

        // Calculate and configure buffer and period times
        let buffer_time_us = (config.buffer_ms.max(5)) * 1000;
        let period_time_us = (config.period_ms.max(1)) * 1000;

        let _ = hwp.set_buffer_time_near(buffer_time_us, ValueOr::Nearest);
        let _ = hwp.set_period_time_near(period_time_us, ValueOr::Nearest);

        pcm.hw_params(&hwp).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!("failed to apply hw_params: {e}"))
        })?;

        let period_frames = hwp.get_period_size().unwrap_or(1024).max(1) as u64;
        let buffer_frames = hwp
            .get_buffer_size()
            .unwrap_or((period_frames * 4) as alsa::pcm::Frames)
            .max(1) as u64;
        drop(hwp);

        // Software parameters
        if let Ok(swp) = pcm.sw_params_current() {
            let start_threshold = period_frames.min(buffer_frames) as alsa::pcm::Frames;
            let _ = swp.set_start_threshold(start_threshold);
            let _ = swp.set_avail_min(period_frames as alsa::pcm::Frames);
            let _ = pcm.sw_params(&swp);
        }

        Ok(Self {
            pcm,
            config,
            alsa_format,
            dop_format,
            frame_bytes,
            residue: Vec::with_capacity(frame_bytes),
            frames_written: 0,
            bytes_written: 0,
            xrun_count: 0,
            period_frames,
            buffer_frames,
        })
    }

    /// Picks the DoP word to open: the first candidate the device accepts with the configured
    /// channel count at exactly the DoP frame rate. A single candidate (a pinned
    /// `dop_sample_format`) is returned as is, so the open reports its own precise error.
    #[cfg(feature = "alsa")]
    fn negotiate_dop_word(
        pcm: &PCM,
        config: &AlsaOutputConfig,
        candidates: &[DopContainerFormat],
    ) -> Result<DopContainerFormat, AlsaOutputError> {
        if let [only] = candidates {
            return Ok(*only);
        }
        let picked = first_accepted_dop_word(candidates, |word| {
            let Ok((format, _)) = parse_alsa_format(dop_alsa_format(word)) else {
                return false;
            };
            // A fresh configuration space per word: a refused word must not narrow the next.
            let Ok(hwp) = HwParams::any(pcm) else {
                return false;
            };
            hwp.set_access(Access::RWInterleaved).is_ok()
                && hwp.set_format(format).is_ok()
                && hwp.set_channels(config.channels).is_ok()
                && hwp.test_rate(config.frame_rate_hz).is_ok()
        });
        match picked {
            Some(word) => {
                tracing::info!(
                    target: "audigma::alsa",
                    device = %config.device,
                    dop_format = ?word,
                    alsa_format = dop_alsa_format(word),
                    "DoP word negotiated"
                );
                Ok(word)
            }
            None => Err(AlsaOutputError::HwParamsFailed(format!(
                "device accepts none of {} for {} channels at {} Hz",
                config.format.describe(),
                config.channels,
                config.frame_rate_hz
            ))),
        }
    }

    #[cfg(not(feature = "alsa"))]
    pub fn open(_config: AlsaOutputConfig) -> Result<Self, AlsaOutputError> {
        Err(AlsaOutputError::Disabled)
    }

    /// Returns the target device path string.
    pub fn device(&self) -> &str {
        &self.config.device
    }

    /// Returns the ALSA format the device was opened with (the negotiated one for DoP `auto`).
    pub fn alsa_format(&self) -> &str {
        &self.alsa_format
    }

    /// Returns the hardware frame rate in Hz.
    pub fn frame_rate_hz(&self) -> u32 {
        self.config.frame_rate_hz
    }

    /// Returns the number of channels.
    pub fn channels(&self) -> u32 {
        self.config.channels
    }

    /// Frame size in bytes across all channels.
    pub fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    /// Period size in frames negotiated with ALSA.
    pub fn period_frames(&self) -> u64 {
        self.period_frames
    }

    /// Buffer size in frames negotiated with ALSA.
    pub fn buffer_frames(&self) -> u64 {
        self.buffer_frames
    }

    /// Total hardware frames written.
    pub fn frames_written(&self) -> u64 {
        self.frames_written
    }

    /// Total bytes written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Total XRun (buffer underrun) events recovered.
    pub fn xrun_count(&self) -> u64 {
        self.xrun_count
    }

    /// Drains any remaining buffered samples to hardware and blocks until playback finishes.
    #[cfg(feature = "alsa")]
    pub fn drain(&mut self) -> Result<(), AlsaOutputError> {
        // First flush any remaining residue padded with zeros
        if !self.residue.is_empty() {
            let pad_len = self.frame_bytes - self.residue.len();
            self.residue.resize(self.frame_bytes, 0);
            let _ = self.write_frames(&self.residue.clone());
            self.residue.clear();
            self.bytes_written += pad_len as u64;
        }

        self.pcm.drain().map_err(|e| {
            let code = e.errno();
            if code == libc::ENODEV || code == libc::EBADFD || code == libc::EIO {
                AlsaOutputError::DeviceDisconnected(format!("{e}"))
            } else {
                AlsaOutputError::Alsa(format!("failed to drain PCM stream: {e}"))
            }
        })?;
        Ok(())
    }

    #[cfg(not(feature = "alsa"))]
    pub fn drain(&mut self) -> Result<(), AlsaOutputError> {
        Err(AlsaOutputError::Disabled)
    }

    /// Internal frame write loop with automatic XRun recovery.
    #[cfg(feature = "alsa")]
    fn write_frames(&mut self, data: &[u8]) -> IoResult<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        let mut offset = 0;
        while offset < data.len() {
            let chunk = &data[offset..];
            let mut io = self.pcm.io_bytes();
            match io.write(chunk) {
                Ok(0) => {
                    break;
                }
                Ok(n) => {
                    offset += n;
                }
                Err(e) => {
                    // Check if XRun occurred
                    drop(io);
                    if self.pcm.state() == State::XRun || e.kind() == ErrorKind::WouldBlock {
                        self.xrun_count += 1;
                        if let Err(prep_err) = self.pcm.prepare() {
                            let prep_code = prep_err.errno();
                            if prep_code == libc::ENODEV
                                || prep_code == libc::EBADFD
                                || prep_code == libc::EIO
                            {
                                return Err(std::io::Error::new(
                                    ErrorKind::BrokenPipe,
                                    format!("ALSA device disconnected during recovery: {prep_err}"),
                                ));
                            }
                            return Err(std::io::Error::other(format!(
                                "ALSA XRun recovery failed (prepare error: {prep_err})"
                            )));
                        }
                        // Retry writing remainder after prepare
                        continue;
                    } else {
                        // Check for device disconnect / removal errors
                        if let Some(code) = e.raw_os_error() {
                            if code == libc::ENODEV
                                || code == -libc::ENODEV
                                || code == libc::EBADFD
                                || code == -libc::EBADFD
                                || code == libc::EIO
                                || code == -libc::EIO
                            {
                                return Err(std::io::Error::new(
                                    ErrorKind::BrokenPipe,
                                    format!("ALSA device disconnected: {e}"),
                                ));
                            }
                        }

                        return Err(e);
                    }
                }
            }
        }

        let written_frames = (offset / self.frame_bytes) as u64;
        self.frames_written += written_frames;
        self.bytes_written += offset as u64;
        Ok(offset)
    }

    #[cfg(not(feature = "alsa"))]
    fn write_frames(&mut self, _data: &[u8]) -> IoResult<usize> {
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "ALSA backend feature is disabled",
        ))
    }
}

impl Write for AlsaAudioOutput {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // If we have residue from a previous write, prepend it
        if !self.residue.is_empty() {
            let mut combined = std::mem::take(&mut self.residue);
            combined.extend_from_slice(buf);

            let whole_frames_len = (combined.len() / self.frame_bytes) * self.frame_bytes;
            if whole_frames_len == 0 {
                // Still not enough for a whole frame
                self.residue = combined;
                return Ok(buf.len());
            }

            let frames_data = &combined[..whole_frames_len];
            let written = self.write_frames(frames_data)?;

            // Store remaining residue: unwritten whole frames + unaligned trailing bytes
            if written < combined.len() {
                self.residue.extend_from_slice(&combined[written..]);
            }
            return Ok(buf.len());
        }

        let whole_frames_len = (buf.len() / self.frame_bytes) * self.frame_bytes;
        if whole_frames_len == 0 {
            // Under 1 frame, buffer in residue
            self.residue.extend_from_slice(buf);
            return Ok(buf.len());
        }

        let frames_data = &buf[..whole_frames_len];
        let written = self.write_frames(frames_data)?;

        // Store any unwritten bytes in residue
        if written < buf.len() {
            self.residue.extend_from_slice(&buf[written..]);
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        #[cfg(feature = "alsa")]
        {
            if !self.residue.is_empty() {
                let pad_len = self.frame_bytes - self.residue.len();
                self.residue.resize(self.frame_bytes, 0);
                let _ = self.write_frames(&self.residue.clone());
                self.residue.clear();
                self.bytes_written += pad_len as u64;
            }
        }
        Ok(())
    }
}

impl AudioOutput for AlsaAudioOutput {
    fn can_seek(&self) -> bool {
        false
    }

    fn dop_format(&self) -> Option<DopContainerFormat> {
        self.dop_format
    }

    fn xrun_count(&self) -> u64 {
        self.xrun_count
    }

    fn buffered_us(&self) -> Option<u64> {
        #[cfg(feature = "alsa")]
        {
            let rate = self.config.frame_rate_hz as u64;
            if rate == 0 {
                return None;
            }
            // Negative delay means the device is in an error/underrun state.
            let frames = self.pcm.delay().ok()?;
            let frames = u64::try_from(frames).ok()?;
            Some(frames.saturating_mul(1_000_000) / rate)
        }
        #[cfg(not(feature = "alsa"))]
        {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_dsp::config::{SampleFormat, TargetRateSpec};
    use audigma_dsp::rates::{DsdBitRateHz, PcmRateHz};
    use audigma_settings::{
        DopSampleFormat, OutputBackend, OutputConfig, OutputTransport, Settings,
    };

    #[test]
    #[cfg(feature = "alsa")]
    fn test_parse_alsa_format() {
        let (fmt, bytes) = parse_alsa_format("DSD_U32_BE").unwrap();
        assert_eq!(fmt, Format::DSDU32BE);
        assert_eq!(bytes, 4);

        let (fmt, bytes) = parse_alsa_format("s24_3le").unwrap();
        assert_eq!(fmt, Format::S243LE);
        assert_eq!(bytes, 3);

        let (fmt, bytes) = parse_alsa_format("s16le").unwrap();
        assert_eq!(fmt, Format::S16LE);
        assert_eq!(bytes, 2);

        let (fmt, bytes) = parse_alsa_format("s32_le").unwrap();
        assert_eq!(fmt, Format::S32LE);
        assert_eq!(bytes, 4);

        assert!(parse_alsa_format("invalid_format").is_err());
    }

    #[test]
    fn test_alsa_output_config_native_dsd() {
        let mut settings = Settings::default_offline(audigma_settings::OutputContainer::Raw);
        settings.output = Some(OutputConfig {
            backend: Some(OutputBackend::Alsa),
            target_node: None,
            device: Some("hw:CARD=K17,DEV=0".to_string()),
            transport: Some(OutputTransport::Native),
            dop_sample_format: None,
            volume_control: None,
            device_wait_ms: None,
            device_wait_expiry: None,
            keep_audio_sink_ms: None,
            mixer_control: None,
            reserve_device: Some(true),
            buffer_ms: Some(50),
            period_ms: Some(10),
            quantum_frames: None,
        });

        let target_spec = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD256,
        };

        let cfg = AlsaOutputConfig::from_settings(&settings, &target_spec).unwrap();
        assert_eq!(cfg.device, "hw:CARD=K17,DEV=0");
        assert_eq!(cfg.channels, 2);
        // DSD256 at 11,289,600 Hz / 32 = 352,800 frames/s
        assert_eq!(cfg.frame_rate_hz, 352_800);
        assert_eq!(cfg.format, AlsaFormatRequest::Exact("DSD_U32_BE".into()));
        assert_eq!(cfg.buffer_ms, 50);
        assert_eq!(cfg.period_ms, 10);
    }

    #[test]
    fn test_alsa_output_config_dop() {
        use DopContainerFormat as C;
        // `auto` (and unset) lists every DoP word to negotiate; a pinned word lists only itself.
        for (requested, candidates) in [
            (None, vec![C::S24_3Le, C::S24Le, C::S32Le]),
            (
                Some(DopSampleFormat::Auto),
                vec![C::S24_3Le, C::S24Le, C::S32Le],
            ),
            (Some(DopSampleFormat::S32Le), vec![C::S32Le]),
            (Some(DopSampleFormat::S24Le), vec![C::S24Le]),
            (Some(DopSampleFormat::S24_3Le), vec![C::S24_3Le]),
        ] {
            let mut settings = Settings::default_offline(audigma_settings::OutputContainer::Raw);
            settings.output = Some(OutputConfig {
                backend: Some(OutputBackend::Alsa),
                target_node: None,
                device: Some("hw:CARD=JA11,DEV=0".to_string()),
                transport: Some(OutputTransport::Dop),
                dop_sample_format: requested,
                volume_control: None,
                device_wait_ms: None,
                device_wait_expiry: None,
                keep_audio_sink_ms: None,
                mixer_control: None,
                reserve_device: Some(true),
                buffer_ms: Some(40),
                period_ms: Some(5),
                quantum_frames: None,
            });

            let target_spec = TargetRateSpec::Dsd {
                bit_rate: DsdBitRateHz::DSD128,
            };

            let cfg = AlsaOutputConfig::from_settings(&settings, &target_spec).unwrap();
            assert_eq!(cfg.device, "hw:CARD=JA11,DEV=0");
            assert_eq!(cfg.channels, 2);
            // DSD128 at 5,644,800 Hz / 16 = 352,800 frames/s
            assert_eq!(cfg.frame_rate_hz, 352_800);
            assert_eq!(
                cfg.format,
                AlsaFormatRequest::Dop(candidates),
                "{requested:?}"
            );
        }
    }

    #[test]
    fn test_dop_word_alsa_formats() {
        // Each DoP word opens the ALSA format whose bit placement it matches, so the marker
        // lands in the DAC's top 24 bits (`Design.md §5.2`).
        assert_eq!(dop_alsa_format(DopContainerFormat::S32Le), "S32_LE");
        assert_eq!(dop_alsa_format(DopContainerFormat::S24Le), "S24_LE");
        assert_eq!(dop_alsa_format(DopContainerFormat::S24_3Le), "S24_3LE");
    }

    #[test]
    fn test_dop_word_negotiation_order() {
        use DopContainerFormat as C;
        let auto = DopSampleFormat::Auto.candidates();
        let device = |supported: &'static [&'static str]| {
            move |w: C| supported.contains(&dop_alsa_format(w))
        };
        // JadeAudio JA11: packed 24 and 32-bit PCM. Its DoP detector works on the 24-bit
        // altset only, so `S24_3LE` must win.
        assert_eq!(
            first_accepted_dop_word(auto, device(&["S16_LE", "S24_3LE", "S32_LE"])),
            Some(C::S24_3Le)
        );
        // FiiO K17: no 24-bit format, so the `S32_LE` fallback.
        assert_eq!(
            first_accepted_dop_word(auto, device(&["S16_LE", "S32_LE"])),
            Some(C::S32Le)
        );
        // A 24-bit word beats `S32_LE` whichever 24-bit form the card has.
        assert_eq!(
            first_accepted_dop_word(auto, device(&["S24_LE", "S32_LE"])),
            Some(C::S24Le)
        );
        // I2S-style card exposing only `S24_LE`.
        assert_eq!(
            first_accepted_dop_word(auto, device(&["S16_LE", "S24_LE"])),
            Some(C::S24Le)
        );
        // No 24-bit carrier at all.
        assert_eq!(first_accepted_dop_word(auto, device(&["S16_LE"])), None);
        // A pinned word never falls back.
        assert_eq!(
            first_accepted_dop_word(DopSampleFormat::S24Le.candidates(), device(&["S32_LE"])),
            None
        );
    }

    #[test]
    fn test_format_request_describe() {
        assert_eq!(
            AlsaFormatRequest::Exact("S32_LE".into()).describe(),
            "S32_LE"
        );
        assert_eq!(
            AlsaFormatRequest::Dop(DopSampleFormat::Auto.candidates().to_vec()).describe(),
            "DoP [S24_3LE, S24_LE, S32_LE]"
        );
    }

    #[test]
    fn test_alsa_output_config_high_rate_pcm() {
        let mut settings = Settings::default_offline(audigma_settings::OutputContainer::Raw);
        settings.output = Some(OutputConfig {
            backend: Some(OutputBackend::Alsa),
            target_node: None,
            device: Some("hw:0,0".to_string()),
            transport: Some(OutputTransport::Pcm),
            dop_sample_format: Some(DopSampleFormat::S32Le),
            volume_control: None,
            device_wait_ms: None,
            device_wait_expiry: None,
            keep_audio_sink_ms: None,
            mixer_control: None,
            reserve_device: None,
            buffer_ms: None,
            period_ms: None,
            quantum_frames: None,
        });

        let target_spec = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_768000,
            sample_format: SampleFormat::S32Le,
        };

        let cfg = AlsaOutputConfig::from_settings(&settings, &target_spec).unwrap();
        assert_eq!(cfg.device, "hw:0,0");
        assert_eq!(cfg.channels, 2);
        assert_eq!(cfg.frame_rate_hz, 768_000);
        assert_eq!(cfg.format, AlsaFormatRequest::Exact("S32_LE".into()));
        assert_eq!(cfg.buffer_ms, 20);
        assert_eq!(cfg.period_ms, 5);
    }

    #[test]
    fn test_alsa_output_error_conversion_to_engine_error() {
        let err1 = AlsaOutputError::DeviceDisconnected("ENODEV".into());
        let engine_err1: EngineError = err1.into();
        match engine_err1 {
            EngineError::DeviceDisconnected(msg) => assert_eq!(msg, "ENODEV"),
            _ => panic!("expected DeviceDisconnected"),
        }

        let err2 = AlsaOutputError::Disabled;
        let engine_err2: EngineError = err2.into();
        match engine_err2 {
            EngineError::AudioBackend { backend, message } => {
                assert_eq!(backend, "alsa");
                assert!(message.contains("disabled"));
            }
            _ => panic!("expected AudioBackend"),
        }

        let err3 = AlsaOutputError::HwParamsFailed("channels unsupported".into());
        let engine_err3: EngineError = err3.into();
        match engine_err3 {
            EngineError::DeviceUnavailable { device, reason } => {
                assert_eq!(device, "alsa");
                assert!(reason.contains("channels unsupported"));
            }
            _ => panic!("expected DeviceUnavailable"),
        }
    }
}
