//! ALSA audio output driver (`Design.md §1.4`, `§3.3`, `§4.5`, `§5.1–§5.2`, `Plan.md Phase 3`).
//!
//! Provides direct exclusive ALSA hardware egress supporting:
//! - Native DSD (`DSD_U32_BE` / `DSD_U32_LE`)
//! - DoP (`S32_LE`, `S24_LE`, `S24_3LE`)
//! - High-Rate PCM (`S16_LE`, `S24_3LE`, `S32_LE`, `FLOAT_LE`)
//!
//! Features buffer and period configuration, automatic XRun recovery,
//! partial-frame residue buffering, and clean hardware draining.

#[cfg(feature = "alsa")]
use alsa::pcm::{Access, Format, HwParams, PCM, State};
#[cfg(feature = "alsa")]
use alsa::{Direction, ValueOr};

use audigma_dsp::config::SampleFormat;
use audigma_engine::traits::AudioOutput;
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

/// Parameters for configuring an ALSA output stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlsaOutputConfig {
    /// ALSA device identifier (e.g. "hw:CARD=K17,DEV=0" or "default").
    pub device: String,
    /// Channel count (default: 2 for stereo).
    pub channels: u32,
    /// ALSA hardware transport frame rate in Hz.
    pub frame_rate_hz: u32,
    /// ALSA format string (e.g. "DSD_U32_BE", "S32_LE", "S24_3LE", "S16_LE").
    pub alsa_format: String,
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
            alsa_format: "S16_LE".to_string(),
            buffer_ms: 40,
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

        let buffer_ms = settings
            .output
            .as_ref()
            .and_then(|o| o.buffer_ms)
            .unwrap_or(40);

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
                            alsa_format: "DSD_U32_BE".to_string(),
                            buffer_ms,
                            period_ms,
                        })
                    }
                    audigma_settings::OutputTransport::Dop => {
                        let frame_rate_hz = bit_rate.to_dop_frame_rate().get();
                        let sample_format = settings
                            .output
                            .as_ref()
                            .and_then(|o| o.sample_format)
                            .unwrap_or(SampleFormat::S32Le);

                        let alsa_format = match sample_format {
                            SampleFormat::S32Le | SampleFormat::S24Le => "S32_LE".to_string(),
                            SampleFormat::S24_3Le => "S24_3LE".to_string(),
                            SampleFormat::S16Le => "S16_LE".to_string(),
                            other => return Err(AlsaOutputError::UnsupportedFormat(other)),
                        };

                        Ok(Self {
                            device,
                            channels,
                            frame_rate_hz,
                            alsa_format,
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
                    alsa_format,
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
        let (format, bytes_per_sample) = parse_alsa_format(&config.alsa_format)?;
        let frame_bytes = bytes_per_sample * (config.channels as usize);
        if frame_bytes == 0 {
            return Err(AlsaOutputError::Alsa("invalid frame byte size 0".into()));
        }

        let pcm = PCM::new(&config.device, Direction::Playback, false).map_err(|e| {
            AlsaOutputError::Alsa(format!(
                "failed to open PCM device '{}': {e}",
                config.device
            ))
        })?;

        let hwp = HwParams::any(&pcm).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!("failed to query hw_params: {e}"))
        })?;

        hwp.set_access(Access::RWInterleaved).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!("failed to set RWInterleaved access: {e}"))
        })?;

        hwp.set_format(format).map_err(|e| {
            AlsaOutputError::HwParamsFailed(format!(
                "device does not support format '{}': {e}",
                config.alsa_format
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
            frame_bytes,
            residue: Vec::with_capacity(frame_bytes),
            frames_written: 0,
            bytes_written: 0,
            xrun_count: 0,
            period_frames,
            buffer_frames,
        })
    }

    #[cfg(not(feature = "alsa"))]
    pub fn open(_config: AlsaOutputConfig) -> Result<Self, AlsaOutputError> {
        Err(AlsaOutputError::Disabled)
    }

    /// Returns the target device path string.
    pub fn device(&self) -> &str {
        &self.config.device
    }

    /// Returns the configured ALSA format string.
    pub fn alsa_format(&self) -> &str {
        &self.config.alsa_format
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
            self.write_frames(frames_data)?;

            // Store remaining residue
            self.residue
                .extend_from_slice(&combined[whole_frames_len..]);
            return Ok(buf.len());
        }

        let whole_frames_len = (buf.len() / self.frame_bytes) * self.frame_bytes;
        if whole_frames_len == 0 {
            // Under 1 frame, buffer in residue
            self.residue.extend_from_slice(buf);
            return Ok(buf.len());
        }

        let frames_data = &buf[..whole_frames_len];
        self.write_frames(frames_data)?;

        // Store any trailing bytes in residue
        if whole_frames_len < buf.len() {
            self.residue.extend_from_slice(&buf[whole_frames_len..]);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use audigma_dsp::config::{SampleFormat, TargetRateSpec};
    use audigma_dsp::rates::{DsdBitRateHz, PcmRateHz};
    use audigma_settings::{OutputBackend, OutputConfig, OutputTransport, Settings};

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
            backend: OutputBackend::Alsa,
            target_node: None,
            device: Some("hw:CARD=K17,DEV=0".to_string()),
            transport: Some(OutputTransport::Native),
            container: None,
            sample_format: None,
            volume_control: None,
            device_wait_ms: None,
            keep_audio_sink_ms: None,
            reserve_device: Some(true),
            buffer_ms: Some(50),
            period_ms: Some(10),
        });

        let target_spec = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD256,
        };

        let cfg = AlsaOutputConfig::from_settings(&settings, &target_spec).unwrap();
        assert_eq!(cfg.device, "hw:CARD=K17,DEV=0");
        assert_eq!(cfg.channels, 2);
        // DSD256 at 11,289,600 Hz / 32 = 352,800 frames/s
        assert_eq!(cfg.frame_rate_hz, 352_800);
        assert_eq!(cfg.alsa_format, "DSD_U32_BE");
        assert_eq!(cfg.buffer_ms, 50);
        assert_eq!(cfg.period_ms, 10);
    }

    #[test]
    fn test_alsa_output_config_dop() {
        let mut settings = Settings::default_offline(audigma_settings::OutputContainer::Raw);
        settings.output = Some(OutputConfig {
            backend: OutputBackend::Alsa,
            target_node: None,
            device: Some("hw:CARD=JA11,DEV=0".to_string()),
            transport: Some(OutputTransport::Dop),
            container: None,
            sample_format: Some(SampleFormat::S24_3Le),
            volume_control: None,
            device_wait_ms: None,
            keep_audio_sink_ms: None,
            reserve_device: Some(true),
            buffer_ms: Some(40),
            period_ms: Some(5),
        });

        let target_spec = TargetRateSpec::Dsd {
            bit_rate: DsdBitRateHz::DSD128,
        };

        let cfg = AlsaOutputConfig::from_settings(&settings, &target_spec).unwrap();
        assert_eq!(cfg.device, "hw:CARD=JA11,DEV=0");
        assert_eq!(cfg.channels, 2);
        // DSD128 at 5,644,800 Hz / 16 = 352,800 frames/s
        assert_eq!(cfg.frame_rate_hz, 352_800);
        assert_eq!(cfg.alsa_format, "S24_3LE");
    }

    #[test]
    fn test_alsa_output_config_high_rate_pcm() {
        let mut settings = Settings::default_offline(audigma_settings::OutputContainer::Raw);
        settings.output = Some(OutputConfig {
            backend: OutputBackend::Alsa,
            target_node: None,
            device: Some("hw:0,0".to_string()),
            transport: Some(OutputTransport::Pcm),
            container: None,
            sample_format: Some(SampleFormat::S32Le),
            volume_control: None,
            device_wait_ms: None,
            keep_audio_sink_ms: None,
            reserve_device: None,
            buffer_ms: None,
            period_ms: None,
        });

        let target_spec = TargetRateSpec::Pcm {
            rate: PcmRateHz::PCM_768000,
            sample_format: SampleFormat::S32Le,
            valid_bits: Some(24),
        };

        let cfg = AlsaOutputConfig::from_settings(&settings, &target_spec).unwrap();
        assert_eq!(cfg.device, "hw:0,0");
        assert_eq!(cfg.channels, 2);
        assert_eq!(cfg.frame_rate_hz, 768_000);
        assert_eq!(cfg.alsa_format, "S32_LE");
        assert_eq!(cfg.buffer_ms, 40);
        assert_eq!(cfg.period_ms, 5);
    }
}
