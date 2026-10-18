//! Channel topology conditioning: mono duplication, stereo passthrough,
//! and ITU-R BS.775 stereo downmixing for multi-channel audio inputs.

use std::f64::consts::SQRT_2;

/// Standard attenuation factor for center and surround downmix channels (-3.01 dB = 1 / sqrt(2)).
const CENTER_SURROUND_SCALE: f64 = 1.0 / SQRT_2;

/// Normalization divisor for 5.1 downmixing (1.0 + 2 * (1 / sqrt(2))) to guarantee no clipping.
const NORM_5_1: f64 = 1.0 / (1.0 + 2.0 * CENTER_SURROUND_SCALE);

/// Normalization divisor for 7.1 downmixing (1.0 + 3 * (1 / sqrt(2))).
const NORM_7_1: f64 = 1.0 / (1.0 + 3.0 * CENTER_SURROUND_SCALE);

/// Downmixes or upmixes an interleaved buffer of `f64` PCM audio with `channels` into
/// separate left and right stereo vectors.
///
/// Rules:
/// - 1 channel (mono): Duplicated to both left and right ($L = C, R = C$) with unity gain.
/// - 2 channels (stereo): Direct 1:1 passthrough ($L = L, R = R$).
/// - 3 channels: Center channel blended to both L and R with -3 dB attenuation.
/// - 6 channels (5.1 surround: FL, FR, C, LFE, SL, SR): ITU-R BS.775 downmix with headroom normalization.
/// - 8 channels (7.1 surround: FL, FR, C, LFE, SL, SR, BL, BR): Full 7.1 fold-down with normalization.
/// - $N$ channels (arbitrary): Slices mapped into left and right, averaged.
pub fn downmix_interleaved_to_stereo(
    channels: usize,
    interleaved: &[f64],
    out_left: &mut Vec<f64>,
    out_right: &mut Vec<f64>,
) {
    if channels == 0 || interleaved.is_empty() {
        return;
    }

    let frames = interleaved.len() / channels;
    out_left.reserve(frames);
    out_right.reserve(frames);

    match channels {
        1 => {
            // Mono: duplicate to L and R
            for &sample in &interleaved[..frames] {
                out_left.push(sample);
                out_right.push(sample);
            }
        }
        2 => {
            // Stereo passthrough
            for chunk in interleaved[..frames * 2].chunks_exact(2) {
                out_left.push(chunk[0]);
                out_right.push(chunk[1]);
            }
        }
        3 => {
            // 3 channels: FL, FR, C
            let norm = 1.0 / (1.0 + CENTER_SURROUND_SCALE);
            for chunk in interleaved[..frames * 3].chunks_exact(3) {
                let fl = chunk[0];
                let fr = chunk[1];
                let c = chunk[2];
                out_left.push((fl + CENTER_SURROUND_SCALE * c) * norm);
                out_right.push((fr + CENTER_SURROUND_SCALE * c) * norm);
            }
        }
        6 => {
            // 5.1 Surround: [0: FL, 1: FR, 2: C, 3: LFE, 4: SL, 5: SR]
            for chunk in interleaved[..frames * 6].chunks_exact(6) {
                let fl = chunk[0];
                let fr = chunk[1];
                let c = chunk[2];
                // LFE chunk[3] is intentionally excluded from stereo acoustic fold-down per BS.775
                let sl = chunk[4];
                let sr = chunk[5];

                let l = (fl + CENTER_SURROUND_SCALE * c + CENTER_SURROUND_SCALE * sl) * NORM_5_1;
                let r = (fr + CENTER_SURROUND_SCALE * c + CENTER_SURROUND_SCALE * sr) * NORM_5_1;

                out_left.push(l);
                out_right.push(r);
            }
        }
        8 => {
            // 7.1 Surround: [0: FL, 1: FR, 2: C, 3: LFE, 4: SL, 5: SR, 6: BL, 7: BR]
            for chunk in interleaved[..frames * 8].chunks_exact(8) {
                let fl = chunk[0];
                let fr = chunk[1];
                let c = chunk[2];
                let sl = chunk[4];
                let sr = chunk[5];
                let bl = chunk[6];
                let br = chunk[7];

                let l = (fl
                    + CENTER_SURROUND_SCALE * c
                    + CENTER_SURROUND_SCALE * sl
                    + CENTER_SURROUND_SCALE * bl)
                    * NORM_7_1;
                let r = (fr
                    + CENTER_SURROUND_SCALE * c
                    + CENTER_SURROUND_SCALE * sr
                    + CENTER_SURROUND_SCALE * br)
                    * NORM_7_1;

                out_left.push(l);
                out_right.push(r);
            }
        }
        _ => {
            // Fallback for arbitrary N channels: distribute even indices to left, odd to right
            let left_count = channels.div_ceil(2);
            let right_count = channels / 2;
            let norm_l = 1.0 / (left_count as f64);
            let norm_r = 1.0 / (right_count as f64).max(1.0);

            for chunk in interleaved[..frames * channels].chunks_exact(channels) {
                let mut sum_l = 0.0;
                let mut sum_r = 0.0;
                for (idx, &s) in chunk.iter().enumerate() {
                    if idx % 2 == 0 {
                        sum_l += s;
                    } else {
                        sum_r += s;
                    }
                }
                out_left.push(sum_l * norm_l);
                out_right.push(sum_r * norm_r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mono_duplication() {
        let mono = vec![0.25, -0.5, 0.75];
        let mut left = Vec::new();
        let mut right = Vec::new();
        downmix_interleaved_to_stereo(1, &mono, &mut left, &mut right);

        assert_eq!(left, mono);
        assert_eq!(right, mono);
    }

    #[test]
    fn test_stereo_passthrough() {
        let stereo = vec![0.1, 0.2, -0.3, -0.4];
        let mut left = Vec::new();
        let mut right = Vec::new();
        downmix_interleaved_to_stereo(2, &stereo, &mut left, &mut right);

        assert_eq!(left, vec![0.1, -0.3]);
        assert_eq!(right, vec![0.2, -0.4]);
    }

    #[test]
    fn test_surround_5_1_downmix_bounds() {
        // Full scale on all 6 channels should normalize within [-1.0, 1.0]
        let full_scale = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let mut left = Vec::new();
        let mut right = Vec::new();
        downmix_interleaved_to_stereo(6, &full_scale, &mut left, &mut right);

        assert_eq!(left.len(), 1);
        assert_eq!(right.len(), 1);
        assert!((left[0] - 1.0).abs() < 1e-12);
        assert!((right[0] - 1.0).abs() < 1e-12);
    }
}
