use alex_lar::{ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, Limits, OpenPath};
use std::fs::File;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct TempArchive(PathBuf);

impl TempArchive {
    fn write(label: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!(
            "alex-lar-index-benchmark-{label}-{}-{}.lar",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).unwrap();
        File::open(&path).unwrap().sync_all().unwrap();
        Self(path)
    }
}

impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn build(sealed: bool) -> (Vec<u8>, Vec<alex_lar::ManifestId>) {
    let mut writer = ArchiveWriter::create(
        Cursor::new(Vec::new()),
        FileHeader::standalone([0x91; 16], 1, b"index-benchmark".to_vec()),
        ChunkerConfig::default(),
        Limits::default(),
    )
    .unwrap();
    let mut manifests = Vec::with_capacity(2_000);
    for index in 0..2_000u32 {
        let mut body = vec![b'x'; 1_024];
        body[..4].copy_from_slice(&index.to_le_bytes());
        manifests.push(writer.append_body(&body).unwrap());
    }
    if sealed {
        writer.seal().unwrap();
    }
    (writer.into_inner().unwrap().into_inner(), manifests)
}

struct FirstByteSink {
    started: Instant,
    first_byte: Option<Duration>,
    bytes: u64,
}

impl Write for FirstByteSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !bytes.is_empty() && self.first_byte.is_none() {
            self.first_byte = Some(self.started.elapsed());
        }
        self.bytes += bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn evict_file_cache(path: &Path) {
    use std::os::fd::AsRawFd;
    let file = File::open(path).unwrap();
    let result = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    assert_eq!(result, 0, "posix_fadvise(DONTNEED) failed: {result}");
}

#[cfg(target_os = "macos")]
fn evict_file_cache(path: &Path) {
    use std::os::fd::AsRawFd;
    let file = File::open(path).unwrap();
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
    assert_ne!(result, -1, "fcntl(F_NOCACHE) failed");
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn evict_file_cache(_path: &Path) {}

fn open_and_read(
    path: &Path,
    manifest: &alex_lar::ManifestId,
    cold: bool,
) -> (Duration, Duration, OpenPath) {
    if cold {
        evict_file_cache(path);
    }
    let started = Instant::now();
    let mut reader = ArchiveReader::open(File::open(path).unwrap(), Limits::default()).unwrap();
    let open_path = reader.open_path();
    let mut sink = FirstByteSink {
        started,
        first_byte: None,
        bytes: 0,
    };
    assert_eq!(reader.write_body(manifest, &mut sink).unwrap(), 1_024);
    assert_eq!(sink.bytes, 1_024);
    (sink.first_byte.unwrap(), started.elapsed(), open_path)
}

fn percentile(samples: &mut [Duration], percentile: usize) -> Duration {
    samples.sort_unstable();
    let index = ((samples.len() - 1) * percentile).div_ceil(100);
    samples[index]
}

/// Manual release-mode benchmark:
/// `cargo test -p alex-lar --test index_benchmark --release -- --ignored --nocapture`
#[test]
#[ignore = "manual random-access benchmark"]
fn persisted_footer_avoids_the_forward_scan() {
    let (scan_bytes, scan_ids) = build(false);
    let (indexed_bytes, indexed_ids) = build(true);
    let scan_file = TempArchive::write("scan", &scan_bytes);
    let indexed_file = TempArchive::write("indexed", &indexed_bytes);

    // This first fresh-descriptor sample includes filesystem open, index load,
    // body lookup, chunk decompression, and the first output byte becoming
    // available. It is not a controlled cold-cache measurement because a
    // portable test cannot evict the host page cache.
    let (first_scan, _, scan_path) = open_and_read(&scan_file.0, scan_ids.last().unwrap(), false);
    let (first_footer, _, footer_path) =
        open_and_read(&indexed_file.0, indexed_ids.last().unwrap(), false);
    assert_eq!(scan_path, OpenPath::ForwardScan);
    assert_eq!(footer_path, OpenPath::Footer);

    let mut warm = (0..100)
        .map(|sample| {
            let index = sample * 7919 % indexed_ids.len();
            open_and_read(&indexed_file.0, &indexed_ids[index], false).0
        })
        .collect::<Vec<_>>();
    let p50 = percentile(&mut warm.clone(), 50);
    let p95 = percentile(&mut warm.clone(), 95);
    let p99 = percentile(&mut warm, 99);
    assert!(
        p99 < Duration::from_millis(10),
        "sealed archive filesystem open+small-body p99 exceeded 10 ms: {p99:?}"
    );
    let mut cold = (0..20)
        .map(|sample| {
            let index = sample * 3571 % indexed_ids.len();
            open_and_read(&indexed_file.0, &indexed_ids[index], true).0
        })
        .collect::<Vec<_>>();
    let cold_p50 = percentile(&mut cold.clone(), 50);
    let cold_p95 = percentile(&mut cold.clone(), 95);
    let cold_p99 = percentile(&mut cold, 99);
    eprintln!(
        "2,000-manifest random filesystem open+1KiB body TTFT: first-forward={first_scan:?}, first-footer={first_footer:?}, footer-warm p50={p50:?} p95={p95:?} p99={p99:?}, advised-cold p50={cold_p50:?} p95={cold_p95:?} p99={cold_p99:?}"
    );
}
