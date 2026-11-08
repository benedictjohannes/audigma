//! Multi-channel power-of-two half-band FIR decimation cascade with partial-chunk residue tracking.

use audigma_dsp::fir::HalfBandDecimationCascade;

use crate::traits::EngineError;

/// Multi-channel power-of-two half-band FIR decimation cascade.
///
/// Converts high-rate baseband PCM down to target intermediate baseband PCM (e.g. 176.4k -> 44.1k,
/// 192k -> 48k, 96k -> 48k) using staged 2x half-band FIR filters.
/// Tracks partial input frames across read calls so arbitrary input chunk sizes never fail alignment.
#[derive(Debug, Clone)]
pub struct ChannelDecimatorPool {
    decimators: Vec<HalfBandDecimationCascade>,
    stages: usize,
    divider: usize,
    residue: Vec<Vec<f64>>,
    scratch: Vec<Vec<f64>>,
    max_input_frames: usize,
}

impl ChannelDecimatorPool {
    /// Creates a new multi-channel decimation cascade pool.
    ///
    /// - `stages`: Number of 2x half-band decimation stages ($2^{\text{stages}}$ divider).
    /// - `channels`: Channel count (typically 2 for stereo).
    /// - `max_input_frames`: Maximum input frames processed per channel in a single call.
    pub fn new(stages: usize, channels: usize, max_input_frames: usize) -> Self {
        let divider = 1usize << stages;
        let scratch_capacity = max_input_frames.max(16384);
        let decimators = (0..channels)
            .map(|_| HalfBandDecimationCascade::new(stages, scratch_capacity))
            .collect();
        let residue = (0..channels).map(|_| Vec::with_capacity(divider)).collect();
        let scratch = (0..channels).map(|_| vec![0.0; scratch_capacity]).collect();

        Self {
            decimators,
            stages,
            divider,
            residue,
            scratch,
            max_input_frames: scratch_capacity,
        }
    }

    /// Number of 2x decimation stages.
    #[inline]
    pub const fn stages(&self) -> usize {
        self.stages
    }

    /// Total decimation rate divider ($2^{\text{stages}}$).
    #[inline]
    pub const fn divider(&self) -> usize {
        self.divider
    }

    /// Resets all internal delay lines and clears partial-chunk residue.
    pub fn reset(&mut self) {
        for dec in &mut self.decimators {
            dec.reset();
        }
        for res in &mut self.residue {
            res.clear();
        }
    }

    /// Decimates multi-channel input frames into output slices.
    ///
    /// Returns the number of decimated frames produced per channel.
    pub fn process(
        &mut self,
        inputs: &[&[f64]],
        outputs: &mut [&mut [f64]],
    ) -> Result<usize, EngineError> {
        let num_channels = inputs.len().min(outputs.len()).min(self.decimators.len());
        if num_channels == 0 {
            return Ok(0);
        }

        let divider = self.divider;
        let mut min_produced = usize::MAX;

        for c in 0..num_channels {
            let in_slice = inputs[c];
            let out_slice = &mut outputs[c];
            let res_len = self.residue[c].len();
            let total_available = res_len + in_slice.len();
            let aligned_len = (total_available / divider) * divider;
            let frames_to_produce = aligned_len / divider;
            let leftover = total_available - aligned_len;

            if frames_to_produce > out_slice.len() {
                return Err(EngineError::Fir(
                    audigma_dsp::fir::FirError::OutputBufferTooSmall {
                        required: frames_to_produce,
                        provided: out_slice.len(),
                    },
                ));
            }

            if aligned_len == 0 {
                // Not enough samples to produce a decimated frame; accumulate into residue
                self.residue[c].extend_from_slice(in_slice);
                min_produced = 0;
                continue;
            }

            if res_len == 0 && in_slice.len() == aligned_len {
                // Fast path: input is already aligned and there is no prior residue
                let produced = self.decimators[c]
                    .process_tile(in_slice, &mut out_slice[..frames_to_produce])?;
                min_produced = min_produced.min(produced);
            } else {
                // Unaligned or buffered path: combine residue and input into scratch
                if aligned_len > self.max_input_frames {
                    return Err(EngineError::Fir(
                        audigma_dsp::fir::FirError::TileSizeExceeded {
                            provided: aligned_len,
                            max: self.max_input_frames,
                        },
                    ));
                }

                // Copy residue first
                if res_len > 0 {
                    self.scratch[c][..res_len].copy_from_slice(&self.residue[c]);
                }
                // Copy needed input samples
                let needed_from_in = aligned_len - res_len;
                self.scratch[c][res_len..aligned_len].copy_from_slice(&in_slice[..needed_from_in]);

                // Update residue with remaining input samples
                self.residue[c].clear();
                if leftover > 0 {
                    self.residue[c].extend_from_slice(&in_slice[needed_from_in..]);
                }

                // Process through the FIR cascade
                let produced = self.decimators[c].process_tile(
                    &self.scratch[c][..aligned_len],
                    &mut out_slice[..frames_to_produce],
                )?;
                min_produced = min_produced.min(produced);
            }
        }

        Ok(if min_produced == usize::MAX {
            0
        } else {
            min_produced
        })
    }

    /// Flushes any unaligned trailing residue by zero-padding to a full decimation block (EOF).
    pub fn flush(&mut self, outputs: &mut [&mut [f64]]) -> Result<usize, EngineError> {
        let num_channels = outputs.len().min(self.decimators.len());
        if num_channels == 0 || self.residue.iter().all(|r| r.is_empty()) {
            return Ok(0);
        }

        let divider = self.divider;
        let mut min_produced = usize::MAX;

        for (c, output) in outputs.iter_mut().enumerate().take(num_channels) {
            let res_len = self.residue[c].len();
            if res_len == 0 {
                min_produced = 0;
                continue;
            }

            let pad = divider - res_len;
            let total_len = divider; // 1 baseband frame
            if output.is_empty() {
                return Err(EngineError::Fir(
                    audigma_dsp::fir::FirError::OutputBufferTooSmall {
                        required: 1,
                        provided: 0,
                    },
                ));
            }

            self.scratch[c][..res_len].copy_from_slice(&self.residue[c]);
            self.scratch[c][res_len..total_len].fill(0.0);
            self.residue[c].clear();

            let produced =
                self.decimators[c].process_tile(&self.scratch[c][..total_len], &mut output[..1])?;
            min_produced = min_produced.min(produced);
            let _ = pad;
        }

        Ok(if min_produced == usize::MAX {
            0
        } else {
            min_produced
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_decimator_exact_aligned() {
        // 4x decimation (stages = 2)
        let mut pool = ChannelDecimatorPool::new(2, 2, 4096);
        assert_eq!(pool.stages(), 2);
        assert_eq!(pool.divider(), 4);

        let input_l = vec![1.0; 256];
        let input_r = vec![0.5; 256];
        let mut out_l = vec![0.0; 64];
        let mut out_r = vec![0.0; 64];

        let mut out_slices = [&mut out_l[..], &mut out_r[..]];
        let produced = pool
            .process(&[&input_l, &input_r], &mut out_slices)
            .unwrap();

        assert_eq!(produced, 64);
    }

    #[test]
    fn test_channel_decimator_unaligned_residue_accumulation() {
        // 4x decimation (divider = 4)
        let mut pool = ChannelDecimatorPool::new(2, 1, 4096);

        let mut out = vec![0.0; 100];

        // Feed 3 samples -> 0 produced, 3 in residue
        let n1 = pool
            .process(&[&[1.0, 1.0, 1.0]], &mut [&mut out[..]])
            .unwrap();
        assert_eq!(n1, 0);

        // Feed 5 samples -> 3 + 5 = 8 samples -> 2 produced, 0 in residue
        let n2 = pool
            .process(&[&[1.0, 1.0, 1.0, 1.0, 1.0]], &mut [&mut out[..]])
            .unwrap();
        assert_eq!(n2, 2);

        // Feed 6 samples -> 0 + 6 = 6 samples -> 1 produced, 2 in residue
        let n3 = pool.process(&[&[1.0; 6]], &mut [&mut out[..]]).unwrap();
        assert_eq!(n3, 1);

        // Flush residue (2 samples padded with 2 zeros to 4) -> 1 produced
        let n_flush = pool.flush(&mut [&mut out[..]]).unwrap();
        assert_eq!(n_flush, 1);
    }
}
