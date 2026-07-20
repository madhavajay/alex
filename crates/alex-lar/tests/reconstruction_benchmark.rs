//! Manual release-mode throughput gate for a large, incompressible body.
//!
//! Run with:
//! `cargo test -p alex-lar --test reconstruction_benchmark --release -- --ignored --nocapture`

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alex_lar::{ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, Limits};

const BODY_BYTES: usize = 64 * 1024 * 1024;
const SAMPLES: usize = 7;

fn temp_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "alex-lar-reconstruction-benchmark-{}-{nonce}.lar",
        std::process::id()
    ))
}

fn pseudo_random_body() -> Vec<u8> {
    let mut body = Vec::with_capacity(BODY_BYTES);
    let mut counter = 0u64;
    while body.len() < BODY_BYTES {
        body.extend_from_slice(blake3::hash(&counter.to_le_bytes()).as_bytes());
        counter += 1;
    }
    body.truncate(BODY_BYTES);
    body
}

fn percentile(samples: &[Duration], percentile: usize) -> Duration {
    let mut values = samples.to_vec();
    values.sort_unstable();
    values[((values.len() - 1) * percentile).div_ceil(100)]
}

#[test]
#[ignore = "manual release large-body benchmark"]
fn large_body_reconstruction_throughput() {
    let path = temp_path();
    let body = pseudo_random_body();
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let mut limits = Limits::default();
    limits.max_body_length = BODY_BYTES as u64;
    let mut writer = ArchiveWriter::create(
        file,
        FileHeader::standalone([0x6b; 16], 1, b"large-body-benchmark".to_vec()),
        ChunkerConfig::default(),
        limits.clone(),
    )
    .unwrap();
    let manifest = writer.append_body(&body).unwrap();
    writer.seal().unwrap();
    writer.get_ref().sync_all().unwrap();
    let archive_bytes = writer.get_mut().seek(SeekFrom::End(0)).unwrap();
    drop(writer);

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let mut reader = ArchiveReader::open(File::open(&path).unwrap(), limits.clone()).unwrap();
        let started = Instant::now();
        let written = reader.write_body(&manifest, std::io::sink()).unwrap();
        samples.push(started.elapsed());
        assert_eq!(written, BODY_BYTES as u64);
    }
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let mib = BODY_BYTES as f64 / (1024.0 * 1024.0);
    println!(
        "LAR_RECONSTRUCTION_BENCHMARK body_bytes={BODY_BYTES} archive_bytes={archive_bytes} \
         p50_ms={:.3} p95_ms={:.3} p50_mib_s={:.1} p95_mib_s={:.1}",
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0,
        mib / p50.as_secs_f64(),
        mib / p95.as_secs_f64(),
    );
    std::fs::remove_file(path).unwrap();
}
