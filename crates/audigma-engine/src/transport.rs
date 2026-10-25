//! Output transport framing, carrier emission, device disconnect translation,
//! and direct bit-exact DSD passthrough (`Design.md §1.6`, `§4.5`).

use std::io::ErrorKind;

use audigma_dsp::config::{DspConfiguration, TargetRateSpec};
use audigma_format::decoder::AudioStreamInfo;
use audigma_format::dop::DopPacker;
use audigma_format::dsd_u32::pack_stereo_dsd_u32_be;

use crate::lifecycle::IdleCarrierFormat;
use crate::traits::{AudioInput, AudioOutput, EngineError};

/// Writes a data buffer to the audio output sink, translating hardware disconnection
/// OS errors to `EngineError::DeviceDisconnected`.
#[inline]
pub fn write_output_all(output: &mut dyn AudioOutput, data: &[u8]) -> Result<(), EngineError> {
    output.write_all(data).map_err(|e| {
        let code = e.raw_os_error().unwrap_or(0);
        if e.kind() == ErrorKind::BrokenPipe
            || code == libc::ENODEV
            || code == -libc::ENODEV
            || code == libc::EBADFD
            || code == -libc::EBADFD
            || code == libc::EIO
            || code == -libc::EIO
        {
            EngineError::DeviceDisconnected(e.to_string())
        } else {
            EngineError::Io(e)
        }
    })
}

/// Packs chronological MSB-first stereo DSD byte slices into the target output transport
/// carrier (`NativeDsd` or `DoP`) and emits them to `output`.
///
/// Returns the number of 1-bit audio frames written.
pub fn pack_and_write_stereo_dsd(
    carrier: IdleCarrierFormat,
    dop_packer: &mut Option<DopPacker>,
    left_dsd: &[u8],
    right_dsd: &[u8],
    transport_staging: &mut [u8],
    output: &mut dyn AudioOutput,
) -> Result<usize, EngineError> {
    let frame_count = left_dsd.len() * 8;
    match carrier {
        IdleCarrierFormat::NativeDsd => {
            let packed_bytes =
                pack_stereo_dsd_u32_be(left_dsd, right_dsd, false, transport_staging)?;
            write_output_all(output, &transport_staging[..packed_bytes])?;
            Ok(frame_count)
        }
        IdleCarrierFormat::DoP(_) => {
            if let Some(packer) = dop_packer {
                let packed_bytes =
                    packer.pack_stereo_frames(left_dsd, right_dsd, transport_staging)?;
                write_output_all(output, &transport_staging[..packed_bytes])?;
                Ok(frame_count)
            } else {
                Ok(0)
            }
        }
        IdleCarrierFormat::Pcm => Ok(0),
    }
}

/// Evaluates whether an input audio stream and target specification qualify for
/// Automatic Opportunistic Bit-Exact DSD Passthrough (`Design.md §1.6`).
///
/// Returns `true` iff:
/// 1. The input stream is native 1-bit DSD.
/// 2. The target format is DSD with an identical bit rate.
/// 3. Software gain attenuation is unity ($0\text{ dBFS}$ / 100%, preamp $= 0\text{ dB}$ and user volume $= 0\text{ dB}$).
#[inline]
pub fn is_dsd_passthrough_eligible(
    input_info: &AudioStreamInfo,
    target_spec: &TargetRateSpec,
    dsp_config: &DspConfiguration,
) -> bool {
    let is_input_dsd = input_info.is_dsd();
    let is_target_matching_dsd = match target_spec {
        TargetRateSpec::Dsd { bit_rate } => input_info.dsd_bit_rate == Some(*bit_rate),
        _ => false,
    };
    is_input_dsd && is_target_matching_dsd && dsp_config.is_volume_unity()
}

/// Transfers bit-exact DSD bytes from `input` to a container writer in 4 KiB chunks,
/// returning the total number of audio frames transferred.
pub fn transfer_offline_dsd_passthrough(
    input: &mut dyn AudioInput,
    mut write_stereo: impl FnMut(&[u8], &[u8]) -> Result<(), EngineError>,
) -> Result<u64, EngineError> {
    let chunk_bytes = 4096;
    let mut dsd_l = vec![0u8; chunk_bytes];
    let mut dsd_r = vec![0u8; chunk_bytes];
    let mut total_input_frames = 0u64;

    loop {
        let bytes_read = input.read_dsd_bytes(&mut [&mut dsd_l, &mut dsd_r])?;
        if bytes_read == 0 {
            break;
        }
        total_input_frames += (bytes_read * 8) as u64;
        write_stereo(&dsd_l[..bytes_read], &dsd_r[..bytes_read])?;
    }
    Ok(total_input_frames)
}

/// Reads one tile of DSD bytes from `input`, packs them into the active carrier transport,
/// and writes them to `output`.
///
/// Returns `Ok(Some(frames_written))` if data was read and emitted, or `Ok(None)` if EOF or empty.
pub fn process_live_dsd_passthrough_tile(
    input: &mut dyn AudioInput,
    output: &mut dyn AudioOutput,
    carrier: IdleCarrierFormat,
    dop_packer: &mut Option<DopPacker>,
    dsd_passthrough_l: &mut [u8],
    dsd_passthrough_r: &mut [u8],
    transport_staging: &mut [u8],
) -> Result<Option<usize>, EngineError> {
    let bytes_read = input.read_dsd_bytes(&mut [dsd_passthrough_l, dsd_passthrough_r])?;
    if bytes_read == 0 {
        return Ok(None);
    }
    let frames_written = pack_and_write_stereo_dsd(
        carrier,
        dop_packer,
        &dsd_passthrough_l[..bytes_read],
        &dsd_passthrough_r[..bytes_read],
        transport_staging,
        output,
    )?;
    Ok(Some(frames_written))
}
