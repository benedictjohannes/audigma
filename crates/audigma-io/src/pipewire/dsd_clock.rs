//! Native DSD capture pacing, independent of the graph's PCM rate and quantum.

#[derive(Clone, Copy, Debug)]
pub(super) struct GraphCycle {
    pub id: u32,
    pub position: u64,
    pub duration: u64,
    pub rate_num: u32,
    pub rate_denom: u32,
}

impl GraphCycle {
    fn seconds_per_tick(self) -> Option<f64> {
        (self.rate_num > 0 && self.rate_denom > 0 && self.duration > 0)
            .then(|| self.rate_num as f64 / self.rate_denom as f64)
    }

    /// Round up to complete interleave groups. Fractional excess is accounted
    /// by the pacing credit, rather than accumulating input faster than real time.
    pub fn buffer_bytes(self, byte_rate: u32, channels: u32, interleave: u32) -> Option<u32> {
        self.seconds_per_tick()?;
        let group = interleave.max(1) as u128;
        let numerator = self.duration as u128 * self.rate_num as u128 * byte_rate as u128;
        let groups = numerator.div_ceil(self.rate_denom as u128 * group);
        let bytes = groups * group * channels as u128;
        (bytes > 0 && bytes <= i32::MAX as u128).then_some(bytes as u32)
    }
}

#[derive(Debug, Default)]
pub(super) struct DsdCaptureClock {
    last: Option<GraphCycle>,
    epoch: u64,
    credit_secs: f64,
}

impl DsdCaptureClock {
    pub fn can_dequeue(&mut self, cycle: GraphCycle, epoch: u64, queue_full: bool) -> bool {
        self.ready(cycle, epoch) && !queue_full
    }

    /// At most one cycle of positive credit may accumulate while a source is
    /// idle. Negative credit retains the duration of an oversized old buffer
    /// during renegotiation; leaving buffers queued applies upstream backpressure.
    fn ready(&mut self, cycle: GraphCycle, epoch: u64) -> bool {
        let Some(seconds_per_tick) = cycle.seconds_per_tick() else {
            return false;
        };
        if self.epoch != epoch {
            *self = Self {
                epoch,
                ..Self::default()
            };
        }
        let ticks = match self.last {
            Some(last)
                if last.id == cycle.id
                    && last.rate_num == cycle.rate_num
                    && last.rate_denom == cycle.rate_denom
                    && cycle.position >= last.position =>
            {
                cycle.position - last.position
            }
            _ => cycle.duration,
        };
        self.last = Some(cycle);
        self.credit_secs = (self.credit_secs + ticks as f64 * seconds_per_tick)
            .min(cycle.duration as f64 * seconds_per_tick);
        self.credit_secs > 1e-12
    }

    pub fn consume(&mut self, bytes_per_channel: usize, byte_rate: u32) {
        if byte_rate > 0 {
            self.credit_secs -= bytes_per_channel as f64 / byte_rate as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle(position: u64, duration: u64, rate: u32) -> GraphCycle {
        GraphCycle {
            id: 1,
            position,
            duration,
            rate_num: 1,
            rate_denom: rate,
        }
    }

    #[test]
    fn sizing_uses_graph_time_not_native_rate_quantum() {
        assert_eq!(
            cycle(0, 256, 44_100).buffer_bytes(352_800, 2, 1),
            Some(4096)
        );
        assert_eq!(
            cycle(0, 256, 352_800).buffer_bytes(352_800, 2, 1),
            Some(512)
        );
        assert_eq!(
            cycle(0, 8192, 44_100).buffer_bytes(352_800, 2, 1),
            Some(131_072)
        );
        assert_eq!(
            cycle(0, 256, 48_000).buffer_bytes(352_800, 2, 4),
            Some(3768)
        );
        assert_eq!(cycle(0, 256, 0).buffer_bytes(352_800, 2, 1), None);
    }

    #[test]
    fn oversized_old_buffers_are_paced_during_renegotiation() {
        let mut clock = DsdCaptureClock::default();
        let mut consumed = 0;
        for n in 0..3200 {
            if clock.ready(cycle(n * 256, 256, 352_800), 1) {
                clock.consume(8192, 352_800);
                consumed += 8192;
            }
        }
        assert_eq!(consumed, 3200 * 256);
    }

    #[test]
    fn fractional_bytes_do_not_cause_long_term_runaway() {
        let mut clock = DsdCaptureClock::default();
        let mut consumed = 0;
        for n in 0..100_000 {
            let cycle = cycle(n * 256, 256, 48_000);
            if clock.ready(cycle, 1) {
                let bytes = cycle.buffer_bytes(352_800, 2, 1).unwrap() / 2;
                clock.consume(bytes as usize, 352_800);
                consumed += bytes as u64;
            }
        }
        let expected = 100_000u64 * 256 * 352_800 / 48_000;
        assert!(consumed.abs_diff(expected) < 1882);
    }

    #[test]
    fn quantum_and_rate_changes_preserve_pacing() {
        let mut clock = DsdCaptureClock::default();
        for (duration, rate) in [
            (1024, 44_100),
            (256, 44_100),
            (256, 352_800),
            (8192, 352_800),
        ] {
            for n in 0..100 {
                let cycle = cycle(n * duration, duration, rate);
                assert!(clock.ready(cycle, 1));
                let bytes = cycle.buffer_bytes(352_800, 2, 1).unwrap() / 2;
                clock.consume(bytes as usize, 352_800);
            }
        }
    }

    #[test]
    fn idle_and_format_changes_do_not_accumulate_burst_credit() {
        let mut clock = DsdCaptureClock::default();
        assert!(clock.ready(cycle(0, 256, 44_100), 1));
        assert!(clock.ready(cycle(44_100 * 60, 256, 44_100), 1));
        clock.consume(2048, 352_800);
        assert!(!clock.ready(cycle(44_100 * 60, 256, 44_100), 1));
        clock.consume(8192, 352_800);
        assert!(clock.ready(cycle(0, 256, 44_100), 2));
    }

    #[test]
    fn full_ring_holds_upstream_buffers_without_accumulating_a_burst() {
        let mut clock = DsdCaptureClock::default();
        for n in 0..1000 {
            assert!(!clock.can_dequeue(cycle(n * 128, 128, 352_800), 1, true));
        }
        let cycle = cycle(1000 * 128, 128, 352_800);
        assert!(clock.can_dequeue(cycle, 1, false));
        clock.consume(128, 352_800);
        assert!(!clock.can_dequeue(cycle, 1, false));
    }
}
