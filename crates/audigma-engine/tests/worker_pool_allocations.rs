//! Separate test binary so allocation accounting cannot include concurrent tests.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use audigma_dsp::{
    Apodization, DitherType, DsdBitRateHz, FilterPhase, FilterPreset, OverloadPolicy,
};
use audigma_engine::worker::{ChannelWorkerPool, TargetDsdConfig};

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
}
