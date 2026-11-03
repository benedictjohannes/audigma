//! Separate test binary so allocation accounting cannot include concurrent tests.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use audigma_dsp::PcmRateHz;
use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, FilterPhase, FilterPreset, OverloadPolicy,
};
use audigma_engine::lifecycle::FrameGain;
use audigma_engine::traits::DsdDecodeConfig;
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};
use audigma_engine::worker::{InputDspConfig, InputResampling, InputTileControls};

struct CountingAllocator;
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

fn record_allocation() {
    if ENABLED.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation();
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn test_dsd_subtiles_allocate_nothing_in_steady_state() {
    for order in [5, 7] {
        let mut pool = ChannelWorkerPool::new(
            2,
            FilterPreset::Balanced,
            FilterPhase::Linear,
            Apodization::Off,
            9,
            512,
            Some(TargetDsdConfig {
                bit_rate: DsdBitRateHz::DSD512,
                modulator_order: order,
                dither: DitherType::Tpdf,
                overload_policy: OverloadPolicy::MuteRecover,
                seed_base: 424242,
            }),
            None,
        )
        .unwrap();
        let input = [0.1; 512];
        let overload = [5.0; 512];
        for _ in 0..5 {
            pool.process_tile_dsd(&[&input, &input]).unwrap();
        }
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        for frames in [0, 1, 31, 32, 33, 256, 511, 512] {
            pool.process_tile_dsd(&[&input[..frames], &input[..frames]])
                .unwrap();
            pool.process_tile_pcm(&[&input[..frames], &input[..frames]])
                .unwrap();
        }
        pool.process_tile_dsd(&[&overload, &overload]).unwrap();
        pool.reset().unwrap();
        ENABLED.store(false, Ordering::Relaxed);
        assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
        assert!(pool.total_overload_count() > 0);
    }
    check_input_dsp_allocations();
}

// Keep all accounting in the same test: concurrent test setup would otherwise
// be counted by the process-wide allocator, despite using separate worker pools.
fn check_input_dsp_allocations() {
    for order in [5, 7] {
        let mut pool = ChannelWorkerPool::new_with_input(
            2,
            FilterPreset::Reference,
            FilterPhase::Linear,
            Apodization::Off,
            9,
            512,
            Some(TargetDsdConfig {
                bit_rate: DsdBitRateHz::DSD512_48,
                modulator_order: order,
                dither: DitherType::Tpdf,
                overload_policy: OverloadPolicy::MuteRecover,
                seed_base: 424242,
            }),
            Some(InputDspConfig {
                pcm_decimation_stages: 2,
                dsd: Some(DsdDecodeConfig {
                    preset: FilterPreset::Reference,
                    decimation_stages: 4,
                    gain_db: -3.0,
                }),
                resampling: InputResampling::Adaptive {
                    input_rate: PcmRateHz::PCM_44100,
                    output_rate: PcmRateHz::PCM_48000,
                    max_correction_ppm: 500.0,
                },
            }),
            None,
        )
        .unwrap();
        let dsd = [0x69; 4096];
        let pcm = [0.1; 2048];
        let overload = [5.0; 2048];
        let gains = [FrameGain::Preserve; 512];
        for _ in 0..5 {
            let frames = pool.dsd_baseband_frames(dsd.len()).unwrap();
            pool.process_input_dsd(
                &[&dsd, &dsd],
                InputTileControls {
                    frame_gains: &gains[..frames],
                    linear_gain: 1.0,
                    drift_ppm: 0.0,
                },
            )
            .unwrap();
        }
        ALLOCATIONS.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        for bytes in [0, 1, 7, 31, 2047, 4096] {
            let frames = pool.dsd_baseband_frames(bytes).unwrap();
            pool.process_input_dsd(
                &[&dsd[..bytes], &dsd[..bytes]],
                InputTileControls {
                    frame_gains: &gains[..frames],
                    linear_gain: 1.0,
                    drift_ppm: -500.0,
                },
            )
            .unwrap();
        }
        for input_frames in [0, 1, 3, 255, 2048] {
            let frames = pool.pcm_baseband_frames(input_frames).unwrap();
            pool.process_input_pcm(
                &[&pcm[..input_frames], &pcm[..input_frames]],
                InputTileControls {
                    frame_gains: &gains[..frames],
                    linear_gain: 1.0,
                    drift_ppm: 500.0,
                },
            )
            .unwrap();
        }
        let frames = pool.pcm_baseband_frames(overload.len()).unwrap();
        pool.process_input_pcm(
            &[&overload, &overload],
            InputTileControls {
                frame_gains: &gains[..frames],
                linear_gain: 1.0,
                drift_ppm: 0.0,
            },
        )
        .unwrap();
        pool.reset_output().unwrap();
        pool.reset().unwrap();
        ENABLED.store(false, Ordering::Relaxed);
        assert_eq!(ALLOCATIONS.load(Ordering::Relaxed), 0);
        assert!(pool.total_overload_count() > 0);
    }
}
