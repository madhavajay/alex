use crate::{BodyManifest, ChunkHash, Error, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use tempfile::NamedTempFile;

const SPILL_BUCKETS: u64 = 4096;
const SPILL_BUCKET_BYTES: u64 = SPILL_BUCKETS * 8;
const SPILL_RECORD_HEADER_BYTES: u64 = 8 + 1 + 32 + 8;
// Conservative accounting for the HashMap bucket, hash key, Vec header, and
// cached-entry metadata. This prevents tiny chunks from bypassing the nominal
// RAM budget through per-entry overhead.
const CACHE_ENTRY_OVERHEAD: u64 = 128;

/// Hard bounds for an exact raw-byte search. `max_cached_chunk_bytes` is a RAM
/// budget, not a corpus-size limit: verified chunks beyond it spill to an
/// auto-cleaned temporary file and remain reusable without decompression.
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
    pub cached_chunk_bytes_peak: u64,
    pub spilled_chunk_bytes: u64,
    pub spill_chunk_reads: u64,
    pub chunk_cache_evictions: u64,
}

/// Searches manifests while retaining each verified decompressed chunk once
/// in a bounded memory/disk store. Create one scanner per physical archive (or
/// one for a live catalog whose chunk hashes have one authoritative location).
pub struct RawBodyScanner {
    literal: Vec<u8>,
    failure: Vec<usize>,
    limits: RawSearchLimits,
    stats: RawSearchStats,
    chunks: ChunkStore,
}

struct CachedChunk {
    bytes: Vec<u8>,
    last_used: u64,
    spilled: bool,
}

/// A bounded hot cache backed by a temporary append-only chunk file. The
/// bucket heads and collision chains live in that file as well, so the number
/// of unique corpus chunks does not grow an in-memory hash index.
struct ChunkStore {
    memory: HashMap<ChunkHash, CachedChunk>,
    memory_bytes: u64,
    memory_charge: u64,
    memory_limit: u64,
    use_clock: u64,
    scratch: Vec<u8>,
    spill: NamedTempFile,
}

impl ChunkStore {
    fn new(memory_limit: u64) -> Result<Self> {
        let spill = tempfile::Builder::new()
            .prefix("alex-lar-grep-")
            .suffix(".spill")
            .tempfile()
            .map_err(Error::Io)?;
        spill
            .as_file()
            .set_len(SPILL_BUCKET_BYTES)
            .map_err(Error::Io)?;
        Ok(Self {
            memory: HashMap::new(),
            memory_bytes: 0,
            memory_charge: 0,
            memory_limit,
            use_clock: 0,
            scratch: Vec::new(),
            spill,
        })
    }

    fn get_or_read<'a, F>(
        &'a mut self,
        hash: &ChunkHash,
        stats: &mut RawSearchStats,
        read_chunk: &mut F,
    ) -> Result<&'a [u8]>
    where
        F: FnMut(&ChunkHash) -> Result<Vec<u8>>,
    {
        self.use_clock = self.use_clock.saturating_add(1);
        if self.memory.contains_key(hash) {
            let cached = self
                .memory
                .get_mut(hash)
                .ok_or(Error::Invalid("grep cache lookup lost its chunk"))?;
            cached.last_used = self.use_clock;
            return Ok(&cached.bytes);
        }

        if let Some(bytes) = self.read_spilled(hash)? {
            stats.spill_chunk_reads = stats.spill_chunk_reads.saturating_add(1);
            return self.retain_or_scratch(*hash, bytes, true, stats);
        }

        let bytes = read_chunk(hash)?;
        if ChunkHash::blake3(&bytes) != *hash {
            return Err(Error::Invalid("grep chunk hash mismatch"));
        }
        stats.unique_chunks_read = stats.unique_chunks_read.saturating_add(1);
        stats.decompressed_chunk_bytes = stats
            .decompressed_chunk_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Limit {
                what: "grep decompressed chunk bytes",
                actual: u64::MAX,
                limit: u64::MAX,
            })?;
        self.retain_or_scratch(*hash, bytes, false, stats)
    }

    fn retain_or_scratch<'a>(
        &'a mut self,
        hash: ChunkHash,
        bytes: Vec<u8>,
        already_spilled: bool,
        stats: &mut RawSearchStats,
    ) -> Result<&'a [u8]> {
        let length = bytes.len() as u64;
        let charge = length
            .checked_add(CACHE_ENTRY_OVERHEAD)
            .ok_or(Error::Invalid("grep cache entry charge overflow"))?;
        if length == 0 || charge > self.memory_limit {
            if !already_spilled {
                self.spill_chunk(hash, &bytes, stats)?;
            }
            self.scratch = bytes;
            return Ok(&self.scratch);
        }
        while self.memory_charge.saturating_add(charge) > self.memory_limit {
            let Some(oldest) = self
                .memory
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(hash, _)| *hash)
            else {
                break;
            };
            let removed = self
                .memory
                .remove(&oldest)
                .ok_or(Error::Invalid("grep cache eviction lost its chunk"))?;
            if !removed.spilled {
                self.spill_chunk(oldest, &removed.bytes, stats)?;
            }
            self.memory_bytes = self.memory_bytes.saturating_sub(removed.bytes.len() as u64);
            self.memory_charge = self
                .memory_charge
                .saturating_sub((removed.bytes.len() as u64).saturating_add(CACHE_ENTRY_OVERHEAD));
            stats.chunk_cache_evictions = stats.chunk_cache_evictions.saturating_add(1);
        }
        self.memory_bytes = self
            .memory_bytes
            .checked_add(length)
            .ok_or(Error::Invalid("grep cache byte count overflow"))?;
        self.memory_charge = self
            .memory_charge
            .checked_add(charge)
            .ok_or(Error::Invalid("grep cache charge overflow"))?;
        stats.cached_chunk_bytes_peak = stats.cached_chunk_bytes_peak.max(self.memory_bytes);
        self.memory.insert(
            hash,
            CachedChunk {
                bytes,
                last_used: self.use_clock,
                spilled: already_spilled,
            },
        );
        Ok(&self
            .memory
            .get(&hash)
            .ok_or(Error::Invalid("grep cache insertion lost its chunk"))?
            .bytes)
    }

    fn spill_chunk(
        &mut self,
        hash: ChunkHash,
        bytes: &[u8],
        stats: &mut RawSearchStats,
    ) -> Result<()> {
        self.append_spilled(hash, bytes)?;
        stats.spilled_chunk_bytes = stats
            .spilled_chunk_bytes
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Limit {
                what: "grep spilled chunk bytes",
                actual: u64::MAX,
                limit: u64::MAX,
            })?;
        Ok(())
    }

    fn bucket(hash: &ChunkHash) -> u64 {
        (((hash.digest[0] as u16) << 4) | ((hash.digest[1] as u16) >> 4)) as u64
    }

    fn read_spilled(&mut self, hash: &ChunkHash) -> Result<Option<Vec<u8>>> {
        let file = self.spill.as_file_mut();
        let bucket_offset = Self::bucket(hash) * 8;
        file.seek(SeekFrom::Start(bucket_offset))?;
        let mut head = [0; 8];
        file.read_exact(&mut head)?;
        let mut record_offset = u64::from_le_bytes(head);
        let spill_length = file.metadata()?.len();
        while record_offset != 0 {
            if record_offset < SPILL_BUCKET_BYTES
                || record_offset.saturating_add(SPILL_RECORD_HEADER_BYTES) > spill_length
            {
                return Err(Error::Invalid("grep spill index points outside its file"));
            }
            file.seek(SeekFrom::Start(record_offset))?;
            let mut header = [0; SPILL_RECORD_HEADER_BYTES as usize];
            file.read_exact(&mut header)?;
            let next = u64::from_le_bytes(header[..8].try_into().unwrap());
            let algorithm = header[8];
            let digest: [u8; 32] = header[9..41].try_into().unwrap();
            let length = u64::from_le_bytes(header[41..49].try_into().unwrap());
            let end = record_offset
                .checked_add(SPILL_RECORD_HEADER_BYTES)
                .and_then(|value| value.checked_add(length))
                .ok_or(Error::Invalid("grep spill record range overflow"))?;
            if end > spill_length {
                return Err(Error::Invalid("grep spill record exceeds its file"));
            }
            if algorithm == hash.algorithm as u8 && digest == hash.digest {
                let size = usize::try_from(length).map_err(|_| Error::Limit {
                    what: "grep spilled chunk address space",
                    actual: length,
                    limit: usize::MAX as u64,
                })?;
                let mut bytes = vec![0; size];
                file.read_exact(&mut bytes)?;
                if ChunkHash::blake3(&bytes) != *hash {
                    return Err(Error::Invalid("grep spilled chunk hash mismatch"));
                }
                return Ok(Some(bytes));
            }
            record_offset = next;
        }
        Ok(None)
    }

    fn append_spilled(&mut self, hash: ChunkHash, bytes: &[u8]) -> Result<()> {
        let file = self.spill.as_file_mut();
        let bucket_offset = Self::bucket(&hash) * 8;
        file.seek(SeekFrom::Start(bucket_offset))?;
        let mut head = [0; 8];
        file.read_exact(&mut head)?;
        let previous_head = u64::from_le_bytes(head);
        let record_offset = file.seek(SeekFrom::End(0))?;
        file.write_all(&previous_head.to_le_bytes())?;
        file.write_all(&[hash.algorithm as u8])?;
        file.write_all(&hash.digest)?;
        file.write_all(&(bytes.len() as u64).to_le_bytes())?;
        file.write_all(bytes)?;
        file.seek(SeekFrom::Start(bucket_offset))?;
        file.write_all(&record_offset.to_le_bytes())?;
        Ok(())
    }

    #[cfg(test)]
    fn spill_path(&self) -> &std::path::Path {
        self.spill.path()
    }
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
            chunks: ChunkStore::new(limits.max_cached_chunk_bytes)?,
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

            let bytes =
                self.chunks
                    .get_or_read(&reference.chunk_hash, &mut self.stats, &mut read_chunk)?;
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
    fn bounded_cache_spills_and_reuses_evicted_chunks_without_decompression() {
        let (first, mut chunks) = manifest(&[b"aaaa", b"bbbb", b"cccc"]);
        let (second, second_chunks) = manifest(&[b"aaaa", b"dddd"]);
        chunks.extend(second_chunks);
        let mut reads = HashMap::<ChunkHash, usize>::new();
        let limits = RawSearchLimits {
            max_cached_chunk_bytes: CACHE_ENTRY_OVERHEAD + 4,
            ..RawSearchLimits::default()
        };
        let mut scanner = RawBodyScanner::new(b"not present", limits).unwrap();
        for body in [&first, &second] {
            assert_eq!(
                scanner
                    .search_manifest(body, |hash| {
                        *reads.entry(*hash).or_default() += 1;
                        Ok(chunks[hash].clone())
                    })
                    .unwrap(),
                None
            );
        }
        assert_eq!(reads[&ChunkHash::blake3(b"aaaa")], 1);
        assert_eq!(scanner.stats.unique_chunks_read, 4);
        assert_eq!(scanner.stats.decompressed_chunk_bytes, 16);
        // The final hot chunk never needs to hit disk; only evicted chunks do.
        assert_eq!(scanner.stats.spilled_chunk_bytes, 12);
        assert_eq!(scanner.stats.spill_chunk_reads, 1);
        assert!(scanner.stats.chunk_cache_evictions >= 3);
        assert!(scanner.stats.cached_chunk_bytes_peak <= 4);
        assert!(scanner.chunks.memory_bytes <= 4);
        assert!(scanner.chunks.memory_charge <= CACHE_ENTRY_OVERHEAD + 4);
    }

    #[test]
    fn tiny_cache_preserves_cross_range_kmp_matching() {
        let (body, chunks) = manifest(&[b"ab", b"cd"]);
        let limits = RawSearchLimits {
            max_cached_chunk_bytes: 1,
            ..RawSearchLimits::default()
        };
        let mut scanner = RawBodyScanner::new(b"bc", limits).unwrap();
        assert_eq!(
            scanner
                .search_manifest(&body, |hash| Ok(chunks[hash].clone()))
                .unwrap(),
            Some(1)
        );
        assert_eq!(scanner.stats.unique_chunks_read, 2);
        assert_eq!(scanner.stats.cached_chunk_bytes_peak, 0);
    }

    #[test]
    fn logical_work_limit_remains_independent_of_cache_budget() {
        let (body, chunks) = manifest(&[b"abcd"]);
        let limits = RawSearchLimits {
            max_cached_chunk_bytes: 1,
            max_logical_bytes: 3,
            ..RawSearchLimits::default()
        };
        let error = RawBodyScanner::new(b"z", limits)
            .unwrap()
            .search_manifest(&body, |hash| Ok(chunks[hash].clone()))
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("grep logical bytes exceeds limit"));
    }

    #[test]
    fn dropping_scanner_cleans_up_spill_file() {
        let path = {
            let (body, chunks) = manifest(&[b"spill me"]);
            let mut scanner = RawBodyScanner::new(
                b"absent",
                RawSearchLimits {
                    max_cached_chunk_bytes: 0,
                    ..RawSearchLimits::default()
                },
            )
            .unwrap();
            let path = scanner.chunks.spill_path().to_path_buf();
            assert!(path.exists());
            assert_eq!(
                scanner
                    .search_manifest(&body, |hash| Ok(chunks[hash].clone()))
                    .unwrap(),
                None
            );
            path
        };
        assert!(!path.exists());
    }
}
