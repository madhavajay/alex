use alex_lar::{ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, Limits, OpenPath};
use std::fs::File;
use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
const CACHE_MODE_ENV: &str = "ALEX_LAR_RANDOM_ACCESS_CACHE_MODE";
const CACHE_HELPER_ENV: &str = "ALEX_LAR_COLD_CACHE_HELPER";
const EXTERNAL_COLD_P99_GATE_ENV: &str = "ALEX_LAR_BENCH_MAX_EXTERNAL_COLD_P99_MS";
const EXTERNAL_READY_MARKER: &str = "alex-lar-cold-cache-ready-v1";

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

#[derive(Clone, Debug, Eq, PartialEq)]
enum CacheMode {
    Warm,
    Advisory,
    ExternalAttested { helper: PathBuf },
}

impl CacheMode {
    fn label(&self) -> &'static str {
        match self {
            Self::Warm => "warm",
            Self::Advisory => {
                #[cfg(target_os = "linux")]
                {
                    "linux-posix-fadvise-dontneed"
                }
                #[cfg(target_os = "macos")]
                {
                    "macos-f-nocache-descriptor"
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                {
                    "advisory-unavailable"
                }
            }
            Self::ExternalAttested { .. } => "external-helper-attested",
        }
    }

    fn is_external_attested(&self) -> bool {
        matches!(self, Self::ExternalAttested { .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ColdCacheGate {
    status: &'static str,
    configured_max_ms: Option<f64>,
    measured_p99_ms: f64,
    failure: Option<String>,
}

fn external_cold_p99_gate(
    mode: &CacheMode,
    configured: Option<&str>,
    measured_p99: Duration,
) -> Result<ColdCacheGate, String> {
    let measured_p99_ms = measured_p99.as_secs_f64() * 1_000.0;
    if !mode.is_external_attested() {
        if configured.is_some() {
            return Err(format!(
                "{EXTERNAL_COLD_P99_GATE_ENV} is only valid when {CACHE_MODE_ENV}=external with a successful {CACHE_HELPER_ENV} attestation"
            ));
        }
        return Ok(ColdCacheGate {
            status: "non_acceptance",
            configured_max_ms: None,
            measured_p99_ms,
            failure: None,
        });
    }

    let Some(raw) = configured else {
        return Ok(ColdCacheGate {
            status: "unconfigured",
            configured_max_ms: None,
            measured_p99_ms,
            failure: None,
        });
    };
    let configured_max_ms = raw
        .parse::<f64>()
        .map_err(|error| format!("{EXTERNAL_COLD_P99_GATE_ENV} must be a number: {error}"))?;
    if !configured_max_ms.is_finite() || configured_max_ms < 0.0 {
        return Err(format!(
            "{EXTERNAL_COLD_P99_GATE_ENV} must be a finite non-negative number"
        ));
    }
    let passed = measured_p99_ms <= configured_max_ms;
    let failure = (!passed).then(|| {
        format!(
            "{EXTERNAL_COLD_P99_GATE_ENV} maximum gate failed: measured {measured_p99_ms:.3} ms, configured {configured_max_ms:.3} ms"
        )
    });
    Ok(ColdCacheGate {
        status: if passed { "pass" } else { "fail" },
        configured_max_ms: Some(configured_max_ms),
        measured_p99_ms,
        failure,
    })
}

fn parse_cache_mode(mode: Option<&str>, helper: Option<PathBuf>) -> Result<CacheMode, String> {
    match mode.unwrap_or("advisory") {
        "warm" if helper.is_none() => Ok(CacheMode::Warm),
        "advisory" if helper.is_none() => Ok(CacheMode::Advisory),
        "external" => helper
            .map(|helper| CacheMode::ExternalAttested { helper })
            .ok_or_else(|| {
                format!("{CACHE_HELPER_ENV} is required when {CACHE_MODE_ENV}=external")
            }),
        "warm" | "advisory" => Err(format!(
            "{CACHE_HELPER_ENV} is only valid when {CACHE_MODE_ENV}=external"
        )),
        other => Err(format!(
            "unsupported {CACHE_MODE_ENV}={other:?}; expected warm, advisory, or external"
        )),
    }
}

fn cache_mode_from_env() -> Result<CacheMode, String> {
    let mode = std::env::var(CACHE_MODE_ENV).ok();
    let helper = std::env::var_os(CACHE_HELPER_ENV).map(PathBuf::from);
    parse_cache_mode(mode.as_deref(), helper)
}

fn timed_file_open(path: &Path) -> io::Result<(File, Duration)> {
    let started = Instant::now();
    let file = File::open(path)?;
    Ok((file, started.elapsed()))
}

fn timed_open_with_configuration<F>(path: &Path, configure: F) -> io::Result<(File, Duration)>
where
    F: FnOnce(&File) -> io::Result<()>,
{
    let (file, open_elapsed) = timed_file_open(path)?;
    configure(&file)?;
    Ok((file, open_elapsed))
}

#[cfg(target_os = "linux")]
fn configure_advisory_cache(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

#[cfg(target_os = "macos")]
fn configure_advisory_cache(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn configure_advisory_cache(_file: &File) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no advisory cache control is implemented for this platform",
    ))
}

fn run_external_cache_helper(helper: &Path, archive: &Path) -> Result<(), String> {
    let output = Command::new(helper)
        .arg(archive)
        .env("ALEX_LAR_COLD_CACHE_PROTOCOL", "1")
        .output()
        .map_err(|error| format!("running cold-cache helper {}: {error}", helper.display()))?;
    if !output.status.success() {
        return Err(format!(
            "cold-cache helper {} failed with {}: {}",
            helper.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let marker = String::from_utf8_lossy(&output.stdout);
    if marker.trim() != EXTERNAL_READY_MARKER {
        return Err(format!(
            "cold-cache helper {} did not emit the required {EXTERNAL_READY_MARKER:?} marker",
            helper.display()
        ));
    }
    Ok(())
}

fn prepare_file(path: &Path, mode: &CacheMode) -> (File, Duration) {
    match mode {
        CacheMode::Warm => timed_file_open(path).unwrap(),
        CacheMode::Advisory => timed_open_with_configuration(path, configure_advisory_cache)
            .unwrap_or_else(|error| panic!("applying advisory cache control: {error}")),
        CacheMode::ExternalAttested { helper } => {
            run_external_cache_helper(helper, path).unwrap_or_else(|error| panic!("{error}"));
            timed_file_open(path).unwrap()
        }
    }
}

fn open_and_read(
    path: &Path,
    manifest: &alex_lar::ManifestId,
    cache_mode: &CacheMode,
) -> (Duration, Duration, OpenPath) {
    // Cache control and ArchiveReader intentionally share this exact File.
    // This is required for descriptor-scoped F_NOCACHE on macOS and removes
    // descriptor-lifetime ambiguity from the Linux advisory path as well.
    let (file, open_elapsed) = prepare_file(path, cache_mode);
    let started = Instant::now();
    let mut reader = ArchiveReader::open(file, Limits::default()).unwrap();
    let open_path = reader.open_path();
    let mut sink = FirstByteSink {
        started,
        first_byte: None,
        bytes: 0,
    };
    assert_eq!(reader.write_body(manifest, &mut sink).unwrap(), 1_024);
    assert_eq!(sink.bytes, 1_024);
    (
        open_elapsed + sink.first_byte.unwrap(),
        open_elapsed + started.elapsed(),
        open_path,
    )
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
    let warm_mode = CacheMode::Warm;
    let advised_or_external_mode = cache_mode_from_env().unwrap_or_else(|error| panic!("{error}"));
    let (first_scan, _, scan_path) =
        open_and_read(&scan_file.0, scan_ids.last().unwrap(), &warm_mode);
    let (first_footer, _, footer_path) =
        open_and_read(&indexed_file.0, indexed_ids.last().unwrap(), &warm_mode);
    assert_eq!(scan_path, OpenPath::ForwardScan);
    assert_eq!(footer_path, OpenPath::Footer);

    let mut warm = (0..100)
        .map(|sample| {
            let index = sample * 7919 % indexed_ids.len();
            open_and_read(&indexed_file.0, &indexed_ids[index], &warm_mode).0
        })
        .collect::<Vec<_>>();
    let p50 = percentile(&mut warm.clone(), 50);
    let p95 = percentile(&mut warm.clone(), 95);
    let p99 = percentile(&mut warm, 99);
    assert!(
        p99 < Duration::from_millis(10),
        "sealed archive filesystem open+small-body p99 exceeded 10 ms: {p99:?}"
    );
    let mut cache_controlled = (0..20)
        .map(|sample| {
            let index = sample * 3571 % indexed_ids.len();
            open_and_read(
                &indexed_file.0,
                &indexed_ids[index],
                &advised_or_external_mode,
            )
            .0
        })
        .collect::<Vec<_>>();
    let cache_p50 = percentile(&mut cache_controlled.clone(), 50);
    let cache_p95 = percentile(&mut cache_controlled.clone(), 95);
    let cache_p99 = percentile(&mut cache_controlled, 99);
    let configured_gate = std::env::var(EXTERNAL_COLD_P99_GATE_ENV).ok();
    let gate = external_cold_p99_gate(
        &advised_or_external_mode,
        configured_gate.as_deref(),
        cache_p99,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    eprintln!(
        "2,000-manifest random filesystem open+1KiB body TTFT: first-forward={first_scan:?}, first-footer={first_footer:?}, footer-warm p50={p50:?} p95={p95:?} p99={p99:?}, cache-mode={} p50={cache_p50:?} p95={cache_p95:?} p99={cache_p99:?}",
        advised_or_external_mode.label(),
    );
    eprintln!(
        "ALEX_LAR_COLD_CACHE_BENCHMARK {}",
        serde_json::to_string(&serde_json::json!({
            "schema": "alex-lar-cold-cache-benchmark-v1",
            "cache_mode": advised_or_external_mode.label(),
            "samples": cache_controlled.len(),
            "p50_ms": cache_p50.as_secs_f64() * 1_000.0,
            "p95_ms": cache_p95.as_secs_f64() * 1_000.0,
            "p99_ms": cache_p99.as_secs_f64() * 1_000.0,
            "gate": {
                "environment": EXTERNAL_COLD_P99_GATE_ENV,
                "status": gate.status,
                "configured_max_ms": gate.configured_max_ms,
                "measured_p99_ms": gate.measured_p99_ms,
                "acceptance_requires": "external-helper-attested",
            },
        }))
        .unwrap()
    );
    assert!(
        gate.failure.is_none(),
        "{}",
        gate.failure.unwrap_or_default()
    );
}

#[test]
fn cache_mode_requires_an_explicit_verified_external_helper() {
    assert_eq!(parse_cache_mode(None, None).unwrap(), CacheMode::Advisory);
    assert_eq!(
        parse_cache_mode(Some("warm"), None).unwrap(),
        CacheMode::Warm
    );
    assert!(parse_cache_mode(Some("external"), None).is_err());
    assert!(parse_cache_mode(Some("advisory"), Some(PathBuf::from("helper"))).is_err());
    assert_eq!(
        parse_cache_mode(Some("external"), Some(PathBuf::from("helper"))).unwrap(),
        CacheMode::ExternalAttested {
            helper: PathBuf::from("helper")
        }
    );
}

#[test]
fn cold_cache_gate_only_passes_for_external_attestation_and_a_configured_limit() {
    let advisory = CacheMode::Advisory;
    let external = CacheMode::ExternalAttested {
        helper: PathBuf::from("helper"),
    };

    let advisory_report =
        external_cold_p99_gate(&advisory, None, Duration::from_millis(7)).unwrap();
    assert_eq!(advisory_report.status, "non_acceptance");
    assert_eq!(advisory_report.configured_max_ms, None);
    assert!(external_cold_p99_gate(&advisory, Some("10"), Duration::from_millis(7)).is_err());

    let unconfigured = external_cold_p99_gate(&external, None, Duration::from_millis(7)).unwrap();
    assert_eq!(unconfigured.status, "unconfigured");
    assert_eq!(unconfigured.configured_max_ms, None);

    let passing = external_cold_p99_gate(&external, Some("10"), Duration::from_millis(7)).unwrap();
    assert_eq!(passing.status, "pass");
    assert_eq!(passing.configured_max_ms, Some(10.0));
    assert!(passing.failure.is_none());

    let failing = external_cold_p99_gate(&external, Some("6.5"), Duration::from_millis(7)).unwrap();
    assert_eq!(failing.status, "fail");
    assert!(failing.failure.is_some());
    for invalid in ["-1", "NaN", "infinity"] {
        assert!(
            external_cold_p99_gate(&external, Some(invalid), Duration::from_millis(7)).is_err()
        );
    }
}

#[cfg(unix)]
#[test]
fn cache_configuration_is_applied_to_the_descriptor_returned_to_reader() {
    use std::cell::Cell;
    use std::os::fd::AsRawFd;

    let archive = TempArchive::write("descriptor-identity", b"descriptor identity");
    let configured = Cell::new(None);
    let (file, _) = timed_open_with_configuration(&archive.0, |file| {
        configured.set(Some(file.as_raw_fd()));
        Ok(())
    })
    .unwrap();
    assert_eq!(configured.get(), Some(file.as_raw_fd()));
}
