use crate::{Error, Result};

/// Content-defined chunking bounds. The measured agent-traffic profile uses a
/// 2 KiB target so repeated message and tool-result ranges in relatively small
/// JSON bodies form reusable chunks. Bodies at least 8 MiB automatically use
/// a measured 8 KiB target/32 KiB maximum to keep reconstruction throughput
/// above the v1 target without weakening ordinary chat-message deduplication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChunkerConfig {
    pub min_size: usize,
    pub target_size: usize,
    pub max_size: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            min_size: 512,
            target_size: 2 * 1024,
            max_size: 8 * 1024,
        }
    }
}

impl ChunkerConfig {
    pub const LARGE_BODY_THRESHOLD: u64 = 8 * 1024 * 1024;

    pub fn validate(self) -> Result<Self> {
        if self.min_size == 0
            || self.min_size > self.target_size
            || self.target_size > self.max_size
        {
            return Err(Error::Invalid(
                "chunk sizes must satisfy 0 < min <= target <= max",
            ));
        }
        Ok(self)
    }

    /// Use a wider CDC profile only for large bodies written with the default
    /// profile. Small agent messages retain fine-grained dedup boundaries;
    /// large tool/media payloads avoid tens of thousands of tiny independent
    /// zstd frames during reconstruction. Explicit caller profiles are never
    /// silently changed.
    pub fn for_body_length(self, body_length: u64) -> Self {
        if self == Self::default() && body_length >= Self::LARGE_BODY_THRESHOLD {
            Self {
                min_size: 2 * 1024,
                target_size: 8 * 1024,
                max_size: 32 * 1024,
            }
        } else {
            self
        }
    }
}

/// A streaming Gear-hash content-defined chunker. Boundaries are independent
/// of how callers split input across `push` calls.
pub struct StreamingChunker {
    config: ChunkerConfig,
    mask: u64,
    hash: u64,
    pending: Vec<u8>,
}

impl StreamingChunker {
    pub fn new(config: ChunkerConfig) -> Result<Self> {
        let config = config.validate()?;
        let boundary = config
            .target_size
            .checked_next_power_of_two()
            .ok_or(Error::Invalid("chunk target size is too large"))?;
        Ok(Self {
            config,
            mask: boundary as u64 - 1,
            hash: 0,
            pending: Vec::with_capacity(config.max_size),
        })
    }

    pub fn push<F>(&mut self, input: &[u8], mut emit: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        for &byte in input {
            self.pending.push(byte);
            self.hash = self.hash.rotate_left(1).wrapping_add(gear(byte));
            let len = self.pending.len();
            if len >= self.config.max_size
                || (len >= self.config.min_size && self.hash & self.mask == 0)
            {
                emit(&self.pending)?;
                self.pending.clear();
                self.hash = 0;
            }
        }
        Ok(())
    }

    pub fn finish<F>(&mut self, mut emit: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if !self.pending.is_empty() {
            emit(&self.pending)?;
            self.pending.clear();
            self.hash = 0;
        }
        Ok(())
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

fn gear(byte: u8) -> u64 {
    // SplitMix64 gives each byte a stable, well-distributed Gear value without
    // embedding a 2 KiB lookup table in the source.
    let mut value = (byte as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_do_not_depend_on_push_sizes() {
        let data: Vec<u8> = (0..400_000).map(|n| ((n * 31) % 251) as u8).collect();
        let collect = |step: usize| {
            let mut chunker = StreamingChunker::new(ChunkerConfig::default()).unwrap();
            let mut lengths = Vec::new();
            for part in data.chunks(step) {
                chunker
                    .push(part, |chunk| {
                        lengths.push(chunk.len());
                        Ok(())
                    })
                    .unwrap();
            }
            chunker
                .finish(|chunk| {
                    lengths.push(chunk.len());
                    Ok(())
                })
                .unwrap();
            lengths
        };
        assert_eq!(collect(1), collect(7777));
        assert_eq!(collect(7777), collect(131_072));
    }

    #[test]
    fn emitted_chunks_obey_bounds_except_final() {
        let config = ChunkerConfig {
            min_size: 64,
            target_size: 128,
            max_size: 256,
        };
        let mut chunker = StreamingChunker::new(config).unwrap();
        let data = vec![42; 4097];
        let mut sizes = Vec::new();
        chunker
            .push(&data, |chunk| {
                sizes.push(chunk.len());
                Ok(())
            })
            .unwrap();
        chunker
            .finish(|chunk| {
                sizes.push(chunk.len());
                Ok(())
            })
            .unwrap();
        for size in &sizes[..sizes.len() - 1] {
            assert!((*size >= config.min_size) && (*size <= config.max_size));
        }
        assert!(sizes.last().copied().unwrap() <= config.max_size);
    }

    #[test]
    fn only_default_large_bodies_use_the_wider_profile() {
        let default = ChunkerConfig::default();
        assert_eq!(
            default.for_body_length(ChunkerConfig::LARGE_BODY_THRESHOLD - 1),
            default
        );
        assert_eq!(
            default.for_body_length(ChunkerConfig::LARGE_BODY_THRESHOLD),
            ChunkerConfig {
                min_size: 2 * 1024,
                target_size: 8 * 1024,
                max_size: 32 * 1024,
            }
        );
        let explicit = ChunkerConfig {
            min_size: 1,
            target_size: 2,
            max_size: 4,
        };
        assert_eq!(explicit.for_body_length(u64::MAX), explicit);
    }
}
