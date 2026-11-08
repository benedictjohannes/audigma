//! End-to-end channel DSP equivalence against the former serial input stages.
//! These are correctness tests, not throughput/deadline benchmarks.

use audigma_dsp::fir::{HalfBandCascade, HalfBandDecimationCascade, PolyphaseResampler};
use audigma_dsp::modulator::Modulator;
use audigma_dsp::{Apodization, DitherType, DsdBitRateHz, FilterPhase, OverloadPolicy, PcmRateHz};
use audigma_engine::decimator::ChannelDecimatorPool;
use audigma_engine::lifecycle::FrameGain;
use audigma_engine::pipeline_topology::PipelineTopology;
use audigma_engine::processor::TileProcessor;
use audigma_engine::traits::DsdDecodeConfig;
use audigma_engine::worker::{
    ChannelWorkerPool, InputDspConfig, InputResampling, InputTileControls, TargetDsdConfig,
};
use audigma_engine::{ThreadInitHook, WorkerSetup};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const FRAMES: usize = 256;
const SEED: u64 = 424242;

struct SerialChannel {
    dsd_decoder: Option<HalfBandDecimationCascade>,
    dsd_alignment: usize,
    dsd_residue: Vec<u8>,
    pcm_decimator: Option<ChannelDecimatorPool>,
    resampler: Option<PolyphaseResampler>,
    cascade: HalfBandCascade,
    modulator: Option<Modulator>,
    input_capacity: usize,
    output_capacity: usize,
}

impl SerialChannel {
    // Mirror the independently prepared serial chain's DSP configuration.
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: InputDspConfig,
        phase: FilterPhase,
        apodization: Apodization,
        stages: usize,
        target: Option<&TargetDsdConfig>,
        channel: usize,
        pool: &ChannelWorkerPool,
    ) -> Self {
        let input_capacity = pool.input_frame_capacity();
        let output_capacity = pool.output_frame_capacity();
        let dsd_decoder = config.dsd.map(|dsd| {
            HalfBandDecimationCascade::new(
                dsd.decimation_stages,
                pool.input_dsd_byte_capacity() * 8,
            )
            .with_gain_db(dsd.gain_db)
        });
        let dsd_alignment = config
            .dsd
            .map_or(1, |dsd| (1usize << dsd.decimation_stages).div_ceil(8));
        let pcm_decimator = (config.pcm_decimation_stages > 0)
            .then(|| ChannelDecimatorPool::new(config.pcm_decimation_stages, 1, input_capacity));
        let resampler = match config.resampling {
            InputResampling::None => None,
            InputResampling::Rational {
                input_rate,
                output_rate,
            } => Some(PolyphaseResampler::new_rational(input_rate, output_rate).unwrap()),
            InputResampling::Adaptive {
                input_rate,
                output_rate,
                ..
            } => Some(PolyphaseResampler::new_asrc(input_rate, output_rate)),
        };
        let modulator = target.map(|target| {
            let mut modulator = Modulator::new(
                target.modulator_order,
                target.bit_rate,
                target.dither,
                target.overload_policy,
            )
            .unwrap();
            modulator.set_dither_seed(target.seed_base.wrapping_add(channel as u64));
            modulator
        });
        Self {
            dsd_decoder,
            dsd_alignment,
            dsd_residue: Vec::new(),
            pcm_decimator,
            resampler,
            cascade: HalfBandCascade::new(phase, apodization, stages, output_capacity),
            modulator,
            input_capacity,
            output_capacity,
        }
    }

    fn pcm(
        &mut self,
        input: &[f64],
        controls: InputTileControls<'_>,
    ) -> (Vec<f64>, Vec<u8>, usize) {
        self.finish(input.to_vec(), controls)
    }

    fn dsd(&mut self, input: &[u8], controls: InputTileControls<'_>) -> (Vec<f64>, Vec<u8>, usize) {
        self.dsd_residue.extend_from_slice(input);
        let aligned = self.dsd_residue.len() / self.dsd_alignment * self.dsd_alignment;
        let mut pcm = vec![0.0; self.input_capacity];
        let frames = self
            .dsd_decoder
            .as_mut()
            .unwrap()
            .process_dsd_bytes(&self.dsd_residue[..aligned], &mut pcm)
            .unwrap();
        self.dsd_residue.drain(..aligned);
        pcm.truncate(frames);
        self.finish(pcm, controls)
    }

    fn finish(
        &mut self,
        mut baseband: Vec<f64>,
        controls: InputTileControls<'_>,
    ) -> (Vec<f64>, Vec<u8>, usize) {
        if let Some(decimator) = self.pcm_decimator.as_mut() {
            let mut decimated = vec![0.0; FRAMES];
            let frames = decimator
                .process(&[&baseband], &mut [&mut decimated])
                .unwrap();
            decimated.truncate(frames);
            baseband = decimated;
        }
        assert_eq!(baseband.len(), controls.frame_gains.len());
        for (sample, gain) in baseband.iter_mut().zip(controls.frame_gains) {
            *sample = gain.apply(*sample);
            *sample *= controls.linear_gain;
        }
        if !baseband.is_empty() {
            if let Some(resampler) = self.resampler.as_mut() {
                resampler.set_drift_ppm(controls.drift_ppm);
                let mut resampled = vec![0.0; self.output_capacity];
                let (consumed, produced) = resampler.process(&baseband, &mut resampled);
                assert_eq!(consumed, baseband.len());
                resampled.truncate(produced);
                baseband = resampled;
            }
        }
        let frames = baseband.len();
        let mut pcm = vec![0.0; frames * self.cascade.rate_multiplier()];
        self.cascade.process_tile(&baseband, &mut pcm).unwrap();
        let mut dsd = Vec::new();
        if let Some(modulator) = self.modulator.as_mut() {
            dsd.resize(pcm.len() / 8, 0);
            modulator.process_tile_bytes(&pcm, &mut dsd).unwrap();
        }
        (pcm, dsd, frames)
    }

    fn reset_output(&mut self) {
        self.cascade.reset();
        if let Some(modulator) = self.modulator.as_mut() {
            modulator.reset();
        }
    }

    fn reset(&mut self) {
        self.reset_output();
        self.dsd_residue.clear();
        if let Some(decoder) = self.dsd_decoder.as_mut() {
            decoder.reset();
        }
        if let Some(decimator) = self.pcm_decimator.as_mut() {
            decimator.reset();
        }
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }
}

fn frame_operations(frames: usize) -> Vec<FrameGain> {
    (0..frames)
        .map(|i| match i % 4 {
            0 => FrameGain::Preserve,
            1 => FrameGain::Multiply(0.75),
            2 => FrameGain::Silence,
            _ => FrameGain::Multiply(0.5),
        })
        .collect()
}

fn compare(
    pool: &ChannelWorkerPool,
    channel: usize,
    expected: (Vec<f64>, Vec<u8>, usize),
    frames: usize,
    dsd: bool,
) {
    assert_eq!(expected.2, frames, "channel {channel} frame count");
    assert_eq!(pool.channel_pcm(channel).len(), expected.0.len());
    for (actual, expected) in pool.channel_pcm(channel).iter().zip(expected.0) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "channel {channel} PCM mismatch"
        );
    }
    if dsd {
        assert_eq!(pool.channel_dsd(channel), expected.1);
    }
}

#[test]
fn pcm_input_workers_match_serial_decimation_gain_and_src() {
    for decimation in [0, 2] {
        let rate_pairs = [
            (PcmRateHz::PCM_44100, PcmRateHz::PCM_48000),
            (PcmRateHz::PCM_48000, PcmRateHz::PCM_44100),
            (PcmRateHz::PCM_176400, PcmRateHz::PCM_48000),
            (PcmRateHz::PCM_48000, PcmRateHz::PCM_176400),
        ];
        let src_modes = rate_pairs
            .into_iter()
            .flat_map(|(input_rate, output_rate)| {
                [
                    InputResampling::Rational {
                        input_rate,
                        output_rate,
                    },
                    InputResampling::Adaptive {
                        input_rate,
                        output_rate,
                        max_correction_ppm: 500.0,
                    },
                ]
            });
        for resampling in std::iter::once(InputResampling::None).chain(src_modes) {
            for phase in [FilterPhase::Linear, FilterPhase::Minimum] {
                let config = InputDspConfig {
                    pcm_decimation_stages: decimation,
                    dsd: None,
                    resampling,
                };
                let apodization = Apodization::PassEdge19000;
                let mut pool = ChannelWorkerPool::new_with_input(
                    2,
                    phase,
                    apodization,
                    2,
                    FRAMES,
                    None,
                    Some(config),
                    None,
                )
                .unwrap();
                let mut serial: Vec<_> = (0..2)
                    .map(|ch| SerialChannel::new(config, phase, apodization, 2, None, ch, &pool))
                    .collect();
                let left: Vec<_> = (0..pool.input_frame_capacity())
                    .map(|i| (i as f64 * 0.17).sin() * 0.2)
                    .collect();
                let right: Vec<_> = (0..pool.input_frame_capacity())
                    .map(|i| (i as f64 * 0.23).cos() * 0.1)
                    .collect();
                for _ in 0..2 {
                    for (block, input_frames) in
                        [0, 1, 3, 7, 31, 127, 255, pool.input_frame_capacity()]
                            .into_iter()
                            .enumerate()
                    {
                        let gains =
                            frame_operations(pool.pcm_baseband_frames(input_frames).unwrap());
                        let controls = InputTileControls {
                            frame_gains: &gains,
                            linear_gain: 0.5,
                            drift_ppm: [-500.0, 0.0, 500.0][block % 3],
                        };
                        let frames = pool
                            .process_input_pcm(
                                &[&left[..input_frames], &right[..input_frames]],
                                controls,
                            )
                            .unwrap();
                        compare(
                            &pool,
                            0,
                            serial[0].pcm(&left[..input_frames], controls),
                            frames,
                            false,
                        );
                        compare(
                            &pool,
                            1,
                            serial[1].pcm(&right[..input_frames], controls),
                            frames,
                            false,
                        );
                        if block == 1 {
                            // A PCM residue can be outstanding across idle reprime.
                            pool.reset_output().unwrap();
                            for channel in &mut serial {
                                channel.reset_output();
                            }
                        }
                    }
                    pool.reset().unwrap();
                    for channel in &mut serial {
                        channel.reset();
                    }
                }
            }
        }
    }
}

#[test]
fn dsd64_to_dsd512_workers_match_serial_pipeline_bit_exactly() {
    for apodization in [Apodization::Off, Apodization::PassEdge18500] {
        for order in [5, 7] {
            for (phase, rate, pcm_rate) in [
                (
                    FilterPhase::Linear,
                    DsdBitRateHz::DSD512,
                    PcmRateHz::PCM_176400,
                ),
                (
                    FilterPhase::Minimum,
                    DsdBitRateHz::DSD512_48,
                    PcmRateHz::PCM_192000,
                ),
            ] {
                let config = InputDspConfig {
                    pcm_decimation_stages: 0,
                    dsd: Some(DsdDecodeConfig {
                        decimation_stages: 4,
                        gain_db: -3.0,
                    }),
                    resampling: InputResampling::Adaptive {
                        input_rate: pcm_rate,
                        output_rate: pcm_rate,
                        max_correction_ppm: 500.0,
                    },
                };
                let target = TargetDsdConfig {
                    bit_rate: rate,
                    modulator_order: order,
                    dither: DitherType::Tpdf,
                    overload_policy: OverloadPolicy::MuteRecover,
                    seed_base: SEED,
                };
                let mut pool = ChannelWorkerPool::new_with_input(
                    2,
                    phase,
                    apodization,
                    7,
                    FRAMES,
                    Some(target.clone()),
                    Some(config),
                    None,
                )
                .unwrap();
                let mut serial: Vec<_> = (0..2)
                    .map(|ch| {
                        SerialChannel::new(config, phase, apodization, 7, Some(&target), ch, &pool)
                    })
                    .collect();
                let left: Vec<_> = (0..512).map(|i| [0x69, 0x96, 0x5a][i % 3]).collect();
                let right: Vec<_> = (0..512).map(|i| [0x96, 0x5a, 0x69][i % 3]).collect();
                for _ in 0..2 {
                    for (block, bytes) in [0, 1, 3, 31, 255, 511, 512].into_iter().enumerate() {
                        let gains = frame_operations(pool.dsd_baseband_frames(bytes).unwrap());
                        let controls = InputTileControls {
                            frame_gains: &gains,
                            linear_gain: 0.5,
                            drift_ppm: [-500.0, 0.0, 500.0][block % 3],
                        };
                        let frames = pool
                            .process_input_dsd(&[&left[..bytes], &right[..bytes]], controls)
                            .unwrap();
                        compare(
                            &pool,
                            0,
                            serial[0].dsd(&left[..bytes], controls),
                            frames,
                            true,
                        );
                        compare(
                            &pool,
                            1,
                            serial[1].dsd(&right[..bytes], controls),
                            frames,
                            true,
                        );
                        for (ch, channel) in serial.iter().enumerate() {
                            assert_eq!(
                                pool.channel_overload_count(ch),
                                channel.modulator.as_ref().unwrap().overload_count()
                            );
                        }
                        if block == 1 {
                            // A raw DSD byte is outstanding across idle reprime.
                            pool.reset_output().unwrap();
                            for channel in &mut serial {
                                channel.reset_output();
                            }
                        }
                    }
                    pool.reset().unwrap();
                    for channel in &mut serial {
                        channel.reset();
                    }
                }
            }
        }
    }
}

#[test]
fn raw_dsd_pcm_downsampling_and_synthetic_drains_preserve_both_residues() {
    let config = InputDspConfig {
        pcm_decimation_stages: 2,
        dsd: Some(DsdDecodeConfig {
            decimation_stages: 4,
            gain_db: 3.0,
        }),
        resampling: InputResampling::Adaptive {
            input_rate: PcmRateHz::PCM_44100,
            output_rate: PcmRateHz::PCM_44100,
            max_correction_ppm: 500.0,
        },
    };
    let mut pool = ChannelWorkerPool::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        0,
        FRAMES,
        None,
        Some(config),
        None,
    )
    .unwrap();
    let mut serial: Vec<_> = (0..2)
        .map(|ch| {
            SerialChannel::new(
                config,
                FilterPhase::Linear,
                Apodization::Off,
                0,
                None,
                ch,
                &pool,
            )
        })
        .collect();
    assert_eq!(pool.input_dsd_byte_capacity(), 2048);
    let input = vec![0x69; 2048];
    for bytes in [1, 3, 5, 7, 511, 2048, 1] {
        let gains = frame_operations(pool.dsd_baseband_frames(bytes).unwrap());
        let controls = InputTileControls {
            frame_gains: &gains,
            linear_gain: 0.75,
            drift_ppm: 500.0,
        };
        let frames = pool
            .process_input_dsd(&[&input[..bytes], &input[..bytes]], controls)
            .unwrap();
        for (ch, channel) in serial.iter_mut().enumerate() {
            compare(
                &pool,
                ch,
                channel.dsd(&input[..bytes], controls),
                frames,
                false,
            );
        }

        let zeros = [0.0; FRAMES];
        let gains = frame_operations(pool.pcm_baseband_frames(FRAMES).unwrap());
        let controls = InputTileControls {
            frame_gains: &gains,
            linear_gain: 0.75,
            drift_ppm: 500.0,
        };
        let frames = pool.process_input_pcm(&[&zeros, &zeros], controls).unwrap();
        for (ch, channel) in serial.iter_mut().enumerate() {
            compare(&pool, ch, channel.pcm(&zeros, controls), frames, false);
        }
    }
}

#[test]
fn input_job_validation_does_not_consume_residue_or_change_phase() {
    let config = InputDspConfig {
        pcm_decimation_stages: 0,
        dsd: Some(DsdDecodeConfig {
            decimation_stages: 4,
            gain_db: 0.0,
        }),
        resampling: InputResampling::Adaptive {
            input_rate: PcmRateHz::PCM_176400,
            output_rate: PcmRateHz::PCM_176400,
            max_correction_ppm: 500.0,
        },
    };
    let mut pool = ChannelWorkerPool::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        0,
        FRAMES,
        None,
        Some(config),
        None,
    )
    .unwrap();
    let controls = InputTileControls {
        frame_gains: &[],
        linear_gain: 1.0,
        drift_ppm: 0.0,
    };
    assert!(pool.process_input_dsd(&[&[0x69]], controls).is_err());
    assert!(pool.process_input_dsd(&[&[0x69], &[]], controls).is_err());
    assert!(
        pool.process_input_dsd(
            &[&[0x69], &[0x69]],
            InputTileControls {
                frame_gains: &[FrameGain::Preserve],
                ..controls
            }
        )
        .is_err()
    );
    assert!(
        pool.process_input_dsd(
            &[&[0x69], &[0x69]],
            InputTileControls {
                drift_ppm: 501.0,
                ..controls
            }
        )
        .is_err()
    );
    assert_eq!(pool.dsd_baseband_frames(1).unwrap(), 0);
    assert_eq!(
        pool.process_input_dsd(&[&[0x69], &[0x69]], controls)
            .unwrap(),
        0
    );
    assert_eq!(pool.dsd_baseband_frames(1).unwrap(), 1);
    pool.reset().unwrap();
    assert_eq!(pool.dsd_baseband_frames(1).unwrap(), 0);
}

#[test]
fn src_downsampling_keeps_output_only_api_capacity() {
    let config = InputDspConfig {
        pcm_decimation_stages: 0,
        dsd: None,
        resampling: InputResampling::Rational {
            input_rate: PcmRateHz::PCM_176400,
            output_rate: PcmRateHz::PCM_48000,
        },
    };
    let mut pool = ChannelWorkerPool::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        2,
        FRAMES,
        None,
        Some(config),
        None,
    )
    .unwrap();
    assert!(pool.output_frame_capacity() < FRAMES);
    let input: Vec<_> = (0..FRAMES).map(|i| (i as f64 * 0.17).sin() * 0.2).collect();
    let mut reference = HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, 2, FRAMES);
    let mut expected = vec![0.0; FRAMES * 4];
    reference.process_tile(&input, &mut expected).unwrap();
    pool.process_tile_pcm(&[&input, &input]).unwrap();
    for channel in 0..2 {
        compare(
            &pool,
            channel,
            (expected.clone(), Vec::new(), FRAMES),
            FRAMES,
            false,
        );
    }
}

#[test]
fn output_only_errors_keep_pcm_view_and_clear_all_channel_errors() {
    let mut pool = ChannelWorkerPool::new(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        9,
        FRAMES,
        Some(TargetDsdConfig {
            bit_rate: DsdBitRateHz::DSD512,
            modulator_order: 5,
            dither: DitherType::Tpdf,
            overload_policy: OverloadPolicy::Error,
            seed_base: SEED,
        }),
        None,
    )
    .unwrap();
    let input = [f64::NAN; 31];
    let mut reference = HalfBandCascade::new(FilterPhase::Linear, Apodization::Off, 9, input.len());
    let mut expected = vec![0.0; input.len() * 512];
    reference.process_tile(&input, &mut expected).unwrap();
    assert!(matches!(
        pool.process_tile_dsd(&[&input, &input]),
        Err(audigma_engine::EngineError::Modulator(_))
    ));
    for channel in 0..2 {
        compare(
            &pool,
            channel,
            (expected.clone(), Vec::new(), input.len()),
            input.len(),
            false,
        );
    }
    pool.reset().unwrap();
    pool.process_tile_dsd(&[&[0.0; 31], &[0.0; 31]]).unwrap();
}

#[test]
fn modulation_errors_still_account_for_consumed_input_residue() {
    let config = InputDspConfig {
        pcm_decimation_stages: 2,
        dsd: Some(DsdDecodeConfig {
            decimation_stages: 4,
            gain_db: 0.0,
        }),
        resampling: InputResampling::None,
    };
    let mut pool = ChannelWorkerPool::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        9,
        FRAMES,
        Some(TargetDsdConfig {
            bit_rate: DsdBitRateHz::DSD512,
            modulator_order: 5,
            dither: DitherType::Tpdf,
            overload_policy: OverloadPolicy::Error,
            seed_base: SEED,
        }),
        Some(config),
        None,
    )
    .unwrap();
    let pcm = [0.1; 1021];
    let gains = [FrameGain::Preserve; FRAMES];
    let frames = pool.pcm_baseband_frames(pcm.len()).unwrap();
    assert!(matches!(
        pool.process_input_pcm(
            &[&pcm, &pcm],
            InputTileControls {
                frame_gains: &gains[..frames],
                linear_gain: f64::INFINITY,
                drift_ppm: 0.0,
            },
        ),
        Err(audigma_engine::EngineError::Modulator(_))
    ));
    assert_eq!(pool.pcm_baseband_frames(3).unwrap(), 1);
    pool.reset_output().unwrap();

    let dsd = [0x69; 513];
    let frames = pool.dsd_baseband_frames(dsd.len()).unwrap();
    assert!(matches!(
        pool.process_input_dsd(
            &[&dsd, &dsd],
            InputTileControls {
                frame_gains: &gains[..frames],
                linear_gain: f64::INFINITY,
                drift_ppm: 0.0,
            },
        ),
        Err(audigma_engine::EngineError::Modulator(_))
    ));
    assert_eq!(pool.dsd_decoded_frames(1).unwrap(), 1);
    assert_eq!(pool.pcm_baseband_frames(3).unwrap(), 1);
    pool.reset().unwrap();
    assert_eq!(pool.dsd_decoded_frames(1).unwrap(), 0);
    assert_eq!(pool.pcm_baseband_frames(3).unwrap(), 0);
}

#[test]
fn early_dispatch_reuses_exactly_one_thread_per_channel() {
    let workers = Arc::new(AtomicUsize::new(0));
    let observed = workers.clone();
    let hook: ThreadInitHook = Arc::new(move |channel, _placement| {
        assert_eq!(
            std::thread::current().name().unwrap(),
            format!("audigma-dsp-{channel}")
        );
        observed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let config = InputDspConfig {
        pcm_decimation_stages: 0,
        dsd: None,
        resampling: InputResampling::None,
    };
    let mut pool = ChannelWorkerPool::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        0,
        FRAMES,
        None,
        Some(config),
        Some(WorkerSetup {
            placements: Vec::new(),
            hook: Some(hook),
            ..Default::default()
        }),
    )
    .unwrap();
    let input = [0.1; FRAMES];
    let gains = [FrameGain::Preserve; FRAMES];
    pool.process_input_pcm(
        &[&input, &input],
        InputTileControls {
            frame_gains: &gains,
            linear_gain: 1.0,
            drift_ppm: 0.0,
        },
    )
    .unwrap();
    assert_eq!(pool.channels(), 2);
    assert_eq!(workers.load(Ordering::Relaxed), 2);
}

#[test]
fn worker_termination_completes_surviving_jobs_and_rejects_reuse() {
    let workers = Arc::new(AtomicUsize::new(0));
    let observed = workers.clone();
    let hook: ThreadInitHook = Arc::new(move |channel, _placement| {
        assert_ne!(channel, 0, "simulate a terminated worker");
        observed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    });
    let mut pool = ChannelWorkerPool::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        2,
        FRAMES,
        None,
        Some(InputDspConfig {
            pcm_decimation_stages: 0,
            dsd: None,
            resampling: InputResampling::None,
        }),
        Some(WorkerSetup {
            placements: Vec::new(),
            hook: Some(hook),
            ..Default::default()
        }),
    )
    .unwrap();
    let input = [0.1; FRAMES];
    let gains = [FrameGain::Preserve; FRAMES];
    let controls = InputTileControls {
        frame_gains: &gains,
        linear_gain: 1.0,
        drift_ppm: 0.0,
    };
    assert!(matches!(
        pool.process_input_pcm(&[&input, &input], controls),
        Err(audigma_engine::EngineError::WorkerThread { channel: 0, .. })
    ));
    assert_eq!(workers.load(Ordering::Relaxed), 1);
    assert!(pool.channel_pcm(0).is_empty());
    assert!(pool.channel_pcm(1).is_empty());
    assert!(matches!(
        pool.process_input_pcm(&[&input, &input], controls),
        Err(audigma_engine::EngineError::WorkerThread { channel: 0, .. })
    ));
    assert!(matches!(
        pool.process_tile_pcm(&[&input, &input]),
        Err(audigma_engine::EngineError::WorkerThread { channel: 0, .. })
    ));
    assert!(matches!(
        pool.reset(),
        Err(audigma_engine::EngineError::WorkerThread { channel: 0, .. })
    ));
}

#[test]
fn input_dsp_timing_counts_one_job_and_uses_pre_src_duration() {
    let target = audigma_dsp::TargetRateSpec::Pcm {
        rate: PcmRateHz::PCM_48000,
        sample_format: audigma_dsp::SampleFormat::F64,
    };
    let topology = PipelineTopology::resolve(PcmRateHz::PCM_44100, target).unwrap();
    let config = InputDspConfig {
        pcm_decimation_stages: 0,
        dsd: None,
        resampling: InputResampling::Rational {
            input_rate: PcmRateHz::PCM_44100,
            output_rate: PcmRateHz::PCM_48000,
        },
    };
    let mut processor = TileProcessor::new_with_input(
        2,
        FilterPhase::Linear,
        Apodization::Off,
        topology,
        FRAMES,
        None,
        true,
        config,
        None,
    )
    .unwrap();
    assert_eq!(
        processor.tile_budget().unwrap(),
        std::time::Duration::from_secs_f64(FRAMES as f64 / 44100.0)
    );
    let input = [0.1; FRAMES];
    let gains = [FrameGain::Preserve; FRAMES];
    let frames = processor
        .process_input_pcm(
            &[&input, &input],
            InputTileControls {
                frame_gains: &gains,
                linear_gain: 1.0,
                drift_ppm: 0.0,
            },
        )
        .unwrap();
    assert!(frames > FRAMES);
    assert_eq!(processor.total_tiles_processed(), 1);
}
