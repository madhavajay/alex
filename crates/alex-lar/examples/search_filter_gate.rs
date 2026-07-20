//! Prototype portable page Bloom/trigram filters for raw archive search.
//!
//! Usage:
//! `cargo run -p alex-lar --release --example search_filter_gate -- --corpus DIR`

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use alex_lar::{ChunkerConfig, StreamingChunker};
use flate2::read::GzDecoder;
use serde_json::{json, Value};

const PAGE_BYTES: usize = 256 * 1024;
const BITS_PER_TRIGRAM: usize = 10;
const BLOOM_HASHES: usize = 7;
const QUERY_BYTES: usize = 12;
const QUERY_COUNT: usize = 128;

#[derive(Debug)]
struct Page {
    chunks: Vec<Vec<u8>>,
    trigrams: HashSet<u32>,
    bloom: Bloom,
}

#[derive(Debug)]
struct Bloom {
    bits: Vec<u8>,
    bit_count: usize,
}

impl Bloom {
    fn new(items: usize) -> Self {
        let bit_count = (items.max(1) * BITS_PER_TRIGRAM).next_multiple_of(8);
        Self {
            bits: vec![0; bit_count / 8],
            bit_count,
        }
    }

    fn insert(&mut self, trigram: u32) {
        for bit in bloom_positions(trigram, self.bit_count) {
            self.bits[bit / 8] |= 1 << (bit % 8);
        }
    }

    fn contains(&self, trigram: u32) -> bool {
        bloom_positions(trigram, self.bit_count)
            .all(|bit| self.bits[bit / 8] & (1 << (bit % 8)) != 0)
    }
}

fn bloom_positions(trigram: u32, bit_count: usize) -> impl Iterator<Item = usize> {
    let digest = blake3::hash(&trigram.to_le_bytes());
    let bytes = digest.as_bytes();
    let first = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
    let second = u64::from_le_bytes(bytes[8..16].try_into().unwrap()) | 1;
    (0..BLOOM_HASHES).map(move |index| {
        first.wrapping_add((index as u64).wrapping_mul(second)) as usize % bit_count
    })
}

fn trigram(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn query_trigrams(bytes: &[u8]) -> HashSet<u32> {
    bytes.windows(3).map(trigram).collect()
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
            .ok_or("valid artifact has no path")?;
        let stored = PathBuf::from(stored);
        let path = if stored.is_absolute() {
            stored
        } else {
            root.join(stored)
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
    Ok(bodies)
}

fn unique_chunks(bodies: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, String> {
    let mut unique = BTreeMap::<[u8; 32], Vec<u8>>::new();
    for body in bodies {
        let mut chunker =
            StreamingChunker::new(ChunkerConfig::default()).map_err(|error| error.to_string())?;
        let mut add = |chunk: &[u8]| -> alex_lar::Result<()> {
            let digest = *blake3::hash(chunk).as_bytes();
            if let Some(existing) = unique.get(&digest) {
                if existing != chunk {
                    return Err(alex_lar::Error::Invalid("BLAKE3 collision"));
                }
            } else {
                unique.insert(digest, chunk.to_vec());
            }
            Ok(())
        };
        chunker
            .push(body, &mut add)
            .map_err(|error| error.to_string())?;
        chunker
            .finish(&mut add)
            .map_err(|error| error.to_string())?;
    }
    Ok(unique.into_values().collect())
}

fn pages(chunks: Vec<Vec<u8>>) -> Vec<Page> {
    let mut groups = Vec::<Vec<Vec<u8>>>::new();
    let mut current = Vec::new();
    let mut current_bytes = 0;
    for chunk in chunks {
        if !current.is_empty() && current_bytes + chunk.len() > PAGE_BYTES {
            groups.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes += chunk.len();
        current.push(chunk);
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
        .into_iter()
        .map(|chunks| {
            let trigrams = chunks
                .iter()
                .flat_map(|chunk| chunk.windows(3).map(trigram))
                .collect::<HashSet<_>>();
            let mut bloom = Bloom::new(trigrams.len());
            for &value in &trigrams {
                bloom.insert(value);
            }
            Page {
                chunks,
                trigrams,
                bloom,
            }
        })
        .collect()
}

fn page_contains(page: &Page, literal: &[u8]) -> bool {
    page.chunks
        .iter()
        .any(|chunk| chunk.windows(literal.len()).any(|window| window == literal))
}

fn positive_queries(pages: &[Page]) -> Vec<Vec<u8>> {
    pages
        .iter()
        .flat_map(|page| page.chunks.iter())
        .filter(|chunk| chunk.len() >= QUERY_BYTES)
        .take(QUERY_COUNT)
        .enumerate()
        .map(|(index, chunk)| {
            let offset = index.wrapping_mul(997) % (chunk.len() - QUERY_BYTES + 1);
            chunk[offset..offset + QUERY_BYTES].to_vec()
        })
        .collect()
}

fn negative_queries(pages: &[Page]) -> Vec<Vec<u8>> {
    let mut output = Vec::new();
    let mut candidate = 0_u64;
    while output.len() < QUERY_COUNT {
        let digest = blake3::hash(format!("negative-search-query-{candidate}").as_bytes());
        candidate += 1;
        let query = digest.as_bytes()[..QUERY_BYTES].to_vec();
        if !pages.iter().any(|page| page_contains(page, &query)) {
            output.push(query);
        }
    }
    output
}

fn candidate_counts(pages: &[Page], queries: &[Vec<u8>], bloom: bool) -> (u64, u64) {
    let mut candidates = 0_u64;
    let mut actual = 0_u64;
    for query in queries {
        let trigrams = query_trigrams(query);
        for page in pages {
            let matches_filter = if bloom {
                trigrams.iter().all(|value| page.bloom.contains(*value))
            } else {
                trigrams.iter().all(|value| page.trigrams.contains(value))
            };
            if matches_filter {
                candidates += 1;
            }
            if page_contains(page, query) {
                actual += 1;
                assert!(matches_filter, "filter produced a false negative");
            }
        }
    }
    (candidates, actual)
}

fn parse_args() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    let mut corpus = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--corpus" => corpus = Some(PathBuf::from(args.next().ok_or("--corpus needs DIR")?)),
            "--help" | "-h" => return Err("usage: search_filter_gate --corpus DIR".into()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    corpus.ok_or("--corpus is required".into())
}

fn run() -> Result<(), String> {
    let corpus = parse_args()?;
    let bodies = read_corpus(&corpus)?;
    let pages = pages(unique_chunks(&bodies)?);
    if pages.is_empty() {
        return Err("corpus produced no searchable pages".into());
    }
    let positives = positive_queries(&pages);
    let negatives = negative_queries(&pages);
    let (bloom_positive_candidates, positive_actual) = candidate_counts(&pages, &positives, true);
    let (exact_positive_candidates, _) = candidate_counts(&pages, &positives, false);
    let (bloom_negative_candidates, negative_actual) = candidate_counts(&pages, &negatives, true);
    let (exact_negative_candidates, _) = candidate_counts(&pages, &negatives, false);
    assert_eq!(negative_actual, 0);

    let bloom_bytes = pages
        .iter()
        .map(|page| page.bloom.bits.len())
        .sum::<usize>();
    let mut exact_postings = HashMap::<u32, Vec<u64>>::new();
    let words = pages.len().div_ceil(64);
    for (page_index, page) in pages.iter().enumerate() {
        for &value in &page.trigrams {
            exact_postings
                .entry(value)
                .or_insert_with(|| vec![0; words])[page_index / 64] |= 1 << (page_index % 64);
        }
    }
    let exact_bytes_lower_bound = exact_postings.len() * 3
        + exact_postings
            .values()
            .map(|bits| bits.len() * 8)
            .sum::<usize>();
    let unique_bytes = pages
        .iter()
        .flat_map(|page| &page.chunks)
        .map(Vec::len)
        .sum::<usize>();
    let comparisons = (pages.len() * negatives.len()) as f64;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": "alex-lar-search-filter-gate-v1",
            "corpus": corpus,
            "unique_chunk_bytes": unique_bytes,
            "pages": pages.len(),
            "page_target_bytes": PAGE_BYTES,
            "positive_queries": positives.len(),
            "negative_queries": negatives.len(),
            "positive_actual_pages": positive_actual,
            "bloom": {
                "bits_per_distinct_page_trigram": BITS_PER_TRIGRAM,
                "hashes": BLOOM_HASHES,
                "bytes": bloom_bytes,
                "bytes_over_unique_chunks": bloom_bytes as f64 / unique_bytes as f64,
                "positive_candidate_pages": bloom_positive_candidates,
                "negative_false_positive_pages": bloom_negative_candidates,
                "negative_page_false_positive_rate": bloom_negative_candidates as f64 / comparisons,
            },
            "exact_trigram": {
                "portable_bytes_lower_bound": exact_bytes_lower_bound,
                "bytes_over_unique_chunks_lower_bound": exact_bytes_lower_bound as f64 / unique_bytes as f64,
                "positive_candidate_pages": exact_positive_candidates,
                "negative_candidate_pages": exact_negative_candidates,
            },
            "privacy": [
                "exact trigram postings reveal page membership for every indexed trigram",
                "Bloom filters still support probabilistic membership tests and equality correlation",
                "filters must inherit archive permissions and must not contain redacted pre-capture values",
                "neither filter accelerates literals shorter than three bytes"
            ]
        }))
        .unwrap()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("search-filter benchmark: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloom_and_exact_filters_have_no_false_negatives() {
        let chunks = vec![
            b"alpha searchable literal omega".to_vec(),
            b"another completely separate chunk".to_vec(),
        ];
        let pages = pages(chunks);
        let query = vec![b"searchable".to_vec()];
        assert_eq!(candidate_counts(&pages, &query, true).1, 1);
        assert_eq!(candidate_counts(&pages, &query, false).1, 1);
    }

    #[test]
    fn bloom_is_deterministic() {
        let mut left = Bloom::new(100);
        let mut right = Bloom::new(100);
        for value in [trigram(b"abc"), trigram(b"xyz"), trigram(b"123")] {
            left.insert(value);
            right.insert(value);
        }
        assert_eq!(left.bits, right.bits);
        assert!(left.contains(trigram(b"abc")));
    }
}
