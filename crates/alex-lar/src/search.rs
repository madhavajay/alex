use crate::{BodyManifest, ChunkHash, Error, Result};
use std::collections::HashMap;

/// Hard bounds for an exact raw-byte search. The chunk cache is what permits
/// deduplicated archives to be searched without decompressing a shared chunk
/// once for every manifest that references it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawSearchLimits {
    pub max_literal_bytes: u64,
    pub max_manifests: u64,
    pub max_manifest_ranges: u64,
    pub max_cached_chunk_bytes: u64,
    pub max_logical_bytes: u64,
}

impl Default for RawSearchLimits {
    fn default() -> Self {
        Self {
            max_literal_bytes: 1024 * 1024,
            max_manifests: 1_000_000,
            max_manifest_ranges: 8_000_000,
            max_cached_chunk_bytes: 512 * 1024 * 1024,
            max_logical_bytes: 64 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RawSearchStats {
    pub manifests_scanned: u64,
    pub manifest_ranges_scanned: u64,
    pub logical_bytes_scanned: u64,
    pub unique_chunks_read: u64,
    pub decompressed_chunk_bytes: u64,
}

/// Searches manifests while retaining each verified decompressed chunk once.
/// Create one scanner per physical archive (or one for a live catalog whose
/// chunk hashes have a single authoritative location).
pub struct RawBodyScanner {
    literal: Vec<u8>,
    failure: Vec<usize>,
    limits: RawSearchLimits,
    stats: RawSearchStats,
    chunks: HashMap<ChunkHash, Vec<u8>>,
}

impl RawBodyScanner {
    pub fn new(literal: &[u8], limits: RawSearchLimits) -> Result<Self> {
        if literal.is_empty() {
            return Err(Error::Invalid("grep literal must not be empty"));
        }
        if literal.len() as u64 > limits.max_literal_bytes {
            return Err(Error::Limit {
                what: "grep literal bytes",
                actual: literal.len() as u64,
                limit: limits.max_literal_bytes,
            });
        }
        let mut failure = vec![0; literal.len()];
        let mut prefix = 0usize;
        for index in 1..literal.len() {
            while prefix > 0 && literal[prefix] != literal[index] {
                prefix = failure[prefix - 1];
            }
            if literal[prefix] == literal[index] {
                prefix += 1;
            }
            failure[index] = prefix;
        }
        Ok(Self {
            literal: literal.to_vec(),
            failure,
            limits,
            stats: RawSearchStats::default(),
            chunks: HashMap::new(),
        })
    }

    pub fn limits(&self) -> RawSearchLimits {
        self.limits
    }

    pub fn stats(&self) -> RawSearchStats {
        self.stats
    }

    /// Return the first exact literal offset in `manifest`, if any. Matcher
    /// state is deliberately carried across every chunk range boundary.
    pub fn search_manifest<F>(
        &mut self,
        manifest: &BodyManifest,
        mut read_chunk: F,
    ) -> Result<Option<u64>>
    where
        F: FnMut(&ChunkHash) -> Result<Vec<u8>>,
    {
        let manifests = self.stats.manifests_scanned.saturating_add(1);
        if manifests > self.limits.max_manifests {
            return Err(Error::Limit {
                what: "grep manifests",
                actual: manifests,
                limit: self.limits.max_manifests,
            });
        }
        self.stats.manifests_scanned = manifests;
        manifest.validate()?;

        let mut matched = 0usize;
        let mut position = 0u64;
        for reference in &manifest.chunks {
            let ranges = self.stats.manifest_ranges_scanned.saturating_add(1);
            if ranges > self.limits.max_manifest_ranges {
                return Err(Error::Limit {
                    what: "grep manifest ranges",
                    actual: ranges,
                    limit: self.limits.max_manifest_ranges,
                });
            }
            self.stats.manifest_ranges_scanned = ranges;

            let charged = self
                .stats
                .logical_bytes_scanned
                .checked_add(reference.length)
                .ok_or(Error::Limit {
                    what: "grep logical bytes",
                    actual: u64::MAX,
                    limit: self.limits.max_logical_bytes,
                })?;
            if charged > self.limits.max_logical_bytes {
                return Err(Error::Limit {
                    what: "grep logical bytes",
                    actual: charged,
                    limit: self.limits.max_logical_bytes,
                });
            }
            self.stats.logical_bytes_scanned = charged;

            if !self.chunks.contains_key(&reference.chunk_hash) {
                let bytes = read_chunk(&reference.chunk_hash)?;
                if ChunkHash::blake3(&bytes) != reference.chunk_hash {
                    return Err(Error::Invalid("grep chunk hash mismatch"));
                }
                let cached = self
                    .stats
                    .decompressed_chunk_bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or(Error::Limit {
                        what: "grep cached chunk bytes",
                        actual: u64::MAX,
                        limit: self.limits.max_cached_chunk_bytes,
                    })?;
                if cached > self.limits.max_cached_chunk_bytes {
                    return Err(Error::Limit {
                        what: "grep cached chunk bytes",
                        actual: cached,
                        limit: self.limits.max_cached_chunk_bytes,
                    });
                }
                self.stats.unique_chunks_read += 1;
                self.stats.decompressed_chunk_bytes = cached;
                self.chunks.insert(reference.chunk_hash, bytes);
            }
            let bytes = self
                .chunks
                .get(&reference.chunk_hash)
                .ok_or_else(|| Error::Missing("cached grep chunk".into()))?;
            let start = usize::try_from(reference.chunk_offset)
                .map_err(|_| Error::Invalid("grep chunk range exceeds address space"))?;
            let end_u64 = reference
                .chunk_offset
                .checked_add(reference.length)
                .ok_or(Error::Invalid("grep chunk range overflow"))?;
            let end = usize::try_from(end_u64)
                .map_err(|_| Error::Invalid("grep chunk range exceeds address space"))?;
            let range = bytes
                .get(start..end)
                .ok_or(Error::Invalid("grep manifest range exceeds chunk"))?;

            for &byte in range {
                while matched > 0 && self.literal[matched] != byte {
                    matched = self.failure[matched - 1];
                }
                if self.literal[matched] == byte {
                    matched += 1;
                }
                position += 1;
                if matched == self.literal.len() {
                    return Ok(Some(position - self.literal.len() as u64));
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChunkRef;
    use std::collections::HashMap;

    fn manifest(parts: &[&[u8]]) -> (BodyManifest, HashMap<ChunkHash, Vec<u8>>) {
        let mut chunks = HashMap::new();
        let mut references = Vec::new();
        let mut body = Vec::new();
        for part in parts {
            let hash = ChunkHash::blake3(part);
            references.push(ChunkRef {
                chunk_hash: hash,
                chunk_offset: 0,
                logical_offset: body.len() as u64,
                length: part.len() as u64,
            });
            body.extend_from_slice(part);
            chunks.insert(hash, part.to_vec());
        }
        (
            BodyManifest::new(
                body.len() as u64,
                ChunkHash::blake3(&body),
                None,
                None,
                references,
            ),
            chunks,
        )
    }

    #[test]
    fn finds_literal_across_ranges_and_reads_shared_chunk_once() {
        let (first, mut chunks) = manifest(&[b"abcd", b"efgh"]);
        let (second, second_chunks) = manifest(&[b"abcd", b"ijkl"]);
        chunks.extend(second_chunks);
        let mut reads = HashMap::<ChunkHash, usize>::new();
        let mut scanner = RawBodyScanner::new(b"cde", RawSearchLimits::default()).unwrap();
        assert_eq!(
            scanner
                .search_manifest(&first, |hash| {
                    *reads.entry(*hash).or_default() += 1;
                    Ok(chunks[hash].clone())
                })
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            scanner
                .search_manifest(&second, |hash| {
                    *reads.entry(*hash).or_default() += 1;
                    Ok(chunks[hash].clone())
                })
                .unwrap(),
            None
        );
        assert_eq!(reads[&ChunkHash::blake3(b"abcd")], 1);
        assert_eq!(scanner.stats().unique_chunks_read, 3);
    }

    #[test]
    fn reports_work_and_cache_limits_explicitly() {
        let (manifest, chunks) = manifest(&[b"abcd"]);
        let limits = RawSearchLimits {
            max_cached_chunk_bytes: 3,
            ..RawSearchLimits::default()
        };
        let error = RawBodyScanner::new(b"z", limits)
            .unwrap()
            .search_manifest(&manifest, |hash| Ok(chunks[hash].clone()))
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("grep cached chunk bytes exceeds limit"));
    }
}
