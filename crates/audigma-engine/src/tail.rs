//! Tail policy management and duration alignment (`Design.md §4.5`, `§5.3`).
//!
//! Handles:
//! - `TrimToDuration`: For album-continuous gapless tracks, discards the initial `D` filter
//!   group-delay samples (ramp-up from silence) and captures exactly `N_target = N_in * (f_out / f_in)`
//!   samples, aligning output phase with `t = 0`.
//! - `PreserveTail`: Flushes the filter tail until digital silence decays completely.

use audigma_settings::TailPolicy;

/// Manages sample counts, group-delay discarding, and tail trimming for an output stream.
#[derive(Debug, Clone)]
pub struct TailManager {
    policy: TailPolicy,
    group_delay_output_samples: usize,
    samples_to_discard: usize,
    target_output_samples: Option<u64>,
    samples_written: u64,
}

impl TailManager {
    /// Creates a new `TailManager`.
    ///
    /// - `policy`: `TrimToDuration` or `PreserveTail`.
    /// - `group_delay_output_samples`: Total filter latency in final output samples.
    /// - `target_output_samples`: Optional exact output sample count if input length is known.
    pub fn new(
        policy: TailPolicy,
        group_delay_output_samples: usize,
        target_output_samples: Option<u64>,
    ) -> Self {
        let samples_to_discard = match policy {
            TailPolicy::TrimToDuration => group_delay_output_samples,
            TailPolicy::PreserveTail => 0,
        };

        Self {
            policy,
            group_delay_output_samples,
            samples_to_discard,
            target_output_samples,
            samples_written: 0,
        }
    }

    /// Sets the target output sample count when total input frames become known at EOF.
    pub fn set_target_output_samples(&mut self, target: u64) {
        self.target_output_samples = Some(target);
    }

    /// Returns the configured tail policy.
    pub fn policy(&self) -> TailPolicy {
        self.policy
    }

    /// Returns the total group delay of the filter cascade in final output samples.
    pub fn group_delay(&self) -> usize {
        self.group_delay_output_samples
    }

    /// Returns the cumulative number of output samples accepted and written.
    pub fn samples_written(&self) -> u64 {
        self.samples_written
    }

    /// Filters and trims a slice of output samples according to group-delay and target length.
    ///
    /// Returns the slice range `[start..end]` of samples to actually write to the output sink.
    /// If no samples should be written (e.g. still discarding ramp-up or already at target), returns `None`.
    pub fn process_slice_range(&mut self, chunk_len: usize) -> Option<(usize, usize)> {
        if chunk_len == 0 {
            return None;
        }

        let mut start = 0;

        // Discard initial group-delay ramp-up if needed
        if self.samples_to_discard > 0 {
            if chunk_len <= self.samples_to_discard {
                self.samples_to_discard -= chunk_len;
                return None;
            } else {
                start = self.samples_to_discard;
                self.samples_to_discard = 0;
            }
        }

        let mut end = chunk_len;

        // Check if we have hit the target sample ceiling in TrimToDuration mode
        if self.policy == TailPolicy::TrimToDuration {
            if let Some(target) = self.target_output_samples {
                let remaining = target.saturating_sub(self.samples_written);
                if remaining == 0 {
                    return None;
                }
                let available = (end - start) as u64;
                if available > remaining {
                    end = start + (remaining as usize);
                }
            }
        }

        let produced = end - start;
        self.samples_written += produced as u64;
        Some((start, end))
    }

    /// Returns `true` if `TrimToDuration` has reached its target output sample count.
    pub fn is_target_satisfied(&self) -> bool {
        if self.policy == TailPolicy::TrimToDuration {
            if let Some(target) = self.target_output_samples {
                return self.samples_written >= target;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tail_manager_trim_to_duration_discards_and_caps() {
        // Delay = 10 samples, target = 25 samples
        let mut tm = TailManager::new(TailPolicy::TrimToDuration, 10, Some(25));
        assert_eq!(tm.policy(), TailPolicy::TrimToDuration);
        assert_eq!(tm.group_delay(), 10);

        // Chunk 1: 6 samples (all should be discarded as part of ramp-up)
        assert_eq!(tm.process_slice_range(6), None);
        assert_eq!(tm.samples_written(), 0);

        // Chunk 2: 8 samples (first 4 discarded to complete 10, remaining 4 written)
        assert_eq!(tm.process_slice_range(8), Some((4, 8)));
        assert_eq!(tm.samples_written(), 4);

        // Chunk 3: 20 samples (all written up to 24 samples written)
        assert_eq!(tm.process_slice_range(20), Some((0, 20)));
        assert_eq!(tm.samples_written(), 24);

        // Chunk 4: 10 samples (only 1 remaining until 25)
        assert_eq!(tm.process_slice_range(10), Some((0, 1)));
        assert_eq!(tm.samples_written(), 25);
        assert!(tm.is_target_satisfied());

        // Chunk 5: should return None since target is satisfied
        assert_eq!(tm.process_slice_range(10), None);
    }

    #[test]
    fn test_tail_manager_preserve_tail() {
        // Delay = 10 samples, no discard
        let mut tm = TailManager::new(TailPolicy::PreserveTail, 10, None);
        assert_eq!(tm.process_slice_range(15), Some((0, 15)));
        assert_eq!(tm.samples_written(), 15);
        assert!(!tm.is_target_satisfied());
    }
}
