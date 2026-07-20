//! Shared legacy gzip-to-LAR importer used by both foreground commands and the
//! startup worker. The importer never deletes or clears legacy paths.
//!
//! Body bytes and legacy-normalized exchange metadata are written into the same
//! deterministic rolling body packs. Metadata records reference validated body
//! manifests and never copy request, response, Dario, or tool bytes.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alex_lar::{
    ArchiveReader, ArchiveWriter, ChunkerConfig, Exchange, ExchangeData, ExchangeMetadataData,
    FileHeader, FileRole, Limits, ManifestId, RangeMatchConfig, Stage, StageData, StageKind,
    TokenUsage, UnknownExchangeMetadataAttribute, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::{
    LarArchiveUnavailableError, LarArtifactError, LarArtifactLocation, LarExchangeCapture,
    LarHeaderCapture, LarMigrationItem, LarMigrationJobSpec, LarPointerSwitch, LarValidation,
    Store,
};

const FORMAT_VERSION: i64 = 1;
// v2 adds exchange/header/stage metadata. Keeping this distinct from the
// body-only v1 source version forces existing completed installations to run
// the additive metadata backfill while reusing their validated manifests.
const SOURCE_VERSION: &str = "legacy-gzip-v2";
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

#[cfg(test)]
static LEGACY_METADATA_PLAN_QUERY_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
fn note_legacy_metadata_plan_query() {
    LEGACY_METADATA_PLAN_QUERY_COUNT.fetch_add(1, Ordering::Relaxed);
}

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
    MetadataAppended,
    MetadataValidated,
    MetadataPublished,
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
    pub metadata_inventoried: u64,
    pub metadata_attempted: u64,
    pub metadata_migrated: u64,
    pub metadata_skipped: u64,
    pub metadata_failed: u64,
    pub metadata_unsupported: u64,
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
            metadata_inventoried: 0,
            metadata_attempted: 0,
            metadata_migrated: 0,
            metadata_skipped: 0,
            metadata_failed: 0,
            metadata_unsupported: 0,
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

#[derive(Clone, Debug)]
struct LegacyTraceMetadata {
    trace_id: String,
    session_id: Option<String>,
    run_id: Option<String>,
    ts_request_ms: i64,
    ts_response_ms: Option<i64>,
    request_headers_json: Option<String>,
    response_headers_json: Option<String>,
    request_body_path: Option<String>,
    upstream_request_body_path: Option<String>,
    response_body_path: Option<String>,
    provider: Option<String>,
    requested_model: Option<String>,
    routed_model: Option<String>,
    account_id: Option<String>,
    status: Option<i64>,
    error_class: Option<String>,
    error: Option<String>,
    substituted: bool,
    original_model: Option<String>,
    served_model: Option<String>,
    substitution_reason: Option<String>,
    attempts_json: Option<String>,
    original_account_id: Option<String>,
    served_account_id: Option<String>,
    injected: bool,
    fixture_name: Option<String>,
    via_dario: bool,
    dario_generation: Option<String>,
    harness: Option<String>,
    client_format: Option<String>,
    upstream_format: Option<String>,
    method: Option<String>,
    path: Option<String>,
    streamed: Option<bool>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cost_usd: Option<f64>,
    billing_bucket: Option<String>,
    error_kind: Option<String>,
    error_code: Option<String>,
    subscription_identity: Option<String>,
    tags_json: Option<String>,
    client_ip: Option<String>,
    key_fingerprint: Option<String>,
    reasoning_effort: Option<String>,
    thinking_budget: Option<i64>,
    synthetic_tool: Option<LegacyToolMetadata>,
}

#[derive(Clone, Debug)]
struct LegacyToolMetadata {
    id: String,
    harness: String,
    session_id: String,
    turn_id: Option<String>,
    trace_id: Option<String>,
    tool_call_id: String,
    tool_name: String,
    ts_start_ms: i64,
    ts_end_ms: Option<i64>,
    is_error: Option<bool>,
    exit_status: Option<i64>,
    arguments_path: Option<String>,
    result_path: Option<String>,
}

#[derive(Debug)]
struct ParsedLegacyHeaders {
    capture: Option<LarHeaderCapture>,
    unsupported: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct LegacyAttemptMetadata {
    account_id: Option<String>,
    model: Option<String>,
    routing_reason: Option<String>,
    opaque_json: Option<String>,
}

#[derive(Debug)]
struct LegacyMetadataPlan {
    owner_kind: &'static str,
    owner_id: String,
    trace_id: String,
    session_id: Option<String>,
    run_id: Option<String>,
    /// Complete SQLite source row retained for the optional exchange
    /// companion record. Stage fields consume the representable subset; the
    /// companion carries the remainder without copying any body bytes.
    #[allow(dead_code)]
    source_metadata: LegacyTraceMetadata,
    exchange_metadata: ExchangeMetadataData,
    catalog_capture: LarExchangeCapture,
    stages: Vec<StageData>,
    external_manifests: Vec<ManifestId>,
    fingerprint: String,
    source_size: u64,
    unsupported_count: u64,
    missing_manifests: Vec<String>,
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
    job_hasher.update(b"alex-lar-legacy-job-v2");
    job_hasher.update(SOURCE_VERSION.as_bytes());
    job_hasher.update(&[0]);
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

fn metadata_item_id(job_id: &str, owner_kind: &str, owner_id: &str, fingerprint: &str) -> String {
    let source = LarLegacyArtifact {
        owner_kind: owner_kind.into(),
        owner_id: owner_id.into(),
        session_id: None,
        artifact_kind: "exchange_metadata".into(),
        stage_id: None,
        source_path: String::new(),
        fidelity: "legacy_normalized".into(),
    };
    item_id(job_id, &source, fingerprint)
}

fn legacy_header_value(value: &serde_json::Value) -> Option<Vec<u8>> {
    match value {
        serde_json::Value::String(value) => Some(value.as_bytes().to_vec()),
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
            Some(value.to_string().into_bytes())
        }
        _ => None,
    }
}

fn parse_legacy_headers(source: Option<&str>) -> ParsedLegacyHeaders {
    let Some(source) = source else {
        return ParsedLegacyHeaders {
            capture: None,
            unsupported: None,
        };
    };
    let parsed = match serde_json::from_str::<serde_json::Value>(source) {
        Ok(parsed) => parsed,
        Err(error) => {
            return ParsedLegacyHeaders {
                capture: None,
                unsupported: Some(format!("invalid legacy header JSON: {error}")),
            };
        }
    };
    let mut pairs = Vec::<(Vec<u8>, Vec<u8>)>::new();
    let represented_order = matches!(parsed, serde_json::Value::Array(_));
    let mut unsupported = Vec::new();
    match parsed {
        serde_json::Value::Object(values) => {
            // A JSON object has no fidelity promise for member order. Sorting
            // makes the derived block stable across serde/platform versions.
            let mut values = values.into_iter().collect::<Vec<_>>();
            values.sort_by(|left, right| left.0.cmp(&right.0));
            for (name, value) in values {
                match value {
                    serde_json::Value::Array(values) => {
                        for value in values {
                            if let Some(value) = legacy_header_value(&value) {
                                pairs.push((name.as_bytes().to_vec(), value));
                            } else {
                                unsupported.push(format!(
                                    "header {name} contains a non-scalar array value"
                                ));
                            }
                        }
                    }
                    value => {
                        if let Some(value) = legacy_header_value(&value) {
                            pairs.push((name.as_bytes().to_vec(), value));
                        } else {
                            unsupported.push(format!("header {name} has a non-scalar value"));
                        }
                    }
                }
            }
        }
        serde_json::Value::Array(values) => {
            for (index, value) in values.into_iter().enumerate() {
                let pair = match value {
                    serde_json::Value::Array(mut pair) if pair.len() == 2 => {
                        let value = pair.pop().expect("pair length checked");
                        let name = pair.pop().expect("pair length checked");
                        name.as_str()
                            .zip(legacy_header_value(&value))
                            .map(|(name, value)| (name.as_bytes().to_vec(), value))
                    }
                    serde_json::Value::Object(mut pair) => {
                        let name = pair.remove("name");
                        let value = pair.remove("value");
                        name.as_ref()
                            .and_then(serde_json::Value::as_str)
                            .zip(value.as_ref().and_then(legacy_header_value))
                            .map(|(name, value)| (name.as_bytes().to_vec(), value))
                    }
                    _ => None,
                };
                if let Some(pair) = pair {
                    pairs.push(pair);
                } else {
                    unsupported.push(format!("header entry {index} is not a name/value pair"));
                }
            }
        }
        _ => unsupported.push("legacy headers are neither an object nor an ordered list".into()),
    }
    let capture = (!pairs.is_empty()).then(|| {
        if represented_order {
            LarHeaderCapture::legacy_ordered(pairs)
        } else {
            LarHeaderCapture::legacy_normalized(pairs)
        }
    });
    ParsedLegacyHeaders {
        capture,
        unsupported: (!unsupported.is_empty()).then(|| unsupported.join("; ")),
    }
}

fn parse_legacy_attempts(source: Option<&str>) -> (Vec<LegacyAttemptMetadata>, Option<String>) {
    let Some(source) = source else {
        return (Vec::new(), None);
    };
    let parsed = match serde_json::from_str::<serde_json::Value>(source) {
        Ok(serde_json::Value::Array(values)) => values,
        Ok(_) => {
            return (
                Vec::new(),
                Some("legacy attempts JSON is not an array".into()),
            )
        }
        Err(error) => {
            return (
                Vec::new(),
                Some(format!("invalid legacy attempts JSON: {error}")),
            )
        }
    };
    let mut attempts = Vec::with_capacity(parsed.len());
    let mut unsupported = Vec::new();
    for (index, value) in parsed.into_iter().enumerate() {
        let serde_json::Value::Object(fields) = &value else {
            unsupported.push(format!("attempt {index} is not an object"));
            continue;
        };
        let account_id = fields
            .get("account_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let model = fields
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let rung = fields
            .get("rung")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let retry = fields.get("retry").and_then(serde_json::Value::as_u64);
        for (name, valid) in [
            (
                "account_id",
                fields
                    .get("account_id")
                    .is_none_or(serde_json::Value::is_string),
            ),
            (
                "model",
                fields.get("model").is_none_or(serde_json::Value::is_string),
            ),
            (
                "rung",
                fields.get("rung").is_none_or(serde_json::Value::is_string),
            ),
            (
                "retry",
                fields.get("retry").is_none_or(serde_json::Value::is_u64),
            ),
        ] {
            if !valid {
                unsupported.push(format!(
                    "attempt {index} field {name} has an unsupported type"
                ));
            }
        }
        let routing_reason = rung.or_else(|| retry.map(|retry| format!("retry {retry}")));
        let has_opaque_fields = fields
            .keys()
            .any(|key| !matches!(key.as_str(), "account_id" | "model" | "rung" | "retry"));
        let opaque_json = has_opaque_fields.then(|| value.to_string());
        attempts.push(LegacyAttemptMetadata {
            account_id,
            model,
            routing_reason,
            opaque_json,
        });
    }
    (
        attempts,
        (!unsupported.is_empty()).then(|| unsupported.join("; ")),
    )
}

fn hash_metadata_field(hasher: &mut blake3::Hasher, label: &str, value: Option<&str>) {
    hasher.update(&(label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    match value {
        None => {
            hasher.update(&[0]);
        }
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
    };
}

fn append_metadata_note(stage: &mut StageData, detail: impl Into<String>) {
    let detail = detail.into();
    stage.error_class = Some(b"legacy_metadata_unsupported".to_vec());
    stage.error_message = Some(detail.into_bytes());
}

fn legacy_stage_field(
    value: Option<&str>,
    label: &str,
    unsupported_count: &mut u64,
) -> Option<Vec<u8>> {
    let value = value?;
    let limit = usize::try_from(Limits::default().max_field_length).unwrap_or(usize::MAX);
    if value.len() <= limit {
        return Some(value.as_bytes().to_vec());
    }
    *unsupported_count = unsupported_count.saturating_add(1);
    Some(
        format!(
            "legacy metadata field {label} was not embedded: {} bytes exceeds LAR limit {limit}; source remains in SQLite",
            value.len()
        )
        .into_bytes(),
    )
}

fn legacy_tool_provenance(tool: &LegacyToolMetadata) -> String {
    format!(
        "legacy_tool_metadata_json:{}",
        serde_json::json!({
            "harness": tool.harness,
            "turn_id": tool.turn_id,
            "legacy_trace_id": tool.trace_id,
            "tool_call_id": tool.tool_call_id,
            "tool_name": tool.tool_name,
        })
    )
}

fn nonnegative_legacy_token(value: Option<i64>, _label: &str, unsupported_count: &mut u64) -> u64 {
    match value {
        Some(value) => match u64::try_from(value) {
            Ok(value) => value,
            Err(_) => {
                *unsupported_count = unsupported_count.saturating_add(1);
                0
            }
        },
        None => 0,
    }
}

fn legacy_token_usage(
    source: &LegacyTraceMetadata,
    unsupported_count: &mut u64,
) -> Option<TokenUsage> {
    let represented = [
        source.input_tokens,
        source.output_tokens,
        source.cached_input_tokens,
        source.reasoning_tokens,
    ]
    .into_iter()
    .any(|value| value.is_some());
    represented.then(|| TokenUsage {
        input_tokens: nonnegative_legacy_token(
            source.input_tokens,
            "input_tokens",
            unsupported_count,
        ),
        output_tokens: nonnegative_legacy_token(
            source.output_tokens,
            "output_tokens",
            unsupported_count,
        ),
        cached_tokens: nonnegative_legacy_token(
            source.cached_input_tokens,
            "cached_input_tokens",
            unsupported_count,
        ),
        reasoning_tokens: nonnegative_legacy_token(
            source.reasoning_tokens,
            "reasoning_tokens",
            unsupported_count,
        ),
    })
}

fn legacy_cost_nanos(value: Option<f64>, unsupported_count: &mut u64) -> Option<u64> {
    let value = value?;
    let nanos = value * 1_000_000_000.0;
    if !nanos.is_finite() || nanos < 0.0 || nanos > u64::MAX as f64 {
        *unsupported_count = unsupported_count.saturating_add(1);
        return None;
    }
    Some(nanos.round() as u64)
}

fn legacy_companion_text(
    value: Option<&str>,
    label: &str,
    unsupported_count: &mut u64,
    unsupported: &mut Vec<String>,
) -> Option<Vec<u8>> {
    let value = value?;
    let limit = usize::try_from(Limits::default().max_field_length).unwrap_or(usize::MAX);
    if value.len() <= limit {
        return Some(value.as_bytes().to_vec());
    }
    *unsupported_count = unsupported_count.saturating_add(1);
    unsupported.push(format!("{label}:{}>{limit}", value.len()));
    None
}

fn legacy_exchange_metadata(
    source: &LegacyTraceMetadata,
    unsupported_count: &mut u64,
) -> ExchangeMetadataData {
    let mut unsupported = Vec::new();
    let mut text =
        |value, label| legacy_companion_text(value, label, unsupported_count, &mut unsupported);
    let mut data = ExchangeMetadataData {
        ts_request_ms: Some(source.ts_request_ms),
        ts_response_ms: source.ts_response_ms,
        harness: text(source.harness.as_deref(), "harness"),
        client_format: text(source.client_format.as_deref(), "client_format"),
        upstream_format: text(source.upstream_format.as_deref(), "upstream_format"),
        method: text(source.method.as_deref(), "method"),
        path: text(source.path.as_deref(), "path"),
        streamed: source.streamed,
        status: source.status,
        cost_usd_bits: source.cost_usd.map(f64::to_bits),
        billing_bucket: text(source.billing_bucket.as_deref(), "billing_bucket"),
        error_kind: text(source.error_kind.as_deref(), "error_kind"),
        error_code: text(source.error_code.as_deref(), "error_code"),
        substituted: source.substituted,
        original_model: text(source.original_model.as_deref(), "original_model"),
        served_model: text(source.served_model.as_deref(), "served_model"),
        substitution_reason: text(source.substitution_reason.as_deref(), "substitution_reason"),
        injected: source.injected,
        fixture_name: text(source.fixture_name.as_deref(), "fixture_name"),
        attempts_json: text(source.attempts_json.as_deref(), "attempts_json"),
        original_account_id: text(source.original_account_id.as_deref(), "original_account_id"),
        served_account_id: text(source.served_account_id.as_deref(), "served_account_id"),
        subscription_identity: text(
            source.subscription_identity.as_deref(),
            "subscription_identity",
        ),
        via_dario: source.via_dario,
        dario_generation: text(source.dario_generation.as_deref(), "dario_generation"),
        tags_json: text(source.tags_json.as_deref(), "tags_json"),
        client_ip: text(source.client_ip.as_deref(), "client_ip"),
        key_fingerprint: text(source.key_fingerprint.as_deref(), "key_fingerprint"),
        reasoning_effort: text(source.reasoning_effort.as_deref(), "reasoning_effort"),
        thinking_budget: source.thinking_budget,
        input_tokens: source.input_tokens,
        cached_input_tokens: source.cached_input_tokens,
        cache_creation_tokens: source.cache_creation_tokens,
        output_tokens: source.output_tokens,
        reasoning_tokens: source.reasoning_tokens,
        unknown_attributes: Vec::new(),
    };
    if !unsupported.is_empty() {
        data.unknown_attributes
            .push(UnknownExchangeMetadataAttribute {
                key: b"alex.legacy_unsupported_metadata".to_vec(),
                value: unsupported.join(";").into_bytes(),
            });
    }
    data
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
    fn legacy_trace_metadata_rows(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<LegacyTraceMetadata>> {
        let offset = i64::try_from(offset).context("legacy metadata offset is too large")?;
        let limit = i64::try_from(limit).context("legacy metadata limit is too large")?;
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, session_id, run_id, ts_request_ms, ts_response_ms,
                    req_headers_json, resp_headers_json, req_body_path,
                    upstream_req_body_path, resp_body_path, upstream_provider,
                    requested_model, routed_model, account_id, status,
                    error_class, error, substituted, original_model, served_model,
                    substitution_reason, attempts, original_account_id,
                    served_account_id, injected, fixture_name, via_dario,
                    dario_generation, harness, client_format, upstream_format,
                    method, path, streamed, input_tokens, cached_input_tokens,
                    cache_creation_tokens, output_tokens, reasoning_tokens,
                    cost_usd, billing_bucket, error_kind, error_code,
                    subscription_identity, tags_json, client_ip, key_fingerprint,
                    reasoning_effort, thinking_budget
               FROM traces ORDER BY ts_request_ms, id LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit, offset], |row| {
            Ok(LegacyTraceMetadata {
                trace_id: row.get(0)?,
                session_id: row.get(1)?,
                run_id: row.get(2)?,
                ts_request_ms: row.get(3)?,
                ts_response_ms: row.get(4)?,
                request_headers_json: row.get(5)?,
                response_headers_json: row.get(6)?,
                request_body_path: row.get(7)?,
                upstream_request_body_path: row.get(8)?,
                response_body_path: row.get(9)?,
                provider: row.get(10)?,
                requested_model: row.get(11)?,
                routed_model: row.get(12)?,
                account_id: row.get(13)?,
                status: row.get(14)?,
                error_class: row.get(15)?,
                error: row.get(16)?,
                substituted: row.get::<_, i64>(17)? != 0,
                original_model: row.get(18)?,
                served_model: row.get(19)?,
                substitution_reason: row.get(20)?,
                attempts_json: row.get(21)?,
                original_account_id: row.get(22)?,
                served_account_id: row.get(23)?,
                injected: row.get::<_, i64>(24)? != 0,
                fixture_name: row.get(25)?,
                via_dario: row.get::<_, i64>(26)? != 0,
                dario_generation: row.get(27)?,
                harness: row.get(28)?,
                client_format: row.get(29)?,
                upstream_format: row.get(30)?,
                method: row.get(31)?,
                path: row.get(32)?,
                streamed: row.get::<_, Option<i64>>(33)?.map(|value| value != 0),
                input_tokens: row.get(34)?,
                cached_input_tokens: row.get(35)?,
                cache_creation_tokens: row.get(36)?,
                output_tokens: row.get(37)?,
                reasoning_tokens: row.get(38)?,
                cost_usd: row.get(39)?,
                billing_bucket: row.get(40)?,
                error_kind: row.get(41)?,
                error_code: row.get(42)?,
                subscription_identity: row.get(43)?,
                tags_json: row.get(44)?,
                client_ip: row.get(45)?,
                key_fingerprint: row.get(46)?,
                reasoning_effort: row.get(47)?,
                thinking_budget: row.get(48)?,
                synthetic_tool: None,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn legacy_unlinked_tool_metadata_rows(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<LegacyTraceMetadata>> {
        let offset = i64::try_from(offset).context("legacy tool metadata offset is too large")?;
        let limit = i64::try_from(limit).context("legacy tool metadata limit is too large")?;
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT c.id, c.harness, c.session_id, c.turn_id, c.trace_id,
                    c.tool_call_id, c.tool_name,
                    c.ts_start_ms, c.ts_end_ms, c.is_error, c.exit_status,
                    c.args_body_path, c.result_body_path
               FROM tool_calls c
              WHERE c.trace_id IS NULL
                 OR NOT EXISTS(SELECT 1 FROM traces t WHERE t.id=c.trace_id)
              ORDER BY c.ts_start_ms, c.id LIMIT ?1 OFFSET ?2",
        )?;
        let rows = statement.query_map(params![limit, offset], |row| {
            let tool = LegacyToolMetadata {
                id: row.get(0)?,
                harness: row.get(1)?,
                session_id: row.get(2)?,
                turn_id: row.get(3)?,
                trace_id: row.get(4)?,
                tool_call_id: row.get(5)?,
                tool_name: row.get(6)?,
                ts_start_ms: row.get(7)?,
                ts_end_ms: row.get(8)?,
                is_error: row.get::<_, Option<i64>>(9)?.map(|value| value != 0),
                exit_status: row.get(10)?,
                arguments_path: row.get(11)?,
                result_path: row.get(12)?,
            };
            Ok(LegacyTraceMetadata {
                trace_id: format!("legacy-tool:{}", tool.id),
                session_id: Some(tool.session_id.clone()),
                run_id: None,
                ts_request_ms: tool.ts_start_ms,
                ts_response_ms: tool.ts_end_ms,
                request_headers_json: None,
                response_headers_json: None,
                request_body_path: None,
                upstream_request_body_path: None,
                response_body_path: None,
                provider: None,
                requested_model: None,
                routed_model: None,
                account_id: None,
                status: None,
                error_class: None,
                error: None,
                substituted: false,
                original_model: None,
                served_model: None,
                substitution_reason: None,
                attempts_json: None,
                original_account_id: None,
                served_account_id: None,
                injected: false,
                fixture_name: None,
                via_dario: false,
                dario_generation: None,
                harness: None,
                client_format: None,
                upstream_format: None,
                method: None,
                path: None,
                streamed: None,
                input_tokens: None,
                cached_input_tokens: None,
                cache_creation_tokens: None,
                output_tokens: None,
                reasoning_tokens: None,
                cost_usd: None,
                billing_bucket: None,
                error_kind: None,
                error_code: None,
                subscription_identity: None,
                tags_json: None,
                client_ip: None,
                key_fingerprint: None,
                reasoning_effort: None,
                thinking_budget: None,
                synthetic_tool: Some(tool),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn legacy_metadata_plans(
        &self,
        sources: &[LegacyTraceMetadata],
    ) -> Result<Vec<LegacyMetadataPlan>> {
        let mut trace_manifests = HashMap::<String, HashMap<String, ManifestId>>::new();
        let mut tools_by_trace = HashMap::<String, Vec<LegacyToolMetadata>>::new();
        for source in sources {
            if let Some(tool) = source.synthetic_tool.clone() {
                tools_by_trace
                    .entry(source.trace_id.clone())
                    .or_default()
                    .push(tool);
            }
        }
        let conn = self.conn.lock().unwrap();
        for chunk in sources.chunks(400) {
            let ids = chunk
                .iter()
                .filter(|source| source.synthetic_tool.is_none())
                .map(|source| source.trace_id.as_str())
                .collect::<Vec<_>>();
            if ids.is_empty() {
                continue;
            }
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(",");
            #[cfg(test)]
            note_legacy_metadata_plan_query();
            let mut statement = conn.prepare(&format!(
                "SELECT a.owner_id, a.artifact_kind, a.manifest_id
                   FROM lar_trace_artifacts a
                   JOIN lar_manifests m ON m.manifest_id=a.manifest_id AND m.state='ready'
                  WHERE a.owner_kind='trace' AND a.stage_id=''
                    AND a.validation_state='validated' AND a.manifest_id IS NOT NULL
                    AND a.owner_id IN ({placeholders})"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(ids), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (trace_id, kind, manifest) = row?;
                trace_manifests
                    .entry(trace_id)
                    .or_default()
                    .insert(kind, ManifestId::from_str(&manifest)?);
            }

            let ids = chunk
                .iter()
                .filter(|source| source.synthetic_tool.is_none())
                .map(|source| source.trace_id.as_str())
                .collect::<Vec<_>>();
            #[cfg(test)]
            note_legacy_metadata_plan_query();
            let mut statement = conn.prepare(&format!(
                "SELECT id, harness, session_id, turn_id, trace_id, tool_call_id,
                        tool_name, ts_start_ms,
                        ts_end_ms, is_error, exit_status, args_body_path, result_body_path
                   FROM tool_calls WHERE trace_id IN ({placeholders})
                   ORDER BY trace_id, ts_start_ms, id"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(ids), |row| {
                Ok(LegacyToolMetadata {
                    id: row.get(0)?,
                    harness: row.get(1)?,
                    session_id: row.get(2)?,
                    turn_id: row.get(3)?,
                    trace_id: row.get(4)?,
                    tool_call_id: row.get(5)?,
                    tool_name: row.get(6)?,
                    ts_start_ms: row.get(7)?,
                    ts_end_ms: row.get(8)?,
                    is_error: row.get::<_, Option<i64>>(9)?.map(|value| value != 0),
                    exit_status: row.get(10)?,
                    arguments_path: row.get(11)?,
                    result_path: row.get(12)?,
                })
            })?;
            for row in rows {
                let tool = row?;
                if let Some(trace_id) = tool.trace_id.clone() {
                    tools_by_trace.entry(trace_id).or_default().push(tool);
                }
            }
        }

        let tool_ids = tools_by_trace
            .values()
            .flatten()
            .map(|tool| tool.id.clone())
            .collect::<Vec<_>>();
        let mut tool_manifests = HashMap::<String, HashMap<String, ManifestId>>::new();
        for chunk in tool_ids.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            #[cfg(test)]
            note_legacy_metadata_plan_query();
            let mut statement = conn.prepare(&format!(
                "SELECT a.owner_id, a.artifact_kind, a.manifest_id
                   FROM lar_trace_artifacts a
                   JOIN lar_manifests m ON m.manifest_id=a.manifest_id AND m.state='ready'
                  WHERE a.owner_kind='tool_call' AND a.stage_id=''
                    AND a.validation_state='validated' AND a.manifest_id IS NOT NULL
                    AND a.owner_id IN ({placeholders})"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (tool_id, kind, manifest) = row?;
                tool_manifests
                    .entry(tool_id)
                    .or_default()
                    .insert(kind, ManifestId::from_str(&manifest)?);
            }
        }
        drop(conn);

        sources
            .iter()
            .map(|source| {
                let manifests = trace_manifests.remove(&source.trace_id).unwrap_or_default();
                let tools = tools_by_trace
                    .remove(&source.trace_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tool| {
                        let manifests = tool_manifests.remove(&tool.id).unwrap_or_default();
                        (tool, manifests)
                    })
                    .collect();
                self.legacy_metadata_plan_from_parts(source, manifests, tools)
            })
            .collect()
    }

    fn legacy_metadata_plan_from_parts(
        &self,
        source: &LegacyTraceMetadata,
        manifests: HashMap<String, ManifestId>,
        tools: Vec<(LegacyToolMetadata, HashMap<String, ManifestId>)>,
    ) -> Result<LegacyMetadataPlan> {
        let mut missing_manifests = Vec::new();
        for (path, kind) in [
            (source.request_body_path.as_ref(), "client_request"),
            (
                source.upstream_request_body_path.as_ref(),
                "upstream_request",
            ),
            (source.response_body_path.as_ref(), "client_response"),
        ] {
            if path.is_some() && !manifests.contains_key(kind) {
                missing_manifests.push(kind.to_string());
            }
        }
        for (tool, tool_manifests) in &tools {
            for (path, kind) in [
                (tool.arguments_path.as_ref(), "tool_arguments"),
                (tool.result_path.as_ref(), "tool_result"),
            ] {
                if path.is_some() && !tool_manifests.contains_key(kind) {
                    missing_manifests.push(format!("tool:{}:{kind}", tool.id));
                }
            }
        }
        if source.synthetic_tool.is_some() {
            return self.legacy_unlinked_tool_plan(source, tools, missing_manifests);
        }

        let request_headers = parse_legacy_headers(source.request_headers_json.as_deref());
        let response_headers = parse_legacy_headers(source.response_headers_json.as_deref());
        let (attempts, attempts_unsupported) =
            parse_legacy_attempts(source.attempts_json.as_deref());
        let mut unsupported_count = u64::from(request_headers.unsupported.is_some())
            + u64::from(response_headers.unsupported.is_some())
            + u64::from(attempts_unsupported.is_some());
        let exchange_metadata = legacy_exchange_metadata(source, &mut unsupported_count);

        let wall_time_ns = u64::try_from(source.ts_request_ms.max(0))
            .unwrap_or(0)
            .saturating_mul(1_000_000);
        let response_time_ns = source
            .ts_response_ms
            .and_then(|value| u64::try_from(value.max(0)).ok())
            .map(|value| value.saturating_mul(1_000_000))
            .unwrap_or(wall_time_ns);
        let mut stages = Vec::new();
        let mut request = StageData::new(StageKind::ClientRequest, wall_time_ns);
        request.request_body_manifest_ref = manifests.get("client_request").copied();
        if let Some(detail) = request_headers.unsupported.as_deref() {
            append_metadata_note(&mut request, detail);
        }
        stages.push(request);

        let mut router = StageData::new(StageKind::RouterDecision, wall_time_ns);
        router.provider = source.provider.as_deref().map(str::as_bytes).map(Vec::from);
        router.requested_model = source
            .original_model
            .as_deref()
            .or(source.requested_model.as_deref())
            .map(str::as_bytes)
            .map(Vec::from);
        router.routed_model = source
            .served_model
            .as_deref()
            .or(source.routed_model.as_deref())
            .map(str::as_bytes)
            .map(Vec::from);
        router.account_id = source
            .served_account_id
            .as_deref()
            .or(source.account_id.as_deref())
            .map(str::as_bytes)
            .map(Vec::from);
        router.routing_reason = source
            .substitution_reason
            .as_deref()
            .or(source.substituted.then_some("legacy_substituted"))
            .map(str::as_bytes)
            .map(Vec::from);
        if let Some(detail) = attempts_unsupported.as_deref() {
            append_metadata_note(&mut router, detail);
        }
        stages.push(router);

        let mut previous_model = source.original_model.as_deref();
        let mut previous_account = source.original_account_id.as_deref();
        for (index, attempt) in attempts.iter().enumerate() {
            let changed_route = previous_model
                .is_some_and(|value| attempt.model.as_deref().is_some_and(|model| model != value))
                || previous_account.is_some_and(|value| {
                    attempt
                        .account_id
                        .as_deref()
                        .is_some_and(|account| account != value)
                });
            let kind = if index == 0 {
                StageKind::AccountRouting
            } else if changed_route {
                StageKind::FailoverDecision
            } else {
                StageKind::RetryDecision
            };
            let mut stage = StageData::new(kind, wall_time_ns);
            stage.attempt_number = Some(u32::try_from(index + 1).unwrap_or(u32::MAX));
            stage.requested_model = previous_model.map(str::as_bytes).map(Vec::from);
            stage.routed_model = attempt.model.as_deref().map(str::as_bytes).map(Vec::from);
            stage.account_id = attempt
                .account_id
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            stage.routing_reason = legacy_stage_field(
                attempt.routing_reason.as_deref(),
                "attempt routing reason",
                &mut unsupported_count,
            );
            if let Some(opaque) = attempt.opaque_json.as_deref() {
                unsupported_count = unsupported_count.saturating_add(1);
                let detail = format!("legacy_opaque_attempt_json:{opaque}");
                let embedded = legacy_stage_field(
                    Some(&detail),
                    "opaque attempt JSON",
                    &mut unsupported_count,
                );
                stage.error_class = Some(b"legacy_opaque_metadata".to_vec());
                stage.error_message = embedded;
            }
            stages.push(stage);
            previous_model = attempt.model.as_deref().or(previous_model);
            previous_account = attempt.account_id.as_deref().or(previous_account);
        }

        let upstream_evidence = source.upstream_request_body_path.is_some()
            || source.response_headers_json.is_some()
            || !attempts.is_empty()
            || source.via_dario;
        if upstream_evidence && !source.injected {
            let mut upstream_request = StageData::new(StageKind::UpstreamRequest, wall_time_ns);
            upstream_request.attempt_number =
                Some(u32::try_from(attempts.len().max(1)).unwrap_or(u32::MAX));
            upstream_request.request_body_manifest_ref = manifests
                .get("upstream_request")
                .or_else(|| manifests.get("client_request"))
                .copied();
            stages.push(upstream_request);

            if source.via_dario {
                if let Some(manifest) = manifests.get("dario_upstream_request") {
                    let mut dario = StageData::new(StageKind::DarioRequest, wall_time_ns);
                    dario.request_body_manifest_ref = Some(*manifest);
                    dario.routing_reason = source
                        .dario_generation
                        .as_deref()
                        .map(str::as_bytes)
                        .map(Vec::from);
                    stages.push(dario);
                }
                if let Some(manifest) = manifests.get("dario_upstream_response") {
                    let mut dario = StageData::new(StageKind::DarioResponse, response_time_ns);
                    dario.response_body_manifest_ref = Some(*manifest);
                    dario.routing_reason = source
                        .dario_generation
                        .as_deref()
                        .map(str::as_bytes)
                        .map(Vec::from);
                    stages.push(dario);
                }
            }

            if response_headers.capture.is_some()
                || source.status.is_some()
                || source.error.is_some()
            {
                let response_kind = if source.status.is_some() {
                    StageKind::UpstreamResponse
                } else {
                    StageKind::UpstreamFailure
                };
                let mut upstream_response = StageData::new(response_kind, response_time_ns);
                upstream_response.attempt_number =
                    Some(u32::try_from(attempts.len().max(1)).unwrap_or(u32::MAX));
                upstream_response.status_code =
                    source.status.and_then(|value| u16::try_from(value).ok());
                upstream_response.error_class = source
                    .error_class
                    .as_deref()
                    .map(str::as_bytes)
                    .map(Vec::from);
                upstream_response.error_message = legacy_stage_field(
                    source.error.as_deref(),
                    "upstream error",
                    &mut unsupported_count,
                );
                if let Some(detail) = response_headers.unsupported.as_deref() {
                    append_metadata_note(&mut upstream_response, detail);
                }
                stages.push(upstream_response);
            }
        } else if let Some(detail) = response_headers.unsupported.as_deref() {
            if let Some(router) = stages
                .iter_mut()
                .find(|stage| stage.kind == StageKind::RouterDecision)
            {
                append_metadata_note(router, detail);
            }
        }

        if source.injected {
            let mut injected = StageData::new(StageKind::InjectedResponse, response_time_ns);
            injected.response_body_manifest_ref = manifests.get("client_response").copied();
            injected.routing_reason = source
                .fixture_name
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            stages.push(injected);
        }

        let mut client_response = StageData::new(StageKind::ClientResponse, response_time_ns);
        client_response.response_body_manifest_ref = manifests.get("client_response").copied();
        client_response.status_code = source.status.and_then(|value| u16::try_from(value).ok());
        client_response.error_class = source
            .error_class
            .as_deref()
            .map(str::as_bytes)
            .map(Vec::from);
        client_response.error_message = legacy_stage_field(
            source.error.as_deref(),
            "client error",
            &mut unsupported_count,
        );
        stages.push(client_response);

        let usage = legacy_token_usage(source, &mut unsupported_count);
        let cost_nanos = legacy_cost_nanos(source.cost_usd, &mut unsupported_count);
        if let Some(response) = stages.iter_mut().find(|stage| {
            matches!(
                stage.kind,
                StageKind::UpstreamResponse
                    | StageKind::InjectedResponse
                    | StageKind::ClientResponse
            )
        }) {
            response.usage = usage;
            response.cost_nanos = cost_nanos;
            if response.cost_nanos.is_some() {
                response.cost_currency = Some(b"USD".to_vec());
            }
        }

        for (tool, tool_manifests) in &tools {
            let tool_time_ns = u64::try_from(tool.ts_start_ms.max(0))
                .unwrap_or(0)
                .saturating_mul(1_000_000);
            let mut call = StageData::new(StageKind::ToolCall, tool_time_ns);
            call.request_body_manifest_ref = tool_manifests.get("tool_arguments").copied();
            let provenance = legacy_tool_provenance(tool);
            call.routing_reason = legacy_stage_field(
                Some(&provenance),
                "tool-call provenance",
                &mut unsupported_count,
            );
            stages.push(call);

            let result_time_ns = tool
                .ts_end_ms
                .and_then(|value| u64::try_from(value.max(0)).ok())
                .map(|value| value.saturating_mul(1_000_000))
                .unwrap_or(tool_time_ns)
                .max(tool_time_ns);
            let mut result = StageData::new(StageKind::ToolResult, result_time_ns);
            result.response_body_manifest_ref = tool_manifests.get("tool_result").copied();
            result.routing_reason = tool
                .exit_status
                .map(|value| format!("legacy_tool_exit_status:{value}").into_bytes());
            if tool.is_error == Some(true) {
                result.error_class = Some(b"tool_error".to_vec());
            }
            stages.push(result);
        }
        // Legacy tool callbacks were recorded asynchronously. Merge their
        // source timestamps into the exchange timeline instead of placing all
        // tools after ClientResponse. Stable sorting retains the semantic
        // construction order when legacy timestamps are equal and therefore
        // do not prove a finer ordering.
        stages.sort_by_key(|stage| stage.wall_time_ns);

        let catalog_capture = LarExchangeCapture {
            trace_id: source.trace_id.clone(),
            session_id: source.session_id.clone(),
            run_id: source.run_id.clone(),
            wall_time_ns,
            client_request_headers: request_headers.capture,
            client_request_trailers: None,
            // The old field was populated from upstream response headers. It
            // is carried here only so the shared catalog helper emits atoms;
            // the stage builder attaches it to UpstreamResponse below.
            client_response_headers: response_headers.capture,
            client_response_trailers: None,
            upstream_attempts: Vec::new(),
            upstream_stream_reads: None,
            provider: source.provider.clone(),
            requested_model: source.requested_model.clone(),
            routed_model: source.routed_model.clone(),
            account_id: source.account_id.clone(),
            routing_reason: source.substitution_reason.clone(),
            status_code: source.status.and_then(|value| u16::try_from(value).ok()),
            error_class: source.error_class.clone(),
            error_message: source.error.clone(),
        };

        let mut external_manifests = manifests.values().copied().collect::<Vec<_>>();
        for (_, tool_manifests) in &tools {
            external_manifests.extend(tool_manifests.values().copied());
        }
        external_manifests.sort_by_key(|id| id.0);
        external_manifests.dedup();
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"alex-legacy-exchange-metadata-v2\0");
        for (label, value) in [
            ("trace_id", Some(source.trace_id.as_str())),
            ("session_id", source.session_id.as_deref()),
            ("run_id", source.run_id.as_deref()),
            ("request_headers", source.request_headers_json.as_deref()),
            ("response_headers", source.response_headers_json.as_deref()),
            ("provider", source.provider.as_deref()),
            ("requested_model", source.requested_model.as_deref()),
            ("routed_model", source.routed_model.as_deref()),
            ("account_id", source.account_id.as_deref()),
            ("error_class", source.error_class.as_deref()),
            ("error", source.error.as_deref()),
            ("original_model", source.original_model.as_deref()),
            ("served_model", source.served_model.as_deref()),
            ("substitution_reason", source.substitution_reason.as_deref()),
            ("attempts", source.attempts_json.as_deref()),
            ("original_account_id", source.original_account_id.as_deref()),
            ("served_account_id", source.served_account_id.as_deref()),
            ("fixture_name", source.fixture_name.as_deref()),
            ("dario_generation", source.dario_generation.as_deref()),
            ("harness", source.harness.as_deref()),
            ("client_format", source.client_format.as_deref()),
            ("upstream_format", source.upstream_format.as_deref()),
            ("method", source.method.as_deref()),
            ("path", source.path.as_deref()),
            ("billing_bucket", source.billing_bucket.as_deref()),
            ("error_kind", source.error_kind.as_deref()),
            ("error_code", source.error_code.as_deref()),
            (
                "subscription_identity",
                source.subscription_identity.as_deref(),
            ),
            ("tags_json", source.tags_json.as_deref()),
            ("client_ip", source.client_ip.as_deref()),
            ("key_fingerprint", source.key_fingerprint.as_deref()),
            ("reasoning_effort", source.reasoning_effort.as_deref()),
        ] {
            hash_metadata_field(&mut hasher, label, value);
        }
        for (label, value) in [
            ("ts_request_ms", source.ts_request_ms),
            ("ts_response_ms", source.ts_response_ms.unwrap_or(i64::MIN)),
            ("status", source.status.unwrap_or(i64::MIN)),
            ("substituted", i64::from(source.substituted)),
            ("injected", i64::from(source.injected)),
            ("via_dario", i64::from(source.via_dario)),
            (
                "streamed",
                source.streamed.map(i64::from).unwrap_or(i64::MIN),
            ),
            ("input_tokens", source.input_tokens.unwrap_or(i64::MIN)),
            (
                "cached_input_tokens",
                source.cached_input_tokens.unwrap_or(i64::MIN),
            ),
            (
                "cache_creation_tokens",
                source.cache_creation_tokens.unwrap_or(i64::MIN),
            ),
            ("output_tokens", source.output_tokens.unwrap_or(i64::MIN)),
            (
                "reasoning_tokens",
                source.reasoning_tokens.unwrap_or(i64::MIN),
            ),
            (
                "thinking_budget",
                source.thinking_budget.unwrap_or(i64::MIN),
            ),
        ] {
            hash_metadata_field(&mut hasher, label, Some(&value.to_string()));
        }
        hash_metadata_field(
            &mut hasher,
            "cost_usd_bits",
            source
                .cost_usd
                .map(f64::to_bits)
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
        );
        let mut manifest_pairs = manifests.into_iter().collect::<Vec<_>>();
        manifest_pairs.sort_by(|left, right| left.0.cmp(&right.0));
        for (kind, manifest) in manifest_pairs {
            hash_metadata_field(
                &mut hasher,
                &format!("manifest:{kind}"),
                Some(&manifest.to_string()),
            );
        }
        for missing in &missing_manifests {
            hash_metadata_field(&mut hasher, "missing_manifest", Some(missing));
        }
        for (tool, tool_manifests) in &tools {
            for (label, value) in [
                ("tool_id", Some(tool.id.as_str())),
                ("tool_harness", Some(tool.harness.as_str())),
                ("tool_session", Some(tool.session_id.as_str())),
                ("tool_turn", tool.turn_id.as_deref()),
                ("tool_trace", tool.trace_id.as_deref()),
                ("tool_call_id", Some(tool.tool_call_id.as_str())),
                ("tool_name", Some(tool.tool_name.as_str())),
                ("tool_arguments_path", tool.arguments_path.as_deref()),
                ("tool_result_path", tool.result_path.as_deref()),
            ] {
                hash_metadata_field(&mut hasher, label, value);
            }
            for (label, value) in [
                ("tool_start_ms", tool.ts_start_ms),
                ("tool_end_ms", tool.ts_end_ms.unwrap_or(i64::MIN)),
                (
                    "tool_is_error",
                    tool.is_error.map(i64::from).unwrap_or(i64::MIN),
                ),
                ("tool_exit_status", tool.exit_status.unwrap_or(i64::MIN)),
            ] {
                hash_metadata_field(&mut hasher, label, Some(&value.to_string()));
            }
            let mut pairs = tool_manifests.iter().collect::<Vec<_>>();
            pairs.sort_by(|left, right| left.0.cmp(right.0));
            for (kind, manifest) in pairs {
                hash_metadata_field(
                    &mut hasher,
                    &format!("tool_manifest:{}:{kind}", tool.id),
                    Some(&manifest.to_string()),
                );
            }
        }
        let source_size = [
            source.request_headers_json.as_deref(),
            source.response_headers_json.as_deref(),
            source.attempts_json.as_deref(),
        ]
        .into_iter()
        .flatten()
        .fold(0u64, |total, value| {
            total.saturating_add(value.len() as u64)
        });
        Ok(LegacyMetadataPlan {
            owner_kind: "trace",
            owner_id: source.trace_id.clone(),
            trace_id: source.trace_id.clone(),
            session_id: source.session_id.clone(),
            run_id: source.run_id.clone(),
            source_metadata: source.clone(),
            exchange_metadata,
            catalog_capture,
            stages,
            external_manifests,
            fingerprint: hex(hasher.finalize().as_bytes()),
            source_size,
            unsupported_count,
            missing_manifests,
        })
    }

    fn legacy_unlinked_tool_plan(
        &self,
        source: &LegacyTraceMetadata,
        tools: Vec<(LegacyToolMetadata, HashMap<String, ManifestId>)>,
        missing_manifests: Vec<String>,
    ) -> Result<LegacyMetadataPlan> {
        let tool = source
            .synthetic_tool
            .as_ref()
            .context("synthetic legacy tool source lost its tool metadata")?;
        let wall_time_ns = u64::try_from(tool.ts_start_ms.max(0))
            .unwrap_or(0)
            .saturating_mul(1_000_000);
        let mut stages = Vec::with_capacity(tools.len().saturating_mul(2));
        // The synthetic exchange itself is an explicit fidelity limitation:
        // the original trace anchor was absent or dangling.
        let mut unsupported_count = 1u64;
        let exchange_metadata = legacy_exchange_metadata(source, &mut unsupported_count);
        let mut external_manifests = Vec::new();
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"alex-legacy-unlinked-tool-metadata-v2\0");
        hash_metadata_field(&mut hasher, "synthetic_trace_id", Some(&source.trace_id));
        for (tool, manifests) in &tools {
            let tool_time_ns = u64::try_from(tool.ts_start_ms.max(0))
                .unwrap_or(0)
                .saturating_mul(1_000_000);
            let mut call = StageData::new(StageKind::ToolCall, tool_time_ns);
            call.request_body_manifest_ref = manifests.get("tool_arguments").copied();
            let provenance = legacy_tool_provenance(tool);
            call.routing_reason = legacy_stage_field(
                Some(&provenance),
                "unlinked tool-call provenance",
                &mut unsupported_count,
            );
            stages.push(call);

            let result_time_ns = tool
                .ts_end_ms
                .and_then(|value| u64::try_from(value.max(0)).ok())
                .map(|value| value.saturating_mul(1_000_000))
                .unwrap_or(tool_time_ns)
                .max(tool_time_ns);
            let mut result = StageData::new(StageKind::ToolResult, result_time_ns);
            result.response_body_manifest_ref = manifests.get("tool_result").copied();
            result.routing_reason = tool
                .exit_status
                .map(|value| format!("legacy_tool_exit_status:{value}").into_bytes());
            if tool.is_error == Some(true) {
                result.error_class = Some(b"tool_error".to_vec());
            }
            stages.push(result);
            external_manifests.extend(manifests.values().copied());

            for (label, value) in [
                ("tool_id", Some(tool.id.as_str())),
                ("tool_harness", Some(tool.harness.as_str())),
                ("tool_session", Some(tool.session_id.as_str())),
                ("tool_turn", tool.turn_id.as_deref()),
                ("legacy_trace_id", tool.trace_id.as_deref()),
                ("tool_call_id", Some(tool.tool_call_id.as_str())),
                ("tool_name", Some(tool.tool_name.as_str())),
                ("tool_arguments_path", tool.arguments_path.as_deref()),
                ("tool_result_path", tool.result_path.as_deref()),
            ] {
                hash_metadata_field(&mut hasher, label, value);
            }
            for (label, value) in [
                ("tool_start_ms", tool.ts_start_ms),
                ("tool_end_ms", tool.ts_end_ms.unwrap_or(i64::MIN)),
                (
                    "tool_is_error",
                    tool.is_error.map(i64::from).unwrap_or(i64::MIN),
                ),
                ("tool_exit_status", tool.exit_status.unwrap_or(i64::MIN)),
            ] {
                hash_metadata_field(&mut hasher, label, Some(&value.to_string()));
            }
            let mut pairs = manifests.iter().collect::<Vec<_>>();
            pairs.sort_by(|left, right| left.0.cmp(right.0));
            for (kind, manifest) in pairs {
                hash_metadata_field(
                    &mut hasher,
                    &format!("tool_manifest:{}:{kind}", tool.id),
                    Some(&manifest.to_string()),
                );
            }
        }
        for missing in &missing_manifests {
            hash_metadata_field(&mut hasher, "missing_manifest", Some(missing));
        }
        external_manifests.sort_by_key(|id| id.0);
        external_manifests.dedup();
        stages.sort_by_key(|stage| stage.wall_time_ns);
        let source_size = tools.iter().fold(0u64, |total, (tool, _)| {
            [
                Some(tool.id.as_str()),
                Some(tool.harness.as_str()),
                Some(tool.session_id.as_str()),
                tool.turn_id.as_deref(),
                tool.trace_id.as_deref(),
                Some(tool.tool_call_id.as_str()),
                Some(tool.tool_name.as_str()),
                tool.arguments_path.as_deref(),
                tool.result_path.as_deref(),
            ]
            .into_iter()
            .flatten()
            .fold(total, |total, value| {
                total.saturating_add(value.len() as u64)
            })
        });
        let catalog_capture = LarExchangeCapture {
            trace_id: source.trace_id.clone(),
            session_id: source.session_id.clone(),
            run_id: None,
            wall_time_ns,
            client_request_headers: None,
            client_request_trailers: None,
            client_response_headers: None,
            client_response_trailers: None,
            upstream_attempts: Vec::new(),
            upstream_stream_reads: None,
            provider: None,
            requested_model: None,
            routed_model: None,
            account_id: None,
            routing_reason: Some("legacy_unlinked_tool_call".into()),
            status_code: None,
            error_class: None,
            error_message: None,
        };
        Ok(LegacyMetadataPlan {
            owner_kind: "tool_call",
            owner_id: tool.id.clone(),
            trace_id: source.trace_id.clone(),
            session_id: source.session_id.clone(),
            run_id: None,
            source_metadata: source.clone(),
            exchange_metadata,
            catalog_capture,
            stages,
            external_manifests,
            fingerprint: hex(hasher.finalize().as_bytes()),
            source_size,
            unsupported_count,
            missing_manifests,
        })
    }

    fn legacy_metadata_page_needs_import(&self, plans: &[LegacyMetadataPlan]) -> Result<bool> {
        if plans.is_empty() {
            return Ok(false);
        }
        let mut migrated = HashSet::<(String, String, String)>::new();
        let conn = self.conn.lock().unwrap();
        for chunk in plans.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = conn.prepare(&format!(
                "SELECT owner_kind, owner_id, source_fingerprint
                   FROM lar_migration_items
                  WHERE artifact_kind='exchange_metadata'
                    AND stage_id='' AND state='migrated'
                    AND validation_state='validated'
                    AND owner_id IN ({placeholders})"
            ))?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(chunk.iter().map(|plan| plan.owner_id.as_str())),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )?;
            for row in rows {
                migrated.insert(row?);
            }
        }
        Ok(plans.iter().any(|plan| {
            !migrated.contains(&(
                plan.owner_kind.to_string(),
                plan.owner_id.clone(),
                plan.fingerprint.clone(),
            ))
        }))
    }

    fn legacy_metadata_item_states(
        &self,
        job_id: &str,
        plans: &[LegacyMetadataPlan],
    ) -> Result<HashMap<String, String>> {
        let item_ids = plans
            .iter()
            .map(|plan| {
                metadata_item_id(job_id, plan.owner_kind, &plan.owner_id, &plan.fingerprint)
            })
            .collect::<Vec<_>>();
        let mut states = HashMap::new();
        let conn = self.conn.lock().unwrap();
        for chunk in item_ids.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = conn.prepare(&format!(
                "SELECT item_id, state FROM lar_migration_items
                  WHERE item_id IN ({placeholders})"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (item_id, state) = row?;
                states.insert(item_id, state);
            }
        }
        Ok(states)
    }

    fn validate_ready_metadata_manifests(&self, plans: &[LegacyMetadataPlan]) -> Result<()> {
        let mut ids = plans
            .iter()
            .flat_map(|plan| plan.external_manifests.iter().copied())
            .collect::<Vec<_>>();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        let conn = self.conn.lock().unwrap();
        for chunk in ids.chunks(400) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let ready: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM lar_manifests
                      WHERE state='ready' AND manifest_id IN ({placeholders})"
                ),
                rusqlite::params_from_iter(chunk.iter().map(ToString::to_string)),
                |row| row.get(0),
            )?;
            if usize::try_from(ready)? != chunk.len() {
                bail!("a legacy metadata body manifest became unavailable during import");
            }
        }
        Ok(())
    }

    fn legacy_source_needs_import(&self, source: &LarLegacyArtifact) -> Result<bool> {
        // LAR-with-fallback deliberately retains a gzip copy. If that live
        // capture became visible after this migration pass began, its already
        // validated pointer is authoritative and the fallback must not be
        // rediscovered as legacy work on a later inventory page.
        let validated_pointer: Option<String> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT a.fidelity FROM lar_trace_artifacts a
               JOIN lar_manifests m ON m.manifest_id=a.manifest_id
               WHERE a.owner_kind=?1 AND a.owner_id=?2 AND a.artifact_kind=?3
                  AND a.stage_id=?4 AND a.validation_state='validated'
                  AND m.state='ready' LIMIT 1",
                params![
                    source.owner_kind,
                    source.owner_id,
                    source.artifact_kind,
                    source.stage_id.as_deref().unwrap_or(""),
                ],
                |row| row.get(0),
            )
            .optional()?;
        if validated_pointer.as_deref() == Some("captured") {
            return Ok(false);
        }
        let resolved = resolve_source_path(&self.data_dir, &source.source_path);
        let (size, mtime_ms) = source_metadata(&resolved);
        if size.is_none() && validated_pointer.is_some() {
            return Ok(false);
        }
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
        let mut offset = 0usize;
        loop {
            let rows = self.legacy_trace_metadata_rows(offset, inventory_batch)?;
            if rows.is_empty() {
                break;
            }
            offset += rows.len();
            let plans = self.legacy_metadata_plans(&rows)?;
            if self.legacy_metadata_page_needs_import(&plans)? {
                return Ok(true);
            }
        }
        let mut offset = 0usize;
        loop {
            let rows = self.legacy_unlinked_tool_metadata_rows(offset, inventory_batch)?;
            if rows.is_empty() {
                break;
            }
            offset += rows.len();
            let plans = self.legacy_metadata_plans(&rows)?;
            if self.legacy_metadata_page_needs_import(&plans)? {
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
            let mut header = FileHeader::body_pack(
                ids.file_uuid_bytes,
                u64::try_from(started_ms.max(0)).unwrap_or(0) * 1_000_000,
                b"alex-store legacy importer v2".to_vec(),
            );
            header.required_feature_bits |= REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS;
            ArchiveWriter::create(file, header, chunker, limits.clone())?
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

    fn import_pack_supports_external_body_refs(&self, ids: &ImportIds) -> Result<bool> {
        let reader = ArchiveReader::open(File::open(&ids.file_path)?, Limits::default())?;
        Ok(reader.header().required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS != 0)
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
                if state == "active" && !self.import_pack_supports_external_body_refs(&ids)? =>
            {
                let mut old_writer = self.open_import_pack(&ids, started_ms, chunker, &limits)?;
                self.seal_import_pack(&ids, &mut old_writer)?;
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
        if inventory_complete && !resource_paused {
            inventory_complete = self.import_legacy_trace_metadata(
                &base_ids,
                &mut ids,
                options,
                &mut writer,
                &mut report,
                started_ms,
                chunker,
                &limits,
                max_attempts,
                &resources,
            )?;
        }
        report.limit_reached = !inventory_complete;
        // A completed batch ends in an append-only persisted checkpoint so
        // mixed-mode trace pages can open this active pack through its index
        // instead of rescanning every preceding body record. Empty resumptions
        // do not append another identical snapshot.
        if report.attempted > 0 || report.metadata_attempted > 0 {
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

    #[allow(clippy::too_many_arguments)]
    fn import_legacy_trace_metadata(
        &self,
        base: &ImportIds,
        ids: &mut ImportIds,
        options: &LarLegacyImportOptions,
        writer: &mut ArchiveWriter<File>,
        report: &mut LarLegacyImportReport,
        started_ms: i64,
        chunker: ChunkerConfig,
        limits: &Limits,
        max_attempts: usize,
        resources: &ResourceController,
    ) -> Result<bool> {
        let batch_size = options.resources.effective_batch_size(options.batch_size);
        for tool_only in [false, true] {
            let mut offset = 0usize;
            loop {
                let rows = if tool_only {
                    self.legacy_unlinked_tool_metadata_rows(offset, batch_size)?
                } else {
                    self.legacy_trace_metadata_rows(offset, batch_size)?
                };
                if rows.is_empty() {
                    break;
                }
                offset += rows.len();
                let plans = self.legacy_metadata_plans(&rows)?;
                let item_states = self.legacy_metadata_item_states(&ids.job_id, &plans)?;
                self.validate_ready_metadata_manifests(&plans)?;
                for plan in plans {
                    let cpu_started = Instant::now();
                    report.metadata_inventoried = report.metadata_inventoried.saturating_add(1);
                    let item_id = metadata_item_id(
                        &ids.job_id,
                        plan.owner_kind,
                        &plan.owner_id,
                        &plan.fingerprint,
                    );
                    self.discover_lar_migration_item(
                        &LarMigrationItem {
                            item_id: item_id.clone(),
                            job_id: ids.job_id.clone(),
                            owner_kind: plan.owner_kind.into(),
                            owner_id: plan.owner_id.clone(),
                            artifact_kind: "exchange_metadata".into(),
                            stage_id: None,
                            source_path: None,
                            source_size: Some(plan.source_size),
                            source_mtime_ms: None,
                            source_fingerprint: plan.fingerprint.clone(),
                            fidelity: "legacy_normalized".into(),
                        },
                        now_ms(),
                    )?;
                    let state = item_states.get(&item_id).map(String::as_str);
                    if matches!(state, Some("migrated" | "skipped")) {
                        report.metadata_skipped = report.metadata_skipped.saturating_add(1);
                        resources.finish_cpu_slice(cpu_started.elapsed());
                        continue;
                    }
                    if report.attempted.saturating_add(report.metadata_attempted) as usize
                        >= max_attempts
                    {
                        resources.finish_cpu_slice(cpu_started.elapsed());
                        return Ok(false);
                    }
                    report.metadata_attempted = report.metadata_attempted.saturating_add(1);
                    if !plan.missing_manifests.is_empty() {
                        let detail = format!(
                        "legacy exchange metadata references body artifacts that are not validated: {}",
                        plan.missing_manifests.join(", ")
                    );
                        self.record_lar_migration_item_failure(
                            &ids.job_id,
                            &item_id,
                            &options.lease_owner,
                            "body_unavailable",
                            &detail,
                            now_ms(),
                        )?;
                        report.metadata_failed = report.metadata_failed.saturating_add(1);
                        report.push_error(LarLegacyImportError {
                            item_id,
                            owner_kind: plan.owner_kind.into(),
                            owner_id: plan.owner_id.clone(),
                            artifact_kind: "exchange_metadata".into(),
                            error_kind: "body_unavailable".into(),
                            detail,
                        });
                        resources.finish_cpu_slice(cpu_started.elapsed());
                        continue;
                    }

                    if import_pack_limit_reached(writer, &options.resources)? {
                        self.rotate_import_pack(
                            base, ids, writer, report, started_ms, chunker, limits,
                        )?;
                    }
                    let request_header = crate::live_body_store::append_capture_header(
                        writer,
                        plan.catalog_capture.client_request_headers.as_ref(),
                    )?;
                    let response_header = crate::live_body_store::append_capture_header(
                        writer,
                        plan.catalog_capture.client_response_headers.as_ref(),
                    )?;
                    let mut stage_ids = Vec::with_capacity(plan.stages.len());
                    for mut stage in plan.stages.clone() {
                        if stage.kind == StageKind::ClientRequest {
                            stage.request_headers_ref = request_header;
                        }
                        if matches!(
                            stage.kind,
                            StageKind::UpstreamResponse
                                | StageKind::UpstreamFailure
                                | StageKind::InjectedResponse
                        ) {
                            stage.response_headers_ref = response_header;
                        }
                        stage_ids.push(writer.append_stage_with_external_manifests(
                            Stage::new(stage),
                            &plan.external_manifests,
                        )?);
                    }
                    let mut exchange_data = ExchangeData::new(
                        plan.trace_id.as_bytes(),
                        plan.catalog_capture.wall_time_ns,
                        plan.catalog_capture.wall_time_ns,
                        stage_ids.clone(),
                    );
                    exchange_data.session_id =
                        plan.session_id.as_deref().map(str::as_bytes).map(Vec::from);
                    exchange_data.run_id = plan.run_id.as_deref().map(str::as_bytes).map(Vec::from);
                    let exchange_id = writer.append_exchange_with_metadata(
                        Exchange::new(exchange_data),
                        plan.exchange_metadata.clone(),
                    )?;
                    writer.flush()?;
                    writer.get_ref().sync_all()?;
                    self.update_import_file_size(ids)?;
                    visit_import_boundary(options, LarLegacyImportBoundary::MetadataAppended)?;

                    let reader = ArchiveReader::open(File::open(&ids.file_path)?, limits.clone())?;
                    let exchange = reader.exchange(&exchange_id).with_context(|| {
                        format!("synced legacy exchange {exchange_id} is missing")
                    })?;
                    if exchange.data.stages != stage_ids {
                        bail!("synced legacy exchange stage order changed during validation");
                    }
                    if reader
                        .exchange_metadata(&exchange_id)
                        .map(|value| &value.data)
                        != Some(&plan.exchange_metadata)
                    {
                        bail!("synced legacy exchange metadata changed during validation");
                    }
                    for stage_id in &stage_ids {
                        if reader.stage(stage_id).is_none() {
                            bail!("synced legacy stage {stage_id} is missing");
                        }
                    }
                    for header_id in [request_header, response_header].into_iter().flatten() {
                        if reader.header_block(&header_id).is_none() {
                            bail!("synced legacy header block {header_id} is missing");
                        }
                    }
                    visit_import_boundary(options, LarLegacyImportBoundary::MetadataValidated)?;

                    let file_uuid = ids.file_uuid.clone();
                    let file_size = std::fs::metadata(&ids.file_path)?.len();
                    let header_count =
                        u64::from(request_header.is_some()) + u64::from(response_header.is_some());
                    let published = self.publish_lar_migration_exchange(
                        &ids.job_id,
                        &item_id,
                        &options.lease_owner,
                        &plan.trace_id,
                        &exchange_id.to_string(),
                        &file_uuid,
                        plan.session_id.as_deref(),
                        stage_ids.len() as u64,
                        header_count,
                        plan.unsupported_count,
                        now_ms(),
                        |tx| {
                            crate::live_body_store::catalog_capture_headers(
                                tx,
                                &file_uuid,
                                &plan.catalog_capture,
                                now_ms(),
                            )?;
                            crate::live_body_store::catalog_capture_exchange(
                                tx,
                                &file_uuid,
                                &plan.trace_id,
                                &exchange_id.to_string(),
                                plan.catalog_capture.wall_time_ns,
                                stage_ids.len() as u64,
                                "legacy_normalized",
                            )?;
                            crate::live_body_store::catalog_capture_stages(
                                tx,
                                &file_uuid,
                                &plan.trace_id,
                                &stage_ids,
                                writer,
                                "legacy_normalized",
                                now_ms(),
                            )?;
                            tx.execute(
                                "UPDATE lar_files SET size_bytes=?2 WHERE file_uuid=?1",
                                params![file_uuid, file_size],
                            )?;
                            Ok(())
                        },
                    )?;
                    visit_import_boundary(options, LarLegacyImportBoundary::MetadataPublished)?;
                    if published {
                        report.metadata_migrated = report.metadata_migrated.saturating_add(1);
                        report.metadata_unsupported = report
                            .metadata_unsupported
                            .saturating_add(plan.unsupported_count);
                    } else {
                        report.metadata_skipped = report.metadata_skipped.saturating_add(1);
                    }
                    self.checkpoint_lar_migration_job(
                        &ids.job_id,
                        &options.lease_owner,
                        &item_id,
                        now_ms(),
                    )?;
                    resources.finish_cpu_slice(cpu_started.elapsed());
                }
            }
        }
        Ok(true)
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
        let mut body = Vec::new();
        self.write_lar_manifest_body(manifest_id, &mut body)?;
        Ok(body)
    }

    /// Stream a cataloged manifest through the ordinary verified reader.
    pub fn write_lar_manifest_body<W: Write>(
        &self,
        manifest_id: &str,
        output: &mut W,
    ) -> Result<u64> {
        if let Some(written) = self.write_catalog_manifest_body(manifest_id, output)? {
            return Ok(written);
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
        reader.write_body(&id, output).map_err(Into::into)
    }

    /// Resolve a batch of mixed legacy/LAR bodies under one reconstructed-byte
    /// budget. Archive-backed manifests share readers, catalog-only manifests
    /// may span several live packs, and every request receives an explicit
    /// result rather than allowing one failure to erase the whole batch.
    fn lar_artifact_locations_batched(
        &self,
        requests: &[LarArtifactReadRequest],
    ) -> Vec<Result<Option<LarArtifactLocation>>> {
        let mut output = (0..requests.len())
            .map(|_| Ok(None))
            .collect::<Vec<Result<Option<LarArtifactLocation>>>>();
        let conn = self.conn.lock().unwrap();
        for indexed in requests.iter().enumerate().collect::<Vec<_>>().chunks(150) {
            let valid = indexed
                .iter()
                .filter(|(_, request)| matches!(request.owner_kind.as_str(), "trace" | "tool_call"))
                .copied()
                .collect::<Vec<_>>();
            for (index, request) in indexed.iter().copied().filter(|(_, request)| {
                !matches!(request.owner_kind.as_str(), "trace" | "tool_call")
            }) {
                output[index] = Err(anyhow::anyhow!(
                    "unsupported LAR artifact owner kind {}",
                    request.owner_kind
                ));
            }
            if valid.is_empty() {
                continue;
            }
            let values_clause = std::iter::repeat_n("(?,?,?,?,?)", valid.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "WITH requested(idx, owner_kind, owner_id, artifact_kind, stage_id) AS
                     (VALUES {values_clause})
                 SELECT r.idx,
                        m.manifest_id, m.total_length, m.hash_algorithm,
                        m.whole_body_hash, a.fidelity,
                        mi.source_path, mi.state, mi.error_kind, mi.validation_error,
                        CASE
                          WHEN r.owner_kind='trace' AND r.artifact_kind='client_request'
                            THEN t.req_body_path
                          WHEN r.owner_kind='trace' AND r.artifact_kind='upstream_request'
                            THEN t.upstream_req_body_path
                          WHEN r.owner_kind='trace' AND r.artifact_kind='client_response'
                            THEN t.resp_body_path
                          WHEN r.owner_kind='tool_call' AND r.artifact_kind='tool_arguments'
                            THEN c.args_body_path
                          WHEN r.owner_kind='tool_call' AND r.artifact_kind='tool_result'
                            THEN c.result_body_path
                        END legacy_path
                   FROM requested r
                   LEFT JOIN lar_trace_artifacts a
                     ON a.owner_kind=r.owner_kind AND a.owner_id=r.owner_id
                    AND a.artifact_kind=r.artifact_kind AND a.stage_id=r.stage_id
                    AND a.validation_state='validated'
                   LEFT JOIN lar_manifests m
                     ON m.manifest_id=a.manifest_id AND m.state='ready'
                   LEFT JOIN lar_migration_items mi ON mi.item_id=(
                     SELECT newest.item_id FROM lar_migration_items newest
                      WHERE newest.owner_kind=r.owner_kind
                        AND newest.owner_id=r.owner_id
                        AND newest.artifact_kind=r.artifact_kind
                        AND newest.stage_id=r.stage_id
                      ORDER BY newest.updated_at_ms DESC LIMIT 1)
                   LEFT JOIN traces t
                     ON r.owner_kind='trace' AND t.id=r.owner_id
                   LEFT JOIN tool_calls c
                     ON r.owner_kind='tool_call' AND c.id=r.owner_id
                  ORDER BY r.idx"
            );
            let mut values = Vec::<rusqlite::types::Value>::with_capacity(valid.len() * 5);
            for (index, request) in &valid {
                values.push(i64::try_from(*index).unwrap_or(i64::MAX).into());
                values.push(request.owner_kind.clone().into());
                values.push(request.owner_id.clone().into());
                values.push(request.artifact_kind.clone().into());
                values.push(request.stage_id.clone().unwrap_or_default().into());
            }
            let queried = (|| -> Result<()> {
                let mut statement = conn.prepare(&sql)?;
                let mut rows = statement.query(rusqlite::params_from_iter(values))?;
                while let Some(row) = rows.next()? {
                    let index = usize::try_from(row.get::<_, i64>(0)?)?;
                    let manifest_id = row.get::<_, Option<String>>(1)?;
                    if let Some(manifest_id) = manifest_id {
                        let length = row.get::<_, i64>(2)?;
                        output[index] = Ok(Some(LarArtifactLocation::Lar {
                            manifest_id,
                            total_length: u64::try_from(length)
                                .context("manifest length is negative")?,
                            hash_algorithm: row.get(3)?,
                            whole_body_hash: row.get(4)?,
                            fidelity: row.get(5)?,
                        }));
                        continue;
                    }
                    let migration_path = row.get::<_, Option<String>>(6)?;
                    let migration_state = row.get::<_, Option<String>>(7)?;
                    let migration_error = row
                        .get::<_, Option<String>>(8)?
                        .zip(row.get::<_, Option<String>>(9)?)
                        .map(|(kind, detail)| LarArtifactError { kind, detail });
                    if migration_state.as_deref() == Some("failed")
                        && migration_error.as_ref().is_some_and(|error| {
                            matches!(error.kind.as_str(), "missing" | "corrupt" | "unsupported")
                        })
                    {
                        output[index] = Ok(Some(LarArtifactLocation::Unavailable {
                            source_path: migration_path,
                            error: migration_error.expect("failed source error was checked"),
                        }));
                    } else if let Some(path) = migration_path {
                        output[index] = Ok(Some(LarArtifactLocation::Legacy {
                            path,
                            migration_error,
                        }));
                    } else if let Some(path) = row.get::<_, Option<String>>(10)? {
                        output[index] = Ok(Some(LarArtifactLocation::Legacy {
                            path,
                            migration_error: None,
                        }));
                    }
                }
                Ok(())
            })();
            if let Err(error) = queried {
                let detail = format!("batching LAR artifact locations: {error:#}");
                for (index, _) in valid {
                    output[index] = Err(anyhow::anyhow!(detail.clone()));
                }
            }
        }
        output
    }

    pub fn read_lar_or_legacy_artifact_batch_bounded(
        &self,
        requests: &[LarArtifactReadRequest],
        byte_budget: u64,
    ) -> Vec<LarArtifactBatchRead> {
        let mut output = vec![LarArtifactBatchRead::Missing; requests.len()];
        let mut lar = Vec::new();
        let mut remaining = byte_budget;
        for (index, location) in self
            .lar_artifact_locations_batched(requests)
            .into_iter()
            .enumerate()
        {
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
            let mut requests_by_manifest = HashMap::<String, Vec<usize>>::new();
            for (index, manifest_id) in lar {
                requests_by_manifest
                    .entry(manifest_id)
                    .or_default()
                    .push(index);
            }
            let manifest_ids = requests_by_manifest.keys().cloned().collect::<Vec<_>>();
            let mut located = HashSet::<String>::new();
            let conn = self.conn.lock().unwrap();
            for chunk in manifest_ids.chunks(400) {
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let queried = (|| -> Result<()> {
                    let mut statement = conn.prepare(&format!(
                        "SELECT m.manifest_id, f.path, f.file_uuid, f.state
                           FROM lar_manifests m JOIN lar_files f ON f.file_uuid=m.file_uuid
                          WHERE m.manifest_id IN ({placeholders}) AND m.state='ready'"
                    ))?;
                    let rows =
                        statement.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                            ))
                        })?;
                    for row in rows {
                        let (manifest_id, path, file_uuid, state) = row?;
                        located.insert(manifest_id.clone());
                        let indexes = requests_by_manifest
                            .get(&manifest_id)
                            .expect("queried manifest originated in request batch");
                        let resolved = resolve_source_path(&self.data_dir, &path);
                        for index in indexes.iter().copied() {
                            if !matches!(state.as_str(), "active" | "sealed") {
                                output[index] = LarArtifactBatchRead::ArchiveUnavailable(
                                    LarArchiveUnavailableError::offline(
                                        file_uuid.clone(),
                                        resolved.to_string_lossy(),
                                    ),
                                );
                                continue;
                            }
                            if !resolved.exists() {
                                output[index] = LarArtifactBatchRead::ArchiveUnavailable(
                                    LarArchiveUnavailableError::missing(
                                        file_uuid.clone(),
                                        resolved.to_string_lossy(),
                                    ),
                                );
                                continue;
                            }
                            match ManifestId::from_str(&manifest_id) {
                                Ok(id) => archives
                                    .entry((resolved.clone(), file_uuid.clone()))
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
                    }
                    Ok(())
                })();
                if let Err(error) = queried {
                    for manifest_id in chunk {
                        for index in requests_by_manifest
                            .get(manifest_id)
                            .into_iter()
                            .flatten()
                            .copied()
                        {
                            located.insert(manifest_id.clone());
                            output[index] = LarArtifactBatchRead::Error {
                                kind: "catalog".into(),
                                detail: format!("batch locating LAR manifests: {error:#}"),
                            };
                        }
                    }
                }
            }
            for manifest_id in manifest_ids {
                if !located.contains(&manifest_id) {
                    for index in requests_by_manifest
                        .remove(&manifest_id)
                        .unwrap_or_default()
                    {
                        catalog_manifests.push((index, manifest_id.clone()));
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

    /// Stream one mixed-mode artifact without materializing its decoded bytes.
    /// Returns `false` only when no artifact is cataloged or referenced.
    pub fn write_lar_or_legacy_artifact<W: Write>(
        &self,
        owner_kind: &str,
        owner_id: &str,
        artifact_kind: &str,
        stage_id: Option<&str>,
        output: &mut W,
    ) -> Result<bool> {
        match self.lar_artifact_location(owner_kind, owner_id, artifact_kind, stage_id)? {
            Some(LarArtifactLocation::Lar { manifest_id, .. }) => {
                self.write_lar_manifest_body(&manifest_id, output)?;
                Ok(true)
            }
            Some(LarArtifactLocation::Legacy { path, .. }) => {
                let path = resolve_source_path(&self.data_dir, &path);
                let file = File::open(&path)
                    .with_context(|| format!("opening legacy body at {}", path.display()))?;
                let mut decoder = GzDecoder::new(file);
                std::io::copy(&mut decoder, output)
                    .with_context(|| format!("decompressing legacy body at {}", path.display()))?;
                Ok(true)
            }
            Some(LarArtifactLocation::Unavailable { error, .. }) => {
                bail!(
                    "legacy artifact is unavailable ({}): {}",
                    error.kind,
                    error.detail
                )
            }
            None => Ok(false),
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
    use alex_core::TraceRecord;

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

    #[test]
    fn metadata_planning_queries_scale_by_page_chunk_not_trace() {
        let path = std::env::temp_dir().join(format!(
            "alex-lar-metadata-plan-query-shape-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        let store = Store::open(path.clone()).unwrap();
        for index in 0..401 {
            store
                .insert_trace(&TraceRecord {
                    id: format!("trace-{index:04}"),
                    ts_request_ms: index,
                    session_id: Some("query-shape".into()),
                    ..TraceRecord::default()
                })
                .unwrap();
        }
        let rows = store.legacy_trace_metadata_rows(0, 500).unwrap();
        LEGACY_METADATA_PLAN_QUERY_COUNT.store(0, Ordering::Relaxed);
        let plans = store.legacy_metadata_plans(&rows).unwrap();
        assert_eq!(plans.len(), 401);
        assert_eq!(
            LEGACY_METADATA_PLAN_QUERY_COUNT.load(Ordering::Relaxed),
            4,
            "planning should issue two set queries per 400-row SQLite chunk"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(path);
    }
}
