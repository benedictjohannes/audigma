//! Lock-free SPSC ring buffers and generation-tagged chunk wrappers (`Design.md §3.4`).
//!
//! Provides:
//! - `TaggedChunk`: Fixed-capacity audio sample chunk holding a monotonic generation tag (`u64`),
//!   channel count, valid sample count, and contiguous `f64` storage.
//! - `AudioSpscQueue`: Lock-free single-producer single-consumer ring buffer backed by `rtrb`.
//! - Real-time safe: Zero heap allocation during push/pop once initialized.

use rtrb::{Consumer, Producer, RingBuffer};

/// Default capacity for intermediate PCM chunk buffers (e.g. 1024 stereo frames = 2048 samples).
pub const DEFAULT_CHUNK_SAMPLES: usize = 4096;

/// A fixed-capacity, generation-tagged block of floating-point audio samples.
#[derive(Debug, Clone)]
pub struct TaggedChunk {
    /// Monotonic generation tag. Consumer drops chunks with mismatched tags.
    pub generation: u64,
    /// Number of channels in this block (e.g. 2 for stereo).
    pub channels: usize,
    /// Whether this chunk holds packed 1-bit DSD byte stream payload.
    pub is_dsd: bool,
    /// Number of valid samples in `samples` (channels * frames) or valid bytes in `dsd_bytes`.
    pub len: usize,
    /// Preallocated sample buffer.
    pub samples: Vec<f64>,
    /// Preallocated DSD bytes buffer.
    pub dsd_bytes: Vec<u8>,
}

impl TaggedChunk {
    /// Creates a preallocated chunk with a specified capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            generation: 0,
            channels: 2,
            is_dsd: false,
            len: 0,
            samples: vec![0.0; capacity],
            dsd_bytes: vec![0; capacity * 4],
        }
    }

    /// Resets this chunk for reuse with floating point PCM samples.
    #[inline]
    pub fn set(&mut self, generation: u64, channels: usize, data: &[f64]) {
        self.generation = generation;
        self.channels = channels;
        self.is_dsd = false;
        let count = data.len().min(self.samples.len());
        self.samples[..count].copy_from_slice(&data[..count]);
        self.len = count;
    }

    /// Resets this chunk for reuse with raw 1-bit DSD bytes.
    #[inline]
    pub fn set_dsd(&mut self, generation: u64, channels: usize, data: &[u8]) {
        self.generation = generation;
        self.channels = channels;
        self.is_dsd = true;
        let count = data.len().min(self.dsd_bytes.len());
        self.dsd_bytes[..count].copy_from_slice(&data[..count]);
        self.len = count;
    }

    /// Returns valid DSD bytes as a slice.
    #[inline]
    pub fn dsd_slice(&self) -> &[u8] {
        &self.dsd_bytes[..self.len]
    }

    /// Returns the number of audio frames (`len / channels`).
    #[inline]
    pub fn frames(&self) -> usize {
        self.len.checked_div(self.channels).unwrap_or(0)
    }

    /// Returns valid audio samples as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[f64] {
        &self.samples[..self.len]
    }

    /// Returns valid audio samples as a mutable slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [f64] {
        &mut self.samples[..self.len]
    }
}

/// A pair of lock-free SPSC producer and consumer handles for audio chunks.
pub struct AudioSpscQueue;

impl AudioSpscQueue {
    /// Allocates an SPSC ring buffer for `TaggedChunk` instances of `num_chunks` depth,
    /// each chunk initialized with `chunk_capacity` floats.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        num_chunks: usize,
        chunk_capacity: usize,
    ) -> (Producer<TaggedChunk>, Consumer<TaggedChunk>) {
        let (mut prod, cons) = RingBuffer::new(num_chunks);
        // Pre-fill ring buffer slots so memory is touched upfront and no allocation occurs later
        for _ in 0..num_chunks {
            let _ = prod.push(TaggedChunk::new(chunk_capacity));
        }
        (prod, cons)
    }

    /// Allocates an empty ring buffer with given capacity.
    pub fn new_empty(num_chunks: usize) -> (Producer<TaggedChunk>, Consumer<TaggedChunk>) {
        RingBuffer::new(num_chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tagged_chunk_basic() {
        let mut chunk = TaggedChunk::new(1024);
        assert_eq!(chunk.generation, 0);
        assert_eq!(chunk.len, 0);

        let data = [0.1, 0.2, 0.3, 0.4];
        chunk.set(42, 2, &data);
        assert_eq!(chunk.generation, 42);
        assert_eq!(chunk.channels, 2);
        assert_eq!(chunk.frames(), 2);
        assert_eq!(chunk.as_slice(), &data);
    }

    #[test]
    fn test_spsc_ring_buffer() {
        let (mut prod, mut cons) = AudioSpscQueue::new_empty(4);
        assert_eq!(prod.slots(), 4);
        assert_eq!(cons.slots(), 0);

        let mut c = TaggedChunk::new(128);
        c.set(1, 2, &[0.5, -0.5]);
        assert!(prod.push(c).is_ok());

        assert_eq!(cons.slots(), 1);
        let popped = cons.pop().unwrap();
        assert_eq!(popped.generation, 1);
        assert_eq!(popped.as_slice(), &[0.5, -0.5]);
    }
}
