//! Pull-driven capture: the virtual sink drives its own graph and triggers a
//! cycle only when downstream consumption has drained the input cushion.
//!
//! Units are PCM frames for PCM and bytes per channel for native DSD.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct PullClock {
    next: Option<Instant>,
}

impl PullClock {
    pub fn ready(&self, now: Instant) -> bool {
        self.next.is_none_or(|next| now >= next)
    }

    pub fn reset(&mut self) {
        self.next = None;
    }

    pub fn pulled(&mut self, now: Instant, units: u64, units_per_sec: u32) {
        // Small refill headroom lets the demand gate follow hardware-clock drift
        // and recover scheduling delays without issuing back-to-back pulls.
        let period =
            Duration::from_secs_f64(0.95 * units.max(1) as f64 / units_per_sec.max(1) as f64);
        let next = self.next.unwrap_or(now);
        // Preserve fractional timing without collecting burst credit during a
        // downstream stall. Async sources need time to prepare the next buffer.
        self.next = Some(if now.saturating_duration_since(next) >= period {
            now + period
        } else {
            next + period
        });
    }
}

/// The Rust stream builder does not expose the native trigger-done callback.
pub(super) struct TriggerDone<'a> {
    hook: Box<libspa::sys::spa_hook>,
    _events: Box<pipewire::sys::pw_stream_events>,
    _pending: Arc<AtomicBool>,
    _stream: PhantomData<&'a pipewire::stream::Stream>,
}

impl<'a> TriggerDone<'a> {
    pub fn new(stream: &'a pipewire::stream::Stream, pending: Arc<AtomicBool>) -> Self {
        unsafe extern "C" fn done(data: *mut std::ffi::c_void) {
            // SAFETY: the listener owns this Arc until after unregistration.
            let pending = unsafe { &*data.cast::<AtomicBool>() };
            pending.store(false, Ordering::Release);
        }

        // SAFETY: SPA hooks and event tables support zero initialization.
        let mut hook = Box::new(unsafe { std::mem::zeroed() });
        let events = Box::new(pipewire::sys::pw_stream_events {
            version: pipewire::sys::PW_VERSION_STREAM_EVENTS,
            trigger_done: Some(done),
            ..unsafe { std::mem::zeroed() }
        });
        // SAFETY: all pointers are stable allocations retained by this listener;
        // PhantomData prevents the stream from being dropped first.
        unsafe {
            pipewire::sys::pw_stream_add_listener(
                stream.as_raw_ptr(),
                &mut *hook,
                &*events,
                Arc::as_ptr(&pending).cast_mut().cast(),
            );
        }
        Self {
            hook,
            _events: events,
            _pending: pending,
            _stream: PhantomData,
        }
    }
}

impl Drop for TriggerDone<'_> {
    fn drop(&mut self) {
        libspa::utils::hook::remove(*self.hook);
    }
}

pub(super) struct PullDemand;

impl PullDemand {
    /// `buffered` includes partially consumed chunks.
    pub fn needs_buffer(buffered: u64, units_per_sec: u32, cushion_ns: u64) -> bool {
        units_per_sec > 0
            && u128::from(buffered) * 1_000_000_000
                < u128::from(units_per_sec) * u128::from(cushion_ns)
    }
}

/// A driving stream owns the graph clock and must publish each cycle's timing
/// before triggering it, as PipeWire's own timer-less drivers do
/// (`module-pipe-tunnel`). Followers derive `pw_buffer.requested`, stream time,
/// and rate matching from these fields.
pub(super) fn advance_driver_clock(clock: &mut libspa::sys::spa_io_clock, now_ns: u64) {
    let rate = clock.target_rate;
    let duration = clock.target_duration;
    if rate.num == 0 || rate.denom == 0 || duration == 0 {
        return;
    }
    clock.position = clock.position.wrapping_add(clock.duration);
    clock.nsec = now_ns;
    clock.rate = rate;
    clock.duration = duration;
    clock.delay = 0;
    clock.rate_diff = 1.0;
    clock.next_nsec = now_ns.saturating_add(
        (u128::from(duration) * u128::from(rate.num) * 1_000_000_000 / u128::from(rate.denom))
            as u64,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulls_only_below_configured_cushion() {
        assert!(PullDemand::needs_buffer(0, 352_800, 90_000_000));
        assert!(PullDemand::needs_buffer(31_751, 352_800, 90_000_000));
        assert!(!PullDemand::needs_buffer(31_752, 352_800, 90_000_000));
        assert!(!PullDemand::needs_buffer(40_960, 352_800, 90_000_000));
    }

    #[test]
    fn downstream_stall_cannot_accumulate_capture_credit() {
        let mut buffered = 0;
        let mut captured = 0;
        for _ in 0..100_000 {
            if PullDemand::needs_buffer(buffered, 352_800, 90_000_000) {
                buffered += 8192;
                captured += 8192;
            }
        }
        assert_eq!(captured, 32_768);
        buffered -= 8192;
        assert!(PullDemand::needs_buffer(buffered, 352_800, 90_000_000));
    }

    #[test]
    fn supports_rate_changes_and_large_counters_without_overflow() {
        assert!(PullDemand::needs_buffer(40_000, 705_600, 90_000_000));
        assert!(!PullDemand::needs_buffer(40_000, 352_800, 90_000_000));
        assert!(!PullDemand::needs_buffer(0, 0, 90_000_000));
        assert!(!PullDemand::needs_buffer(u64::MAX, u32::MAX, 1_000_000_000));
    }

    #[test]
    fn pull_clock_preserves_fractional_periods_without_stall_bursts() {
        let start = Instant::now();
        let mut clock = PullClock::default();
        assert!(clock.ready(start));
        clock.pulled(start, 1024, 352_800);
        assert!(!clock.ready(start + Duration::from_millis(2)));
        let second = start + Duration::from_millis(3);
        assert!(clock.ready(second));
        clock.pulled(second, 1024, 352_800);
        assert!(clock.ready(start + Duration::from_micros(5805)));
        let resumed = start + Duration::from_secs(1);
        clock.pulled(resumed, 1024, 352_800);
        assert!(!clock.ready(resumed + Duration::from_millis(1)));
        assert!(clock.ready(resumed + Duration::from_millis(3)));
    }

    #[test]
    fn driver_clock_advances_by_published_cycles() {
        let mut clock: libspa::sys::spa_io_clock = unsafe { std::mem::zeroed() };
        advance_driver_clock(&mut clock, 1_000);
        assert_eq!(clock.nsec, 0, "unconfigured target leaves clock untouched");

        clock.target_rate = libspa::sys::spa_fraction {
            num: 1,
            denom: 48_000,
        };
        clock.target_duration = 1024;
        advance_driver_clock(&mut clock, 1_000);
        assert_eq!(
            (clock.position, clock.duration, clock.rate.denom),
            (0, 1024, 48_000)
        );
        assert_eq!(clock.next_nsec, 1_000 + 21_333_333);

        clock.target_duration = 512;
        advance_driver_clock(&mut clock, 2_000);
        assert_eq!(
            (clock.position, clock.duration, clock.nsec),
            (1024, 512, 2_000)
        );
    }

    #[test]
    #[ignore = "requires PipeWire, pw-dsdplay, and AUDIGMA_DSD_TEST_{FILE,EXPECTED}; silent sink only"]
    fn silent_capture_preserves_bytes_across_downstream_stall() {
        use crate::pipewire::PipeWireVirtualSink;
        use audigma_engine::traits::AudioInput;
        use audigma_settings::RateMode;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct Source(Child);
        impl Drop for Source {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let file = std::env::var("AUDIGMA_DSD_TEST_FILE").expect("DSD64 stereo source file");
        let expected = std::fs::read(
            std::env::var("AUDIGMA_DSD_TEST_EXPECTED")
                .expect("normalized interleaved MSB DSD bytes"),
        )
        .unwrap();
        assert!(expected.len() >= 705_600);
        let name = format!("audigma_silent_pull_test_{}", std::process::id());
        let mut sink = PipeWireVirtualSink::new_internal(
            &name,
            44_100,
            2,
            vec!["FL".into(), "FR".into()],
            RateMode::Dynamic,
            vec![44_100, 192_000],
            true,
            0.0,
            128,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let _source = Source(
            Command::new("pw-dsdplay")
                .args(["--target", &name, &file])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while sink.buffer_fill_secs().unwrap() < 0.09 {
            assert!(Instant::now() < deadline, "DSD capture did not preroll");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(100));
        let before = sink.processed_chunks();
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            before,
            sink.processed_chunks(),
            "capture ran ahead while downstream stalled"
        );
        assert!(sink.buffer_fill_secs().unwrap() < 0.12);

        let mut captured = Vec::new();
        let mut left = [0; 2048];
        let mut right = [0; 2048];
        while captured.len() < 705_600 {
            let count = sink.read_dsd_bytes(&mut [&mut left, &mut right]).unwrap();
            for i in 0..count {
                captured.extend_from_slice(&[left[i], right[i]]);
            }
            assert!(
                Instant::now() < deadline,
                "DSD capture stalled after resuming"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let mismatch = captured[..705_600]
            .iter()
            .zip(&expected)
            .position(|(a, b)| a != b);
        if let Some(i) = mismatch {
            panic!(
                "first mismatched byte: {i}; interleave={}",
                sink.dsd_interleave()
            );
        }
        assert_eq!(sink.dropped_chunks(), 0);
    }
}
