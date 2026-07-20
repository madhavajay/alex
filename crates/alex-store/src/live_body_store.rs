//! Rollback-safe live body writes into rolling, globally deduplicated LAR packs.
//! Production defaults to legacy gzip; experimental modes retain rollback copies.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{MutexGuard, TryLockError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alex_lar::{
    read_chunk_record_at, read_file_header, ArchiveReader, ArchiveWriter, BodyManifest,
    CheckpointRecordDescriptor, ChunkHash, ChunkRecordDescriptor, ChunkRef, ChunkerConfig,
    Exchange, ExchangeData, FileHeader, FrameRead, FrameReader, HeaderAtom, HeaderBlock,
    HeaderFidelity, Limits, ManifestId, RecordFrame, RecordType, RecoveryStatus, Stage, StageData,
    StageKind, StreamIndex, StreamRead, StreamingChunker, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
};
use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::{
    lar_archive_ops::{compute_lar_file_identity, record_lar_file_identity, resolved_catalog_path},
    LarArchiveUnavailableError, Store,
};

static LIVE_PACK_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LarBodyStoreMode {
    #[default]
    Legacy,
    DualWriteValidated,
    LarWithFallback,
}

impl std::str::FromStr for LarBodyStoreMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "legacy" => Ok(Self::Legacy),
            "dual" | "dual-write" | "dual-write-validated" => Ok(Self::DualWriteValidated),
            "lar-with-fallback" | "lar_fallback" => Ok(Self::LarWithFallback),
            other => bail!(
                "unsupported LAR body-store mode '{other}'; expected legacy, dual-write-validated, or lar-with-fallback"
            ),
        }
    }
}

impl std::fmt::Display for LarBodyStoreMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Legacy => "legacy",
            Self::DualWriteValidated => "dual-write-validated",
            Self::LarWithFallback => "lar-with-fallback",
        })
    }
}

#[derive(Clone, Debug)]
pub struct LarBodyStoreConfig {
    pub mode: LarBodyStoreMode,
    pub max_pack_bytes: u64,
    pub max_pack_age: Duration,
    pub checkpoint_bytes: u64,
    pub checkpoint_interval: Duration,
    pub writer_lock_timeout: Duration,
    pub chunker: ChunkerConfig,
    pub limits: Limits,
}

impl Default for LarBodyStoreConfig {
    fn default() -> Self {
        Self {
            mode: LarBodyStoreMode::Legacy,
            max_pack_bytes: 512 * 1024 * 1024,
            max_pack_age: Duration::from_secs(60 * 60),
            checkpoint_bytes: 8 * 1024 * 1024,
            checkpoint_interval: Duration::from_secs(30),
            writer_lock_timeout: Duration::from_millis(25),
            chunker: ChunkerConfig::default(),
            limits: Limits::default(),
        }
    }
}

impl LarBodyStoreConfig {
    fn validate(&self) -> Result<()> {
        if self.max_pack_bytes == 0
            || self.max_pack_age.is_zero()
            || self.checkpoint_bytes == 0
            || self.checkpoint_interval.is_zero()
        {
            bail!("LAR body-pack size, age, and checkpoint cadence must be positive");
        }
        self.chunker.validate().map_err(anyhow::Error::new)?;
        if self
            .chunker
            .for_body_length(self.limits.max_body_length)
            .max_size as u64
            > self.limits.max_chunk_uncompressed
        {
            bail!("LAR chunker maximum exceeds reader limit");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LarBodyOwnerKind {
    Trace,
    ToolCall,
}

impl LarBodyOwnerKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::ToolCall => "tool_call",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarBodyArtifact {
    pub owner_kind: LarBodyOwnerKind,
    pub owner_id: String,
    pub artifact_kind: String,
    pub stage_id: Option<String>,
}

impl LarBodyArtifact {
    pub fn trace(owner_id: impl Into<String>, artifact_kind: impl Into<String>) -> Self {
        Self {
            owner_kind: LarBodyOwnerKind::Trace,
            owner_id: owner_id.into(),
            artifact_kind: artifact_kind.into(),
            stage_id: None,
        }
    }

    pub fn tool_call(owner_id: impl Into<String>, artifact_kind: impl Into<String>) -> Self {
        Self {
            owner_kind: LarBodyOwnerKind::ToolCall,
            owner_id: owner_id.into(),
            artifact_kind: artifact_kind.into(),
            stage_id: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarBodyWriteResult {
    pub legacy_path: String,
    pub manifest_id: Option<String>,
    pub lar_error: Option<String>,
}

pub const LAR_HEADER_FLAG_REDACTED: u32 = 1;
const REDACTED_HEADER_VALUE: &[u8] = b"<redacted>";

/// One ordered header list as exposed by the HTTP stack. Duplicate fields and
/// their observed order are retained. Axum/reqwest normalize field-name casing,
/// so their live captures use `LegacyCasingUnknown` while remaining ordered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarHeaderCapture {
    fidelity: HeaderFidelity,
    atoms: Vec<HeaderAtom>,
}

impl LarHeaderCapture {
    pub fn observed<I, N, V>(headers: I) -> Self
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        Self::from_pairs(HeaderFidelity::LegacyCasingUnknown, headers)
    }

    /// Legacy SQLite headers were normalized into a JSON object. Their source
    /// order, duplicate fields, and original casing are irrecoverable.
    pub fn legacy_normalized<I, N, V>(headers: I) -> Self
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        Self::from_pairs(HeaderFidelity::LegacyOrderAndCasingUnknown, headers)
    }

    fn from_pairs<I, N, V>(fidelity: HeaderFidelity, headers: I) -> Self
    where
        I: IntoIterator<Item = (N, V)>,
        N: AsRef<[u8]>,
        V: AsRef<[u8]>,
    {
        let atoms = headers
            .into_iter()
            .map(|(name, value)| {
                let name = name.as_ref().to_vec();
                let sensitive = sensitive_header_name(&name);
                HeaderAtom {
                    original_name: name,
                    value: if sensitive {
                        REDACTED_HEADER_VALUE.to_vec()
                    } else {
                        value.as_ref().to_vec()
                    },
                    flags: if sensitive {
                        LAR_HEADER_FLAG_REDACTED
                    } else {
                        0
                    },
                }
            })
            .collect();
        Self { fidelity, atoms }
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarUpstreamAttemptCapture {
    pub attempt_number: u32,
    pub wall_time_ns: u64,
    pub request_headers: Option<LarHeaderCapture>,
    pub response_headers: Option<LarHeaderCapture>,
    pub status_code: Option<u16>,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStreamReadCapture {
    pub byte_offset: u64,
    pub byte_length: u64,
    pub delta_from_first_byte_ns: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LarExchangeBodyRefs {
    pub client_request_manifest_id: Option<String>,
    pub upstream_request_manifest_id: Option<String>,
    pub upstream_response_manifest_id: Option<String>,
    pub client_response_manifest_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarExchangeCapture {
    pub trace_id: String,
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub wall_time_ns: u64,
    pub client_request_headers: Option<LarHeaderCapture>,
    pub client_response_headers: Option<LarHeaderCapture>,
    pub upstream_attempts: Vec<LarUpstreamAttemptCapture>,
    /// Exact read boundaries observed while consuming the final raw upstream
    /// stream. `None` means timing was not observed or capture overflowed.
    pub upstream_stream_reads: Option<Vec<LarStreamReadCapture>>,
    pub provider: Option<String>,
    pub requested_model: Option<String>,
    pub routed_model: Option<String>,
    pub account_id: Option<String>,
    pub routing_reason: Option<String>,
    pub status_code: Option<u16>,
    pub error_class: Option<String>,
    pub error_message: Option<String>,
}

fn sensitive_header_name(name: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "set-cookie"
            | "api-key"
            | "x-api-key"
            | "x-goog-api-key"
            | "x-openai-api-key"
            | "anthropic-api-key"
            | "chatgpt-account-id"
            | "x-amz-security-token"
            | "access-token"
            | "refresh-token"
            | "x-auth-token"
            | "x-access-token"
            | "x-refresh-token"
            | "x-session-token"
            | "client-secret"
            | "secret"
    ) || lower.ends_with("-api-key")
        || lower.ends_with("-access-token")
        || lower.ends_with("-refresh-token")
        || lower.ends_with("-auth-token")
        || lower.ends_with("-secret")
        || lower.contains("credential")
}

pub(crate) struct LiveLarCoordinator {
    config: LarBodyStoreConfig,
    active: Option<ActivePack>,
    fail_next_catalog_commit: bool,
    fail_next_append: Option<InjectedAppendFailure>,
    recovery_scans: u64,
    source_pack_opens: u64,
    packs_reconciled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InjectedAppendFailure {
    Before,
    During,
}

struct ActivePack {
    file_uuid: String,
    writer: ArchiveWriter<File>,
    created_at_ms: i64,
    last_checkpoint_size: u64,
    last_checkpoint_at_ms: i64,
    next_checkpoint_sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingCheckpoint {
    sequence: u64,
    descriptor: CheckpointRecordDescriptor,
    created_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveFlush {
    size: u64,
    checkpoint: Option<PendingCheckpoint>,
}

impl LiveLarCoordinator {
    pub(crate) fn new(config: LarBodyStoreConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            active: None,
            fail_next_catalog_commit: false,
            fail_next_append: None,
            recovery_scans: 0,
            source_pack_opens: 0,
            packs_reconciled: false,
        })
    }

    /// Close any active pack and return the coordinator to its initial state.
    /// Reset holds this coordinator lock while it clears the catalog and owned
    /// archive directory, so a subsequent write starts a fresh archive set.
    pub(crate) fn reset(&mut self) -> Result<()> {
        let config = self.config.clone();
        *self = Self::new(config)?;
        Ok(())
    }
}

#[derive(Clone)]
struct CatalogChunk {
    path: PathBuf,
    descriptor: ChunkRecordDescriptor,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .try_into()
        .unwrap_or(u64::MAX)
}

/// Flush every publication boundary, but emit a complete derived index only
/// after a bounded amount of new data or time. Rewriting the full chunk/event
/// index after every body would make both write cost and archive growth
/// quadratic. Catalog-backed reads use their verified frame descriptors, so
/// they do not depend on a fresh checkpoint.
fn flush_active_pack(active: &mut ActivePack, config: &LarBodyStoreConfig) -> Result<ActiveFlush> {
    active.writer.flush().map_err(anyhow::Error::new)?;
    let before = active.writer.get_mut().seek(std::io::SeekFrom::End(0))?;
    let current_ms = now_ms();
    let bytes_since = before.saturating_sub(active.last_checkpoint_size);
    let elapsed_ms = current_ms.saturating_sub(active.last_checkpoint_at_ms) as u64;
    let descriptor = if bytes_since >= config.checkpoint_bytes
        || elapsed_ms >= config.checkpoint_interval.as_millis() as u64
    {
        Some(active.writer.checkpoint().map_err(anyhow::Error::new)?)
    } else {
        None
    };
    active.writer.get_ref().sync_all()?;
    let size = active.writer.get_mut().seek(std::io::SeekFrom::End(0))?;
    let checkpoint = if let Some(descriptor) = descriptor {
        if descriptor.append_offset != size {
            bail!("LAR checkpoint append boundary does not match the synced file size");
        }
        let sequence = active.next_checkpoint_sequence;
        active.next_checkpoint_sequence = sequence
            .checked_add(1)
            .context("LAR checkpoint sequence overflow")?;
        active.last_checkpoint_size = descriptor.append_offset;
        active.last_checkpoint_at_ms = current_ms;
        Some(PendingCheckpoint {
            sequence,
            descriptor,
            created_at_ms: current_ms,
        })
    } else {
        None
    };
    Ok(ActiveFlush { size, checkpoint })
}

fn publish_checkpoint(
    conn: &Connection,
    file_uuid: &str,
    checkpoint: PendingCheckpoint,
) -> Result<()> {
    let descriptor = checkpoint.descriptor;
    let frame_offset = i64::try_from(descriptor.frame_offset)
        .context("LAR checkpoint frame offset exceeds SQLite range")?;
    let frame_length = i64::try_from(descriptor.frame_length)
        .context("LAR checkpoint frame length exceeds SQLite range")?;
    let append_offset = i64::try_from(descriptor.append_offset)
        .context("LAR checkpoint append offset exceeds SQLite range")?;
    let sequence = i64::try_from(checkpoint.sequence)
        .context("LAR checkpoint sequence exceeds SQLite range")?;
    let record_id = format!("checkpoint:{}", descriptor.frame_offset);

    let by_frame: Option<(i64, String, i64, i64, Vec<u8>)> = conn
        .query_row(
            "SELECT checkpoint_sequence, record_id, frame_length, append_offset, checksum
             FROM lar_checkpoints WHERE file_uuid=?1 AND frame_offset=?2
             ORDER BY checkpoint_sequence DESC LIMIT 1",
            params![file_uuid, frame_offset],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    if let Some(stored) = by_frame {
        if stored.1 != record_id
            || stored.2 != frame_length
            || stored.3 != append_offset
            || stored.4.as_slice() != descriptor.payload_hash
        {
            bail!("cataloged LAR checkpoint frame is bound to different immutable bytes");
        }
        return Ok(());
    }

    let latest: Option<i64> = conn.query_row(
        "SELECT MAX(checkpoint_sequence) FROM lar_checkpoints WHERE file_uuid=?1",
        [file_uuid],
        |row| row.get(0),
    )?;
    if latest.is_some_and(|latest| sequence <= latest) {
        bail!("LAR checkpoint sequence is not monotonically increasing");
    }
    conn.execute(
        "INSERT INTO lar_checkpoints
           (file_uuid, checkpoint_sequence, record_id, frame_offset, frame_length,
            append_offset, created_at_ms, checksum)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            file_uuid,
            sequence,
            record_id,
            frame_offset,
            frame_length,
            append_offset,
            checkpoint.created_at_ms,
            descriptor.payload_hash.as_slice(),
        ],
    )?;
    Ok(())
}

fn next_checkpoint_sequence(conn: &Connection, file_uuid: &str) -> Result<u64> {
    let latest: Option<i64> = conn.query_row(
        "SELECT MAX(checkpoint_sequence) FROM lar_checkpoints WHERE file_uuid=?1",
        [file_uuid],
        |row| row.get(0),
    )?;
    let latest = latest.unwrap_or(0);
    let next = latest
        .checked_add(1)
        .context("LAR checkpoint sequence overflow")?;
    u64::try_from(next).context("LAR checkpoint sequence is negative")
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn archive_set_id(data_dir: &Path) -> String {
    let digest = blake3::hash(data_dir.to_string_lossy().as_bytes());
    format!("live-{}", hex(&digest.as_bytes()[..16]))
}

fn new_file_id(data_dir: &Path) -> ([u8; 16], String) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(data_dir.to_string_lossy().as_bytes());
    hasher.update(&now_ns().to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(
        &LIVE_PACK_COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .to_le_bytes(),
    );
    let digest = hasher.finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    let id = hex(&bytes);
    (bytes, id)
}

fn split_body(bytes: &[u8], config: ChunkerConfig) -> Result<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    let mut chunker = StreamingChunker::new(config.for_body_length(bytes.len() as u64))
        .map_err(anyhow::Error::new)?;
    chunker
        .push(bytes, |chunk| {
            chunks.push(chunk.to_vec());
            Ok(())
        })
        .map_err(anyhow::Error::new)?;
    chunker
        .finish(|chunk| {
            chunks.push(chunk.to_vec());
            Ok(())
        })
        .map_err(anyhow::Error::new)?;
    Ok(chunks)
}

fn artifact_for_legacy_kind(owner_id: &str, kind: &str) -> Option<LarBodyArtifact> {
    let (owner_kind, artifact_kind) = match kind {
        "request" | "request.json" => (LarBodyOwnerKind::Trace, "client_request"),
        "upstream-request" | "upstream-request.json" => {
            (LarBodyOwnerKind::Trace, "upstream_request")
        }
        "response" | "response.body" => (LarBodyOwnerKind::Trace, "client_response"),
        "tool-args.json" | "args.json" => (LarBodyOwnerKind::ToolCall, "tool_arguments"),
        "tool-result.json" | "result.json" => (LarBodyOwnerKind::ToolCall, "tool_result"),
        _ => return None,
    };
    Some(LarBodyArtifact {
        owner_kind,
        owner_id: owner_id.to_string(),
        artifact_kind: artifact_kind.to_string(),
        stage_id: None,
    })
}

impl Store {
    pub fn open_with_lar_body_store(data_dir: PathBuf, config: LarBodyStoreConfig) -> Result<Self> {
        Self::open_inner(data_dir, config)
    }

    pub fn lar_body_store_mode(&self) -> LarBodyStoreMode {
        self.live_lar_mode
    }

    /// Typed routing seam for request/upstream/response/tool/Dario capture.
    pub fn write_body_artifact(
        &self,
        artifact: &LarBodyArtifact,
        legacy_kind: &str,
        bytes: &[u8],
    ) -> Result<LarBodyWriteResult> {
        if artifact.owner_id.is_empty() || artifact.artifact_kind.is_empty() {
            bail!("LAR body artifact owner and kind must not be empty");
        }
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let legacy_path =
            self.write_legacy_body_dated(&date, &artifact.owner_id, legacy_kind, bytes)?;
        if self.lar_body_store_mode() == LarBodyStoreMode::Legacy {
            return Ok(LarBodyWriteResult {
                legacy_path,
                manifest_id: None,
                lar_error: None,
            });
        }

        let deadline = std::time::Instant::now() + self.live_lar_lock_timeout;
        let mut state = loop {
            match self.live_lar.try_lock() {
                Ok(state) => break state,
                Err(TryLockError::Poisoned(error)) => {
                    return Ok(LarBodyWriteResult {
                        legacy_path,
                        manifest_id: None,
                        lar_error: Some(format!("LAR writer lock poisoned: {error}")),
                    });
                }
                Err(TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(TryLockError::WouldBlock) => {
                    return Ok(LarBodyWriteResult {
                        legacy_path,
                        manifest_id: None,
                        lar_error: Some("LAR writer busy; retained legacy fallback".into()),
                    });
                }
            }
        };

        match self.write_lar_body_locked(&mut state, artifact, &legacy_path, bytes) {
            Ok(manifest_id) => Ok(LarBodyWriteResult {
                legacy_path,
                manifest_id: Some(manifest_id.to_string()),
                lar_error: None,
            }),
            Err(error) => {
                // Any append/flush failure can leave the writer cursor or tail
                // unusable. Drop it and force physical reconciliation before
                // another publication attempt; the legacy body is already
                // durable and remains the authoritative fallback.
                state.active = None;
                state.packs_reconciled = false;
                tracing::warn!(
                    owner_id = artifact.owner_id,
                    artifact_kind = artifact.artifact_kind,
                    "live LAR write failed; retaining legacy body: {error:#}"
                );
                Ok(LarBodyWriteResult {
                    legacy_path,
                    manifest_id: None,
                    lar_error: Some(format!("{error:#}")),
                })
            }
        }
    }

    /// Append the ordered transport metadata for one completed live exchange.
    /// Body fields are content IDs only: no request or response bytes are
    /// copied into header, stage, or exchange records.
    pub fn write_lar_exchange_capture(
        &self,
        capture: &LarExchangeCapture,
        bodies: &LarExchangeBodyRefs,
    ) -> Result<Option<String>> {
        if self.lar_body_store_mode() == LarBodyStoreMode::Legacy {
            return Ok(None);
        }
        if capture.trace_id.is_empty() {
            bail!("LAR exchange trace ID must not be empty");
        }

        let deadline = std::time::Instant::now() + self.live_lar_lock_timeout;
        let mut state = loop {
            match self.live_lar.try_lock() {
                Ok(state) => break state,
                Err(TryLockError::Poisoned(error)) => {
                    bail!("LAR writer lock poisoned: {error}")
                }
                Err(TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(TryLockError::WouldBlock) => bail!("LAR writer busy"),
            }
        };
        let result = (|| -> Result<Option<String>> {
            let mut conn = self.conn.lock().unwrap();
            ensure_active_pack(self, &mut state, &mut conn, 0)?;
            if state.active.as_ref().is_some_and(|active| {
                active.writer.header().required_feature_bits
                    & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS
                    == 0
            }) {
                let active = state.active.as_mut().expect("active pack exists");
                active.writer.seal().map_err(anyhow::Error::new)?;
                active.writer.get_ref().sync_all()?;
                let size = active.writer.get_mut().seek(std::io::SeekFrom::End(0))?;
                conn.execute(
                    "UPDATE lar_files SET state='sealed', sealed_at_ms=?2, size_bytes=?3
                 WHERE file_uuid=?1 AND state='active'",
                    params![active.file_uuid, now_ms(), size],
                )?;
                catalog_sealed_file_identity(self, &conn, &active.file_uuid, "live_rotation")?;
                state.active = None;
                ensure_active_pack(self, &mut state, &mut conn, 0)?;
            }

            let external_manifests = body_manifest_ids(bodies)?;
            for id in &external_manifests {
                let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM lar_manifests WHERE manifest_id=?1 AND state='ready')",
                [id.to_string()],
                |row| row.get(0),
            )?;
                if !exists {
                    bail!("LAR exchange references uncataloged body manifest {id}");
                }
            }

            let capture_sequence = if let Some(session_id) = capture.session_id.as_deref() {
                conn.query_row(
                "SELECT COALESCE(revision, 0) + 1 FROM lar_session_revisions WHERE session_id=?1",
                [session_id],
                |row| row.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(1)
            } else {
                capture.wall_time_ns
            };

            let external_stream = match capture.upstream_stream_reads.as_deref() {
                None | Some([]) => None,
                Some(_) if capture.upstream_attempts.is_empty() => {
                    bail!("LAR stream timing has no upstream attempt")
                }
                Some(reads) => {
                    let manifest_id = parse_optional_manifest(
                        bodies.upstream_response_manifest_id.as_deref(),
                    )?
                    .context("LAR stream timing is missing its raw upstream body manifest")?;
                    let body_length: u64 = conn.query_row(
                        "SELECT total_length FROM lar_manifests
                     WHERE manifest_id=?1 AND state='ready'",
                        [manifest_id.to_string()],
                        |row| row.get(0),
                    )?;
                    let reads = reads
                        .iter()
                        .map(|read| StreamRead {
                            byte_offset: read.byte_offset,
                            byte_length: read.byte_length,
                            delta_from_first_byte_ns: read.delta_from_first_byte_ns,
                        })
                        .collect();
                    Some((
                        StreamIndex::new(manifest_id, reads, Vec::new()),
                        body_length,
                    ))
                }
            };

            let checkpoint_config = state.config.clone();
            let active = state.active.as_mut().expect("active pack was established");
            let writer = &mut active.writer;
            let client_request_headers =
                append_capture_header(writer, capture.client_request_headers.as_ref())?;
            let client_response_headers =
                append_capture_header(writer, capture.client_response_headers.as_ref())?;
            let stream_index_ref = external_stream
                .map(|(index, body_length)| {
                    writer.append_stream_index_with_external_manifest(index, body_length)
                })
                .transpose()?;
            let mut stages = Vec::new();

            let mut client_request = StageData::new(StageKind::ClientRequest, capture.wall_time_ns);
            client_request.request_headers_ref = client_request_headers;
            client_request.request_body_manifest_ref =
                parse_optional_manifest(bodies.client_request_manifest_id.as_deref())?;
            stages.push(writer.append_stage_with_external_manifests(
                Stage::new(client_request),
                &external_manifests,
            )?);

            let mut router = StageData::new(StageKind::RouterDecision, capture.wall_time_ns);
            router.provider = capture
                .provider
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            router.requested_model = capture
                .requested_model
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            router.routed_model = capture
                .routed_model
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            router.account_id = capture
                .account_id
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            router.routing_reason = capture
                .routing_reason
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            stages.push(writer.append_stage(Stage::new(router))?);

            let last_attempt = capture.upstream_attempts.len().saturating_sub(1);
            for (index, attempt) in capture.upstream_attempts.iter().enumerate() {
                let request_headers =
                    append_capture_header(writer, attempt.request_headers.as_ref())?;
                let response_headers =
                    append_capture_header(writer, attempt.response_headers.as_ref())?;
                let mut request = StageData::new(StageKind::UpstreamRequest, attempt.wall_time_ns);
                request.attempt_number = Some(attempt.attempt_number);
                request.request_headers_ref = request_headers;
                if index == last_attempt {
                    request.request_body_manifest_ref =
                        parse_optional_manifest(bodies.upstream_request_manifest_id.as_deref())?;
                }
                stages.push(writer.append_stage_with_external_manifests(
                    Stage::new(request),
                    &external_manifests,
                )?);

                let response_kind =
                    if attempt.response_headers.is_some() || attempt.status_code.is_some() {
                        StageKind::UpstreamResponse
                    } else {
                        StageKind::UpstreamFailure
                    };
                let mut response = StageData::new(response_kind, attempt.wall_time_ns);
                response.attempt_number = Some(attempt.attempt_number);
                response.response_headers_ref = response_headers;
                response.status_code = attempt.status_code;
                response.error_class = attempt
                    .error_class
                    .as_deref()
                    .map(str::as_bytes)
                    .map(Vec::from);
                response.error_message = attempt
                    .error_message
                    .as_deref()
                    .map(str::as_bytes)
                    .map(Vec::from);
                if index == last_attempt {
                    response.response_body_manifest_ref =
                        parse_optional_manifest(bodies.upstream_response_manifest_id.as_deref())?;
                    response.stream_index_ref = stream_index_ref;
                }
                stages.push(writer.append_stage_with_external_manifests(
                    Stage::new(response),
                    &external_manifests,
                )?);
            }

            let response_time = capture.wall_time_ns.max(
                capture
                    .upstream_attempts
                    .last()
                    .map(|attempt| attempt.wall_time_ns)
                    .unwrap_or(capture.wall_time_ns),
            );
            let mut client_response = StageData::new(StageKind::ClientResponse, response_time);
            client_response.response_headers_ref = client_response_headers;
            client_response.status_code = capture.status_code;
            client_response.error_class = capture
                .error_class
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            client_response.error_message = capture
                .error_message
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            client_response.response_body_manifest_ref =
                parse_optional_manifest(bodies.client_response_manifest_id.as_deref())?;
            stages.push(writer.append_stage_with_external_manifests(
                Stage::new(client_response),
                &external_manifests,
            )?);

            let mut exchange_data = ExchangeData::new(
                capture.trace_id.as_bytes(),
                capture_sequence,
                capture.wall_time_ns,
                stages.clone(),
            );
            exchange_data.session_id = capture
                .session_id
                .as_deref()
                .map(str::as_bytes)
                .map(Vec::from);
            exchange_data.run_id = capture.run_id.as_deref().map(str::as_bytes).map(Vec::from);
            let exchange_id = writer.append_exchange(Exchange::new(exchange_data))?;
            let file_uuid = active.file_uuid.clone();
            let flush = flush_active_pack(active, &checkpoint_config)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            catalog_capture_headers(&tx, &file_uuid, capture, now_ms())?;
            catalog_capture_stages(
                &tx,
                &file_uuid,
                &capture.trace_id,
                &stages,
                &active.writer,
                now_ms(),
            )?;
            if let Some(session_id) = capture.session_id.as_deref() {
                tx.execute(
                    "INSERT INTO lar_session_revisions (session_id, revision, updated_at_ms)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET
                   revision=MAX(lar_session_revisions.revision, excluded.revision),
                   updated_at_ms=excluded.updated_at_ms",
                    params![session_id, capture_sequence, now_ms()],
                )?;
            }
            tx.execute(
                "UPDATE lar_files SET size_bytes=?2 WHERE file_uuid=?1",
                params![file_uuid, flush.size],
            )?;
            if let Some(checkpoint) = flush.checkpoint {
                publish_checkpoint(&tx, &file_uuid, checkpoint)?;
            }
            tx.commit()?;
            Ok(Some(exchange_id.to_string()))
        })();
        if result.is_err() {
            state.active = None;
            state.packs_reconciled = false;
        }
        result
    }

    pub(crate) fn write_body_through_configured_store(
        &self,
        owner_id: &str,
        legacy_kind: &str,
        bytes: &[u8],
    ) -> Result<String> {
        let Some(artifact) = artifact_for_legacy_kind(owner_id, legacy_kind) else {
            let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
            return self.write_legacy_body_dated(&date, owner_id, legacy_kind, bytes);
        };
        Ok(self
            .write_body_artifact(&artifact, legacy_kind, bytes)?
            .legacy_path)
    }

    fn write_lar_body_locked(
        &self,
        state: &mut MutexGuard<'_, LiveLarCoordinator>,
        artifact: &LarBodyArtifact,
        legacy_path: &str,
        bytes: &[u8],
    ) -> Result<ManifestId> {
        if bytes.len() as u64 > state.config.limits.max_body_length {
            bail!("body exceeds configured LAR limit");
        }
        let chunks = split_body(bytes, state.config.chunker)?;
        let mut conn = self.conn.lock().unwrap();
        let whole_hash = ChunkHash::blake3(bytes);
        let existing_manifest: Option<String> = conn
            .query_row(
                "SELECT manifest_id FROM lar_manifests
                 WHERE hash_algorithm='blake3' AND whole_body_hash=?1
                   AND total_length=?2 AND state='ready'",
                params![whole_hash.digest.as_slice(), bytes.len() as u64],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing_manifest) = existing_manifest {
            let id: ManifestId = existing_manifest.parse().map_err(anyhow::Error::new)?;
            let manifest = load_catalog_manifest(&conn, id)?
                .context("catalog body identity points to a missing manifest")?;
            let (reconstructed, pack_opens) = reconstruct_manifest_counted(
                &conn,
                &self.data_dir,
                &manifest,
                &state.config.limits,
            )?;
            state.source_pack_opens = state.source_pack_opens.saturating_add(pack_opens);
            if reconstructed != bytes {
                bail!("catalog body identity reconstructed to different bytes");
            }
            if state.config.mode == LarBodyStoreMode::LarWithFallback {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let published_at_ms = now_ms();
                publish_artifact(&tx, artifact, legacy_path, &id, published_at_ms)?;
                crate::lar_fts::index_artifact_bytes(
                    &tx,
                    artifact,
                    &id.to_string(),
                    bytes,
                    published_at_ms,
                )?;
                tx.commit()?;
            }
            return Ok(id);
        }

        let mut locations = HashMap::<ChunkHash, CatalogChunk>::new();
        let mut missing = Vec::<(ChunkHash, usize)>::new();
        let mut missing_hashes = HashSet::new();
        for (index, chunk) in chunks.iter().enumerate() {
            let hash = ChunkHash::blake3(chunk);
            if locations.contains_key(&hash) || !missing_hashes.insert(hash) {
                continue;
            }
            if let Some(existing) = catalog_chunk(&conn, &self.data_dir, &hash)? {
                locations.insert(hash, existing);
            } else {
                missing.push((hash, index));
            }
        }

        let mut appended = Vec::new();
        let mut active_size = None;
        if !missing.is_empty() {
            let injected_failure = state.fail_next_append.take();
            if injected_failure == Some(InjectedAppendFailure::Before) {
                return Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
                    .context("injected storage-full failure before LAR append");
            }
            let projected = missing
                .iter()
                .map(|(_, index)| chunks[*index].len() as u64)
                .sum();
            ensure_active_pack(self, state, &mut conn, projected)?;
            let checkpoint_config = state.config.clone();
            let active = state.active.as_mut().expect("active pack was established");
            if injected_failure == Some(InjectedAppendFailure::During) {
                // Model an ENOSPC-shortened record at the real file boundary.
                // The next attempt must prove and truncate the valid prefix,
                // never append after this incomplete frame.
                active.writer.get_mut().write_all(b"LREC\x01\0\x01\0\0")?;
                active.writer.get_ref().sync_all()?;
                return Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
                    .context("injected storage-full failure during LAR append");
            }
            let file_uuid = active.file_uuid.clone();
            let path: String = conn.query_row(
                "SELECT path FROM lar_files WHERE file_uuid=?1",
                [&file_uuid],
                |row| row.get(0),
            )?;
            let path = PathBuf::from(path);
            for (hash, index) in missing {
                let descriptor = active
                    .writer
                    .append_chunk_record(&chunks[index])
                    .map_err(anyhow::Error::new)?;
                if descriptor.hash != hash {
                    bail!("live LAR writer returned a mismatched chunk hash");
                }
                locations.insert(
                    hash,
                    CatalogChunk {
                        path: path.clone(),
                        descriptor,
                    },
                );
                appended.push((file_uuid.clone(), descriptor));
            }
            active_size = Some((file_uuid, flush_active_pack(active, &checkpoint_config)?));
        }

        let mut references = Vec::with_capacity(chunks.len());
        let mut logical_offset = 0u64;
        for chunk in &chunks {
            let hash = ChunkHash::blake3(chunk);
            let descriptor = locations
                .get(&hash)
                .context("planned LAR chunk location disappeared")?
                .descriptor;
            references.push(ChunkRef {
                chunk_hash: descriptor.hash,
                chunk_offset: 0,
                logical_offset,
                length: chunk.len() as u64,
            });
            logical_offset += chunk.len() as u64;
        }
        let manifest = BodyManifest::new(bytes.len() as u64, whole_hash, None, None, references);

        // Validate from the synced archive bytes before starting the immediate
        // SQLite publication transaction. One reader is shared by every chunk
        // in the same source pack.
        let (reconstructed, pack_opens) =
            reconstruct_manifest_from_locations(&manifest, &locations, &state.config.limits)?;
        state.source_pack_opens = state.source_pack_opens.saturating_add(pack_opens);
        if reconstructed != bytes {
            bail!("catalog reconstruction did not match captured body");
        }

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (file_uuid, descriptor) in &appended {
            insert_catalog_chunk(&tx, file_uuid, *descriptor, now_ms())?;
        }
        insert_catalog_manifest(&tx, &manifest, now_ms())?;
        if let Some((file_uuid, flush)) = active_size {
            tx.execute(
                "UPDATE lar_files SET size_bytes=?2 WHERE file_uuid=?1",
                params![file_uuid, flush.size],
            )?;
            if let Some(checkpoint) = flush.checkpoint {
                publish_checkpoint(&tx, &file_uuid, checkpoint)?;
            }
        }
        if state.fail_next_catalog_commit {
            state.fail_next_catalog_commit = false;
            bail!("injected catalog commit failure");
        }
        if state.config.mode == LarBodyStoreMode::LarWithFallback {
            let published_at_ms = now_ms();
            publish_artifact(&tx, artifact, legacy_path, &manifest.id, published_at_ms)?;
            crate::lar_fts::index_artifact_bytes(
                &tx,
                artifact,
                &manifest.id.to_string(),
                bytes,
                published_at_ms,
            )?;
        }
        tx.commit()?;
        Ok(manifest.id)
    }

    pub(crate) fn read_catalog_manifest_body(&self, manifest_id: &str) -> Result<Option<Vec<u8>>> {
        let id: ManifestId = manifest_id.parse().map_err(anyhow::Error::new)?;
        let conn = self.conn.lock().unwrap();
        let physical_manifest_file: Option<Option<String>> = conn
            .query_row(
                "SELECT file_uuid FROM lar_manifests WHERE manifest_id=?1",
                [manifest_id],
                |row| row.get(0),
            )
            .optional()?;
        if physical_manifest_file.flatten().is_some() {
            return Ok(None);
        }
        load_catalog_manifest(&conn, id)?
            .map(|manifest| {
                reconstruct_manifest(&conn, &self.data_dir, &manifest, &Limits::default())
            })
            .transpose()
    }

    pub(crate) fn read_catalog_manifest_ranges(
        &self,
        manifest_id: &str,
        ranges: &[(u64, u64)],
    ) -> Result<Option<Vec<Vec<u8>>>> {
        let id: ManifestId = manifest_id.parse().map_err(anyhow::Error::new)?;
        let conn = self.conn.lock().unwrap();
        let physical_manifest_file: Option<Option<String>> = conn
            .query_row(
                "SELECT file_uuid FROM lar_manifests WHERE manifest_id=?1",
                [manifest_id],
                |row| row.get(0),
            )
            .optional()?;
        if physical_manifest_file.flatten().is_some() {
            return Ok(None);
        }
        load_catalog_manifest(&conn, id)?
            .map(|manifest| {
                reconstruct_manifest_ranges(
                    &conn,
                    &self.data_dir,
                    &manifest,
                    ranges,
                    &Limits::default(),
                )
            })
            .transpose()
    }

    /// Publish chunk locations and manifest-to-chunk edges for manifests in a
    /// fully synced importer archive. This makes imported chunks immediately
    /// eligible for live cross-pack deduplication without copying them.
    pub(crate) fn catalog_synced_archive_manifests<R: Read + Seek>(
        &self,
        file_uuid: &str,
        reader: &ArchiveReader<R>,
        manifest_ids: &[ManifestId],
    ) -> Result<()> {
        if manifest_ids.is_empty() {
            return Ok(());
        }
        let descriptors: HashMap<ChunkHash, ChunkRecordDescriptor> = reader
            .chunk_records()
            .map(|descriptor| (descriptor.hash, descriptor))
            .collect();
        let manifests = manifest_ids
            .iter()
            .map(|id| {
                reader
                    .manifest(id)
                    .cloned()
                    .with_context(|| format!("synced archive is missing manifest {id}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let created = now_ms();
        for manifest in &manifests {
            for reference in &manifest.chunks {
                let descriptor = descriptors.get(&reference.chunk_hash).with_context(|| {
                    format!(
                        "synced archive manifest {} references a missing chunk",
                        manifest.id
                    )
                })?;
                insert_catalog_chunk(&tx, file_uuid, *descriptor, created)?;
            }
            insert_archive_catalog_manifest(&tx, file_uuid, manifest, created)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn recover_lar_body_store_orphans(&self) -> Result<u64> {
        let mut state = self
            .live_lar
            .lock()
            .map_err(|error| anyhow::anyhow!("LAR writer lock poisoned: {error}"))?;
        state.recovery_scans = state.recovery_scans.saturating_add(1);
        let mut conn = self.conn.lock().unwrap();
        reconcile_live_pack_files(self, &mut conn)?;
        state.packs_reconciled = true;
        if let Some(active) = state.active.as_mut() {
            active.next_checkpoint_sequence = next_checkpoint_sequence(&conn, &active.file_uuid)?;
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before: i64 = tx.query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row.get(0))?;
        recover_orphan_chunks(&tx, &Limits::default())?;
        let after: i64 = tx.query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row.get(0))?;
        tx.commit()?;
        Ok((after - before).max(0) as u64)
    }

    /// Move completed Node-preload Dario captures through the same typed body
    /// store as ordinary proxy traffic. Partially written/corrupt spool files
    /// are retained for a later retry.
    pub fn ingest_dario_capture_spool(&self, trace_id: &str) -> Result<u64> {
        if trace_id.is_empty()
            || trace_id.len() > 128
            || !trace_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            bail!("invalid Dario capture trace ID");
        }
        let root = self.data_dir.join("dario-capture-spool");
        let mut days: Vec<PathBuf> = match std::fs::read_dir(&root) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    (entry.path().is_dir() && crate::date_dir_name(&name)).then(|| entry.path())
                })
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        days.sort();
        let mut ingested = 0u64;
        for (capture_kind, artifact_kind) in [
            ("dario-upstream-request", "dario_upstream_request"),
            ("dario-upstream-response", "dario_upstream_response"),
        ] {
            for day in &days {
                let path = day.join(format!("{trace_id}.{capture_kind}.json.gz"));
                if !path.is_file() {
                    continue;
                }
                let mut decoder = GzDecoder::new(
                    File::open(&path)
                        .with_context(|| format!("opening Dario spool at {}", path.display()))?,
                );
                let mut bytes = Vec::new();
                if let Err(error) = decoder.read_to_end(&mut bytes) {
                    tracing::warn!(
                        path = %path.display(),
                        "Dario spool is incomplete or corrupt; retaining for retry: {error}"
                    );
                    continue;
                }
                let result = self.write_body_artifact(
                    &LarBodyArtifact::trace(trace_id, artifact_kind),
                    &format!("{capture_kind}.json"),
                    &bytes,
                )?;
                if let Some(error) = result.lar_error {
                    tracing::warn!(
                        trace_id,
                        capture_kind,
                        "Dario capture retained as legacy fallback after LAR error: {error}"
                    );
                }
                std::fs::remove_file(&path)
                    .with_context(|| format!("removing ingested Dario spool {}", path.display()))?;
                ingested += 1;
            }
        }
        for day in days {
            if std::fs::read_dir(&day)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false)
            {
                let _ = std::fs::remove_dir(&day);
            }
        }
        Ok(ingested)
    }

    /// Startup recovery for completed captures left in the Dario spool by a
    /// daemon crash. Individual corrupt/partial files remain for later retry.
    pub fn ingest_pending_dario_captures(&self) -> Result<u64> {
        let root = self.data_dir.join("dario-capture-spool");
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        let suffixes = [
            ".dario-upstream-request.json.gz",
            ".dario-upstream-response.json.gz",
        ];
        let mut trace_ids = BTreeSet::new();
        for day in entries.filter_map(|entry| entry.ok()) {
            if !day.path().is_dir() {
                continue;
            }
            for file in std::fs::read_dir(day.path())?.filter_map(|entry| entry.ok()) {
                let name = file.file_name().to_string_lossy().into_owned();
                if let Some(trace_id) = suffixes.iter().find_map(|suffix| name.strip_suffix(suffix))
                {
                    trace_ids.insert(trace_id.to_string());
                }
            }
        }
        let mut ingested = 0u64;
        for trace_id in trace_ids {
            if self.get_trace(&trace_id)?.is_none() {
                tracing::debug!(
                    trace_id,
                    "retaining Dario spool until its owning trace is present"
                );
                continue;
            }
            ingested += self.ingest_dario_capture_spool(&trace_id)?;
        }
        Ok(ingested)
    }

    #[doc(hidden)]
    pub fn inject_lar_catalog_commit_failure_once(&self) {
        if let Ok(mut state) = self.live_lar.lock() {
            state.fail_next_catalog_commit = true;
        }
    }

    #[doc(hidden)]
    pub fn inject_lar_disk_full_before_append_once(&self) {
        if let Ok(mut state) = self.live_lar.lock() {
            state.fail_next_append = Some(InjectedAppendFailure::Before);
        }
    }

    #[doc(hidden)]
    pub fn inject_lar_disk_full_during_append_once(&self) {
        if let Ok(mut state) = self.live_lar.lock() {
            state.fail_next_append = Some(InjectedAppendFailure::During);
        }
    }
}

fn parse_optional_manifest(value: Option<&str>) -> Result<Option<ManifestId>> {
    value
        .map(|value| value.parse::<ManifestId>().map_err(anyhow::Error::new))
        .transpose()
}

fn body_manifest_ids(bodies: &LarExchangeBodyRefs) -> Result<Vec<ManifestId>> {
    let mut ids = Vec::new();
    for value in [
        bodies.client_request_manifest_id.as_deref(),
        bodies.upstream_request_manifest_id.as_deref(),
        bodies.upstream_response_manifest_id.as_deref(),
        bodies.client_response_manifest_id.as_deref(),
    ] {
        if let Some(id) = parse_optional_manifest(value)? {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}

fn append_capture_header(
    writer: &mut ArchiveWriter<File>,
    capture: Option<&LarHeaderCapture>,
) -> Result<Option<alex_lar::HeaderBlockId>> {
    capture
        .filter(|capture| !capture.is_empty())
        .map(|capture| {
            writer
                .append_header_block(HeaderBlock::new(capture.fidelity, capture.atoms.clone()))
                .map_err(anyhow::Error::new)
        })
        .transpose()
}

fn all_capture_headers(capture: &LarExchangeCapture) -> Vec<&LarHeaderCapture> {
    let mut headers = Vec::new();
    if let Some(value) = capture.client_request_headers.as_ref() {
        headers.push(value);
    }
    for attempt in &capture.upstream_attempts {
        if let Some(value) = attempt.request_headers.as_ref() {
            headers.push(value);
        }
        if let Some(value) = attempt.response_headers.as_ref() {
            headers.push(value);
        }
    }
    if let Some(value) = capture.client_response_headers.as_ref() {
        headers.push(value);
    }
    headers
}

fn header_atom_id(atom: &HeaderAtom) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(&(atom.original_name.len() as u64).to_le_bytes());
    hash.update(&atom.original_name);
    hash.update(&(atom.value.len() as u64).to_le_bytes());
    hash.update(&atom.value);
    hash.update(&atom.flags.to_le_bytes());
    hash.finalize().to_hex().to_string()
}

fn catalog_capture_headers(
    conn: &Connection,
    file_uuid: &str,
    capture: &LarExchangeCapture,
    created_at_ms: i64,
) -> Result<()> {
    for captured in all_capture_headers(capture) {
        if captured.is_empty() {
            continue;
        }
        let block = HeaderBlock::new(captured.fidelity, captured.atoms.clone());
        let fidelity = match captured.fidelity {
            HeaderFidelity::Exact | HeaderFidelity::LegacyCasingUnknown => "observed_ordered",
            HeaderFidelity::LegacyOrderUnknown | HeaderFidelity::LegacyOrderAndCasingUnknown => {
                "legacy_normalized"
            }
        };
        conn.execute(
            "INSERT INTO lar_header_blocks
               (block_id, fidelity, atom_count, file_uuid, record_id, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?1, ?5)
             ON CONFLICT(block_id) DO NOTHING",
            params![
                block.id.to_string(),
                fidelity,
                block.atoms.len() as u64,
                file_uuid,
                created_at_ms
            ],
        )?;
        for (ordinal, atom) in block.atoms.iter().enumerate() {
            let atom_id = header_atom_id(atom);
            conn.execute(
                "INSERT INTO lar_header_atoms
                   (atom_id, original_name_bytes, value_bytes, flags, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(atom_id) DO NOTHING",
                params![
                    atom_id,
                    atom.original_name,
                    atom.value,
                    atom.flags,
                    created_at_ms
                ],
            )?;
            conn.execute(
                "INSERT INTO lar_header_block_atoms (block_id, ordinal, atom_id)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(block_id, ordinal) DO NOTHING",
                params![block.id.to_string(), ordinal as u64, atom_id],
            )?;
        }
    }
    Ok(())
}

fn stage_kind_name(kind: StageKind) -> &'static str {
    match kind {
        StageKind::ClientRequest => "client_request",
        StageKind::NormalizedRequest => "normalized_request",
        StageKind::RouterDecision => "router_decision",
        StageKind::RetryDecision => "retry_decision",
        StageKind::FailoverDecision => "failover_decision",
        StageKind::UpstreamRequest => "upstream_request",
        StageKind::UpstreamResponse => "upstream_response",
        StageKind::UpstreamFailure => "upstream_failure",
        StageKind::ClientResponse => "client_response",
        StageKind::ClientTrailers => "client_trailers",
        StageKind::ToolCall => "tool_call",
        StageKind::ToolResult => "tool_result",
        StageKind::AuthRefresh => "auth_refresh",
        StageKind::AccountRouting => "account_routing",
        StageKind::DarioRequest => "dario_request",
        StageKind::DarioResponse => "dario_response",
        StageKind::InjectedResponse => "injected_response",
        StageKind::Cancellation => "cancellation",
        StageKind::Unknown(_) => "unknown",
    }
}

fn catalog_capture_stages(
    conn: &Connection,
    file_uuid: &str,
    trace_id: &str,
    stage_ids: &[alex_lar::StageId],
    writer: &ArchiveWriter<File>,
    created_at_ms: i64,
) -> Result<()> {
    for (sequence, stage_id) in stage_ids.iter().enumerate() {
        let stage = writer
            .stage(stage_id)
            .with_context(|| format!("live LAR writer lost stage {stage_id}"))?;
        let data = &stage.data;
        conn.execute(
            "INSERT INTO lar_stage_records
               (stage_id, trace_id, capture_sequence, kind, attempt_number,
                wall_time_ns, monotonic_delta_ns, request_headers_ref,
                request_body_manifest_ref, response_headers_ref,
                response_body_manifest_ref, trailers_ref, stream_index_ref,
                file_uuid, record_id, fidelity)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?1, 'captured')
             ON CONFLICT(stage_id) DO NOTHING",
            params![
                stage_id.to_string(),
                trace_id,
                sequence as u64,
                stage_kind_name(data.kind),
                data.attempt_number,
                data.wall_time_ns,
                data.monotonic_delta_ns,
                data.request_headers_ref.map(|id| id.to_string()),
                data.request_body_manifest_ref.map(|id| id.to_string()),
                data.response_headers_ref.map(|id| id.to_string()),
                data.response_body_manifest_ref.map(|id| id.to_string()),
                data.trailers_ref.map(|id| id.to_string()),
                data.stream_index_ref.map(|id| id.to_string()),
                file_uuid,
            ],
        )?;
        for manifest_id in [
            data.request_body_manifest_ref,
            data.response_body_manifest_ref,
        ]
        .into_iter()
        .flatten()
        {
            crate::lar_fts::attach_stage_manifest_refs(
                conn,
                trace_id,
                &stage_id.to_string(),
                &manifest_id.to_string(),
                created_at_ms,
            )?;
        }
    }
    Ok(())
}

fn ensure_active_pack(
    store: &Store,
    state: &mut LiveLarCoordinator,
    conn: &mut Connection,
    projected_bytes: u64,
) -> Result<()> {
    let current_ms = now_ms();
    if let Some(active) = state.active.as_mut() {
        let size = active.writer.get_mut().seek(std::io::SeekFrom::End(0))?;
        let age = current_ms.saturating_sub(active.created_at_ms) as u64;
        if size.saturating_add(projected_bytes) <= state.config.max_pack_bytes
            && age <= state.config.max_pack_age.as_millis() as u64
        {
            return Ok(());
        }
        active.writer.seal().map_err(anyhow::Error::new)?;
        active.writer.get_ref().sync_all()?;
        let size = active.writer.get_mut().seek(std::io::SeekFrom::End(0))?;
        conn.execute(
            "UPDATE lar_files SET state='sealed', sealed_at_ms=?2, size_bytes=?3
             WHERE file_uuid=?1 AND state='active'",
            params![active.file_uuid, current_ms, size],
        )?;
        catalog_sealed_file_identity(store, conn, &active.file_uuid, "live_rotation")?;
        state.active = None;
    }

    if !state.packs_reconciled {
        reconcile_live_pack_files(store, conn)?;
        state.packs_reconciled = true;
    }
    let candidate: Option<(String, String, i64)> = conn
        .query_row(
            "SELECT file_uuid, path, created_at_ms FROM lar_files
             WHERE archive_set_uuid=?1 AND role='body-pack' AND state='active'
             ORDER BY created_at_ms DESC LIMIT 1",
            [archive_set_id(&store.data_dir)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((file_uuid, path, created_at_ms)) = candidate {
        let path = PathBuf::from(path);
        let size = std::fs::metadata(&path)
            .map(|value| value.len())
            .unwrap_or(u64::MAX);
        let age = current_ms.saturating_sub(created_at_ms) as u64;
        if size.saturating_add(projected_bytes) <= state.config.max_pack_bytes
            && age <= state.config.max_pack_age.as_millis() as u64
        {
            let file = OpenOptions::new().read(true).write(true).open(&path)?;
            match ArchiveWriter::open_append(
                file,
                state.config.chunker,
                state.config.limits.clone(),
            ) {
                Ok(mut writer) => {
                    writer.enable_metadata_pages();
                    state.active = Some(ActivePack {
                        next_checkpoint_sequence: next_checkpoint_sequence(conn, &file_uuid)?,
                        file_uuid,
                        created_at_ms,
                        writer,
                        last_checkpoint_size: size,
                        last_checkpoint_at_ms: current_ms,
                    });
                    return Ok(());
                }
                Err(error) => {
                    tracing::warn!("cannot append active LAR pack; marking repairing: {error}");
                    conn.execute(
                        "UPDATE lar_files SET state='repairing' WHERE file_uuid=?1",
                        [file_uuid],
                    )?;
                }
            }
        } else {
            match seal_pack_path(&path, &state.config) {
                Ok(sealed_size) => {
                    conn.execute(
                        "UPDATE lar_files SET state='sealed', sealed_at_ms=?2, size_bytes=?3
                         WHERE file_uuid=?1",
                        params![file_uuid, current_ms, sealed_size],
                    )?;
                    catalog_sealed_file_identity(store, conn, &file_uuid, "live_reconcile_seal")?;
                }
                Err(error) => {
                    tracing::warn!("cannot seal rotated LAR pack; marking repairing: {error:#}");
                    conn.execute(
                        "UPDATE lar_files SET state='repairing' WHERE file_uuid=?1",
                        [file_uuid],
                    )?;
                }
            }
        }
    }

    state.active = Some(create_pack(store, conn, &state.config)?);
    Ok(())
}

fn seal_pack_path(path: &Path, config: &LarBodyStoreConfig) -> Result<u64> {
    let reader = ArchiveReader::open(File::open(path)?, config.limits.clone())
        .map_err(anyhow::Error::new)?;
    if reader.is_sealed() {
        return Ok(std::fs::metadata(path)?.len());
    }
    if !matches!(reader.recovery_status(), RecoveryStatus::Clean) {
        bail!("cannot seal a pack with an interrupted tail");
    }
    drop(reader);
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut writer = ArchiveWriter::open_append(file, config.chunker, config.limits.clone())
        .map_err(anyhow::Error::new)?;
    writer.seal().map_err(anyhow::Error::new)?;
    writer.get_ref().sync_all()?;
    Ok(writer.get_mut().seek(std::io::SeekFrom::End(0))?)
}

fn create_pack(
    store: &Store,
    conn: &Connection,
    config: &LarBodyStoreConfig,
) -> Result<ActivePack> {
    let dir = store.data_dir.join("lar").join("live");
    std::fs::create_dir_all(&dir)?;
    let (uuid_bytes, file_uuid) = new_file_id(&store.data_dir);
    let path = dir.join(format!("body-{file_uuid}.lar"));
    let temp = dir.join(format!(".body-{file_uuid}.tmp"));
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&temp)?;
    let mut header = FileHeader::body_pack(uuid_bytes, now_ns(), b"alex-store/live-v1".to_vec());
    header.required_feature_bits |= REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS;
    let mut writer = ArchiveWriter::create(file, header, config.chunker, config.limits.clone())
        .map_err(anyhow::Error::new)?;
    writer.flush().map_err(anyhow::Error::new)?;
    writer.get_ref().sync_all()?;
    std::fs::rename(&temp, &path)?;
    #[cfg(unix)]
    File::open(&dir)?.sync_all()?;

    let created = now_ms();
    let archive_set_uuid = archive_set_id(&store.data_dir);
    conn.execute(
        "INSERT INTO lar_archive_sets
           (archive_set_uuid, created_at_ms, updated_at_ms, state, description)
         VALUES (?1, ?2, ?2, 'active', 'live body packs')
         ON CONFLICT(archive_set_uuid) DO UPDATE SET updated_at_ms=excluded.updated_at_ms",
        params![archive_set_uuid, created],
    )?;
    conn.execute(
        "INSERT INTO lar_files
           (file_uuid, archive_set_uuid, role, path, state, container_major,
            container_minor, created_at_ms, size_bytes)
         VALUES (?1, ?2, 'body-pack', ?3, 'active', 1, 0, ?4, ?5)",
        params![
            file_uuid,
            archive_set_uuid,
            path.to_string_lossy(),
            created,
            std::fs::metadata(&path)?.len()
        ],
    )?;
    let file = OpenOptions::new().read(true).write(true).open(&path)?;
    let mut writer = ArchiveWriter::open_append(file, config.chunker, config.limits.clone())
        .map_err(anyhow::Error::new)?;
    writer.enable_metadata_pages();
    Ok(ActivePack {
        file_uuid,
        created_at_ms: created,
        writer,
        last_checkpoint_size: std::fs::metadata(&path)?.len(),
        last_checkpoint_at_ms: created,
        next_checkpoint_sequence: 1,
    })
}

const CHECKPOINT_LOCATOR_FRAME_LENGTH: u64 = 20 + 56 + 4;

fn frame_length(frame: &RecordFrame) -> Result<u64> {
    24u64
        .checked_add(
            u64::try_from(frame.payload.len()).context("LAR frame payload length overflow")?,
        )
        .context("LAR frame length overflow")
}

fn checkpoint_frame_descriptor(frame: &RecordFrame) -> Result<Option<CheckpointRecordDescriptor>> {
    if frame.record_type != RecordType::Checkpoint || frame.schema_version != 1 || frame.flags != 0
    {
        return Ok(None);
    }
    Ok(Some(CheckpointRecordDescriptor {
        frame_offset: frame.offset,
        frame_length: frame_length(frame)?,
        payload_hash: *blake3::hash(&frame.payload).as_bytes(),
        append_offset: 0,
    }))
}

fn checkpoint_locator_pointer(frame: &RecordFrame) -> Option<CheckpointRecordDescriptor> {
    if frame.record_type != RecordType::CheckpointLocator
        || frame.schema_version != 1
        || frame.flags != 0
        || frame.payload.len() != 56
        || &frame.payload[..4] != b"LCPT"
        || u16::from_le_bytes(frame.payload[4..6].try_into().ok()?) != 1
        || u16::from_le_bytes(frame.payload[6..8].try_into().ok()?) != 0
    {
        return None;
    }
    Some(CheckpointRecordDescriptor {
        frame_offset: u64::from_le_bytes(frame.payload[8..16].try_into().ok()?),
        frame_length: u64::from_le_bytes(frame.payload[16..24].try_into().ok()?),
        payload_hash: frame.payload[24..56].try_into().ok()?,
        append_offset: frame.offset.checked_add(CHECKPOINT_LOCATOR_FRAME_LENGTH)?,
    })
}

fn checkpoint_at_append_offset(
    path: &Path,
    append_offset: u64,
    limits: &Limits,
) -> Result<Option<CheckpointRecordDescriptor>> {
    let file_length = std::fs::metadata(path)?.len();
    if append_offset < CHECKPOINT_LOCATOR_FRAME_LENGTH || append_offset > file_length {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    file.seek(std::io::SeekFrom::Start(
        append_offset - CHECKPOINT_LOCATOR_FRAME_LENGTH,
    ))?;
    let locator = {
        let mut frames = FrameReader::new(&mut file, limits);
        match frames.read_next() {
            Ok((FrameRead::Frame, Some(frame))) => frame,
            _ => return Ok(None),
        }
    };
    let Some(pointer) = checkpoint_locator_pointer(&locator) else {
        return Ok(None);
    };
    if pointer.append_offset != append_offset {
        return Ok(None);
    }
    file.seek(std::io::SeekFrom::Start(pointer.frame_offset))?;
    let checkpoint = {
        let mut frames = FrameReader::new(&mut file, limits);
        match frames.read_next() {
            Ok((FrameRead::Frame, Some(frame))) => frame,
            _ => return Ok(None),
        }
    };
    let Some(mut descriptor) = checkpoint_frame_descriptor(&checkpoint)? else {
        return Ok(None);
    };
    descriptor.append_offset = append_offset;
    if descriptor.frame_offset != pointer.frame_offset
        || descriptor.frame_length != pointer.frame_length
        || descriptor.payload_hash != pointer.payload_hash
    {
        return Ok(None);
    }
    Ok(Some(descriptor))
}

fn scan_checkpoint_frames(
    path: &Path,
    start_offset: Option<u64>,
    limits: &Limits,
) -> Result<Option<CheckpointRecordDescriptor>> {
    let mut file = File::open(path)?;
    if let Some(start_offset) = start_offset {
        file.seek(std::io::SeekFrom::Start(start_offset))?;
    } else {
        read_file_header(&mut file, limits).map_err(anyhow::Error::new)?;
    }
    let mut checkpoints = HashMap::<u64, CheckpointRecordDescriptor>::new();
    let mut latest = None;
    let mut frames = FrameReader::new(&mut file, limits);
    loop {
        let (status, frame) = frames.read_next().map_err(anyhow::Error::new)?;
        match (status, frame) {
            (FrameRead::Frame, Some(frame)) => match frame.record_type {
                RecordType::Checkpoint => {
                    if let Some(descriptor) = checkpoint_frame_descriptor(&frame)? {
                        checkpoints.insert(descriptor.frame_offset, descriptor);
                    }
                }
                RecordType::CheckpointLocator => {
                    if let Some(pointer) = checkpoint_locator_pointer(&frame) {
                        if let Some(checkpoint) = checkpoints.get(&pointer.frame_offset) {
                            if checkpoint.frame_length == pointer.frame_length
                                && checkpoint.payload_hash == pointer.payload_hash
                            {
                                latest = Some(pointer);
                            }
                        }
                    }
                }
                _ => {}
            },
            (FrameRead::CleanEof, None) => break,
            (FrameRead::Truncated, None) => {
                bail!("clean active LAR pack became truncated during checkpoint reconciliation")
            }
            _ => bail!("invalid LAR frame reader state during checkpoint reconciliation"),
        }
    }
    Ok(latest)
}

fn load_latest_catalog_checkpoint(
    conn: &Connection,
    file_uuid: &str,
) -> Result<Option<CheckpointRecordDescriptor>> {
    let row: Option<(Option<i64>, Option<i64>, i64, Vec<u8>)> = conn
        .query_row(
            "SELECT frame_offset, frame_length, append_offset, checksum
             FROM lar_checkpoints WHERE file_uuid=?1
             ORDER BY checkpoint_sequence DESC LIMIT 1",
            [file_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((Some(frame_offset), Some(frame_length), append_offset, checksum)) = row else {
        return Ok(None);
    };
    let Ok(payload_hash) = <Vec<u8> as TryInto<[u8; 32]>>::try_into(checksum) else {
        return Ok(None);
    };
    Ok(Some(CheckpointRecordDescriptor {
        frame_offset: u64::try_from(frame_offset).context("negative checkpoint frame offset")?,
        frame_length: u64::try_from(frame_length).context("negative checkpoint frame length")?,
        payload_hash,
        append_offset: u64::try_from(append_offset).context("negative checkpoint append offset")?,
    }))
}

fn reconcile_checkpoint_catalog(
    conn: &Connection,
    file_uuid: &str,
    path: &Path,
    limits: &Limits,
) -> Result<()> {
    let cataloged = load_latest_catalog_checkpoint(conn, file_uuid)?;
    let verified_cataloged = if let Some(cataloged) = cataloged {
        checkpoint_at_append_offset(path, cataloged.append_offset, limits)?
            .filter(|actual| actual == &cataloged)
    } else {
        None
    };
    let file_length = std::fs::metadata(path)?.len();
    let latest = if let Some(cataloged) = verified_cataloged {
        if cataloged.append_offset == file_length {
            Some(cataloged)
        } else if let Some(at_end) = checkpoint_at_append_offset(path, file_length, limits)? {
            Some(at_end)
        } else {
            scan_checkpoint_frames(path, Some(cataloged.append_offset), limits)?.or(Some(cataloged))
        }
    } else if let Some(at_end) = checkpoint_at_append_offset(path, file_length, limits)? {
        Some(at_end)
    } else {
        scan_checkpoint_frames(path, None, limits)?
    };
    let Some(latest) = latest else {
        return Ok(());
    };
    if cataloged.is_some_and(|cataloged| cataloged == latest) {
        return Ok(());
    }
    publish_checkpoint(
        conn,
        file_uuid,
        PendingCheckpoint {
            sequence: next_checkpoint_sequence(conn, file_uuid)?,
            descriptor: latest,
            created_at_ms: now_ms(),
        },
    )
}

fn reconcile_live_pack_files(store: &Store, conn: &mut Connection) -> Result<()> {
    let dir = store.data_dir.join("lar").join("live");
    if !dir.is_dir() {
        return Ok(());
    }
    let archive_set_uuid = archive_set_id(&store.data_dir);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = now_ms();
    tx.execute(
        "INSERT INTO lar_archive_sets
           (archive_set_uuid, created_at_ms, updated_at_ms, state, description)
         VALUES (?1, ?2, ?2, 'active', 'live body packs')
         ON CONFLICT(archive_set_uuid) DO UPDATE SET updated_at_ms=excluded.updated_at_ms",
        params![archive_set_uuid, current],
    )?;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(file_uuid) = name
            .strip_prefix("body-")
            .and_then(|value| value.strip_suffix(".lar"))
        else {
            continue;
        };
        if file_uuid.len() != 32 || !file_uuid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let path = entry.path();
        let catalog_state: Option<String> = tx
            .query_row(
                "SELECT state FROM lar_files WHERE file_uuid=?1",
                [file_uuid],
                |row| row.get(0),
            )
            .optional()?;
        let mut reader = ArchiveReader::open(File::open(&path)?, Limits::default())
            .map_err(anyhow::Error::new)?;
        if hex(&reader.header().file_uuid) != file_uuid {
            bail!("live LAR pack filename UUID does not match its header");
        }
        let recoverable_tail = if !reader.is_sealed()
            && matches!(
                catalog_state.as_deref(),
                None | Some("active") | Some("repairing")
            ) {
            match reader.recovery_status() {
                RecoveryStatus::TruncatedTail {
                    last_valid_offset, ..
                }
                | RecoveryStatus::CorruptIndexFallback {
                    last_valid_offset, ..
                } => Some(last_valid_offset),
                RecoveryStatus::Clean => None,
            }
        } else {
            None
        };
        if let Some(last_valid_offset) = recoverable_tail {
            drop(reader);
            let file = OpenOptions::new().write(true).open(&path)?;
            file.set_len(last_valid_offset)?;
            file.sync_all()?;
            drop(file);
            reader = ArchiveReader::open(File::open(&path)?, Limits::default())
                .map_err(anyhow::Error::new)?;
            if reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
                bail!("interrupted active LAR pack did not recover at its valid boundary");
            }
            tracing::warn!(
                path = %path.display(),
                last_valid_offset,
                "recovered interrupted active LAR pack tail"
            );
        }
        let state = if reader.is_sealed() {
            "sealed"
        } else if matches!(reader.recovery_status(), RecoveryStatus::Clean) {
            "active"
        } else {
            "repairing"
        };
        tx.execute(
            "INSERT INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, created_at_ms, size_bytes)
             VALUES (?1, ?2, 'body-pack', ?3, ?4, 1, 0, ?5, ?6)
             ON CONFLICT(file_uuid) DO UPDATE SET
               path=excluded.path,
               size_bytes=excluded.size_bytes,
               state=CASE WHEN excluded.state IN ('sealed','repairing')
                          THEN excluded.state ELSE lar_files.state END",
            params![
                file_uuid,
                archive_set_uuid,
                path.to_string_lossy(),
                state,
                current,
                std::fs::metadata(&path)?.len()
            ],
        )?;
        drop(reader);
        if state == "sealed" {
            catalog_sealed_file_identity(store, &tx, file_uuid, "live_startup_reconcile")?;
        } else if state == "active" {
            reconcile_checkpoint_catalog(&tx, file_uuid, &path, &Limits::default())?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn catalog_sealed_file_identity(
    store: &Store,
    conn: &Connection,
    file_uuid: &str,
    source: &str,
) -> Result<()> {
    let catalog_path: String = conn.query_row(
        "SELECT path FROM lar_files WHERE file_uuid=?1",
        [file_uuid],
        |row| row.get(0),
    )?;
    let path = resolved_catalog_path(&store.data_dir, &catalog_path);
    let reader =
        ArchiveReader::open(File::open(&path)?, Limits::default()).map_err(anyhow::Error::new)?;
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        bail!("cannot publish identity for an unsealed or interrupted LAR pack");
    }
    if hex(&reader.header().file_uuid) != file_uuid {
        bail!("sealed LAR pack UUID does not match its catalog row");
    }
    conn.execute(
        "UPDATE lar_files SET container_major=?2, container_minor=?3,
           required_feature_bits=?4, optional_feature_bits=?5
         WHERE file_uuid=?1",
        params![
            file_uuid,
            reader.header().container_major,
            reader.header().container_minor,
            reader.header().required_feature_bits,
            reader.header().optional_feature_bits,
        ],
    )?;
    drop(reader);
    let identity = compute_lar_file_identity(&path)?;
    record_lar_file_identity(conn, file_uuid, &identity, source, now_ms())
}

fn recover_orphan_chunks(conn: &Connection, limits: &Limits) -> Result<()> {
    let mut statement = conn.prepare(
        "SELECT file_uuid, path FROM lar_files
         WHERE role='body-pack' AND state IN ('active','sealed','repairing')
         ORDER BY created_at_ms, file_uuid",
    )?;
    let files = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (file_uuid, path) in files {
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!("cannot scan LAR body pack {path}: {error}");
                continue;
            }
        };
        let reader = ArchiveReader::open(file, limits.clone()).map_err(anyhow::Error::new)?;
        for descriptor in reader.chunk_records() {
            insert_catalog_chunk(conn, &file_uuid, descriptor, now_ms())?;
        }
    }
    Ok(())
}

fn insert_catalog_chunk(
    conn: &Connection,
    file_uuid: &str,
    descriptor: ChunkRecordDescriptor,
    created_at_ms: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO lar_chunks
           (hash_algorithm, chunk_hash, uncompressed_length, compression,
            compressed_length, file_uuid, record_id, page_offset, record_offset,
            checksum, created_at_ms, state)
         VALUES ('blake3', ?1, ?2, 'zstd', ?3, ?4, ?5, ?6, ?6, ?1, ?7, 'ready')
         ON CONFLICT(hash_algorithm, chunk_hash) DO NOTHING",
        params![
            descriptor.hash.digest.as_slice(),
            descriptor.uncompressed_length,
            descriptor.compressed_length,
            file_uuid,
            format!("chunk:{}", hex(&descriptor.hash.digest)),
            descriptor.frame_offset,
            created_at_ms,
        ],
    )?;
    let stored: i64 = conn.query_row(
        "SELECT uncompressed_length FROM lar_chunks
         WHERE hash_algorithm='blake3' AND chunk_hash=?1",
        [descriptor.hash.digest.as_slice()],
        |row| row.get(0),
    )?;
    if stored != descriptor.uncompressed_length as i64 {
        bail!("catalog chunk hash is bound to a different length");
    }
    Ok(())
}

fn catalog_chunk(
    conn: &Connection,
    data_dir: &Path,
    hash: &ChunkHash,
) -> Result<Option<CatalogChunk>> {
    let value = conn
        .query_row(
            "SELECT f.path, c.page_offset, c.uncompressed_length, c.compressed_length,
                f.file_uuid, f.state
         FROM lar_chunks c JOIN lar_files f ON f.file_uuid=c.file_uuid
         WHERE c.hash_algorithm='blake3' AND c.chunk_hash=?1 AND c.state='ready'",
            [hash.digest.as_slice()],
            |row| {
                let frame_offset: i64 = row.get(1)?;
                let uncompressed_length: i64 = row.get(2)?;
                let compressed_length: i64 = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    ChunkRecordDescriptor {
                        hash: *hash,
                        frame_offset: nonnegative(frame_offset, 1)?,
                        uncompressed_length: nonnegative(uncompressed_length, 2)?,
                        compressed_length: nonnegative(compressed_length, 3)?,
                    },
                ))
            },
        )
        .optional()?;
    let Some((path, file_uuid, state, descriptor)) = value else {
        return Ok(None);
    };
    let path = resolved_catalog_path(data_dir, &path);
    if !matches!(state.as_str(), "active" | "sealed") {
        return Err(LarArchiveUnavailableError::offline(file_uuid, path.to_string_lossy()).into());
    }
    if !path.exists() {
        return Err(LarArchiveUnavailableError::missing(file_uuid, path.to_string_lossy()).into());
    }
    Ok(Some(CatalogChunk { path, descriptor }))
}

fn nonnegative(value: i64, column: usize) -> rusqlite::Result<u64> {
    value.try_into().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn insert_catalog_manifest(conn: &Connection, manifest: &BodyManifest, created: i64) -> Result<()> {
    let manifest_id = manifest.id.to_string();
    conn.execute(
        "INSERT INTO lar_manifests
           (manifest_id, total_length, hash_algorithm, whole_body_hash, created_at_ms, state)
         VALUES (?1, ?2, 'blake3', ?3, ?4, 'ready')
         ON CONFLICT(manifest_id) DO NOTHING",
        params![
            manifest_id,
            manifest.total_length,
            manifest.whole_body_hash.digest.as_slice(),
            created
        ],
    )?;
    let stored: (i64, Vec<u8>) = conn.query_row(
        "SELECT total_length, whole_body_hash FROM lar_manifests WHERE manifest_id=?1",
        [manifest_id.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored.0 != manifest.total_length as i64
        || stored.1.as_slice() != manifest.whole_body_hash.digest
    {
        bail!("manifest ID is already bound to different body content");
    }
    conn.execute(
        "DELETE FROM lar_manifest_chunks WHERE manifest_id=?1",
        [manifest_id.as_str()],
    )?;
    for (ordinal, reference) in manifest.chunks.iter().enumerate() {
        conn.execute(
            "INSERT INTO lar_manifest_chunks
               (manifest_id, ordinal, hash_algorithm, chunk_hash, logical_offset,
                chunk_offset, length)
             VALUES (?1, ?2, 'blake3', ?3, ?4, ?5, ?6)",
            params![
                manifest_id,
                ordinal as u64,
                reference.chunk_hash.digest.as_slice(),
                reference.logical_offset,
                reference.chunk_offset,
                reference.length,
            ],
        )?;
    }
    Ok(())
}

fn insert_archive_catalog_manifest(
    conn: &Connection,
    file_uuid: &str,
    manifest: &BodyManifest,
    created: i64,
) -> Result<()> {
    let manifest_id = manifest.id.to_string();
    conn.execute(
        "INSERT INTO lar_manifests
           (manifest_id, total_length, hash_algorithm, whole_body_hash,
            media_type, content_encoding, file_uuid, record_id, created_at_ms, state)
         VALUES (?1, ?2, 'blake3', ?3, ?4, ?5, ?6, ?1, ?7, 'ready')
         ON CONFLICT(manifest_id) DO NOTHING",
        params![
            manifest_id,
            manifest.total_length,
            manifest.whole_body_hash.digest.as_slice(),
            manifest
                .media_type
                .as_deref()
                .map(|value| String::from_utf8_lossy(value).into_owned()),
            manifest
                .content_encoding
                .as_deref()
                .map(|value| String::from_utf8_lossy(value).into_owned()),
            file_uuid,
            created,
        ],
    )?;
    let stored: (i64, String, Vec<u8>) = conn.query_row(
        "SELECT total_length, hash_algorithm, whole_body_hash
         FROM lar_manifests WHERE manifest_id=?1",
        [manifest_id.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if stored.0 != manifest.total_length as i64
        || stored.1 != "blake3"
        || stored.2.as_slice() != manifest.whole_body_hash.digest
    {
        bail!("imported manifest ID is already bound to different content");
    }
    conn.execute(
        "DELETE FROM lar_manifest_chunks WHERE manifest_id=?1",
        [manifest_id.as_str()],
    )?;
    for (ordinal, reference) in manifest.chunks.iter().enumerate() {
        conn.execute(
            "INSERT INTO lar_manifest_chunks
               (manifest_id, ordinal, hash_algorithm, chunk_hash, logical_offset,
                chunk_offset, length)
             VALUES (?1, ?2, 'blake3', ?3, ?4, ?5, ?6)",
            params![
                manifest_id,
                ordinal as u64,
                reference.chunk_hash.digest.as_slice(),
                reference.logical_offset,
                reference.chunk_offset,
                reference.length,
            ],
        )?;
    }
    Ok(())
}

fn load_catalog_manifest(conn: &Connection, id: ManifestId) -> Result<Option<BodyManifest>> {
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT total_length, whole_body_hash FROM lar_manifests
             WHERE manifest_id=?1 AND hash_algorithm='blake3' AND state='ready'",
            [id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((total_length, whole_hash)) = row else {
        return Ok(None);
    };
    let digest: [u8; 32] = whole_hash
        .try_into()
        .map_err(|_| anyhow::anyhow!("catalog manifest has invalid digest length"))?;
    let mut statement = conn.prepare(
        "SELECT chunk_hash, logical_offset, chunk_offset, length
         FROM lar_manifest_chunks WHERE manifest_id=?1 ORDER BY ordinal",
    )?;
    let chunks = statement
        .query_map([id.to_string()], |row| {
            let digest: Vec<u8> = row.get(0)?;
            let digest: [u8; 32] = digest.try_into().map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Blob,
                    "invalid BLAKE3 digest length".into(),
                )
            })?;
            Ok(ChunkRef {
                chunk_hash: ChunkHash {
                    algorithm: alex_lar::HashAlgorithm::Blake3,
                    digest,
                },
                logical_offset: nonnegative(row.get(1)?, 1)?,
                chunk_offset: nonnegative(row.get(2)?, 2)?,
                length: nonnegative(row.get(3)?, 3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let manifest = BodyManifest::new(
        nonnegative(total_length, 0)?,
        ChunkHash {
            algorithm: alex_lar::HashAlgorithm::Blake3,
            digest,
        },
        None,
        None,
        chunks,
    );
    if manifest.id != id {
        bail!("catalog manifest identity does not match its chunk list");
    }
    Ok(Some(manifest))
}

fn reconstruct_manifest(
    conn: &Connection,
    data_dir: &Path,
    manifest: &BodyManifest,
    limits: &Limits,
) -> Result<Vec<u8>> {
    reconstruct_manifest_counted(conn, data_dir, manifest, limits).map(|(body, _)| body)
}

fn reconstruct_manifest_counted(
    conn: &Connection,
    data_dir: &Path,
    manifest: &BodyManifest,
    limits: &Limits,
) -> Result<(Vec<u8>, u64)> {
    let mut locations = HashMap::new();
    for reference in &manifest.chunks {
        if locations.contains_key(&reference.chunk_hash) {
            continue;
        }
        let chunk = catalog_chunk(conn, data_dir, &reference.chunk_hash)?
            .with_context(|| format!("missing catalog chunk {:?}", reference.chunk_hash))?;
        locations.insert(reference.chunk_hash, chunk);
    }
    reconstruct_manifest_from_locations(manifest, &locations, limits)
}

fn reconstruct_manifest_from_locations(
    manifest: &BodyManifest,
    locations: &HashMap<ChunkHash, CatalogChunk>,
    limits: &Limits,
) -> Result<(Vec<u8>, u64)> {
    manifest.validate().map_err(anyhow::Error::new)?;
    let capacity: usize = manifest
        .total_length
        .try_into()
        .context("manifest too large for address space")?;
    let mut result = Vec::with_capacity(capacity);
    let mut readers: HashMap<PathBuf, File> = HashMap::new();
    let mut chunk_bytes: HashMap<ChunkHash, Vec<u8>> = HashMap::new();
    for reference in &manifest.chunks {
        let location = locations
            .get(&reference.chunk_hash)
            .with_context(|| format!("missing planned chunk {:?}", reference.chunk_hash))?;
        if location.descriptor.hash != reference.chunk_hash {
            bail!("planned chunk location has the wrong content hash");
        }
        if !chunk_bytes.contains_key(&reference.chunk_hash) {
            let path = location.path.clone();
            if !readers.contains_key(&path) {
                let file = File::open(&path)
                    .with_context(|| format!("opening LAR pack {}", path.display()))?;
                readers.insert(path.clone(), file);
            }
            let bytes = read_chunk_record_at(
                readers.get_mut(&path).expect("reader was inserted"),
                &location.descriptor,
                limits,
            )
            .map_err(anyhow::Error::new)?;
            chunk_bytes.insert(reference.chunk_hash, bytes);
        }
        let bytes = chunk_bytes
            .get(&reference.chunk_hash)
            .expect("chunk bytes were inserted");
        let start: usize = reference
            .chunk_offset
            .try_into()
            .context("chunk offset exceeds address space")?;
        let end: usize = reference
            .chunk_offset
            .checked_add(reference.length)
            .context("chunk range overflow")?
            .try_into()
            .context("chunk range exceeds address space")?;
        result.extend_from_slice(
            bytes
                .get(start..end)
                .context("manifest range exceeds catalog chunk")?,
        );
    }
    if result.len() as u64 != manifest.total_length
        || ChunkHash::blake3(&result) != manifest.whole_body_hash
    {
        bail!("reconstructed catalog manifest failed length/hash validation");
    }
    Ok((result, readers.len() as u64))
}

fn reconstruct_manifest_ranges(
    conn: &Connection,
    data_dir: &Path,
    manifest: &BodyManifest,
    ranges: &[(u64, u64)],
    limits: &Limits,
) -> Result<Vec<Vec<u8>>> {
    manifest.validate().map_err(anyhow::Error::new)?;
    let mut ends = Vec::with_capacity(ranges.len());
    let mut outputs = Vec::with_capacity(ranges.len());
    for &(offset, length) in ranges {
        let end = offset.checked_add(length).context("body range overflow")?;
        if end > manifest.total_length {
            bail!("body range exceeds manifest");
        }
        let capacity = usize::try_from(length).context("body range exceeds address space")?;
        ends.push(end);
        outputs.push(Vec::with_capacity(capacity));
    }

    let mut locations = HashMap::<ChunkHash, CatalogChunk>::new();
    for reference in &manifest.chunks {
        let reference_end = reference
            .logical_offset
            .checked_add(reference.length)
            .context("manifest range overflow")?;
        let touched = ranges.iter().zip(&ends).any(|(&(start, length), &end)| {
            length != 0 && start.max(reference.logical_offset) < end.min(reference_end)
        });
        if touched && !locations.contains_key(&reference.chunk_hash) {
            let chunk = catalog_chunk(conn, data_dir, &reference.chunk_hash)?
                .with_context(|| format!("missing catalog chunk {:?}", reference.chunk_hash))?;
            locations.insert(reference.chunk_hash, chunk);
        }
    }

    let mut readers = HashMap::<PathBuf, File>::new();
    let mut chunks = HashMap::<ChunkHash, Vec<u8>>::new();
    for (index, &(range_start, range_length)) in ranges.iter().enumerate() {
        if range_length == 0 {
            continue;
        }
        let range_end = ends[index];
        for reference in &manifest.chunks {
            let reference_end = reference
                .logical_offset
                .checked_add(reference.length)
                .context("manifest range overflow")?;
            let overlap_start = range_start.max(reference.logical_offset);
            let overlap_end = range_end.min(reference_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let location = locations
                .get(&reference.chunk_hash)
                .with_context(|| format!("missing planned chunk {:?}", reference.chunk_hash))?;
            if !chunks.contains_key(&reference.chunk_hash) {
                let path = location.path.clone();
                if !readers.contains_key(&path) {
                    readers.insert(
                        path.clone(),
                        File::open(&path)
                            .with_context(|| format!("opening LAR pack {}", path.display()))?,
                    );
                }
                let bytes = read_chunk_record_at(
                    readers.get_mut(&path).expect("reader was inserted"),
                    &location.descriptor,
                    limits,
                )
                .map_err(anyhow::Error::new)?;
                chunks.insert(reference.chunk_hash, bytes);
            }
            let chunk = chunks
                .get(&reference.chunk_hash)
                .expect("requested chunk was cached");
            let within_reference = overlap_start - reference.logical_offset;
            let chunk_start = reference
                .chunk_offset
                .checked_add(within_reference)
                .context("body range chunk offset overflow")?;
            let chunk_end = chunk_start
                .checked_add(overlap_end - overlap_start)
                .context("body range chunk end overflow")?;
            let chunk_start = usize::try_from(chunk_start)
                .context("body range chunk offset exceeds address space")?;
            let chunk_end =
                usize::try_from(chunk_end).context("body range chunk end exceeds address space")?;
            outputs[index].extend_from_slice(
                chunk
                    .get(chunk_start..chunk_end)
                    .context("body range exceeds catalog chunk")?,
            );
        }
        if outputs[index].len() as u64 != range_length {
            bail!("reconstructed body range length mismatch");
        }
    }
    Ok(outputs)
}

fn publish_artifact(
    conn: &Connection,
    artifact: &LarBodyArtifact,
    legacy_path: &str,
    manifest_id: &ManifestId,
    validated_at_ms: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO lar_trace_artifacts
           (owner_kind, owner_id, artifact_kind, stage_id, manifest_id,
            legacy_path, fidelity, validation_state, validated_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'captured', 'validated', ?7)
         ON CONFLICT(owner_kind, owner_id, artifact_kind, stage_id) DO UPDATE SET
           manifest_id=excluded.manifest_id,
           legacy_path=excluded.legacy_path,
           fidelity='captured', validation_state='validated',
           validated_at_ms=excluded.validated_at_ms,
           pointer_revision=lar_trace_artifacts.pointer_revision+1",
        params![
            artifact.owner_kind.as_str(),
            artifact.owner_id,
            artifact.artifact_kind,
            artifact.stage_id.as_deref().unwrap_or(""),
            manifest_id.to_string(),
            legacy_path,
            validated_at_ms,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alex_lar::OpenPath;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use std::sync::Arc;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "alex-live-lar-{name}-{}-{}",
            std::process::id(),
            LIVE_PACK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(max_pack_bytes: u64) -> LarBodyStoreConfig {
        LarBodyStoreConfig {
            mode: LarBodyStoreMode::DualWriteValidated,
            max_pack_bytes,
            max_pack_age: Duration::from_secs(3600),
            checkpoint_bytes: 1024 * 1024,
            checkpoint_interval: Duration::from_secs(60),
            writer_lock_timeout: Duration::from_secs(2),
            chunker: ChunkerConfig {
                min_size: 4,
                target_size: 4,
                max_size: 4,
            },
            limits: Limits::default(),
        }
    }

    #[test]
    fn concurrent_duplicate_writes_create_one_catalog_chunk() {
        let store = Arc::new(
            Store::open_with_lar_body_store(tmpdir("concurrent"), config(1 << 20)).unwrap(),
        );
        let mut threads = Vec::new();
        for index in 0..8 {
            let store = store.clone();
            threads.push(std::thread::spawn(move || {
                store
                    .write_body_artifact(
                        &LarBodyArtifact::trace(format!("trace-{index}"), "client_request"),
                        "request.json",
                        b"same",
                    )
                    .unwrap()
            }));
        }
        for thread in threads {
            assert!(thread.join().unwrap().lar_error.is_none());
        }
        let conn = store.conn.lock().unwrap();
        let chunks: i64 = conn
            .query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(chunks, 1);
        let pointers: i64 = conn
            .query_row("SELECT COUNT(*) FROM lar_trace_artifacts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(pointers, 0, "dual-write remains a shadow mode");
    }

    #[test]
    fn orphan_recovery_scans_once_at_open_not_on_each_write() {
        let store =
            Store::open_with_lar_body_store(tmpdir("startup-recovery-count"), config(1 << 20))
                .unwrap();
        assert_eq!(store.live_lar.lock().unwrap().recovery_scans, 1);

        for (id, body) in [("trace-one", b"AAAA"), ("trace-two", b"BBBB")] {
            let result = store
                .write_body_artifact(
                    &LarBodyArtifact::trace(id, "client_request"),
                    "request.json",
                    body,
                )
                .unwrap();
            assert!(result.lar_error.is_none());
        }
        assert_eq!(store.live_lar.lock().unwrap().recovery_scans, 1);

        store.recover_lar_body_store_orphans().unwrap();
        assert_eq!(store.live_lar.lock().unwrap().recovery_scans, 2);
    }

    #[test]
    fn many_reused_chunks_open_their_source_pack_once() {
        let store = Store::open_with_lar_body_store(tmpdir("grouped-reused-pack"), config(1 << 20))
            .unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-source", "client_request"),
                "request.json",
                b"AAAABBBBCCCCDDDD",
            )
            .unwrap();
        let before = store.live_lar.lock().unwrap().source_pack_opens;

        let result = store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-reuse", "client_request"),
                "request.json",
                b"DDDDCCCCBBBBAAAA",
            )
            .unwrap();
        assert!(result.lar_error.is_none());
        let after = store.live_lar.lock().unwrap().source_pack_opens;
        assert_eq!(after - before, 1, "opened one source pack per reused chunk");
    }

    #[test]
    fn catalog_reads_do_not_require_a_checkpoint_after_every_write() {
        let mut settings = config(1 << 20);
        settings.mode = LarBodyStoreMode::LarWithFallback;
        settings.checkpoint_bytes = 1 << 20;
        settings.checkpoint_interval = Duration::from_secs(3600);
        let store =
            Store::open_with_lar_body_store(tmpdir("checkpoint-cadence"), settings).unwrap();
        let body = b"AAAABBBBCCCCDDDD";
        let result = store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-no-checkpoint", "client_request"),
                "request.json",
                body,
            )
            .unwrap();

        let path: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT path FROM lar_files WHERE state='active' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let reader = ArchiveReader::open(File::open(path).unwrap(), Limits::default()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::ForwardScan);
        let checkpoints: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM lar_checkpoints", [], |row| row.get(0))
            .unwrap();
        assert_eq!(checkpoints, 0);
        assert_eq!(
            store
                .read_lar_manifest_body(result.manifest_id.as_deref().unwrap())
                .unwrap(),
            body
        );
    }

    #[test]
    fn checkpoint_is_emitted_when_the_byte_cadence_is_reached() {
        let mut settings = config(1 << 20);
        settings.checkpoint_bytes = 1;
        settings.checkpoint_interval = Duration::from_secs(3600);
        let store =
            Store::open_with_lar_body_store(tmpdir("checkpoint-threshold"), settings).unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-checkpoint", "client_request"),
                "request.json",
                b"AAAA",
            )
            .unwrap();
        let path: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT path FROM lar_files WHERE state='active' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let reader = ArchiveReader::open(File::open(path).unwrap(), Limits::default()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);
        let checkpoint: (i64, String, i64, i64, i64, Vec<u8>) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT checkpoint_sequence, record_id, frame_offset, frame_length,
                        append_offset, checksum
                 FROM lar_checkpoints",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(checkpoint.0, 1);
        assert_eq!(checkpoint.1, format!("checkpoint:{}", checkpoint.2));
        assert!(checkpoint.2 > 0);
        assert!(checkpoint.3 > 24);
        assert!(checkpoint.4 > checkpoint.2 + checkpoint.3);
        assert_eq!(checkpoint.5.len(), 32);
    }

    #[test]
    fn checkpoint_is_emitted_when_the_time_cadence_is_reached() {
        let mut settings = config(1 << 20);
        settings.checkpoint_bytes = 1 << 20;
        settings.checkpoint_interval = Duration::from_secs(3600);
        let store = Store::open_with_lar_body_store(tmpdir("checkpoint-time"), settings).unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-before-time", "client_request"),
                "request.json",
                b"AAAA",
            )
            .unwrap();
        assert_eq!(
            store
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM lar_checkpoints", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        store
            .live_lar
            .lock()
            .unwrap()
            .active
            .as_mut()
            .unwrap()
            .last_checkpoint_at_ms = 0;
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-after-time", "client_request"),
                "request.json",
                b"BBBB",
            )
            .unwrap();
        assert_eq!(
            store
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM lar_checkpoints", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
    }

    #[test]
    fn reopen_reconciles_a_synced_checkpoint_missing_from_sqlite() {
        let data_dir = tmpdir("checkpoint-reconcile");
        let mut settings = config(1 << 20);
        settings.checkpoint_bytes = 1;
        settings.checkpoint_interval = Duration::from_secs(3600);
        let store = Store::open_with_lar_body_store(data_dir.clone(), settings.clone()).unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-checkpoint-one", "client_request"),
                "request.json",
                b"AAAA",
            )
            .unwrap();
        let expected: (String, String, i64, i64, i64, Vec<u8>) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT c.file_uuid, f.path, c.frame_offset, c.frame_length,
                        c.append_offset, c.checksum
                 FROM lar_checkpoints c JOIN lar_files f USING(file_uuid)",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM lar_checkpoints WHERE file_uuid=?1",
                [&expected.0],
            )
            .unwrap();
        drop(store);

        let reopened = Store::open_with_lar_body_store(data_dir, settings).unwrap();
        let reconciled: (i64, i64, i64, Vec<u8>) = reopened
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT frame_offset, frame_length, append_offset, checksum
                 FROM lar_checkpoints WHERE file_uuid=?1",
                [&expected.0],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(reconciled, (expected.2, expected.3, expected.4, expected.5));
        let reader =
            ArchiveReader::open(File::open(&expected.1).unwrap(), Limits::default()).unwrap();
        assert_eq!(reader.open_path(), OpenPath::Checkpoint);

        reopened
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-checkpoint-two", "client_request"),
                "request.json",
                b"BBBB",
            )
            .unwrap();
        let sequences: Vec<i64> = reopened
            .conn
            .lock()
            .unwrap()
            .prepare(
                "SELECT checkpoint_sequence FROM lar_checkpoints
                 WHERE file_uuid=?1 ORDER BY checkpoint_sequence",
            )
            .unwrap()
            .query_map([&expected.0], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(sequences, vec![1, 2]);
    }

    #[test]
    fn replacement_manifest_keeps_its_matching_legacy_fallback() {
        let mut settings = config(1 << 20);
        settings.mode = LarBodyStoreMode::LarWithFallback;
        let store =
            Store::open_with_lar_body_store(tmpdir("replacement-fallback"), settings).unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-replaced", "tool_result"),
                "tool-result.json",
                b"old bytes",
            )
            .unwrap();
        let replacement = store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-replaced", "tool_result"),
                "result.json",
                b"replacement bytes",
            )
            .unwrap();

        let fallback: String = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT legacy_path FROM lar_trace_artifacts
                  WHERE owner_kind='tool_call' AND owner_id='tool-replaced'
                    AND artifact_kind='tool_result' AND stage_id=''",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fallback, replacement.legacy_path);
    }

    #[test]
    fn rotation_manifest_references_multiple_packs() {
        let mut settings = config(1);
        settings.mode = LarBodyStoreMode::LarWithFallback;
        let store = Store::open_with_lar_body_store(tmpdir("rotation"), settings).unwrap();
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-a", "client_request"),
                "request.json",
                b"AAAA",
            )
            .unwrap();
        let result = store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-b", "client_request"),
                "request.json",
                b"AAAABBBB",
            )
            .unwrap();
        let id = result.manifest_id.clone().unwrap();
        let conn = store.conn.lock().unwrap();
        let packs: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT c.file_uuid)
                 FROM lar_manifest_chunks mc JOIN lar_chunks c
                   ON c.hash_algorithm=mc.hash_algorithm AND c.chunk_hash=mc.chunk_hash
                 WHERE mc.manifest_id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(packs, 2);
        drop(conn);
        assert_eq!(
            store
                .read_lar_manifest_body(result.manifest_id.as_deref().unwrap())
                .unwrap(),
            b"AAAABBBB"
        );
        let batch = store
            .read_lar_or_legacy_artifact_batch(&[crate::LarArtifactReadRequest::new(
                "trace",
                "trace-b",
                "client_request",
            )])
            .unwrap();
        assert_eq!(batch, vec![Some(b"AAAABBBB".to_vec())]);
    }

    #[test]
    fn interrupted_active_tail_recovers_then_rotates_without_losing_published_bodies() {
        let data_dir = tmpdir("interrupted-rotation");
        let mut initial = config(1 << 20);
        initial.mode = LarBodyStoreMode::LarWithFallback;
        let store = Store::open_with_lar_body_store(data_dir.clone(), initial).unwrap();
        let first = store
            .write_body_artifact(
                &LarBodyArtifact::trace("before-crash", "client_request"),
                "request.json",
                b"AAAABBBB",
            )
            .unwrap();
        let first_manifest = first.manifest_id.unwrap();
        let (first_uuid, first_path): (String, String) = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT file_uuid, path FROM lar_files WHERE state='active'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(store);

        let clean_length = std::fs::metadata(&first_path).unwrap().len();
        let mut interrupted = OpenOptions::new().append(true).open(&first_path).unwrap();
        interrupted
            .write_all(b"LREC\x01\x00\x01\x00\0\0\0")
            .unwrap();
        interrupted.sync_all().unwrap();
        drop(interrupted);
        assert!(std::fs::metadata(&first_path).unwrap().len() > clean_length);
        assert!(matches!(
            ArchiveReader::open(File::open(&first_path).unwrap(), Limits::default())
                .unwrap()
                .recovery_status(),
            RecoveryStatus::TruncatedTail { .. }
        ));

        let mut after_crash = config(1);
        after_crash.mode = LarBodyStoreMode::LarWithFallback;
        let reopened = Store::open_with_lar_body_store(data_dir, after_crash).unwrap();
        assert_eq!(std::fs::metadata(&first_path).unwrap().len(), clean_length);
        assert_eq!(
            reopened.read_lar_manifest_body(&first_manifest).unwrap(),
            b"AAAABBBB"
        );

        let second = reopened
            .write_body_artifact(
                &LarBodyArtifact::trace("after-crash", "client_request"),
                "request.json",
                b"CCCCDDDD",
            )
            .unwrap();
        assert_eq!(
            reopened
                .read_lar_manifest_body(second.manifest_id.as_deref().unwrap())
                .unwrap(),
            b"CCCCDDDD"
        );
        let states: Vec<(String, String)> = reopened
            .conn
            .lock()
            .unwrap()
            .prepare("SELECT file_uuid, state FROM lar_files ORDER BY created_at_ms, file_uuid")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(states.contains(&(first_uuid, "sealed".into())));
        assert_eq!(
            states.iter().filter(|(_, state)| state == "active").count(),
            1
        );
        let old = ArchiveReader::open(File::open(first_path).unwrap(), Limits::default()).unwrap();
        assert!(old.is_sealed());
        assert_eq!(old.recovery_status(), RecoveryStatus::Clean);
    }

    #[test]
    fn disk_full_before_and_during_append_keeps_fallback_and_never_publishes_partial_lar() {
        for during_append in [false, true] {
            let mut settings = config(1 << 20);
            settings.mode = LarBodyStoreMode::LarWithFallback;
            let store = Store::open_with_lar_body_store(
                tmpdir(if during_append {
                    "disk-full-during"
                } else {
                    "disk-full-before"
                }),
                settings,
            )
            .unwrap();
            if during_append {
                store.inject_lar_disk_full_during_append_once();
            } else {
                store.inject_lar_disk_full_before_append_once();
            }
            let failed = store
                .write_body_artifact(
                    &LarBodyArtifact::trace("disk-full", "client_request"),
                    "request.json",
                    b"AAAABBBB",
                )
                .unwrap();
            assert!(failed.manifest_id.is_none());
            assert!(failed
                .lar_error
                .as_deref()
                .is_some_and(|error| error.contains("No space left on device")));
            let mut decoder = GzDecoder::new(File::open(&failed.legacy_path).unwrap());
            let mut fallback = Vec::new();
            decoder.read_to_end(&mut fallback).unwrap();
            assert_eq!(fallback, b"AAAABBBB");
            let published: i64 = store
                .conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM lar_trace_artifacts WHERE owner_id='disk-full'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(published, 0);

            let recovered = store
                .write_body_artifact(
                    &LarBodyArtifact::trace("after-disk-full", "client_request"),
                    "request.json",
                    b"AAAABBBB",
                )
                .unwrap();
            assert!(recovered.lar_error.is_none());
            assert_eq!(
                store
                    .read_lar_manifest_body(recovered.manifest_id.as_deref().unwrap())
                    .unwrap(),
                b"AAAABBBB"
            );
            let repairing: i64 = store
                .conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM lar_files WHERE state='repairing'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(repairing, 0);
        }
    }

    #[test]
    fn rolled_back_append_is_recovered_without_duplicate() {
        let store = Store::open_with_lar_body_store(tmpdir("orphan"), config(1 << 20)).unwrap();
        store.inject_lar_catalog_commit_failure_once();
        let failed = store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-failed", "client_request"),
                "request.json",
                b"orphan",
            )
            .unwrap();
        assert!(failed.manifest_id.is_none());
        assert!(failed.lar_error.is_some());
        assert_eq!(store.recover_lar_body_store_orphans().unwrap(), 2);
        let successful = store
            .write_body_artifact(
                &LarBodyArtifact::trace("trace-ok", "client_request"),
                "request.json",
                b"orphan",
            )
            .unwrap();
        assert!(successful.lar_error.is_none());
        let conn = store.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM lar_chunks", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        // "orphan" is two four-byte chunks. Recovery reuses both; it does not
        // append a second physical/catalog copy.
        let physical: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM lar_chunks WHERE chunk_hash IN
                   (SELECT chunk_hash FROM lar_manifest_chunks WHERE manifest_id=?1)",
                [successful.manifest_id.unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(physical, 2);
    }

    #[test]
    fn exact_reconstruction_and_legacy_fallback_are_kept() {
        let mut settings = config(1 << 20);
        settings.mode = LarBodyStoreMode::LarWithFallback;
        let store = Store::open_with_lar_body_store(tmpdir("exact"), settings).unwrap();
        let body: Vec<u8> = (0..32_000).map(|value| (value % 251) as u8).collect();
        let result = store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-1", "tool_result"),
                "tool-result.json",
                &body,
            )
            .unwrap();
        let manifest = result.manifest_id.unwrap();
        assert_eq!(store.read_lar_manifest_body(&manifest).unwrap(), body);
        assert!(Path::new(&result.legacy_path).is_file());
        assert!(matches!(
            store
                .lar_artifact_location("tool_call", "tool-1", "tool_result", None)
                .unwrap(),
            Some(crate::LarArtifactLocation::Lar { manifest_id, .. }) if manifest_id == manifest
        ));
    }

    #[test]
    fn ordinary_store_open_remains_legacy_only() {
        let store = Store::open(tmpdir("default")).unwrap();
        assert_eq!(store.lar_body_store_mode(), LarBodyStoreMode::Legacy);
        let path = store
            .write_body("trace", "request.json", b"legacy")
            .unwrap();
        assert!(Path::new(&path).is_file());
        assert!(!store.data_dir.join("lar").exists());
    }

    #[test]
    fn dario_spool_is_ingested_through_typed_lar_fallback() {
        let mut settings = config(1 << 20);
        settings.mode = LarBodyStoreMode::LarWithFallback;
        let store = Store::open_with_lar_body_store(tmpdir("dario-spool"), settings).unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "trace-dario".into(),
                ts_request_ms: 1,
                via_dario: true,
                ..Default::default()
            })
            .unwrap();
        let day = store.data_dir.join("dario-capture-spool/2026-07-20");
        std::fs::create_dir_all(&day).unwrap();
        let spool = day.join("trace-dario.dario-upstream-request.json.gz");
        let file = File::create(&spool).unwrap();
        let mut encoder = GzEncoder::new(file, Compression::default());
        encoder
            .write_all(br#"{"direction":"dario->anthropic"}"#)
            .unwrap();
        encoder.finish().unwrap().sync_all().unwrap();

        assert_eq!(store.ingest_pending_dario_captures().unwrap(), 1);
        assert!(!spool.exists());
        assert_eq!(
            store
                .read_lar_or_legacy_artifact(
                    "trace",
                    "trace-dario",
                    "dario_upstream_request",
                    None,
                )
                .unwrap()
                .unwrap(),
            br#"{"direction":"dario->anthropic"}"#
        );
        assert_eq!(
            store
                .ingest_dario_capture_spool("../escape")
                .unwrap_err()
                .to_string(),
            "invalid Dario capture trace ID"
        );
    }

    #[test]
    fn body_store_mode_parser_is_explicit_and_backward_safe() {
        assert_eq!(
            "legacy".parse::<LarBodyStoreMode>().unwrap(),
            LarBodyStoreMode::Legacy
        );
        assert_eq!(
            "dual-write-validated".parse::<LarBodyStoreMode>().unwrap(),
            LarBodyStoreMode::DualWriteValidated
        );
        assert_eq!(
            "lar-with-fallback".parse::<LarBodyStoreMode>().unwrap(),
            LarBodyStoreMode::LarWithFallback
        );
        assert!("lar".parse::<LarBodyStoreMode>().is_err());
    }
}
