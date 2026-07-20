//! Shared legacy gzip-to-LAR importer used by both foreground commands and the
//! startup worker. The importer never deletes or clears legacy paths.
//!
//! This vertical slice is intentionally body-only: the three trace body
//! columns, two tool body columns, and conventionally named suffix artifacts.
//! Legacy header JSON plus attempt/stage metadata require the event-log writer
//! and are not represented by this body-pack importer.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, FileHeader, FileRole, Limits, ManifestId,
    RangeMatchConfig,
};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::{
    LarArchiveUnavailableError, LarArtifactLocation, LarMigrationItem, LarMigrationJobSpec,
    LarPointerSwitch, LarValidation, Store,
};

const FORMAT_VERSION: i64 = 1;
const SOURCE_VERSION: &str = "legacy-gzip-v1";
const DEFAULT_BATCH_SIZE: usize = 64;
const MAX_BATCH_SIZE: usize = 4096;
const MAX_WORKER_COUNT: usize = 16;
const DEFAULT_MAX_MEMORY_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_PACK_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_PACK_INDEX_ENTRIES: usize = 262_144;
const MAX_REPORTED_ERRORS: usize = 256;
const ESTIMATED_PENDING_ARTIFACT_BYTES: u64 = 4 * 1024;
/// Conservative planning charge for one chunk or manifest entry retained by
/// both the active writer and the validation reader during a batch.
const ESTIMATED_INDEX_ENTRY_BYTES: u64 = 256;

/// Resource policy shared by foreground and startup legacy importers. Defaults
/// retain the former single-worker, unthrottled behavior.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LarLegacyResourceControls {
    pub worker_count: usize,
    pub io_bytes_per_second: Option<u64>,
    pub cpu_budget_percent: u8,
    pub yield_every_artifacts: usize,
    pub max_memory_bytes: u64,
    pub max_pack_bytes: u64,
    pub max_pack_index_entries: usize,
    pub min_free_disk_bytes: Option<u64>,
}

impl Default for LarLegacyResourceControls {
    fn default() -> Self {
        Self {
            worker_count: 1,
            io_bytes_per_second: None,
            cpu_budget_percent: 100,
            yield_every_artifacts: 0,
            max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            max_pack_bytes: DEFAULT_MAX_PACK_BYTES,
            max_pack_index_entries: DEFAULT_MAX_PACK_INDEX_ENTRIES,
            min_free_disk_bytes: None,
        }
    }
}

impl LarLegacyResourceControls {
    fn validate(&self) -> Result<()> {
        if self.worker_count == 0 || self.worker_count > MAX_WORKER_COUNT {
            bail!("LAR legacy import worker_count must be between 1 and {MAX_WORKER_COUNT}");
        }
        if self.io_bytes_per_second == Some(0) {
            bail!("LAR legacy import I/O rate must be greater than zero");
        }
        if !(1..=100).contains(&self.cpu_budget_percent) {
            bail!("LAR legacy import CPU budget must be between 1 and 100 percent");
        }
        if self.max_memory_bytes < 1024 * 1024 {
            bail!("LAR legacy import memory limit must be at least 1 MiB");
        }
        if self.max_pack_bytes == 0 {
            bail!("LAR legacy import pack byte limit must be greater than zero");
        }
        if self.max_pack_index_entries == 0 {
            bail!("LAR legacy import pack index-entry limit must be greater than zero");
        }
        Ok(())
    }

    fn effective_pack_index_entries(&self) -> usize {
        let memory_entries = usize::try_from(
            self.max_memory_bytes
                .saturating_div(ESTIMATED_INDEX_ENTRY_BYTES),
        )
        .unwrap_or(usize::MAX)
        .max(1);
        self.max_pack_index_entries.min(memory_entries).max(1)
    }

    fn effective_batch_size(&self, configured: usize) -> usize {
        let memory_items = usize::try_from(
            self.max_memory_bytes
                .saturating_div(ESTIMATED_PENDING_ARTIFACT_BYTES),
        )
        .unwrap_or(usize::MAX)
        .max(1);
        configured.min(memory_items).max(1)
    }
}

/// A generic legacy body source. This is also the extension point for body
/// artifacts that were named by convention rather than stored in a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarLegacyArtifact {
    pub owner_kind: String,
    pub owner_id: String,
    pub session_id: Option<String>,
    pub artifact_kind: String,
    pub stage_id: Option<String>,
    pub source_path: String,
    pub fidelity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarArtifactReadRequest {
    pub owner_kind: String,
    pub owner_id: String,
    pub artifact_kind: String,
    pub stage_id: Option<String>,
}

impl LarArtifactReadRequest {
    pub fn new(owner_kind: &str, owner_id: &str, artifact_kind: &str) -> Self {
        Self {
            owner_kind: owner_kind.into(),
            owner_id: owner_id.into(),
            artifact_kind: artifact_kind.into(),
            stage_id: None,
        }
    }
}

/// One bounded mixed-store read. Batch callers receive an outcome for every
/// requested artifact so one corrupt body cannot silently blank a whole
/// transcript page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LarArtifactBatchRead {
    Read(Vec<u8>),
    Missing,
    Truncated {
        total_length: Option<u64>,
        budget_remaining: u64,
    },
    Error {
        kind: String,
        detail: String,
    },
    /// The daemon and metadata catalog remain available, but the immutable
    /// archive containing this body must be located or reattached.
    ArchiveUnavailable(LarArchiveUnavailableError),
}

/// Describes conventionally named trace artifacts under `bodies/<day>/`.
/// New Dario capture suffixes can be added without changing importer logic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LarLegacySuffixArtifact {
    pub artifact_kind: String,
    /// Filename suffix without the leading `<trace-id>.`.
    pub file_suffix: String,
    pub require_via_dario: bool,
}

impl LarLegacySuffixArtifact {
    fn dario(artifact_kind: &str, file_suffix: &str) -> Self {
        Self {
            artifact_kind: artifact_kind.into(),
            file_suffix: file_suffix.into(),
            require_via_dario: true,
        }
    }
}

/// Durable importer boundaries exposed to deterministic integration tests and
/// embedding supervisors. Hooks run only after the named filesystem/SQLite
/// publication has completed and while no Store lock is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LarLegacyImportBoundary {
    JobClaimed,
    BodyAppended,
    BodyValidated,
    PointerSwitched,
    JobCompleted,
}

#[derive(Clone)]
pub struct LarLegacyImportHook(
    Arc<dyn Fn(LarLegacyImportBoundary) -> Result<()> + Send + Sync + 'static>,
);

impl LarLegacyImportHook {
    pub fn new<F>(hook: F) -> Self
    where
        F: Fn(LarLegacyImportBoundary) -> Result<()> + Send + Sync + 'static,
    {
        Self(Arc::new(hook))
    }

    fn call(&self, boundary: LarLegacyImportBoundary) -> Result<()> {
        (self.0)(boundary)
    }
}

impl std::fmt::Debug for LarLegacyImportHook {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LarLegacyImportHook(..)")
    }
}

#[derive(Debug, Clone)]
pub struct LarLegacyImportOptions {
    /// Maximum artifacts attempted during this invocation. `None` drains all
    /// currently inventoried legacy sources.
    pub limit: Option<usize>,
    pub batch_size: usize,
    pub lease_owner: String,
    pub lease_duration: Duration,
    pub resources: LarLegacyResourceControls,
    /// Test/embedding seam for deterministic disk-pressure decisions. Normal
    /// callers use the platform free-space probe.
    pub disk_free_bytes_probe: Option<fn(&Path) -> std::io::Result<u64>>,
    pub suffix_artifacts: Vec<LarLegacySuffixArtifact>,
    pub additional_artifacts: Vec<LarLegacyArtifact>,
    /// Optional observation/fault seam. Production callers leave this unset.
    pub boundary_hook: Option<LarLegacyImportHook>,
}

impl Default for LarLegacyImportOptions {
    fn default() -> Self {
        Self {
            limit: None,
            batch_size: DEFAULT_BATCH_SIZE,
            lease_owner: format!("alex-legacy-import-{}", std::process::id()),
            lease_duration: Duration::from_secs(60),
            resources: LarLegacyResourceControls::default(),
            disk_free_bytes_probe: None,
            suffix_artifacts: vec![
                LarLegacySuffixArtifact::dario(
                    "dario_upstream_request",
                    "dario-upstream-request.json.gz",
                ),
                LarLegacySuffixArtifact::dario(
                    "dario_upstream_response",
                    "dario-upstream-response.json.gz",
                ),
            ],
            additional_artifacts: Vec::new(),
            boundary_hook: None,
        }
    }
}

fn visit_import_boundary(
    options: &LarLegacyImportOptions,
    boundary: LarLegacyImportBoundary,
) -> Result<()> {
    if let Some(hook) = &options.boundary_hook {
        hook.call(boundary)?;
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct LarLegacyImportError {
    pub item_id: String,
    pub owner_kind: String,
    pub owner_id: String,
    pub artifact_kind: String,
    pub error_kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct LarLegacyImportReport {
    pub job_id: String,
    pub job_state: String,
    pub claimed: bool,
    pub archive_set_uuid: String,
    pub file_uuid: String,
    pub file_path: PathBuf,
    pub inventoried: u64,
    pub attempted: u64,
    pub migrated: u64,
    pub skipped: u64,
    pub failed: u64,
    pub bytes_read: u64,
    pub unique_bytes_written: u64,
    pub bytes_deduplicated: u64,
    pub total_items: u64,
    pub completed_items: u64,
    pub remaining_items: u64,
    pub progress_percent: f64,
    pub elapsed_ms: u64,
    pub throughput_bytes_per_second: u64,
    pub throughput_artifacts_per_second: f64,
    pub dedup_ratio: f64,
    pub eta_seconds: Option<u64>,
    pub last_error: Option<String>,
    pub configured_worker_count: usize,
    pub workers_used: usize,
    pub configured_batch_size: usize,
    pub effective_batch_size: usize,
    pub configured_io_bytes_per_second: Option<u64>,
    pub configured_cpu_budget_percent: u8,
    pub configured_max_memory_bytes: u64,
    pub configured_max_pack_bytes: u64,
    pub configured_max_pack_index_entries: usize,
    pub effective_max_pack_index_entries: usize,
    pub pack_sequence: u64,
    pub packs_rotated: u64,
    pub throttled_ms: u64,
    pub yield_count: u64,
    pub free_disk_bytes: Option<u64>,
    pub paused_reason: Option<String>,
    pub limit_reached: bool,
    pub errors: Vec<LarLegacyImportError>,
    pub errors_truncated: u64,
}

impl LarLegacyImportReport {
    fn new(ids: &ImportIds, job_state: String) -> Self {
        Self {
            job_id: ids.job_id.clone(),
            job_state,
            claimed: false,
            archive_set_uuid: ids.archive_set_uuid.clone(),
            file_uuid: ids.file_uuid.clone(),
            file_path: ids.file_path.clone(),
            inventoried: 0,
            attempted: 0,
            migrated: 0,
            skipped: 0,
            failed: 0,
            bytes_read: 0,
            unique_bytes_written: 0,
            bytes_deduplicated: 0,
            total_items: 0,
            completed_items: 0,
            remaining_items: 0,
            progress_percent: 0.0,
            elapsed_ms: 0,
            throughput_bytes_per_second: 0,
            throughput_artifacts_per_second: 0.0,
            dedup_ratio: 0.0,
            eta_seconds: None,
            last_error: None,
            configured_worker_count: 1,
            workers_used: 0,
            configured_batch_size: DEFAULT_BATCH_SIZE,
            effective_batch_size: DEFAULT_BATCH_SIZE,
            configured_io_bytes_per_second: None,
            configured_cpu_budget_percent: 100,
            configured_max_memory_bytes: DEFAULT_MAX_MEMORY_BYTES,
            configured_max_pack_bytes: DEFAULT_MAX_PACK_BYTES,
            configured_max_pack_index_entries: DEFAULT_MAX_PACK_INDEX_ENTRIES,
            effective_max_pack_index_entries: DEFAULT_MAX_PACK_INDEX_ENTRIES,
            pack_sequence: ids.pack_sequence,
            packs_rotated: 0,
            throttled_ms: 0,
            yield_count: 0,
            free_disk_bytes: None,
            paused_reason: None,
            limit_reached: false,
            errors: Vec::new(),
            errors_truncated: 0,
        }
    }

    fn push_error(&mut self, error: LarLegacyImportError) {
        if self.errors.len() < MAX_REPORTED_ERRORS {
            self.errors.push(error);
        } else {
            self.errors_truncated = self.errors_truncated.saturating_add(1);
        }
    }
}

#[derive(Clone, Debug)]
struct ImportIds {
    source_key: String,
    job_id: String,
    archive_set_uuid: String,
    file_uuid: String,
    file_uuid_bytes: [u8; 16],
    file_path: PathBuf,
    pack_sequence: u64,
}

#[derive(Debug)]
struct SourceProvenance {
    size: Option<u64>,
    mtime_ms: Option<i64>,
    fingerprint: String,
}

#[derive(Debug)]
struct PreparedSource {
    source: LarLegacyArtifact,
    resolved_path: PathBuf,
    provenance: SourceProvenance,
}

#[derive(Debug)]
struct PendingArtifact {
    source: LarLegacyArtifact,
    resolved_path: PathBuf,
    item_id: String,
}

#[derive(Debug)]
struct PendingValidation {
    source: LarLegacyArtifact,
    item_id: String,
    manifest_id: ManifestId,
    source_length: u64,
    source_hash: [u8; 32],
    unique_bytes_written: u64,
}

#[derive(Debug)]
struct IoThrottleState {
    started: Instant,
    bytes: u64,
    throttled: Duration,
}

#[derive(Clone, Debug)]
struct ResourceController {
    controls: LarLegacyResourceControls,
    io: Arc<Mutex<IoThrottleState>>,
    artifact_count: Arc<AtomicU64>,
    yield_count: Arc<AtomicU64>,
    workers_used: Arc<AtomicUsize>,
}

impl ResourceController {
    fn new(controls: LarLegacyResourceControls) -> Self {
        Self {
            controls,
            io: Arc::new(Mutex::new(IoThrottleState {
                started: Instant::now(),
                bytes: 0,
                throttled: Duration::ZERO,
            })),
            artifact_count: Arc::new(AtomicU64::new(0)),
            yield_count: Arc::new(AtomicU64::new(0)),
            workers_used: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn account_io(&self, bytes: usize) {
        let Some(rate) = self.controls.io_bytes_per_second else {
            return;
        };
        let mut state = self.io.lock().unwrap();
        state.bytes = state.bytes.saturating_add(bytes as u64);
        let delay = required_io_delay(state.bytes, rate, state.started.elapsed());
        if !delay.is_zero() {
            std::thread::sleep(delay);
            state.throttled = state.throttled.saturating_add(delay);
        }
    }

    fn finish_cpu_slice(&self, work: Duration) {
        let delay = required_cpu_delay(work, self.controls.cpu_budget_percent);
        if !delay.is_zero() {
            std::thread::sleep(delay);
            let mut state = self.io.lock().unwrap();
            state.throttled = state.throttled.saturating_add(delay);
        }
        let artifact = self.artifact_count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.controls.yield_every_artifacts > 0
            && artifact % self.controls.yield_every_artifacts as u64 == 0
        {
            std::thread::yield_now();
            self.yield_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn note_workers(&self, workers: usize) {
        self.workers_used.fetch_max(workers, Ordering::Relaxed);
    }

    fn throttled_ms(&self) -> u64 {
        self.io
            .lock()
            .unwrap()
            .throttled
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

struct ThrottledReader<R> {
    inner: R,
    resources: ResourceController,
}

impl<R> ThrottledReader<R> {
    fn new(inner: R, resources: ResourceController) -> Self {
        Self { inner, resources }
    }
}

impl<R: Read> Read for ThrottledReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.resources.account_io(read);
        Ok(read)
    }
}

fn required_io_delay(total_bytes: u64, bytes_per_second: u64, elapsed: Duration) -> Duration {
    if bytes_per_second == 0 {
        return Duration::ZERO;
    }
    let target = Duration::from_secs_f64(total_bytes as f64 / bytes_per_second as f64);
    target.saturating_sub(elapsed)
}

fn required_cpu_delay(work: Duration, budget_percent: u8) -> Duration {
    if budget_percent >= 100 || budget_percent == 0 {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(
        work.as_secs_f64() * (100_u64.saturating_sub(budget_percent as u64)) as f64
            / budget_percent as f64,
    )
}

struct HashingReader<R> {
    inner: R,
    hasher: blake3::Hasher,
    length: u64,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
            length: 0,
        }
    }

    fn identity(&self) -> (u64, [u8; 32]) {
        (self.length, *self.hasher.clone().finalize().as_bytes())
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        self.length = self.length.saturating_add(read as u64);
        Ok(read)
    }
}

#[derive(Default)]
struct HashingWriter {
    hasher: blake3::Hasher,
    length: u64,
}

impl Write for HashingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(bytes);
        self.length = self.length.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(unix)]
fn platform_free_disk_bytes(path: &Path) -> std::io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "disk probe path contains a NUL byte",
        )
    })?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `stats` points to writable storage
    // for one `statvfs` value. A successful call initializes it completely.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful statvfs call above initialized `stats`.
    let stats = unsafe { stats.assume_init() };
    Ok((stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
}

#[cfg(not(unix))]
fn platform_free_disk_bytes(_path: &Path) -> std::io::Result<u64> {
    Ok(u64::MAX)
}

fn check_disk_pressure(
    data_dir: &Path,
    options: &LarLegacyImportOptions,
    report: &mut LarLegacyImportReport,
) -> Result<bool> {
    let Some(threshold) = options.resources.min_free_disk_bytes else {
        return Ok(false);
    };
    let probe = options
        .disk_free_bytes_probe
        .unwrap_or(platform_free_disk_bytes);
    let available = probe(data_dir)
        .with_context(|| format!("checking free disk below {}", data_dir.display()))?;
    report.free_disk_bytes = Some(available);
    if available < threshold {
        report.paused_reason = Some(format!(
            "low_disk: {available} bytes free is below the configured {threshold}-byte threshold"
        ));
        return Ok(true);
    }
    Ok(false)
}

fn progress_metrics(
    elapsed: Duration,
    attempted: u64,
    bytes_read: u64,
    unique_bytes_written: u64,
    total_items: u64,
    completed_items: u64,
) -> (u64, u64, f64, f64, Option<u64>, f64) {
    let elapsed_ms: u64 = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
    let elapsed_seconds = elapsed.as_secs_f64();
    let bytes_per_second = if elapsed_seconds > 0.0 {
        (bytes_read as f64 / elapsed_seconds) as u64
    } else {
        0
    };
    let artifacts_per_second = if elapsed_seconds > 0.0 {
        attempted as f64 / elapsed_seconds
    } else {
        0.0
    };
    let remaining = total_items.saturating_sub(completed_items);
    let eta_seconds = (artifacts_per_second > 0.0 && remaining > 0)
        .then(|| (remaining as f64 / artifacts_per_second).ceil() as u64);
    let progress_percent = if total_items == 0 {
        100.0
    } else {
        completed_items.min(total_items) as f64 * 100.0 / total_items as f64
    };
    let dedup_ratio = if bytes_read == 0 {
        0.0
    } else {
        bytes_read.saturating_sub(unique_bytes_written) as f64 / bytes_read as f64
    };
    (
        elapsed_ms,
        bytes_per_second,
        artifacts_per_second,
        progress_percent,
        eta_seconds,
        dedup_ratio,
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn deterministic_uuid(namespace: &[u8], source_key: &str) -> ([u8; 16], String) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(namespace);
    hasher.update(source_key.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    // RFC 4122 variant with a deterministic v5-shaped identifier.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let text = format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..])
    );
    (bytes, text)
}

fn base_source_key(store: &Store) -> String {
    store
        .data_dir
        .join("alexandria.sqlite3")
        .to_string_lossy()
        .into_owned()
}

fn import_ids(store: &Store, generation: u64) -> ImportIds {
    let base_source_key = base_source_key(store);
    let source_key = if generation == 0 {
        base_source_key.clone()
    } else {
        format!("{base_source_key}#generation-{generation}")
    };
    let (_, archive_set_uuid) = deterministic_uuid(b"alex-lar-archive-set-v1", &base_source_key);
    let (file_uuid_bytes, file_uuid) =
        deterministic_uuid(b"alex-lar-body-pack-v1", &base_source_key);
    let mut job_hasher = blake3::Hasher::new();
    job_hasher.update(b"alex-lar-legacy-job-v1");
    job_hasher.update(source_key.as_bytes());
    let job_id = format!("legacy-{}", hex(&job_hasher.finalize().as_bytes()[..16]));
    let file_path = store.data_dir.join("lar").join(format!("{file_uuid}.lar"));
    ImportIds {
        source_key,
        job_id,
        archive_set_uuid,
        file_uuid,
        file_uuid_bytes,
        file_path,
        pack_sequence: 0,
    }
}

fn import_pack_ids(store: &Store, base: &ImportIds, pack_sequence: u64) -> ImportIds {
    if pack_sequence == 0 {
        return base.clone();
    }
    let base_source_key = base_source_key(store);
    let pack_key = format!("{base_source_key}#pack-{pack_sequence}");
    let (file_uuid_bytes, file_uuid) = deterministic_uuid(b"alex-lar-body-pack-v1", &pack_key);
    let file_path = store
        .data_dir
        .join("lar")
        .join(format!("{}~{pack_sequence:020}.lar", base.file_uuid));
    ImportIds {
        source_key: base.source_key.clone(),
        job_id: base.job_id.clone(),
        archive_set_uuid: base.archive_set_uuid.clone(),
        file_uuid,
        file_uuid_bytes,
        file_path,
        pack_sequence,
    }
}

fn import_pack_limit_reached(
    writer: &ArchiveWriter<File>,
    controls: &LarLegacyResourceControls,
) -> Result<bool> {
    if writer.manifest_count() == 0 {
        return Ok(false);
    }
    let index_entries = writer.chunk_count().saturating_add(writer.manifest_count());
    let size = writer.get_ref().metadata()?.len();
    Ok(size >= controls.max_pack_bytes || index_entries >= controls.effective_pack_index_entries())
}

fn resolve_source_path(data_dir: &Path, source_path: &str) -> PathBuf {
    let path = PathBuf::from(source_path);
    if path.is_absolute() {
        path
    } else {
        data_dir.join(path)
    }
}

fn source_metadata(path: &Path) -> (Option<u64>, Option<i64>) {
    let metadata = std::fs::metadata(path).ok();
    let size = metadata.as_ref().map(std::fs::Metadata::len);
    let mtime_ms = metadata
        .and_then(|value| value.modified().ok())
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .and_then(|value| i64::try_from(value.as_millis()).ok());
    (size, mtime_ms)
}

fn source_provenance(
    path: &Path,
    source: &LarLegacyArtifact,
    resources: &ResourceController,
) -> SourceProvenance {
    let (size, mtime_ms) = source_metadata(path);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"alex-legacy-source-fingerprint-v1\0");
    for value in [
        source.owner_kind.as_str(),
        source.owner_id.as_str(),
        source.artifact_kind.as_str(),
        source.stage_id.as_deref().unwrap_or(""),
        source.source_path.as_str(),
    ] {
        hasher.update(value.as_bytes());
        hasher.update(&[0]);
    }
    match File::open(path) {
        Ok(mut file) => {
            hasher.update(b"present\0");
            let mut buffer = [0u8; 64 * 1024];
            loop {
                match file.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        resources.account_io(read);
                        hasher.update(&buffer[..read]);
                    }
                    Err(error) => {
                        hasher.update(b"read-error\0");
                        hasher.update(error.to_string().as_bytes());
                        break;
                    }
                }
            }
        }
        Err(error) => {
            hasher.update(b"open-error\0");
            hasher.update(error.kind().to_string().as_bytes());
        }
    }
    SourceProvenance {
        size,
        mtime_ms,
        fingerprint: hex(hasher.finalize().as_bytes()),
    }
}

fn prepare_provenance_parallel(
    data_dir: &Path,
    sources: Vec<LarLegacyArtifact>,
    resources: &ResourceController,
) -> Vec<PreparedSource> {
    if sources.is_empty() {
        return Vec::new();
    }
    let workers = resources.controls.worker_count.min(sources.len());
    resources.note_workers(workers);
    let next = AtomicUsize::new(0);
    let results = Mutex::new(
        std::iter::repeat_with(|| None)
            .take(sources.len())
            .collect::<Vec<Option<PreparedSource>>>(),
    );
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(source) = sources.get(index).cloned() else {
                    break;
                };
                let resolved_path = resolve_source_path(data_dir, &source.source_path);
                let provenance = source_provenance(&resolved_path, &source, resources);
                results.lock().unwrap()[index] = Some(PreparedSource {
                    source,
                    resolved_path,
                    provenance,
                });
            });
        }
    });
    results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|item| item.expect("each provenance worker input produced one result"))
        .collect()
}

fn item_id(job_id: &str, source: &LarLegacyArtifact, fingerprint: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for value in [
        job_id,
        source.owner_kind.as_str(),
        source.owner_id.as_str(),
        source.artifact_kind.as_str(),
        source.stage_id.as_deref().unwrap_or(""),
        fingerprint,
    ] {
        hasher.update(value.as_bytes());
        hasher.update(&[0]);
    }
    format!("item-{}", hex(hasher.finalize().as_bytes()))
}

fn semantic_predecessor_key(source: &LarLegacyArtifact) -> Option<(String, String)> {
    if source.owner_kind != "trace"
        || !matches!(
            source.artifact_kind.as_str(),
            "client_request" | "upstream_request" | "dario_upstream_request"
        )
    {
        return None;
    }
    source
        .session_id
        .as_ref()
        .filter(|session| !session.is_empty())
        .map(|session| (session.clone(), source.artifact_kind.clone()))
}

fn same_trace_predecessor_kinds(source: &LarLegacyArtifact) -> &'static [&'static str] {
    if source.owner_kind != "trace" {
        return &[];
    }
    match source.artifact_kind.as_str() {
        // These are successive capture boundaries for the same exchange, so
        // they are related even when routing mutates a small part of the body.
        "upstream_request" => &["client_request"],
        "dario_upstream_request" => &["upstream_request", "client_request"],
        _ => &[],
    }
}

impl Store {
    fn legacy_source_needs_import(&self, source: &LarLegacyArtifact) -> Result<bool> {
        // LAR-with-fallback deliberately retains a gzip copy. If that live
        // capture became visible after this migration pass began, its already
        // validated pointer is authoritative and the fallback must not be
        // rediscovered as legacy work on a later inventory page.
        let captured_pointer: bool = self.conn.lock().unwrap().query_row(
            "SELECT EXISTS(
               SELECT 1 FROM lar_trace_artifacts a
               JOIN lar_manifests m ON m.manifest_id=a.manifest_id
                WHERE a.owner_kind=?1 AND a.owner_id=?2 AND a.artifact_kind=?3
                  AND a.stage_id=?4 AND a.validation_state='validated'
                  AND a.fidelity='captured' AND m.state='ready'
             )",
            params![
                source.owner_kind,
                source.owner_id,
                source.artifact_kind,
                source.stage_id.as_deref().unwrap_or(""),
            ],
            |row| row.get(0),
        )?;
        if captured_pointer {
            return Ok(false);
        }
        let resolved = resolve_source_path(&self.data_dir, &source.source_path);
        let (size, mtime_ms) = source_metadata(&resolved);
        let size = size.and_then(|value| i64::try_from(value).ok());
        let metadata_matches: bool = self.conn.lock().unwrap().query_row(
            "SELECT EXISTS(
               SELECT 1 FROM lar_migration_items
                WHERE owner_kind=?1 AND owner_id=?2 AND artifact_kind=?3
                  AND stage_id=?4 AND source_path=?5
                  AND source_size IS ?6 AND source_mtime_ms IS ?7
                  AND state='migrated' AND validation_state='validated'
             )",
            params![
                source.owner_kind,
                source.owner_id,
                source.artifact_kind,
                source.stage_id.as_deref().unwrap_or(""),
                source.source_path,
                size,
                mtime_ms,
            ],
            |row| row.get(0),
        )?;
        if metadata_matches {
            return Ok(false);
        }

        // Hashing a new or metadata-changed source happens in the bounded
        // provenance worker pool. Normal startup still does only this cheap
        // stat/query for unchanged sources.
        Ok(true)
    }

    fn has_unmigrated_legacy_sources(&self, options: &LarLegacyImportOptions) -> Result<bool> {
        let inventory_batch = options.resources.effective_batch_size(options.batch_size);
        let mut offset = 0usize;
        loop {
            let rows = self.legacy_column_artifacts(offset, inventory_batch)?;
            if rows.is_empty() {
                break;
            }
            offset += rows.len();
            for source in rows {
                if self.legacy_source_needs_import(&source)? {
                    return Ok(true);
                }
            }
        }
        let mut found = false;
        self.visit_legacy_suffix_artifact_batches(
            &options.suffix_artifacts,
            inventory_batch,
            |sources| {
                for source in sources {
                    if self.legacy_source_needs_import(&source)? {
                        found = true;
                        return Ok(false);
                    }
                }
                Ok(true)
            },
        )?;
        if found {
            return Ok(true);
        }
        for source in &options.additional_artifacts {
            if self.legacy_source_needs_import(&source)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn select_import_ids(&self, options: &LarLegacyImportOptions) -> Result<ImportIds> {
        let base = base_source_key(self);
        let prefix = format!("{base}#generation-");
        let latest = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT source_key, state FROM lar_migration_jobs
                 WHERE format_version=?1 AND source_version=?2",
            )?;
            let rows = statement.query_map(params![FORMAT_VERSION, SOURCE_VERSION], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let mut latest = None;
            for row in rows {
                let (source_key, state) = row?;
                let generation = if source_key == base {
                    Some(0)
                } else {
                    source_key
                        .strip_prefix(&prefix)
                        .and_then(|value| value.parse::<u64>().ok())
                };
                if let Some(generation) = generation {
                    if latest
                        .as_ref()
                        .is_none_or(|(current, _)| generation > *current)
                    {
                        latest = Some((generation, state));
                    }
                }
            }
            latest
        };
        match latest {
            None => Ok(import_ids(self, 0)),
            Some((generation, state)) if state != "complete" => Ok(import_ids(self, generation)),
            Some((generation, _)) if !self.has_unmigrated_legacy_sources(options)? => {
                Ok(import_ids(self, generation))
            }
            Some((generation, _)) => Ok(import_ids(
                self,
                generation
                    .checked_add(1)
                    .context("legacy migration generation overflow")?,
            )),
        }
    }

    fn latest_import_pack(&self, base: &ImportIds) -> Result<Option<(ImportIds, String)>> {
        let prefix = base
            .file_path
            .parent()
            .expect("legacy import pack path has a parent")
            .join(format!("{}~", base.file_uuid))
            .to_string_lossy()
            .into_owned();
        let base_path = base.file_path.to_string_lossy().into_owned();
        let row: Option<(String, String, String)> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT file_uuid, path, state FROM lar_files
                 WHERE archive_set_uuid=?1 AND role='body-pack'
                   AND (path=?2 OR substr(path, 1, length(?3))=?3)
                 ORDER BY path DESC LIMIT 1",
                params![base.archive_set_uuid, base_path, prefix],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((stored_uuid, stored_path, state)) = row else {
            return Ok(None);
        };
        let sequence = if stored_path == base.file_path.to_string_lossy() {
            0
        } else {
            let file_name = Path::new(&stored_path)
                .file_name()
                .and_then(|value| value.to_str())
                .context("legacy import pack path has no UTF-8 file name")?;
            file_name
                .strip_prefix(&format!("{}~", base.file_uuid))
                .and_then(|value| value.strip_suffix(".lar"))
                .and_then(|value| value.parse::<u64>().ok())
                .context("legacy import pack path has an invalid sequence")?
        };
        let ids = import_pack_ids(self, base, sequence);
        if ids.file_uuid != stored_uuid || ids.file_path != PathBuf::from(stored_path) {
            bail!("legacy import pack identity does not match its deterministic sequence");
        }
        Ok(Some((ids, state)))
    }

    fn open_import_pack(
        &self,
        ids: &ImportIds,
        started_ms: i64,
        chunker: ChunkerConfig,
        limits: &Limits,
    ) -> Result<ArchiveWriter<File>> {
        std::fs::create_dir_all(ids.file_path.parent().expect("LAR path has a parent"))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&ids.file_path)
            .with_context(|| format!("opening LAR body pack at {}", ids.file_path.display()))?;
        let mut writer = if file.metadata()?.len() == 0 {
            ArchiveWriter::create(
                file,
                FileHeader::body_pack(
                    ids.file_uuid_bytes,
                    u64::try_from(started_ms.max(0)).unwrap_or(0) * 1_000_000,
                    b"alex-store legacy importer v1".to_vec(),
                ),
                chunker,
                limits.clone(),
            )?
        } else {
            ArchiveWriter::open_append(file, chunker, limits.clone())?
        };
        if writer.header().file_uuid != ids.file_uuid_bytes
            || writer.header().file_role != FileRole::BodyPack
        {
            bail!("legacy import body pack identity or role does not match its catalog identity");
        }
        writer.enable_metadata_pages();
        writer.flush()?;
        writer.get_ref().sync_all()?;
        self.ensure_import_archive_file(ids, started_ms)?;
        Ok(writer)
    }

    fn backfill_import_archive_catalogs(
        &self,
        base: &ImportIds,
        through_sequence: u64,
        limits: &Limits,
        controls: &LarLegacyResourceControls,
    ) -> Result<()> {
        for sequence in 0..=through_sequence {
            self.backfill_import_archive_catalog(
                &import_pack_ids(self, base, sequence),
                limits,
                controls,
            )?;
        }
        Ok(())
    }

    fn catalog_import_pack_index_entries(&self, ids: &ImportIds) -> Result<usize> {
        let (references, manifests): (i64, i64) = self.conn.lock().unwrap().query_row(
            "SELECT
               (SELECT COUNT(*) FROM lar_manifest_chunks mc
                 JOIN lar_manifests m ON m.manifest_id=mc.manifest_id
                WHERE m.file_uuid=?1),
               (SELECT COUNT(*) FROM lar_manifests WHERE file_uuid=?1)",
            [&ids.file_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        usize::try_from(references.saturating_add(manifests))
            .context("legacy import catalog index count is negative or too large")
    }

    fn catalog_import_pack_exceeds_index_limit(
        &self,
        ids: &ImportIds,
        controls: &LarLegacyResourceControls,
    ) -> Result<bool> {
        let entries = self.catalog_import_pack_index_entries(ids)?;
        Ok(entries > controls.effective_pack_index_entries())
    }

    fn catalog_import_pack_is_complete(&self, ids: &ImportIds) -> Result<bool> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT NOT EXISTS(
               SELECT 1 FROM lar_manifests m
                WHERE m.file_uuid=?1 AND (
                  (m.total_length > 0 AND NOT EXISTS(
                     SELECT 1 FROM lar_manifest_chunks mc
                      WHERE mc.manifest_id=m.manifest_id
                   )) OR EXISTS(
                     SELECT 1 FROM lar_manifest_chunks mc
                     LEFT JOIN lar_chunks c
                       ON c.hash_algorithm=mc.hash_algorithm
                      AND c.chunk_hash=mc.chunk_hash
                      WHERE mc.manifest_id=m.manifest_id
                        AND c.chunk_hash IS NULL
                   )
                )
             ) AND EXISTS(
               SELECT 1 FROM lar_manifests WHERE file_uuid=?1
             )",
                [&ids.file_uuid],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn seal_import_pack(&self, ids: &ImportIds, writer: &mut ArchiveWriter<File>) -> Result<()> {
        writer.seal()?;
        writer.get_ref().sync_all()?;
        let size = i64::try_from(writer.get_ref().metadata()?.len())?;
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE lar_files SET state='sealed', sealed_at_ms=?2, size_bytes=?3
             WHERE file_uuid=?1 AND state='active'",
            params![ids.file_uuid, now_ms(), size],
        )?;
        if changed != 1 {
            bail!("legacy import pack was not active while sealing");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn rotate_import_pack(
        &self,
        base: &ImportIds,
        ids: &mut ImportIds,
        writer: &mut ArchiveWriter<File>,
        report: &mut LarLegacyImportReport,
        started_ms: i64,
        chunker: ChunkerConfig,
        limits: &Limits,
    ) -> Result<()> {
        self.seal_import_pack(ids, writer)?;
        let next_sequence = ids
            .pack_sequence
            .checked_add(1)
            .context("legacy import pack sequence overflow")?;
        let next = import_pack_ids(self, base, next_sequence);
        let next_writer = self.open_import_pack(&next, started_ms, chunker, limits)?;
        *ids = next;
        *writer = next_writer;
        report.file_uuid = ids.file_uuid.clone();
        report.file_path = ids.file_path.clone();
        report.pack_sequence = ids.pack_sequence;
        report.packs_rotated = report.packs_rotated.saturating_add(1);
        Ok(())
    }

    /// Import legacy gzip body artifacts into a deterministic sequence of
    /// bounded rolling body packs. It is safe to invoke repeatedly and safe to
    /// stop between validated artifacts or batches.
    pub fn run_lar_legacy_import(
        &self,
        options: &LarLegacyImportOptions,
    ) -> Result<LarLegacyImportReport> {
        let run_started = Instant::now();
        if options.batch_size == 0 || options.batch_size > MAX_BATCH_SIZE {
            bail!("LAR legacy import batch_size must be between 1 and {MAX_BATCH_SIZE}");
        }
        if options.lease_owner.is_empty() {
            bail!("LAR legacy import lease_owner must not be empty");
        }
        options.resources.validate()?;
        let resources = ResourceController::new(options.resources.clone());
        let base_ids = self.select_import_ids(options)?;
        let latest_pack = self.latest_import_pack(&base_ids)?;
        let started_ms = now_ms();
        let job = self.ensure_lar_migration_job(
            &LarMigrationJobSpec {
                job_id: base_ids.job_id.clone(),
                format_version: FORMAT_VERSION,
                source_version: SOURCE_VERSION.into(),
                source_key: base_ids.source_key.clone(),
            },
            started_ms,
        )?;
        let report_ids = latest_pack
            .as_ref()
            .map(|(ids, _)| ids)
            .unwrap_or(&base_ids);
        let mut report = LarLegacyImportReport::new(report_ids, job.state.clone());
        report.configured_worker_count = options.resources.worker_count;
        report.configured_batch_size = options.batch_size;
        report.effective_batch_size = options.resources.effective_batch_size(options.batch_size);
        report.configured_io_bytes_per_second = options.resources.io_bytes_per_second;
        report.configured_cpu_budget_percent = options.resources.cpu_budget_percent;
        report.configured_max_memory_bytes = options.resources.max_memory_bytes;
        report.configured_max_pack_bytes = options.resources.max_pack_bytes;
        report.configured_max_pack_index_entries = options.resources.max_pack_index_entries;
        report.effective_max_pack_index_entries = options.resources.effective_pack_index_entries();
        if job.state == "complete" {
            if let Some((ids, _)) = latest_pack.as_ref() {
                self.backfill_import_archive_catalogs(
                    &base_ids,
                    ids.pack_sequence,
                    &Limits::default(),
                    &options.resources,
                )?;
            }
            self.backfill_import_conversations(options, &mut report)?;
            self.finalize_import_report(&mut report, &resources, run_started.elapsed())?;
            return Ok(report);
        }
        if !self.claim_lar_migration_job(
            &job.job_id,
            &options.lease_owner,
            started_ms,
            options.lease_duration,
        )? {
            report.job_state = self
                .lar_migration_job(&job.job_id)?
                .map(|value| value.state)
                .unwrap_or_else(|| "missing".into());
            self.finalize_import_report(&mut report, &resources, run_started.elapsed())?;
            return Ok(report);
        }
        report.claimed = true;
        visit_import_boundary(options, LarLegacyImportBoundary::JobClaimed)?;
        if check_disk_pressure(&self.data_dir, options, &mut report)? {
            self.release_import_lease(&job.job_id, &options.lease_owner, false, now_ms())?;
            report.job_state = "pending".into();
            self.finalize_import_report(&mut report, &resources, run_started.elapsed())?;
            return Ok(report);
        }
        let limits = Limits::default();
        let chunker = ChunkerConfig::default();
        if let Some((latest, _)) = latest_pack.as_ref() {
            self.backfill_import_archive_catalogs(
                &base_ids,
                latest.pack_sequence,
                &limits,
                &options.resources,
            )?;
        }
        let (mut ids, rolled_over_existing) = match latest_pack {
            None => (base_ids.clone(), false),
            Some((ids, state))
                if state == "active"
                    && self
                        .catalog_import_pack_exceeds_index_limit(&ids, &options.resources)? =>
            {
                (
                    import_pack_ids(
                        self,
                        &base_ids,
                        ids.pack_sequence
                            .checked_add(1)
                            .context("legacy import pack sequence overflow")?,
                    ),
                    true,
                )
            }
            Some((ids, state)) if state == "active" => (ids, false),
            Some((ids, state)) if state == "sealed" => (
                import_pack_ids(
                    self,
                    &base_ids,
                    ids.pack_sequence
                        .checked_add(1)
                        .context("legacy import pack sequence overflow")?,
                ),
                false,
            ),
            Some((_, state)) => bail!("legacy import cannot continue from a {state} pack"),
        };
        let mut writer = self.open_import_pack(&ids, started_ms, chunker, &limits)?;
        report.file_uuid = ids.file_uuid.clone();
        report.file_path = ids.file_path.clone();
        report.pack_sequence = ids.pack_sequence;
        if rolled_over_existing {
            report.packs_rotated = report.packs_rotated.saturating_add(1);
        }

        let batch_size = report.effective_batch_size;
        let max_attempts = options.limit.unwrap_or(usize::MAX);
        let mut pending = Vec::with_capacity(batch_size);
        let mut session_predecessors = HashMap::new();
        let mut trace_artifacts = HashMap::new();
        let mut offset = 0usize;
        let mut inventory_complete = true;
        let mut resource_paused = false;

        'inventory: loop {
            let rows = self.legacy_column_artifacts(offset, batch_size)?;
            if rows.is_empty() {
                break;
            }
            let row_count = rows.len();
            offset += row_count;
            let prepared_sources = self.prepare_import_sources(rows, &resources, &mut report)?;
            for source in prepared_sources {
                if self.prepare_import_source(&ids, source, &mut pending, &mut report)?
                    && report.attempted as usize + pending.len() >= max_attempts
                {
                    inventory_complete = false;
                    break 'inventory;
                }
                if pending.len() >= batch_size {
                    if check_disk_pressure(&self.data_dir, options, &mut report)? {
                        resource_paused = true;
                        inventory_complete = false;
                        break 'inventory;
                    }
                    self.drain_pending_with_rotation(
                        &base_ids,
                        &mut ids,
                        options,
                        &resources,
                        &limits,
                        chunker,
                        &mut writer,
                        &mut session_predecessors,
                        &mut trace_artifacts,
                        &mut pending,
                        &mut report,
                        started_ms,
                    )?;
                }
            }
            if row_count < batch_size {
                break;
            }
        }

        if inventory_complete {
            let suffix_complete = self.visit_legacy_suffix_artifact_batches(
                &options.suffix_artifacts,
                batch_size,
                |sources| {
                    let prepared_sources =
                        self.prepare_import_sources(sources, &resources, &mut report)?;
                    for source in prepared_sources {
                        if self.prepare_import_source(&ids, source, &mut pending, &mut report)?
                            && report.attempted as usize + pending.len() >= max_attempts
                        {
                            inventory_complete = false;
                            return Ok(false);
                        }
                        if pending.len() >= batch_size {
                            if check_disk_pressure(&self.data_dir, options, &mut report)? {
                                resource_paused = true;
                                inventory_complete = false;
                                return Ok(false);
                            }
                            self.drain_pending_with_rotation(
                                &base_ids,
                                &mut ids,
                                options,
                                &resources,
                                &limits,
                                chunker,
                                &mut writer,
                                &mut session_predecessors,
                                &mut trace_artifacts,
                                &mut pending,
                                &mut report,
                                started_ms,
                            )?;
                        }
                    }
                    Ok(true)
                },
            )?;
            inventory_complete &= suffix_complete;
        }

        if inventory_complete {
            'extras: for sources in options.additional_artifacts.chunks(batch_size) {
                let prepared_sources =
                    self.prepare_import_sources(sources.to_vec(), &resources, &mut report)?;
                for source in prepared_sources {
                    if self.prepare_import_source(&ids, source, &mut pending, &mut report)?
                        && report.attempted as usize + pending.len() >= max_attempts
                    {
                        inventory_complete = false;
                        break 'extras;
                    }
                    if pending.len() >= batch_size {
                        if check_disk_pressure(&self.data_dir, options, &mut report)? {
                            resource_paused = true;
                            inventory_complete = false;
                            break 'extras;
                        }
                        self.drain_pending_with_rotation(
                            &base_ids,
                            &mut ids,
                            options,
                            &resources,
                            &limits,
                            chunker,
                            &mut writer,
                            &mut session_predecessors,
                            &mut trace_artifacts,
                            &mut pending,
                            &mut report,
                            started_ms,
                        )?;
                    }
                }
            }
        }
        if !resource_paused && !pending.is_empty() && (report.attempted as usize) < max_attempts {
            let remaining = max_attempts - report.attempted as usize;
            if pending.len() > remaining {
                pending.truncate(remaining);
                inventory_complete = false;
            }
            if check_disk_pressure(&self.data_dir, options, &mut report)? {
                inventory_complete = false;
            } else {
                self.drain_pending_with_rotation(
                    &base_ids,
                    &mut ids,
                    options,
                    &resources,
                    &limits,
                    chunker,
                    &mut writer,
                    &mut session_predecessors,
                    &mut trace_artifacts,
                    &mut pending,
                    &mut report,
                    started_ms,
                )?;
            }
        }
        report.limit_reached = !inventory_complete;
        // A completed batch ends in an append-only persisted checkpoint so
        // mixed-mode trace pages can open this active pack through its index
        // instead of rescanning every preceding body record. Empty resumptions
        // do not append another identical snapshot.
        if report.attempted > 0 {
            writer.checkpoint()?;
        } else {
            writer.flush()?;
        }
        writer.get_ref().sync_all()?;
        self.update_import_file_size(&ids)?;

        let current = self
            .lar_migration_job(&ids.job_id)?
            .context("legacy LAR migration job disappeared")?;
        if inventory_complete && current.pending_count == 0 && current.failed_count == 0 {
            self.complete_lar_migration_job(&ids.job_id, &options.lease_owner, now_ms())?;
            visit_import_boundary(options, LarLegacyImportBoundary::JobCompleted)?;
        } else {
            self.release_import_lease(
                &ids.job_id,
                &options.lease_owner,
                current.failed_count > 0,
                now_ms(),
            )?;
        }
        report.job_state = self
            .lar_migration_job(&ids.job_id)?
            .context("legacy LAR migration job disappeared after import")?
            .state;
        self.backfill_import_conversations(options, &mut report)?;
        self.finalize_import_report(&mut report, &resources, run_started.elapsed())?;
        Ok(report)
    }

    fn backfill_import_conversations(
        &self,
        options: &LarLegacyImportOptions,
        report: &mut LarLegacyImportReport,
    ) -> Result<()> {
        let limit = options
            .limit
            .unwrap_or_else(|| options.resources.effective_batch_size(options.batch_size))
            .clamp(1, MAX_BATCH_SIZE);
        let migration_limit_reached = report.limit_reached;
        loop {
            let backfill = self.backfill_lar_conversations(limit)?;
            report.limit_reached = migration_limit_reached || backfill.remaining;
            if options.limit.is_some() || !backfill.remaining {
                return Ok(());
            }
            if backfill.populated == 0 {
                bail!("conversation catalog backfill made no progress");
            }
        }
    }

    /// Read a cataloged manifest through the ordinary archive reader.
    pub fn read_lar_manifest_body(&self, manifest_id: &str) -> Result<Vec<u8>> {
        if let Some(body) = self.read_catalog_manifest_body(manifest_id)? {
            return Ok(body);
        }
        let (path, file_uuid, file_state): (String, String, String) = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT f.path, f.file_uuid, f.state FROM lar_manifests m
                 JOIN lar_files f ON f.file_uuid=m.file_uuid
                 WHERE m.manifest_id=?1 AND m.state='ready'",
                [manifest_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .with_context(|| format!("locating LAR manifest {manifest_id}"))?
        };
        let path = resolve_source_path(&self.data_dir, &path);
        if !matches!(file_state.as_str(), "active" | "sealed") {
            return Err(
                LarArchiveUnavailableError::offline(file_uuid, path.to_string_lossy()).into(),
            );
        }
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(
                    LarArchiveUnavailableError::missing(file_uuid, path.to_string_lossy()).into(),
                )
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("opening LAR archive at {}", path.display()))
            }
        };
        let id = ManifestId::from_str(manifest_id)?;
        let mut reader = ArchiveReader::open(file, Limits::default())?;
        reader.read_body(&id).map_err(Into::into)
    }

    /// Resolve a batch of mixed legacy/LAR bodies under one reconstructed-byte
    /// budget. Archive-backed manifests share readers, catalog-only manifests
    /// may span several live packs, and every request receives an explicit
    /// result rather than allowing one failure to erase the whole batch.
    pub fn read_lar_or_legacy_artifact_batch_bounded(
        &self,
        requests: &[LarArtifactReadRequest],
        byte_budget: u64,
    ) -> Vec<LarArtifactBatchRead> {
        let mut output = vec![LarArtifactBatchRead::Missing; requests.len()];
        let mut lar = Vec::new();
        let mut remaining = byte_budget;
        for (index, request) in requests.iter().enumerate() {
            let location = self.lar_artifact_location(
                &request.owner_kind,
                &request.owner_id,
                &request.artifact_kind,
                request.stage_id.as_deref(),
            );
            match location {
                Ok(Some(LarArtifactLocation::Lar {
                    manifest_id,
                    total_length,
                    ..
                })) => {
                    if total_length > remaining {
                        output[index] = LarArtifactBatchRead::Truncated {
                            total_length: Some(total_length),
                            budget_remaining: remaining,
                        };
                    } else {
                        remaining -= total_length;
                        lar.push((index, manifest_id));
                    }
                }
                Ok(Some(LarArtifactLocation::Legacy { path, .. })) => {
                    let path = resolve_source_path(&self.data_dir, &path);
                    let read = (|| -> Result<Vec<u8>> {
                        let file = File::open(&path).with_context(|| {
                            format!("opening legacy body at {}", path.display())
                        })?;
                        let decoder = GzDecoder::new(file);
                        let mut limited = decoder.take(remaining.saturating_add(1));
                        let mut bytes = Vec::new();
                        limited.read_to_end(&mut bytes).with_context(|| {
                            format!("decompressing legacy body at {}", path.display())
                        })?;
                        Ok(bytes)
                    })();
                    match read {
                        Ok(bytes) if bytes.len() as u64 <= remaining => {
                            remaining -= bytes.len() as u64;
                            output[index] = LarArtifactBatchRead::Read(bytes);
                        }
                        Ok(_) => {
                            output[index] = LarArtifactBatchRead::Truncated {
                                // Determining the exact gzip expansion would
                                // defeat the page budget. The caller still gets
                                // an explicit omission with its remaining cap.
                                total_length: None,
                                budget_remaining: remaining,
                            };
                            remaining = 0;
                        }
                        Err(error) => {
                            output[index] = LarArtifactBatchRead::Error {
                                kind: "legacy_read".into(),
                                detail: format!("{error:#}"),
                            };
                        }
                    }
                }
                Ok(Some(LarArtifactLocation::Unavailable { error, .. })) => {
                    output[index] = LarArtifactBatchRead::Error {
                        kind: error.kind,
                        detail: error.detail,
                    };
                }
                Ok(None) => {}
                Err(error) => {
                    output[index] = LarArtifactBatchRead::Error {
                        kind: "catalog".into(),
                        detail: format!("{error:#}"),
                    };
                }
            }
        }

        let mut archives: HashMap<(PathBuf, String), Vec<(usize, ManifestId)>> = HashMap::new();
        let mut catalog_manifests = Vec::new();
        if !lar.is_empty() {
            let conn = self.conn.lock().unwrap();
            let statement = conn.prepare(
                "SELECT f.path, f.file_uuid, f.state FROM lar_manifests m
                 JOIN lar_files f ON f.file_uuid=m.file_uuid
                 WHERE m.manifest_id=?1 AND m.state='ready'",
            );
            match statement {
                Ok(mut statement) => {
                    for (index, manifest_id) in lar {
                        let location = statement
                            .query_row([&manifest_id], |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, String>(2)?,
                                ))
                            })
                            .optional();
                        match location {
                            Ok(Some((path, file_uuid, state))) => {
                                let resolved = resolve_source_path(&self.data_dir, &path);
                                if !matches!(state.as_str(), "active" | "sealed") {
                                    let error = LarArchiveUnavailableError::offline(
                                        file_uuid,
                                        resolved.to_string_lossy(),
                                    );
                                    output[index] = LarArtifactBatchRead::ArchiveUnavailable(error);
                                    continue;
                                }
                                if !resolved.exists() {
                                    let error = LarArchiveUnavailableError::missing(
                                        file_uuid,
                                        resolved.to_string_lossy(),
                                    );
                                    output[index] = LarArtifactBatchRead::ArchiveUnavailable(error);
                                    continue;
                                }
                                match ManifestId::from_str(&manifest_id) {
                                    Ok(id) => archives
                                        .entry((resolved, file_uuid))
                                        .or_default()
                                        .push((index, id)),
                                    Err(error) => {
                                        output[index] = LarArtifactBatchRead::Error {
                                            kind: "manifest_id".into(),
                                            detail: error.to_string(),
                                        };
                                    }
                                }
                            }
                            Ok(None) => catalog_manifests.push((index, manifest_id)),
                            Err(error) => {
                                output[index] = LarArtifactBatchRead::Error {
                                    kind: "catalog".into(),
                                    detail: format!("locating LAR manifest {manifest_id}: {error}"),
                                };
                            }
                        }
                    }
                }
                Err(error) => {
                    for (index, _) in lar {
                        output[index] = LarArtifactBatchRead::Error {
                            kind: "catalog".into(),
                            detail: format!("preparing LAR manifest lookup: {error}"),
                        };
                    }
                }
            }
        }
        for ((path, file_uuid), manifests) in archives {
            let opened = File::open(&path)
                .with_context(|| format!("opening LAR archive at {}", path.display()))
                .and_then(|file| {
                    ArchiveReader::open(file, Limits::default()).map_err(anyhow::Error::new)
                });
            match opened {
                Ok(mut reader) => {
                    for (index, manifest_id) in manifests {
                        output[index] = match reader.read_body(&manifest_id) {
                            Ok(bytes) => LarArtifactBatchRead::Read(bytes),
                            Err(error) => LarArtifactBatchRead::Error {
                                kind: "archive_read".into(),
                                detail: error.to_string(),
                            },
                        };
                    }
                }
                Err(error) => {
                    let missing = !path.exists();
                    for (index, _) in manifests {
                        output[index] = if missing {
                            LarArtifactBatchRead::ArchiveUnavailable(
                                LarArchiveUnavailableError::missing(
                                    file_uuid.clone(),
                                    path.to_string_lossy(),
                                ),
                            )
                        } else {
                            LarArtifactBatchRead::Error {
                                kind: "archive_open".into(),
                                detail: format!("{error:#}"),
                            }
                        };
                    }
                }
            }
        }
        for (index, manifest_id) in catalog_manifests {
            output[index] = match self.read_lar_manifest_body(&manifest_id) {
                Ok(bytes) => LarArtifactBatchRead::Read(bytes),
                Err(error) => match error.downcast_ref::<LarArchiveUnavailableError>() {
                    Some(unavailable) => {
                        LarArtifactBatchRead::ArchiveUnavailable(unavailable.clone())
                    }
                    None => LarArtifactBatchRead::Error {
                        kind: "catalog_manifest_read".into(),
                        detail: format!("{error:#}"),
                    },
                },
            };
        }
        output
    }

    /// Compatibility wrapper for callers that need all-or-error semantics and
    /// do not impose a reconstructed-byte cap.
    pub fn read_lar_or_legacy_artifact_batch(
        &self,
        requests: &[LarArtifactReadRequest],
    ) -> Result<Vec<Option<Vec<u8>>>> {
        self.read_lar_or_legacy_artifact_batch_bounded(requests, u64::MAX)
            .into_iter()
            .map(|outcome| match outcome {
                LarArtifactBatchRead::Read(bytes) => Ok(Some(bytes)),
                LarArtifactBatchRead::Missing => Ok(None),
                LarArtifactBatchRead::Truncated { .. } => {
                    bail!("artifact exceeds the unbounded reader address space")
                }
                LarArtifactBatchRead::Error { kind, detail } => {
                    bail!("artifact read failed ({kind}): {detail}")
                }
                LarArtifactBatchRead::ArchiveUnavailable(error) => {
                    bail!("artifact read failed ({}): {error}", error.code())
                }
            })
            .collect()
    }

    /// Mixed-mode body reader for startup migration: validated LAR wins, while
    /// an unconverted legacy gzip remains readable from its original path.
    pub fn read_lar_or_legacy_artifact(
        &self,
        owner_kind: &str,
        owner_id: &str,
        artifact_kind: &str,
        stage_id: Option<&str>,
    ) -> Result<Option<Vec<u8>>> {
        match self.lar_artifact_location(owner_kind, owner_id, artifact_kind, stage_id)? {
            Some(LarArtifactLocation::Lar { manifest_id, .. }) => {
                self.read_lar_manifest_body(&manifest_id).map(Some)
            }
            Some(LarArtifactLocation::Legacy { path, .. }) => {
                let path = resolve_source_path(&self.data_dir, &path);
                let file = File::open(&path)
                    .with_context(|| format!("opening legacy body at {}", path.display()))?;
                let mut decoder = GzDecoder::new(file);
                let mut bytes = Vec::new();
                decoder
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("decompressing legacy body at {}", path.display()))?;
                Ok(Some(bytes))
            }
            Some(LarArtifactLocation::Unavailable { error, .. }) => {
                bail!(
                    "legacy artifact is unavailable ({}): {}",
                    error.kind,
                    error.detail
                )
            }
            None => Ok(None),
        }
    }

    fn backfill_import_archive_catalog(
        &self,
        ids: &ImportIds,
        limits: &Limits,
        controls: &LarLegacyResourceControls,
    ) -> Result<()> {
        let file = match File::open(&ids.file_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if self.catalog_import_pack_is_complete(ids)? {
            return Ok(());
        }
        let size = file.metadata()?.len();
        if size > controls.max_pack_bytes
            || self.catalog_import_pack_index_entries(ids)?
                > controls.effective_pack_index_entries()
        {
            bail!(
                "legacy import pack {} needs catalog repair but exceeds the configured bounded-open limits",
                ids.file_uuid
            );
        }
        let reader = ArchiveReader::open(file, limits.clone())?;
        let manifest_ids: Vec<ManifestId> = reader.manifest_ids().copied().collect();
        self.catalog_synced_archive_manifests(&ids.file_uuid, &reader, &manifest_ids)
    }

    fn legacy_column_artifacts(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<LarLegacyArtifact>> {
        let offset = i64::try_from(offset).context("legacy inventory offset is too large")?;
        let limit = i64::try_from(limit).context("legacy inventory batch is too large")?;
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT owner_kind, owner_id, session_id, artifact_kind, source_path FROM (
               SELECT 'trace' owner_kind, id owner_id, session_id, 'client_request' artifact_kind,
                      req_body_path source_path, ts_request_ms sort_ms, 0 artifact_order
                      FROM traces WHERE req_body_path IS NOT NULL
               UNION ALL
               SELECT 'trace', id, session_id, 'upstream_request', upstream_req_body_path,
                      ts_request_ms, 1
                      FROM traces WHERE upstream_req_body_path IS NOT NULL
               UNION ALL
               SELECT 'trace', id, session_id, 'client_response', resp_body_path,
                      ts_request_ms, 2
                      FROM traces WHERE resp_body_path IS NOT NULL
               UNION ALL
               SELECT 'tool_call', id, session_id, 'tool_arguments', args_body_path,
                      ts_start_ms, 0
                      FROM tool_calls WHERE args_body_path IS NOT NULL
               UNION ALL
               SELECT 'tool_call', id, session_id, 'tool_result', result_body_path,
                      ts_start_ms, 1
                      FROM tool_calls WHERE result_body_path IS NOT NULL
             ) ORDER BY owner_kind, COALESCE(session_id, ''), sort_ms, owner_id, artifact_order
               LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit, offset], |row| {
            Ok(LarLegacyArtifact {
                owner_kind: row.get(0)?,
                owner_id: row.get(1)?,
                session_id: row.get(2)?,
                artifact_kind: row.get(3)?,
                stage_id: None,
                source_path: row.get(4)?,
                fidelity: "legacy_exact_body".into(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn visit_legacy_suffix_artifact_batches<F>(
        &self,
        specs: &[LarLegacySuffixArtifact],
        batch_size: usize,
        mut visit: F,
    ) -> Result<bool>
    where
        F: FnMut(Vec<LarLegacyArtifact>) -> Result<bool>,
    {
        let bodies = self.data_dir.join("bodies");
        if !bodies.is_dir() || specs.is_empty() {
            return Ok(true);
        }
        let mut batch = Vec::with_capacity(batch_size);
        for day in std::fs::read_dir(&bodies)? {
            let day = day?;
            if !day.file_type()?.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(day.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_file() {
                    continue;
                }
                let filename = entry.file_name().to_string_lossy().into_owned();
                for (spec_index, spec) in specs.iter().enumerate() {
                    let marker = format!(".{}", spec.file_suffix);
                    let Some(trace_id) = filename.strip_suffix(&marker) else {
                        continue;
                    };
                    if specs[..spec_index].iter().any(|previous| {
                        previous.artifact_kind == spec.artifact_kind
                            && previous.file_suffix == spec.file_suffix
                            && previous.require_via_dario == spec.require_via_dario
                    }) {
                        continue;
                    }
                    let trace: Option<(Option<String>, bool)> = {
                        let conn = self.conn.lock().unwrap();
                        conn.query_row(
                            "SELECT session_id, via_dario != 0 FROM traces WHERE id=?1",
                            [trace_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()?
                    };
                    let Some((session_id, via_dario)) = trace else {
                        continue;
                    };
                    if spec.require_via_dario && !via_dario {
                        continue;
                    }
                    batch.push(LarLegacyArtifact {
                        owner_kind: "trace".into(),
                        owner_id: trace_id.into(),
                        session_id,
                        artifact_kind: spec.artifact_kind.clone(),
                        stage_id: None,
                        source_path: entry.path().to_string_lossy().into_owned(),
                        fidelity: "legacy_exact_body".into(),
                    });
                    if batch.len() == batch_size && !visit(std::mem::take(&mut batch))? {
                        return Ok(false);
                    }
                }
            }
        }
        if !batch.is_empty() && !visit(batch)? {
            return Ok(false);
        }
        Ok(true)
    }

    fn prepare_import_sources(
        &self,
        sources: Vec<LarLegacyArtifact>,
        resources: &ResourceController,
        report: &mut LarLegacyImportReport,
    ) -> Result<Vec<PreparedSource>> {
        report.inventoried = report.inventoried.saturating_add(sources.len() as u64);
        let mut needed = Vec::new();
        for source in sources {
            if self.legacy_source_needs_import(&source)? {
                needed.push(source);
            } else {
                report.skipped = report.skipped.saturating_add(1);
            }
        }
        Ok(prepare_provenance_parallel(
            &self.data_dir,
            needed,
            resources,
        ))
    }

    fn prepare_import_source(
        &self,
        ids: &ImportIds,
        prepared: PreparedSource,
        pending: &mut Vec<PendingArtifact>,
        report: &mut LarLegacyImportReport,
    ) -> Result<bool> {
        let PreparedSource {
            source,
            resolved_path,
            provenance,
        } = prepared;
        let already_migrated: bool = self.conn.lock().unwrap().query_row(
            "SELECT EXISTS(
               SELECT 1 FROM lar_migration_items
                WHERE owner_kind=?1 AND owner_id=?2 AND artifact_kind=?3
                  AND stage_id=?4 AND source_fingerprint=?5
                  AND state='migrated' AND validation_state='validated'
             )",
            params![
                source.owner_kind,
                source.owner_id,
                source.artifact_kind,
                source.stage_id.as_deref().unwrap_or(""),
                provenance.fingerprint,
            ],
            |row| row.get(0),
        )?;
        if already_migrated {
            report.skipped = report.skipped.saturating_add(1);
            return Ok(false);
        }
        let item_id = item_id(&ids.job_id, &source, &provenance.fingerprint);
        self.discover_lar_migration_item(
            &LarMigrationItem {
                item_id: item_id.clone(),
                job_id: ids.job_id.clone(),
                owner_kind: source.owner_kind.clone(),
                owner_id: source.owner_id.clone(),
                artifact_kind: source.artifact_kind.clone(),
                stage_id: source.stage_id.clone(),
                source_path: Some(source.source_path.clone()),
                source_size: provenance.size,
                source_mtime_ms: provenance.mtime_ms,
                source_fingerprint: provenance.fingerprint,
                fidelity: source.fidelity.clone(),
            },
            now_ms(),
        )?;
        let state: String = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT state FROM lar_migration_items WHERE item_id=?1",
                [&item_id],
                |row| row.get(0),
            )?
        };
        if state == "pending" || state == "migrating" {
            pending.push(PendingArtifact {
                source,
                resolved_path,
                item_id,
            });
            Ok(true)
        } else {
            report.skipped += 1;
            Ok(false)
        }
    }

    fn legacy_predecessor_manifest(
        &self,
        source: &LarLegacyArtifact,
    ) -> Result<Option<ManifestId>> {
        let Some((session_id, artifact_kind)) = semantic_predecessor_key(source) else {
            return Ok(None);
        };
        let conn = self.conn.lock().unwrap();
        let identity: Option<(i64, String)> = conn
            .query_row(
                "SELECT ts_request_ms, id FROM traces WHERE id=?1 AND session_id=?2",
                params![source.owner_id, session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((timestamp, trace_id)) = identity else {
            return Ok(None);
        };
        let manifest: Option<String> = conn
            .query_row(
                "SELECT a.manifest_id
                   FROM traces predecessor
                   JOIN lar_trace_artifacts a
                     ON a.owner_kind='trace' AND a.owner_id=predecessor.id
                    AND a.artifact_kind=?1 AND a.stage_id=''
                   JOIN lar_manifests m
                     ON m.manifest_id=a.manifest_id AND m.state='ready'
                  WHERE predecessor.session_id=?2
                    AND (predecessor.ts_request_ms < ?3
                         OR (predecessor.ts_request_ms = ?3 AND predecessor.id < ?4))
                  ORDER BY predecessor.ts_request_ms DESC, predecessor.id DESC
                  LIMIT 1",
                params![artifact_kind, session_id, timestamp, trace_id],
                |row| row.get(0),
            )
            .optional()?;
        manifest
            .map(|value| ManifestId::from_str(&value).map_err(Into::into))
            .transpose()
    }

    fn legacy_same_trace_predecessor(
        &self,
        source: &LarLegacyArtifact,
    ) -> Result<Option<ManifestId>> {
        let candidates = same_trace_predecessor_kinds(source);
        if candidates.is_empty() {
            return Ok(None);
        }
        let conn = self.conn.lock().unwrap();
        for artifact_kind in candidates {
            let manifest: Option<String> = conn
                .query_row(
                    "SELECT a.manifest_id
                       FROM lar_trace_artifacts a
                       JOIN lar_manifests m
                         ON m.manifest_id=a.manifest_id AND m.state='ready'
                      WHERE a.owner_kind='trace' AND a.owner_id=?1
                        AND a.artifact_kind=?2 AND a.stage_id=''
                      LIMIT 1",
                    params![source.owner_id, artifact_kind],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(manifest) = manifest {
                return Ok(Some(ManifestId::from_str(&manifest)?));
            }
        }
        Ok(None)
    }

    fn append_legacy_with_predecessor(
        &self,
        writer: &mut ArchiveWriter<File>,
        source_path: &Path,
        file: File,
        predecessor: ManifestId,
        limits: &Limits,
        resources: &ResourceController,
    ) -> alex_lar::Result<(ManifestId, u64, [u8; 32])> {
        let memory_limit = RangeMatchConfig::default()
            .max_current_bytes
            .min(usize::try_from(resources.controls.max_memory_bytes / 2).unwrap_or(usize::MAX))
            .min(usize::try_from(limits.max_body_length).unwrap_or(usize::MAX));
        let mut source_reader = HashingReader::new(GzDecoder::new(ThrottledReader::new(
            file,
            resources.clone(),
        )));
        let mut body = Vec::with_capacity(memory_limit.min(1024 * 1024));
        let mut buffer = [0u8; 64 * 1024];
        let exceeded = loop {
            let read = source_reader.read(&mut buffer)?;
            if read == 0 {
                break false;
            }
            if body.len().saturating_add(read) > memory_limit {
                break true;
            }
            body.extend_from_slice(&buffer[..read]);
        };
        if exceeded {
            // The predecessor path stays bounded: large bodies are reopened and
            // fed to the ordinary streaming CDC writer instead of being held in
            // memory or partially range encoded.
            let file = File::open(source_path)?;
            let mut streaming = HashingReader::new(GzDecoder::new(ThrottledReader::new(
                file,
                resources.clone(),
            )));
            let manifest_id = writer.append_reader(&mut streaming)?;
            let (source_length, source_hash) = streaming.identity();
            return Ok((manifest_id, source_length, source_hash));
        }
        let (source_length, source_hash) = source_reader.identity();
        let manifest_id = writer.append_body_with_predecessor(&body, predecessor)?;
        Ok((manifest_id, source_length, source_hash))
    }

    #[allow(clippy::too_many_arguments)]
    fn drain_pending_with_rotation(
        &self,
        base: &ImportIds,
        ids: &mut ImportIds,
        options: &LarLegacyImportOptions,
        resources: &ResourceController,
        limits: &Limits,
        chunker: ChunkerConfig,
        writer: &mut ArchiveWriter<File>,
        session_predecessors: &mut HashMap<(String, String), ManifestId>,
        trace_artifacts: &mut HashMap<(String, String), ManifestId>,
        pending: &mut Vec<PendingArtifact>,
        report: &mut LarLegacyImportReport,
        started_ms: i64,
    ) -> Result<()> {
        while !pending.is_empty() {
            if import_pack_limit_reached(writer, &options.resources)? {
                self.rotate_import_pack(base, ids, writer, report, started_ms, chunker, limits)?;
                // Cached predecessors can name manifests in the sealed pack.
                // Clear them at the boundary; any durable SQL predecessor ID
                // absent from the new writer safely falls back to ordinary CDC.
                session_predecessors.clear();
                trace_artifacts.clear();
            }
            let full = self.import_pending_batch(
                ids,
                options,
                resources,
                limits,
                writer,
                session_predecessors,
                trace_artifacts,
                pending,
                report,
            )?;
            if !full {
                break;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn import_pending_batch(
        &self,
        ids: &ImportIds,
        options: &LarLegacyImportOptions,
        resources: &ResourceController,
        limits: &Limits,
        writer: &mut ArchiveWriter<File>,
        session_predecessors: &mut HashMap<(String, String), ManifestId>,
        trace_artifacts: &mut HashMap<(String, String), ManifestId>,
        pending: &mut Vec<PendingArtifact>,
        report: &mut LarLegacyImportReport,
    ) -> Result<bool> {
        if pending.is_empty() {
            return Ok(false);
        }
        if !self.renew_lar_migration_lease(
            &ids.job_id,
            &options.lease_owner,
            now_ms(),
            options.lease_duration,
        )? {
            bail!("lost the LAR legacy migration lease before importing a batch");
        }
        let artifacts = std::mem::take(pending);
        let mut artifacts = artifacts.into_iter();
        let mut validations = Vec::with_capacity(artifacts.len());
        while let Some(artifact) = artifacts.next() {
            let cpu_started = Instant::now();
            report.attempted += 1;
            let file = match File::open(&artifact.resolved_path) {
                Ok(file) => file,
                Err(error) => {
                    let kind = if error.kind() == std::io::ErrorKind::NotFound {
                        "missing"
                    } else {
                        "source_io"
                    };
                    self.record_import_failure(
                        ids,
                        options,
                        &artifact,
                        kind,
                        &error.to_string(),
                        report,
                    )?;
                    continue;
                }
            };
            let before = writer.chunk_uncompressed_bytes();
            let predecessor_key = semantic_predecessor_key(&artifact.source);
            let same_trace = same_trace_predecessor_kinds(&artifact.source)
                .iter()
                .find_map(|kind| {
                    trace_artifacts
                        .get(&(artifact.source.owner_id.clone(), (*kind).to_string()))
                        .copied()
                })
                .or(self.legacy_same_trace_predecessor(&artifact.source)?);
            let predecessor = same_trace.or(match predecessor_key.as_ref() {
                Some(key) => session_predecessors
                    .get(key)
                    .copied()
                    .or(self.legacy_predecessor_manifest(&artifact.source)?),
                None => None,
            });
            let imported = if let Some(predecessor) = predecessor {
                self.append_legacy_with_predecessor(
                    writer,
                    &artifact.resolved_path,
                    file,
                    predecessor,
                    limits,
                    resources,
                )
            } else {
                let mut source_reader = HashingReader::new(GzDecoder::new(ThrottledReader::new(
                    file,
                    resources.clone(),
                )));
                writer.append_reader(&mut source_reader).map(|manifest_id| {
                    let (source_length, source_hash) = source_reader.identity();
                    (manifest_id, source_length, source_hash)
                })
            };
            let (manifest_id, source_length, source_hash) = match imported {
                Ok(value) => value,
                Err(error) => {
                    self.record_import_failure(
                        ids,
                        options,
                        &artifact,
                        "corrupt",
                        &format!("legacy gzip could not be decoded: {error}"),
                        report,
                    )?;
                    continue;
                }
            };
            resources.finish_cpu_slice(cpu_started.elapsed());
            if let Some(key) = predecessor_key {
                session_predecessors.insert(key, manifest_id);
            }
            if artifact.source.owner_kind == "trace"
                && matches!(
                    artifact.source.artifact_kind.as_str(),
                    "client_request" | "upstream_request" | "dario_upstream_request"
                )
            {
                trace_artifacts.insert(
                    (
                        artifact.source.owner_id.clone(),
                        artifact.source.artifact_kind.clone(),
                    ),
                    manifest_id,
                );
            }
            let cache_entry_limit = usize::try_from(
                resources
                    .controls
                    .max_memory_bytes
                    .saturating_div(2)
                    .saturating_div(256),
            )
            .unwrap_or(usize::MAX)
            .max(1);
            if session_predecessors.len() > cache_entry_limit {
                session_predecessors.clear();
            }
            if trace_artifacts.len() > cache_entry_limit {
                trace_artifacts.clear();
            }
            let unique_bytes_written = writer.chunk_uncompressed_bytes().saturating_sub(before);
            validations.push(PendingValidation {
                source: artifact.source,
                item_id: artifact.item_id,
                manifest_id,
                source_length,
                source_hash,
                unique_bytes_written,
            });
            if import_pack_limit_reached(writer, &options.resources)? {
                pending.extend(artifacts);
                break;
            }
        }
        writer.flush()?;
        writer.get_ref().sync_all()?;
        visit_import_boundary(options, LarLegacyImportBoundary::BodyAppended)?;
        if validations.is_empty() {
            return Ok(import_pack_limit_reached(writer, &options.resources)?);
        }
        let read_file = File::open(&ids.file_path)?;
        let mut reader = match ArchiveReader::open(read_file, limits.clone()) {
            Ok(reader) => reader,
            Err(error) => {
                for validation in validations {
                    let artifact = PendingArtifact {
                        resolved_path: resolve_source_path(
                            &self.data_dir,
                            &validation.source.source_path,
                        ),
                        item_id: validation.item_id,
                        source: validation.source,
                    };
                    self.record_import_failure(
                        ids,
                        options,
                        &artifact,
                        "validation",
                        &format!("reopening the LAR archive failed: {error}"),
                        report,
                    )?;
                }
                return Ok(import_pack_limit_reached(writer, &options.resources)?);
            }
        };
        let mut last_committed_item = None;
        for validation in validations {
            if !self.renew_lar_migration_lease(
                &ids.job_id,
                &options.lease_owner,
                now_ms(),
                options.lease_duration,
            )? {
                bail!("lost the LAR legacy migration lease during readback validation");
            }
            let manifest = reader.manifest(&validation.manifest_id).cloned();
            let mut reconstructed = HashingWriter::default();
            let read_result = reader.write_body(&validation.manifest_id, &mut reconstructed);
            let reconstructed_hash = *reconstructed.hasher.finalize().as_bytes();
            let failure = match (manifest.as_ref(), read_result) {
                (None, _) => Some("the appended manifest was absent after reopen".to_string()),
                (_, Err(error)) => Some(format!("normal ArchiveReader readback failed: {error}")),
                (Some(_), Ok(length))
                    if length != validation.source_length
                        || reconstructed.length != validation.source_length =>
                {
                    Some(format!(
                        "readback length mismatch: source {}, reconstructed {}",
                        validation.source_length, reconstructed.length
                    ))
                }
                (Some(_), Ok(_)) if reconstructed_hash != validation.source_hash => {
                    Some("readback BLAKE3 mismatch".into())
                }
                _ => None,
            };
            if let Some(detail) = failure {
                let artifact = PendingArtifact {
                    resolved_path: resolve_source_path(
                        &self.data_dir,
                        &validation.source.source_path,
                    ),
                    item_id: validation.item_id,
                    source: validation.source,
                };
                self.record_import_failure(ids, options, &artifact, "validation", &detail, report)?;
                continue;
            }
            self.catalog_synced_archive_manifests(
                &ids.file_uuid,
                &reader,
                &[validation.manifest_id],
            )?;
            visit_import_boundary(options, LarLegacyImportBoundary::BodyValidated)?;
            let deduplicated = validation
                .source_length
                .saturating_sub(validation.unique_bytes_written);
            let conversation_trace_id = (validation.source.owner_kind == "trace"
                && matches!(
                    validation.source.artifact_kind.as_str(),
                    "client_request" | "client_response"
                ))
            .then(|| validation.source.owner_id.clone());
            let pointer_published = match self.switch_validated_lar_artifact(
                &ids.job_id,
                &validation.item_id,
                &options.lease_owner,
                &validation.manifest_id.to_string(),
                &LarValidation {
                    source_length: validation.source_length,
                    source_hash_algorithm: "blake3".into(),
                    source_hash: validation.source_hash.to_vec(),
                    reconstructed_length: reconstructed.length,
                    reconstructed_hash: reconstructed_hash.to_vec(),
                    bytes_read: validation.source_length,
                    unique_bytes_written: validation.unique_bytes_written,
                    bytes_deduplicated: deduplicated,
                },
                validation.source.session_id.as_deref(),
                now_ms(),
            )? {
                LarPointerSwitch::Switched { .. } => {
                    visit_import_boundary(options, LarLegacyImportBoundary::PointerSwitched)?;
                    last_committed_item = Some(validation.item_id.clone());
                    report.migrated += 1;
                    report.bytes_read += validation.source_length;
                    report.unique_bytes_written += validation.unique_bytes_written;
                    report.bytes_deduplicated += deduplicated;
                    true
                }
                LarPointerSwitch::AlreadySwitched { .. } => {
                    report.skipped += 1;
                    true
                }
                LarPointerSwitch::ValidationFailed { reason } => {
                    report.failed += 1;
                    report.push_error(LarLegacyImportError {
                        item_id: validation.item_id,
                        owner_kind: validation.source.owner_kind,
                        owner_id: validation.source.owner_id,
                        artifact_kind: validation.source.artifact_kind,
                        error_kind: "validation".into(),
                        detail: reason,
                    });
                    false
                }
            };
            if pointer_published {
                if let Some(trace_id) = conversation_trace_id {
                    self.populate_lar_conversation_for_trace(
                        &trace_id,
                        crate::LarConversationEvidenceSource::Import,
                    )?;
                }
            }
        }
        if let Some(last) = last_committed_item {
            self.checkpoint_lar_migration_job(&ids.job_id, &options.lease_owner, &last, now_ms())?;
        }
        import_pack_limit_reached(writer, &options.resources)
    }

    fn record_import_failure(
        &self,
        ids: &ImportIds,
        options: &LarLegacyImportOptions,
        artifact: &PendingArtifact,
        error_kind: &str,
        detail: &str,
        report: &mut LarLegacyImportReport,
    ) -> Result<()> {
        self.record_lar_migration_item_failure(
            &ids.job_id,
            &artifact.item_id,
            &options.lease_owner,
            error_kind,
            detail,
            now_ms(),
        )?;
        report.failed += 1;
        report.push_error(LarLegacyImportError {
            item_id: artifact.item_id.clone(),
            owner_kind: artifact.source.owner_kind.clone(),
            owner_id: artifact.source.owner_id.clone(),
            artifact_kind: artifact.source.artifact_kind.clone(),
            error_kind: error_kind.into(),
            detail: detail.into(),
        });
        Ok(())
    }

    fn ensure_import_archive_file(&self, ids: &ImportIds, created_at_ms: i64) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO lar_archive_sets
               (archive_set_uuid, created_at_ms, updated_at_ms, description)
             VALUES (?1, ?2, ?2, 'legacy gzip import')
             ON CONFLICT(archive_set_uuid) DO UPDATE SET updated_at_ms=excluded.updated_at_ms",
            params![ids.archive_set_uuid, created_at_ms],
        )?;
        tx.execute(
            "INSERT INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, created_at_ms, size_bytes)
             VALUES (?1, ?2, 'body-pack', ?3, 'active', 1, 0, ?4, ?5)
             ON CONFLICT(file_uuid) DO NOTHING",
            params![
                ids.file_uuid,
                ids.archive_set_uuid,
                ids.file_path.to_string_lossy(),
                created_at_ms,
                i64::try_from(std::fs::metadata(&ids.file_path)?.len())?
            ],
        )?;
        let stored: (String, String, String) = tx.query_row(
            "SELECT archive_set_uuid, role, path FROM lar_files WHERE file_uuid=?1",
            [&ids.file_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if stored
            != (
                ids.archive_set_uuid.clone(),
                "body-pack".into(),
                ids.file_path.to_string_lossy().into_owned(),
            )
        {
            bail!("LAR file UUID is already registered with a different archive identity");
        }
        tx.commit()?;
        Ok(())
    }

    fn update_import_file_size(&self, ids: &ImportIds) -> Result<()> {
        let size = i64::try_from(std::fs::metadata(&ids.file_path)?.len())?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE lar_files SET size_bytes=?2 WHERE file_uuid=?1",
            params![ids.file_uuid, size],
        )?;
        Ok(())
    }

    fn finalize_import_report(
        &self,
        report: &mut LarLegacyImportReport,
        resources: &ResourceController,
        elapsed: Duration,
    ) -> Result<()> {
        if let Some(job) = self.lar_migration_job(&report.job_id)? {
            report.total_items = job.discovered_count;
            report.completed_items = job
                .migrated_count
                .saturating_add(job.skipped_count)
                .saturating_add(job.failed_count);
            report.remaining_items = job.pending_count;
            report.last_error = job.last_error;
        }
        let (
            elapsed_ms,
            bytes_per_second,
            artifacts_per_second,
            progress_percent,
            eta_seconds,
            dedup_ratio,
        ) = progress_metrics(
            elapsed,
            report.attempted,
            report.bytes_read,
            report.unique_bytes_written,
            report.total_items,
            report.completed_items,
        );
        report.elapsed_ms = elapsed_ms;
        report.throughput_bytes_per_second = bytes_per_second;
        report.throughput_artifacts_per_second = artifacts_per_second;
        report.progress_percent = progress_percent;
        report.eta_seconds = eta_seconds;
        report.dedup_ratio = dedup_ratio;
        report.workers_used = resources.workers_used.load(Ordering::Relaxed);
        report.throttled_ms = resources.throttled_ms();
        report.yield_count = resources.yield_count.load(Ordering::Relaxed);
        Ok(())
    }

    fn release_import_lease(
        &self,
        job_id: &str,
        lease_owner: &str,
        failed: bool,
        updated_at_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let state = if failed { "failed" } else { "pending" };
        let changed = conn.execute(
            "UPDATE lar_migration_jobs SET state=?3, lease_owner=NULL,
                    lease_expires_at_ms=NULL, updated_at_ms=?4
             WHERE job_id=?1 AND lease_owner=?2 AND state='running'",
            params![job_id, lease_owner, state, updated_at_ms],
        )?;
        if changed != 1 {
            bail!("lost the LAR migration lease before releasing it");
        }
        Ok(())
    }
}

#[cfg(test)]
mod resource_control_tests {
    use super::*;

    #[test]
    fn rate_and_cpu_delay_calculations_are_deterministic() {
        assert_eq!(
            required_io_delay(1_000, 1_000, Duration::from_millis(250)),
            Duration::from_millis(750)
        );
        assert_eq!(
            required_io_delay(1_000, 1_000, Duration::from_secs(2)),
            Duration::ZERO
        );
        assert_eq!(
            required_cpu_delay(Duration::from_millis(100), 25),
            Duration::from_millis(300)
        );
        assert_eq!(
            required_cpu_delay(Duration::from_millis(100), 100),
            Duration::ZERO
        );
    }

    #[test]
    fn progress_metrics_include_throughput_eta_progress_and_dedup() {
        let metrics = progress_metrics(Duration::from_secs(2), 4, 1_000, 250, 10, 4);
        assert_eq!(metrics.0, 2_000);
        assert_eq!(metrics.1, 500);
        assert_eq!(metrics.2, 2.0);
        assert_eq!(metrics.3, 40.0);
        assert_eq!(metrics.4, Some(3));
        assert_eq!(metrics.5, 0.75);
    }

    #[test]
    fn invalid_resource_bounds_are_rejected() {
        let mut controls = LarLegacyResourceControls::default();
        controls.worker_count = MAX_WORKER_COUNT + 1;
        assert!(controls
            .validate()
            .unwrap_err()
            .to_string()
            .contains("worker"));
        controls = LarLegacyResourceControls::default();
        controls.io_bytes_per_second = Some(0);
        assert!(controls.validate().unwrap_err().to_string().contains("I/O"));
        controls = LarLegacyResourceControls::default();
        controls.cpu_budget_percent = 0;
        assert!(controls.validate().unwrap_err().to_string().contains("CPU"));
        controls = LarLegacyResourceControls::default();
        controls.max_pack_bytes = 0;
        assert!(controls
            .validate()
            .unwrap_err()
            .to_string()
            .contains("pack byte"));
        controls = LarLegacyResourceControls::default();
        controls.max_pack_index_entries = 0;
        assert!(controls
            .validate()
            .unwrap_err()
            .to_string()
            .contains("index-entry"));
    }

    #[test]
    fn memory_budget_tightens_inventory_and_archive_index_caps() {
        let controls = LarLegacyResourceControls {
            max_memory_bytes: 1024 * 1024,
            max_pack_index_entries: usize::MAX,
            ..Default::default()
        };
        assert_eq!(controls.effective_batch_size(MAX_BATCH_SIZE), 256);
        assert_eq!(controls.effective_pack_index_entries(), 4096);
    }
}
