//! Measure candidate chunkers and zstd settings on a generated or anonymized
//! legacy corpus without writing a LAR archive.
//!
//! Usage:
//!
//! ```text
//! cargo run -p alex-lar --release --example design_gate -- \
//!   --corpus /path/to/holdout-corpus \
//!   [--dictionary-corpus /path/to/distinct-training-corpus] [--json]
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use alex_lar::{ChunkerConfig, StreamingChunker};
use flate2::read::GzDecoder;
use serde_json::{json, Value};

const MIN: usize = 512;
const TARGET: usize = 2 * 1024;
const MAX: usize = 8 * 1024;
const DICTIONARY_BYTES: usize = 32 * 1024;
const MAX_DICTIONARY_SAMPLES: usize = 4_096;
const MAX_DICTIONARY_SAMPLE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
enum Strategy {
    Fixed,
    Gear,
    GearLarge,
    FastCdc,
    Buzhash,
}

impl Strategy {
    const ALL: [Self; 5] = [
        Self::Fixed,
        Self::Gear,
        Self::GearLarge,
        Self::FastCdc,
        Self::Buzhash,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Fixed => "fixed-2k",
            Self::Gear => "gear-512-2k-8k",
            Self::GearLarge => "gear-2k-8k-32k",
            Self::FastCdc => "fastcdc-normalized-512-2k-8k",
            Self::Buzhash => "buzhash64-512-2k-8k",
        }
    }

    fn split(self, body: &[u8]) -> Result<Vec<&[u8]>, String> {
        match self {
            Self::Fixed => Ok(body.chunks(TARGET).collect()),
            Self::Gear => split_gear(body, MIN, TARGET, MAX),
            Self::GearLarge => split_gear(body, 2 * 1024, 8 * 1024, 32 * 1024),
            Self::FastCdc => Ok(split_fastcdc(body)),
            Self::Buzhash => Ok(split_buzhash(body)),
        }
    }

    #[cfg(test)]
    fn bounds(self) -> (usize, usize) {
        match self {
            Self::GearLarge => (2 * 1024, 32 * 1024),
            _ => (MIN, MAX),
        }
    }
}

#[derive(Default)]
struct UniqueChunks {
    chunks: HashMap<[u8; 32], Vec<u8>>,
    logical_bytes: u64,
    chunk_references: u64,
}

impl UniqueChunks {
    fn insert(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.logical_bytes = self
            .logical_bytes
            .checked_add(chunk.len() as u64)
            .ok_or("logical byte counter overflow")?;
        self.chunk_references += 1;
        let digest = *blake3::hash(chunk).as_bytes();
        match self.chunks.entry(digest) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(chunk.to_vec());
            }
            std::collections::hash_map::Entry::Occupied(entry) if entry.get() != chunk => {
                return Err("BLAKE3 collision while measuring corpus".into());
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
        Ok(())
    }

    fn unique_bytes(&self) -> u64 {
        self.chunks.values().map(|chunk| chunk.len() as u64).sum()
    }

    fn ordered_chunks(&self) -> Vec<&[u8]> {
        let mut chunks = self.chunks.iter().collect::<Vec<_>>();
        chunks.sort_by_key(|(digest, _)| *digest);
        chunks
            .into_iter()
            .map(|(_, bytes)| bytes.as_slice())
            .collect()
    }
}

fn split_gear(
    body: &[u8],
    min_size: usize,
    target_size: usize,
    max_size: usize,
) -> Result<Vec<&[u8]>, String> {
    let mut chunker = StreamingChunker::new(ChunkerConfig {
        min_size,
        target_size,
        max_size,
    })
    .map_err(|error| error.to_string())?;
    let mut lengths = Vec::new();
    chunker
        .push(body, |chunk| {
            lengths.push(chunk.len());
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    chunker
        .finish(|chunk| {
            lengths.push(chunk.len());
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    let mut chunks = Vec::with_capacity(lengths.len());
    let mut offset = 0;
    for length in lengths {
        chunks.push(&body[offset..offset + length]);
        offset += length;
    }
    Ok(chunks)
}

// A compact FastCDC design prototype: Gear hashing plus the normalization
// phase's stricter pre-target and looser post-target masks. It intentionally
// lives in the benchmark rather than the format implementation until corpus
// measurements justify a format-visible algorithm change.
fn split_fastcdc(body: &[u8]) -> Vec<&[u8]> {
    let early_mask = (TARGET.next_power_of_two() * 2 - 1) as u64;
    let late_mask = (TARGET.next_power_of_two() / 2 - 1) as u64;
    split_with_boundary(body, |chunk, index, hash| {
        let length = index + 1;
        let mask = if length < TARGET {
            early_mask
        } else {
            late_mask
        };
        length >= MAX || (length >= MIN && hash & mask == 0) || length == chunk.len()
    })
}

fn split_with_boundary<F>(body: &[u8], mut boundary: F) -> Vec<&[u8]>
where
    F: FnMut(&[u8], usize, u64) -> bool,
{
    let mut output = Vec::new();
    let mut start = 0;
    while start < body.len() {
        let end_limit = body.len().min(start + MAX);
        let chunk = &body[start..end_limit];
        let mut hash = 0_u64;
        let mut end = chunk.len();
        for (index, &byte) in chunk.iter().enumerate() {
            hash = hash.rotate_left(1).wrapping_add(gear(byte));
            if boundary(chunk, index, hash) {
                end = index + 1;
                break;
            }
        }
        output.push(&body[start..start + end]);
        start += end;
    }
    output
}

fn split_buzhash(body: &[u8]) -> Vec<&[u8]> {
    const WINDOW: usize = 64;
    let mask = (TARGET.next_power_of_two() - 1) as u64;
    let mut output = Vec::new();
    let mut start = 0;
    while start < body.len() {
        let limit = body.len().min(start + MAX);
        let chunk = &body[start..limit];
        let mut hash = 0_u64;
        let mut end = chunk.len();
        for (index, &byte) in chunk.iter().enumerate() {
            hash = hash.rotate_left(1) ^ gear(byte);
            if index >= WINDOW {
                hash ^= gear(chunk[index - WINDOW]).rotate_left(WINDOW as u32);
            }
            let length = index + 1;
            if length >= MAX || (length >= MIN && hash & mask == 0) {
                end = length;
                break;
            }
        }
        output.push(&body[start..start + end]);
        start += end;
    }
    output
}

fn gear(byte: u8) -> u64 {
    let mut value = (byte as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn read_corpus(root: &Path) -> Result<Vec<Vec<u8>>, String> {
    let manifest_path = root.join("corpus-manifest.json");
    let manifest: Value = serde_json::from_reader(
        File::open(&manifest_path)
            .map_err(|error| format!("opening {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("parsing {}: {error}", manifest_path.display()))?;
    let artifacts = manifest["artifacts"]
        .as_array()
        .ok_or("corpus manifest has no artifacts array")?;
    let mut bodies = Vec::new();
    for artifact in artifacts {
        if artifact["state"].as_str() != Some("valid") {
            continue;
        }
        let stored = artifact["path"]
            .as_str()
            .ok_or("valid corpus artifact has no path")?;
        let candidate = PathBuf::from(stored);
        let path = if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        };
        let mut decoder = GzDecoder::new(
            File::open(&path).map_err(|error| format!("opening {}: {error}", path.display()))?,
        );
        let mut body = Vec::new();
        decoder
            .read_to_end(&mut body)
            .map_err(|error| format!("decompressing {}: {error}", path.display()))?;
        if artifact["length"].as_u64() != Some(body.len() as u64) {
            return Err(format!("{} length does not match manifest", path.display()));
        }
        bodies.push(body);
    }
    if bodies.is_empty() {
        return Err("corpus contains no valid artifacts".into());
    }
    Ok(bodies)
}

fn compressed_bytes(
    chunks: &[&[u8]],
    level: i32,
    dictionary: Option<&[u8]>,
) -> Result<u64, String> {
    let mut compressor = match dictionary {
        Some(dictionary) => zstd::bulk::Compressor::with_dictionary(level, dictionary),
        None => zstd::bulk::Compressor::new(level),
    }
    .map_err(|error| format!("creating zstd compressor: {error}"))?;
    chunks.iter().try_fold(0_u64, |total, chunk| {
        let compressed = compressor
            .compress(chunk)
            .map_err(|error| format!("compressing chunk: {error}"))?;
        total
            .checked_add(compressed.len() as u64)
            .ok_or_else(|| "compressed byte counter overflow".into())
    })
}

fn train_dictionary(chunks: &[&[u8]]) -> Result<Option<Vec<u8>>, String> {
    let mut samples = Vec::new();
    let mut bytes = 0_usize;
    for chunk in chunks.iter().filter(|chunk| chunk.len() >= 32) {
        if samples.len() >= MAX_DICTIONARY_SAMPLES
            || bytes.saturating_add(chunk.len()) > MAX_DICTIONARY_SAMPLE_BYTES
        {
            break;
        }
        samples.push(*chunk);
        bytes += chunk.len();
    }
    if samples.len() < 8 || bytes < DICTIONARY_BYTES * 2 {
        return Ok(None);
    }
    zstd::dict::from_samples(&samples, DICTIONARY_BYTES)
        .map(Some)
        .map_err(|error| format!("training zstd dictionary: {error}"))
}

fn parse_args() -> Result<(PathBuf, Option<PathBuf>, bool), String> {
    let mut args = std::env::args().skip(1);
    let mut corpus = None;
    let mut dictionary_corpus = None;
    let mut json_output = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--corpus" => {
                corpus = Some(PathBuf::from(
                    args.next().ok_or("--corpus requires a directory")?,
                ));
            }
            "--dictionary-corpus" => {
                dictionary_corpus = Some(PathBuf::from(
                    args.next()
                        .ok_or("--dictionary-corpus requires a directory")?,
                ));
            }
            "--json" => json_output = true,
            "--help" | "-h" => {
                return Err(
                    "usage: design_gate --corpus <holdout-corpus> [--dictionary-corpus <distinct-training-corpus>] [--json]".into(),
                );
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok((
        corpus.ok_or("--corpus is required")?,
        dictionary_corpus,
        json_output,
    ))
}

fn run() -> Result<(), String> {
    let (corpus, dictionary_corpus, json_output) = parse_args()?;
    let bodies = read_corpus(&corpus)?;
    let dictionary_bodies = dictionary_corpus.as_deref().map(read_corpus).transpose()?;
    let source_bytes = bodies.iter().map(|body| body.len() as u64).sum::<u64>();
    let mut results = Vec::new();
    for strategy in Strategy::ALL {
        let started = Instant::now();
        let mut unique = UniqueChunks::default();
        for body in &bodies {
            for chunk in strategy.split(body)? {
                unique.insert(chunk)?;
            }
        }
        let chunking_ns = started.elapsed().as_nanos();
        let chunks = unique.ordered_chunks();
        let dictionary = if let Some(training_bodies) = &dictionary_bodies {
            let mut training = UniqueChunks::default();
            for body in training_bodies {
                for chunk in strategy.split(body)? {
                    training.insert(chunk)?;
                }
            }
            train_dictionary(&training.ordered_chunks())?
        } else {
            None
        };
        let zstd = [1, 3, 7]
            .into_iter()
            .map(|level| {
                compressed_bytes(&chunks, level, None)
                    .map(|bytes| (level.to_string(), Value::from(bytes)))
            })
            .collect::<Result<serde_json::Map<String, Value>, String>>()?;
        let dictionary_bytes = dictionary
            .as_deref()
            .map(|dictionary| {
                compressed_bytes(&chunks, 3, Some(dictionary))
                    .map(|compressed| compressed + dictionary.len() as u64)
            })
            .transpose()?;
        results.push(json!({
            "strategy": strategy.name(),
            "source_bytes": unique.logical_bytes,
            "chunk_references": unique.chunk_references,
            "unique_chunks": unique.chunks.len(),
            "unique_bytes": unique.unique_bytes(),
            "deduplicated_fraction": 1.0 - unique.unique_bytes() as f64 / unique.logical_bytes as f64,
            "chunking_ns": u64::try_from(chunking_ns).unwrap_or(u64::MAX),
            "zstd_unique_chunk_bytes": zstd,
            "zstd_level_3_with_32k_dictionary_and_dictionary_bytes": dictionary_bytes,
            "trained_dictionary_bytes": dictionary.as_ref().map(Vec::len),
        }));
    }
    let report = json!({
        "schema": "alex-lar-design-gate-v1",
        "corpus": corpus,
        "dictionary_training_corpus": dictionary_corpus,
        "artifacts": bodies.len(),
        "source_bytes": source_bytes,
        "bounds": {"min": MIN, "target": TARGET, "max": MAX},
        "results": results,
        "notes": [
            "compressed sizes cover holdout unique chunk payloads plus dictionary bytes, not LAR framing or metadata",
            "dictionary results are emitted only when a distinct --dictionary-corpus is supplied",
            "run release builds repeatedly on the same idle hardware before making a format decision"
        ]
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!(
            "corpus: {} artifacts, {} source bytes",
            bodies.len(),
            source_bytes
        );
        for result in report["results"].as_array().unwrap() {
            println!(
                "{:<36} unique {:>10} ({:>6.2}% removed), zstd-3 {:>10}, dict {:>10}",
                result["strategy"].as_str().unwrap(),
                result["unique_bytes"].as_u64().unwrap(),
                result["deduplicated_fraction"].as_f64().unwrap() * 100.0,
                result["zstd_unique_chunk_bytes"]["3"].as_u64().unwrap(),
                result["zstd_level_3_with_32k_dictionary_and_dictionary_bytes"]
                    .as_u64()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into()),
            );
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("design-gate benchmark: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_strategy_reconstructs_input_and_obeys_bounds() {
        let input = (0..100_003)
            .map(|index| ((index * 31 + index / 97) % 251) as u8)
            .collect::<Vec<_>>();
        for strategy in Strategy::ALL {
            let (minimum, maximum) = strategy.bounds();
            let chunks = strategy.split(&input).unwrap();
            assert!(!chunks.is_empty(), "{}", strategy.name());
            assert_eq!(chunks.concat(), input, "{}", strategy.name());
            for chunk in chunks.iter().take(chunks.len().saturating_sub(1)) {
                assert!(chunk.len() <= maximum, "{}", strategy.name());
                if !matches!(strategy, Strategy::Fixed) {
                    assert!(chunk.len() >= minimum, "{}", strategy.name());
                }
            }
        }
    }

    #[test]
    fn duplicate_chunks_are_counted_once_and_collisions_are_verified() {
        let mut chunks = UniqueChunks::default();
        chunks.insert(b"same").unwrap();
        chunks.insert(b"same").unwrap();
        chunks.insert(b"different").unwrap();
        assert_eq!(chunks.chunk_references, 3);
        assert_eq!(chunks.chunks.len(), 2);
        assert_eq!(chunks.unique_bytes(), 13);
    }
}
