use crate::{Error, Result};
use std::collections::HashMap;

const ROLLING_BASE: u64 = 0x9e37_79b1;

/// Bounds for predecessor-aware body segmentation. The matcher is deliberately
/// small and deterministic: it indexes a sampled set of fixed-width windows
/// from one predecessor, verifies every hash hit byte-for-byte, and stops
/// looking when its explicit work budget is exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RangeMatchConfig {
    pub window_size: usize,
    pub min_match: usize,
    pub max_base_bytes: usize,
    pub max_current_bytes: usize,
    pub max_candidates: usize,
    pub max_candidates_per_hash: usize,
    pub max_work_bytes: usize,
    pub max_segments: usize,
}

impl Default for RangeMatchConfig {
    fn default() -> Self {
        Self {
            window_size: 64,
            min_match: 128,
            max_base_bytes: 64 * 1024 * 1024,
            max_current_bytes: 64 * 1024 * 1024,
            max_candidates: 262_144,
            max_candidates_per_hash: 4,
            max_work_bytes: 256 * 1024 * 1024,
            max_segments: 262_144,
        }
    }
}

impl RangeMatchConfig {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.window_size == 0
            || self.min_match < self.window_size
            || self.max_candidates == 0
            || self.max_candidates_per_hash == 0
            || self.max_segments == 0
        {
            return Err(Error::Invalid(
                "range matcher requires a nonzero window/candidate/segment limit and min_match >= window_size",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Segment {
    Copy {
        base_offset: usize,
        length: usize,
    },
    Literal {
        current_offset: usize,
        length: usize,
    },
}

impl Segment {
    fn length(self) -> usize {
        match self {
            Self::Copy { length, .. } | Self::Literal { length, .. } => length,
        }
    }
}

/// Returns `None` when the inputs or estimated work exceed configured bounds,
/// or when the predecessor supplies no useful range. Callers then use their
/// ordinary content-defined chunking path.
pub(crate) fn segment_against_predecessor(
    current: &[u8],
    base: &[u8],
    config: RangeMatchConfig,
) -> Result<Option<Vec<Segment>>> {
    let config = config.validate()?;
    if current.len() > config.max_current_bytes
        || base.len() > config.max_base_bytes
        || current.is_empty()
        || base.is_empty()
    {
        return Ok(None);
    }
    // Two linear passes cover prefix/suffix discovery plus building/scanning
    // the rolling-hash index. Bytewise hash-hit verification and extension are
    // charged separately below.
    let baseline_work = current.len().saturating_add(base.len()).saturating_mul(2);
    if baseline_work > config.max_work_bytes {
        return Ok(None);
    }
    let mut work_left = config.max_work_bytes - baseline_work;

    let prefix = common_prefix(current, base);
    let suffix = common_suffix(&current[prefix..], &base[prefix..]);
    let current_middle_end = current.len() - suffix;
    let base_middle_end = base.len() - suffix;
    let mut segments = Vec::new();
    if prefix != 0 {
        push_segment(
            &mut segments,
            Segment::Copy {
                base_offset: 0,
                length: prefix,
            },
        );
    }

    let current_middle = &current[prefix..current_middle_end];
    let base_middle = &base[prefix..base_middle_end];
    match_middle(
        current_middle,
        base_middle,
        prefix,
        prefix,
        config,
        &mut work_left,
        &mut segments,
    );

    if suffix != 0 {
        push_segment(
            &mut segments,
            Segment::Copy {
                base_offset: base.len() - suffix,
                length: suffix,
            },
        );
    }
    if segments.len() > config.max_segments
        || segments
            .iter()
            .map(|segment| segment.length())
            .sum::<usize>()
            != current.len()
    {
        return Ok(None);
    }
    let copied = segments
        .iter()
        .filter_map(|segment| match segment {
            Segment::Copy { length, .. } => Some(*length),
            Segment::Literal { .. } => None,
        })
        .sum::<usize>();
    if copied < config.min_match.min(current.len()) {
        return Ok(None);
    }
    Ok(Some(segments))
}

#[allow(clippy::too_many_arguments)]
fn match_middle(
    current: &[u8],
    base: &[u8],
    current_origin: usize,
    base_origin: usize,
    config: RangeMatchConfig,
    work_left: &mut usize,
    output: &mut Vec<Segment>,
) {
    if current.is_empty() {
        return;
    }
    if current.len() < config.window_size || base.len() < config.window_size {
        push_segment(
            output,
            Segment::Literal {
                current_offset: current_origin,
                length: current.len(),
            },
        );
        return;
    }

    let available = base.len() - config.window_size + 1;
    let stride = available
        .saturating_add(config.max_candidates - 1)
        .checked_div(config.max_candidates)
        .unwrap_or(1)
        .max(1);
    let mut index: HashMap<u64, Vec<usize>> = HashMap::new();
    let factor = rolling_factor(config.window_size);
    let mut position = 0usize;
    let mut candidates = 0usize;
    let mut hash = rolling_hash(&base[..config.window_size]);
    while position < available && candidates < config.max_candidates {
        if position.is_multiple_of(stride) {
            let bucket = index.entry(hash).or_default();
            if bucket.len() < config.max_candidates_per_hash {
                bucket.push(position);
                candidates += 1;
            }
        }
        if position + 1 < available {
            hash = roll_hash(
                hash,
                base[position],
                base[position + config.window_size],
                factor,
            );
        }
        position += 1;
    }

    let mut cursor = 0usize;
    let mut literal_start = 0usize;
    let mut current_hash = rolling_hash(&current[..config.window_size]);
    while cursor + config.window_size <= current.len() && output.len() < config.max_segments {
        if *work_left < config.window_size {
            break;
        }
        let mut best: Option<(usize, usize, usize)> = None;
        if let Some(base_candidates) = index.get(&current_hash) {
            for &candidate in base_candidates {
                if *work_left < config.window_size {
                    break;
                }
                *work_left -= config.window_size;
                if base[candidate..candidate + config.window_size]
                    != current[cursor..cursor + config.window_size]
                {
                    continue;
                }
                let mut current_start = cursor;
                let mut base_start = candidate;
                while current_start > literal_start
                    && base_start > 0
                    && *work_left > 0
                    && current[current_start - 1] == base[base_start - 1]
                {
                    current_start -= 1;
                    base_start -= 1;
                    *work_left -= 1;
                }
                let mut length = cursor - current_start + config.window_size;
                while current_start + length < current.len()
                    && base_start + length < base.len()
                    && *work_left > 0
                    && current[current_start + length] == base[base_start + length]
                {
                    length += 1;
                    *work_left -= 1;
                }
                if length >= config.min_match
                    && best.is_none_or(|(best_start, best_length, _)| {
                        length > best_length || (length == best_length && base_start < best_start)
                    })
                {
                    best = Some((base_start, length, current_start));
                }
            }
        }
        if let Some((base_start, length, current_start)) = best {
            if current_start > literal_start {
                push_segment(
                    output,
                    Segment::Literal {
                        current_offset: current_origin + literal_start,
                        length: current_start - literal_start,
                    },
                );
            }
            push_segment(
                output,
                Segment::Copy {
                    base_offset: base_origin + base_start,
                    length,
                },
            );
            cursor = current_start + length;
            literal_start = cursor;
            if cursor + config.window_size <= current.len() {
                current_hash = rolling_hash(&current[cursor..cursor + config.window_size]);
            }
        } else {
            if cursor + config.window_size < current.len() {
                current_hash = roll_hash(
                    current_hash,
                    current[cursor],
                    current[cursor + config.window_size],
                    factor,
                );
            }
            cursor += 1;
        }
    }
    if literal_start < current.len() {
        push_segment(
            output,
            Segment::Literal {
                current_offset: current_origin + literal_start,
                length: current.len() - literal_start,
            },
        );
    }
}

fn common_prefix(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .position(|(left, right)| left != right)
        .unwrap_or(left.len().min(right.len()))
}

fn common_suffix(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .rev()
        .zip(right.iter().rev())
        .position(|(left, right)| left != right)
        .unwrap_or(left.len().min(right.len()))
}

fn rolling_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |hash, byte| {
        hash.wrapping_mul(ROLLING_BASE)
            .wrapping_add(u64::from(*byte) + 1)
    })
}

fn rolling_factor(window_size: usize) -> u64 {
    (1..window_size).fold(1u64, |factor, _| factor.wrapping_mul(ROLLING_BASE))
}

fn roll_hash(hash: u64, outgoing: u8, incoming: u8, factor: u64) -> u64 {
    hash.wrapping_sub((u64::from(outgoing) + 1).wrapping_mul(factor))
        .wrapping_mul(ROLLING_BASE)
        .wrapping_add(u64::from(incoming) + 1)
}

fn push_segment(segments: &mut Vec<Segment>, next: Segment) {
    if next.length() == 0 {
        return;
    }
    if let Some(previous) = segments.last_mut() {
        match (previous, next) {
            (
                Segment::Literal {
                    current_offset,
                    length,
                },
                Segment::Literal {
                    current_offset: next_offset,
                    length: next_length,
                },
            ) if *current_offset + *length == next_offset => {
                *length += next_length;
                return;
            }
            (
                Segment::Copy {
                    base_offset,
                    length,
                },
                Segment::Copy {
                    base_offset: next_offset,
                    length: next_length,
                },
            ) if *base_offset + *length == next_offset => {
                *length += next_length;
                return;
            }
            _ => {}
        }
    }
    segments.push(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_prefix_insertion_and_suffix_without_recursive_data() {
        let base = b"prefix:abcdefghijklmnopqrstuvwxyz:suffix";
        let current = b"prefix:abcdefghijk--new--lmnopqrstuvwxyz:suffix";
        let config = RangeMatchConfig {
            window_size: 4,
            min_match: 4,
            ..RangeMatchConfig::default()
        };
        let segments = segment_against_predecessor(current, base, config)
            .unwrap()
            .unwrap();
        let copied = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::Copy { length, .. } => Some(*length),
                Segment::Literal { .. } => None,
            })
            .sum::<usize>();
        assert!(copied > current.len() / 2, "{segments:?}");
    }

    #[test]
    fn refuses_work_or_candidate_configuration_overrun() {
        let bytes = vec![7; 1024];
        let config = RangeMatchConfig {
            max_work_bytes: 1024,
            ..RangeMatchConfig::default()
        };
        assert_eq!(
            segment_against_predecessor(&bytes, &bytes, config).unwrap(),
            None
        );
    }

    #[test]
    fn rolling_hash_matches_full_recomputation() {
        let bytes: Vec<u8> = (0..4096).map(|index| (index % 251) as u8).collect();
        let window = 64;
        let factor = rolling_factor(window);
        let mut hash = rolling_hash(&bytes[..window]);
        for position in 0..=bytes.len() - window {
            assert_eq!(hash, rolling_hash(&bytes[position..position + window]));
            if position + window < bytes.len() {
                hash = roll_hash(hash, bytes[position], bytes[position + window], factor);
            }
        }
    }
}
