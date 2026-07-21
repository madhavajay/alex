//! LAR V1: an append-only body archive with a lazy, fixed-width footer index.
//!
//! The format implemented here intentionally covers the V1 Trace Browser path:
//! crash-safe raw body capture, bounded random reads, legacy migration, and
//! sanitized replay fixtures. Sequence generations, dictionaries, and content
//! deduplication are intentionally deferred.

use crc32fast::Hasher as Crc32;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const FILE_MAGIC: &[u8; 4] = b"LAR1";
const RECORD_MAGIC: &[u8; 4] = b"LREC";
const INDEX_MAGIC: &[u8; 4] = b"LIDX";
const FOOTER_MAGIC: &[u8; 4] = b"LFTR";
const VERSION: u16 = 1;
const FILE_HEADER_LEN: u64 = 16;
const RECORD_HEADER_LEN: u64 = 32;
const INDEX_ENTRY_LEN: u64 = 48;
const INDEX_HEADER_LEN: u64 = 24;
const FOOTER_LEN: u64 = 48;
const RECORD_TYPE_BODY: u8 = 1;
const IO_CHUNK: usize = 64 * 1024;
const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_INDEX_ENTRIES: u64 = 1_000_000;
const DEFAULT_MAX_BODY_BYTES: u64 = 512 * 1024 * 1024;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub type Result<T> = std::result::Result<T, LarError>;

#[derive(Debug, thiserror::Error)]
pub enum LarError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid LAR data: {0}")]
    Corrupt(String),
    #[error("LAR safety limit exceeded: {0}")]
    Limit(String),
    #[error("body {trace_id}/{body_kind} already exists with different content")]
    Conflict { trace_id: String, body_kind: String },
    #[error("body {trace_id}/{body_kind} was not found")]
    NotFound { trace_id: String, body_kind: String },
    #[error("fixture export refused unsafe body: {0}")]
    UnsafeFixture(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseEvent {
    pub byte_offset: u64,
    pub ms_delta: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyMetadata {
    pub trace_id: String,
    pub body_kind: String,
    pub sha256: String,
    #[serde(default)]
    pub sanitized: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sse_timing: Vec<SseEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BodyKey {
    pub trace_id: String,
    pub body_kind: String,
}

impl BodyKey {
    pub fn new(trace_id: impl Into<String>, body_kind: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            body_kind: body_kind.into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct IndexEntry {
    key: [u8; 32],
    record_offset: u64,
    record_len: u64,
}

#[derive(Clone, Copy, Debug)]
struct RecordHeader {
    metadata_len: u64,
    payload_len: u64,
    metadata_crc: u32,
    payload_crc: u32,
}

#[derive(Clone, Copy, Debug)]
struct Footer {
    index_offset: u64,
    index_count: u64,
    records_end: u64,
}

enum ReaderIndex {
    Footer(Footer),
    Recovered(Vec<IndexEntry>),
}

/// A read handle. A healthy, closed archive retains no in-memory index; every
/// lookup binary-searches the fixed-width on-disk footer.
pub struct ArchiveReader {
    path: PathBuf,
    file: File,
    index: ReaderIndex,
}

impl ArchiveReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new().read(true).open(&path)?;
        validate_file_header(&mut file)?;
        let len = file.metadata()?.len();
        let index = match read_footer(&mut file, len)? {
            Some(footer) => ReaderIndex::Footer(footer),
            None => {
                let scan = scan_records(&mut file, len)?;
                ReaderIndex::Recovered(scan.entries)
            }
        };
        Ok(Self { path, file, index })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> u64 {
        match &self.index {
            ReaderIndex::Footer(footer) => footer.index_count,
            ReaderIndex::Recovered(entries) => entries.len() as u64,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes retained for the index. This is zero for normal footer-backed
    /// archives; footerless recovery is explicitly capped.
    pub fn resident_index_bytes(&self) -> usize {
        match &self.index {
            ReaderIndex::Footer(_) => 0,
            ReaderIndex::Recovered(entries) => entries.len() * std::mem::size_of::<IndexEntry>(),
        }
    }

    pub fn metadata(&mut self, trace_id: &str, body_kind: &str) -> Result<BodyMetadata> {
        let entry = self
            .find_entry(trace_id, body_kind)?
            .ok_or_else(|| LarError::NotFound {
                trace_id: trace_id.to_owned(),
                body_kind: body_kind.to_owned(),
            })?;
        read_record_metadata(&mut self.file, entry)
    }

    pub fn contains(&mut self, trace_id: &str, body_kind: &str) -> Result<bool> {
        Ok(self.find_entry(trace_id, body_kind)?.is_some())
    }

    /// Read exactly one body, refusing allocation above `max_bytes`.
    pub fn read_body(
        &mut self,
        trace_id: &str,
        body_kind: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.copy_body_to(trace_id, body_kind, max_bytes, &mut bytes)?;
        Ok(bytes)
    }

    /// Stream exactly one indexed body to `output`, validating both CRC32 and
    /// SHA-256 as it is read. No other body bytes are touched.
    pub fn copy_body_to(
        &mut self,
        trace_id: &str,
        body_kind: &str,
        max_bytes: u64,
        output: &mut impl Write,
    ) -> Result<BodyMetadata> {
        let entry = self
            .find_entry(trace_id, body_kind)?
            .ok_or_else(|| LarError::NotFound {
                trace_id: trace_id.to_owned(),
                body_kind: body_kind.to_owned(),
            })?;
        stream_record(&mut self.file, entry, max_bytes, output)
    }

    pub fn verify(&mut self, max_body_bytes: u64) -> Result<VerifyReport> {
        let mut checked = 0u64;
        let count = self.len();
        for position in 0..count {
            let entry = self.index_entry(position)?;
            stream_record(&mut self.file, entry, max_body_bytes, &mut io::sink())?;
            checked += 1;
        }
        Ok(VerifyReport { checked })
    }

    /// Page body metadata without materializing the entire archive catalogue.
    pub fn list(&mut self, offset: u64, limit: usize) -> Result<Vec<BodyMetadata>> {
        let end = self.len().min(offset.saturating_add(limit as u64));
        let mut result = Vec::with_capacity((end - offset) as usize);
        for position in offset..end {
            let entry = self.index_entry(position)?;
            result.push(read_record_metadata(&mut self.file, entry)?);
        }
        Ok(result)
    }

    fn find_entry(&mut self, trace_id: &str, body_kind: &str) -> Result<Option<IndexEntry>> {
        let key = index_key(trace_id, body_kind);
        match &self.index {
            ReaderIndex::Recovered(entries) => Ok(entries
                .binary_search_by(|entry| entry.key.cmp(&key))
                .ok()
                .map(|position| entries[position])),
            ReaderIndex::Footer(footer) => {
                let count = footer.index_count;
                let index_offset = footer.index_offset + INDEX_HEADER_LEN;
                let mut low = 0u64;
                let mut high = count;
                while low < high {
                    let mid = low + (high - low) / 2;
                    let entry = read_index_entry(&mut self.file, index_offset, mid)?;
                    match entry.key.cmp(&key) {
                        std::cmp::Ordering::Less => low = mid + 1,
                        std::cmp::Ordering::Greater => high = mid,
                        std::cmp::Ordering::Equal => return Ok(Some(entry)),
                    }
                }
                Ok(None)
            }
        }
    }

    fn index_entry(&mut self, position: u64) -> Result<IndexEntry> {
        match &self.index {
            ReaderIndex::Footer(footer) => read_index_entry(
                &mut self.file,
                footer.index_offset + INDEX_HEADER_LEN,
                position,
            ),
            ReaderIndex::Recovered(entries) => entries
                .get(position as usize)
                .copied()
                .ok_or_else(|| LarError::Corrupt("index position outside archive".into())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub checked: u64,
}

/// Append-only writer. Records are never rewritten; closing appends a sorted
/// index and footer. Reopening removes only the prior derived footer/index.
pub struct ArchiveWriter {
    path: PathBuf,
    file: File,
    entries: Vec<IndexEntry>,
    by_key: HashMap<[u8; 32], usize>,
    max_body_bytes: u64,
}

impl ArchiveWriter {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limit(path, DEFAULT_MAX_BODY_BYTES)
    }

    pub fn open_with_limit(path: impl AsRef<Path>, max_body_bytes: u64) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let len = file.metadata()?.len();
        let entries = if len == 0 {
            write_file_header(&mut file)?;
            file.sync_data()?;
            Vec::new()
        } else {
            validate_file_header(&mut file)?;
            if let Some(footer) = read_footer(&mut file, len)? {
                let entries = read_all_index_entries(&mut file, footer)?;
                file.set_len(footer.records_end)?;
                entries
            } else {
                let scan = scan_records(&mut file, len)?;
                if scan.records_end < len {
                    file.set_len(scan.records_end)?;
                    file.sync_data()?;
                }
                scan.entries
            }
        };
        if entries.len() as u64 > MAX_INDEX_ENTRIES {
            return Err(LarError::Limit(format!(
                "archive has more than {MAX_INDEX_ENTRIES} records"
            )));
        }
        let mut by_key = HashMap::with_capacity(entries.len());
        for (position, entry) in entries.iter().enumerate() {
            if by_key.insert(entry.key, position).is_some() {
                return Err(LarError::Corrupt(
                    "duplicate trace/body key in archive".into(),
                ));
            }
        }
        file.seek(SeekFrom::End(0))?;
        Ok(Self {
            path,
            file,
            entries,
            by_key,
            max_body_bytes,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn append_body(
        &mut self,
        trace_id: &str,
        body_kind: &str,
        bytes: &[u8],
        sse_timing: Vec<SseEvent>,
    ) -> Result<AppendOutcome> {
        if bytes.len() as u64 > self.max_body_bytes {
            return Err(LarError::Limit(format!(
                "body is {} bytes; configured maximum is {}",
                bytes.len(),
                self.max_body_bytes
            )));
        }
        let sha256 = sha256_bytes(bytes);
        if let Some(existing) = self.existing_metadata(trace_id, body_kind)? {
            if existing.sha256 == sha256 {
                return Ok(AppendOutcome::AlreadyPresent);
            }
            return Err(LarError::Conflict {
                trace_id: trace_id.to_owned(),
                body_kind: body_kind.to_owned(),
            });
        }
        let metadata = BodyMetadata {
            trace_id: trace_id.to_owned(),
            body_kind: body_kind.to_owned(),
            sha256,
            sanitized: false,
            sse_timing,
        };
        let payload_crc = crc32fast::hash(bytes);
        self.append_prepared(&metadata, bytes.len() as u64, payload_crc, &mut &bytes[..])?;
        Ok(AppendOutcome::Appended)
    }

    pub fn append_reader(
        &mut self,
        trace_id: &str,
        body_kind: &str,
        reader: impl Read,
        sse_timing: Vec<SseEvent>,
    ) -> Result<AppendOutcome> {
        self.append_reader_with_sanitized(trace_id, body_kind, reader, sse_timing, false)
    }

    fn append_reader_with_sanitized(
        &mut self,
        trace_id: &str,
        body_kind: &str,
        mut reader: impl Read,
        sse_timing: Vec<SseEvent>,
        sanitized: bool,
    ) -> Result<AppendOutcome> {
        let spool = temporary_path(&self.path, "spool");
        let result = (|| {
            let mut output = BufWriter::new(File::create(&spool)?);
            let mut sha = Sha256::new();
            let mut crc = Crc32::new();
            let mut len = 0u64;
            let mut buffer = [0u8; IO_CHUNK];
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                len = len
                    .checked_add(read as u64)
                    .ok_or_else(|| LarError::Limit("body length overflow".into()))?;
                if len > self.max_body_bytes {
                    return Err(LarError::Limit(format!(
                        "body exceeds configured maximum of {} bytes",
                        self.max_body_bytes
                    )));
                }
                sha.update(&buffer[..read]);
                crc.update(&buffer[..read]);
                output.write_all(&buffer[..read])?;
            }
            output.flush()?;
            output.get_ref().sync_data()?;
            let digest = hex(&sha.finalize());
            if let Some(existing) = self.existing_metadata(trace_id, body_kind)? {
                if existing.sha256 == digest {
                    return Ok(AppendOutcome::AlreadyPresent);
                }
                return Err(LarError::Conflict {
                    trace_id: trace_id.to_owned(),
                    body_kind: body_kind.to_owned(),
                });
            }
            let metadata = BodyMetadata {
                trace_id: trace_id.to_owned(),
                body_kind: body_kind.to_owned(),
                sha256: digest,
                sanitized,
                sse_timing,
            };
            let mut input = BufReader::new(File::open(&spool)?);
            self.append_prepared(&metadata, len, crc.finalize(), &mut input)?;
            Ok(AppendOutcome::Appended)
        })();
        let _ = fs::remove_file(&spool);
        result
    }

    pub fn sync(&mut self) -> Result<()> {
        self.file.flush()?;
        self.file.sync_data()?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.sync()?;
        self.entries.sort_unstable_by_key(|entry| entry.key);
        let index_offset = self.file.seek(SeekFrom::End(0))?;
        self.file
            .write_all(&encode_index_header(self.entries.len() as u64))?;
        let mut index_crc = Crc32::new();
        for entry in &self.entries {
            let bytes = encode_index_entry(*entry);
            self.file.write_all(&bytes)?;
            index_crc.update(&bytes);
        }
        let footer = encode_footer(
            Footer {
                index_offset,
                index_count: self.entries.len() as u64,
                records_end: index_offset,
            },
            index_crc.finalize(),
        );
        self.file.write_all(&footer)?;
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(())
    }

    fn existing_metadata(
        &mut self,
        trace_id: &str,
        body_kind: &str,
    ) -> Result<Option<BodyMetadata>> {
        let key = index_key(trace_id, body_kind);
        let Some(position) = self.by_key.get(&key).copied() else {
            return Ok(None);
        };
        let metadata = read_record_metadata(&mut self.file, self.entries[position])?;
        if metadata.trace_id != trace_id || metadata.body_kind != body_kind {
            return Err(LarError::Corrupt("SHA-256 index collision".into()));
        }
        Ok(Some(metadata))
    }

    fn append_prepared(
        &mut self,
        metadata: &BodyMetadata,
        payload_len: u64,
        payload_crc: u32,
        payload: &mut impl Read,
    ) -> Result<()> {
        if self.entries.len() as u64 >= MAX_INDEX_ENTRIES {
            return Err(LarError::Limit(format!(
                "archive record limit of {MAX_INDEX_ENTRIES} reached"
            )));
        }
        let metadata_bytes = serde_json::to_vec(metadata)?;
        if metadata_bytes.len() as u64 > MAX_METADATA_BYTES {
            return Err(LarError::Limit("body metadata is too large".into()));
        }
        let offset = self.file.seek(SeekFrom::End(0))?;
        let append_result = (|| {
            let header = encode_record_header(
                metadata_bytes.len() as u64,
                payload_len,
                crc32fast::hash(&metadata_bytes),
                payload_crc,
            );
            self.file.write_all(&header)?;
            self.file.write_all(&metadata_bytes)?;
            let copied = io::copy(payload, &mut self.file)?;
            if copied != payload_len {
                return Err(LarError::Corrupt(format!(
                    "prepared body length changed: expected {payload_len}, read {copied}"
                )));
            }
            Ok(())
        })();
        if let Err(error) = append_result {
            self.file.set_len(offset)?;
            self.file.seek(SeekFrom::Start(offset))?;
            return Err(error);
        }
        let record_len = RECORD_HEADER_LEN + metadata_bytes.len() as u64 + payload_len;
        let entry = IndexEntry {
            key: index_key(&metadata.trace_id, &metadata.body_kind),
            record_offset: offset,
            record_len,
        };
        let position = self.entries.len();
        self.entries.push(entry);
        self.by_key.insert(entry.key, position);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    Appended,
    AlreadyPresent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyBodyRef {
    pub trace_id: String,
    pub body_kind: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    pub max_body_bytes: u64,
    /// Primarily useful for controlled batches and deterministic resume tests.
    pub max_entries_this_run: Option<usize>,
    /// Sync the archive and atomically advance the checkpoint after this many
    /// bodies. A crash can replay at most this batch; archive key
    /// idempotency makes that replay safe.
    pub checkpoint_every: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_entries_this_run: None,
            checkpoint_every: 128,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub imported: u64,
    pub already_present: u64,
    pub next_index: usize,
    pub complete: bool,
    pub validated: bool,
    pub originals_preserved: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ImportState {
    refs_sha256: String,
    next_index: usize,
    complete: bool,
    validated: bool,
}

/// Import the existing `bodies/**/*.gz` representation. Originals are opened
/// read-only and are never deleted. A checkpoint is atomically replaced after
/// the archive has been synced, making restart safe and idempotent.
pub fn import_legacy(
    refs: &[LegacyBodyRef],
    archive_path: impl AsRef<Path>,
    checkpoint_path: impl AsRef<Path>,
    options: ImportOptions,
) -> Result<ImportReport> {
    let archive_path = archive_path.as_ref();
    let checkpoint_path = checkpoint_path.as_ref();
    let refs_sha256 = refs_digest(refs);
    let mut state = if checkpoint_path.exists() {
        let state: ImportState =
            serde_json::from_reader(BufReader::new(File::open(checkpoint_path)?))?;
        if state.refs_sha256 != refs_sha256 {
            return Err(LarError::Corrupt(
                "migration checkpoint belongs to a different body reference list".into(),
            ));
        }
        state
    } else {
        ImportState {
            refs_sha256,
            next_index: 0,
            complete: false,
            validated: false,
        }
    };
    let mut writer = ArchiveWriter::open_with_limit(archive_path, options.max_body_bytes)?;
    let mut report = ImportReport {
        next_index: state.next_index,
        originals_preserved: true,
        ..ImportReport::default()
    };
    let end = options
        .max_entries_this_run
        .map(|limit| refs.len().min(state.next_index.saturating_add(limit)))
        .unwrap_or(refs.len());
    let start_index = state.next_index;
    let checkpoint_every = options.checkpoint_every.max(1);
    for (index, reference) in refs.iter().enumerate().take(end).skip(state.next_index) {
        let file = File::open(&reference.path)?;
        let outcome = writer.append_reader(
            &reference.trace_id,
            &reference.body_kind,
            GzDecoder::new(BufReader::new(file)),
            Vec::new(),
        )?;
        match outcome {
            AppendOutcome::Appended => report.imported += 1,
            AppendOutcome::AlreadyPresent => report.already_present += 1,
        }
        state.next_index = index + 1;
        report.next_index = state.next_index;
        if (state.next_index - start_index).is_multiple_of(checkpoint_every) {
            writer.sync()?;
            write_state_atomic(checkpoint_path, &state)?;
        }
    }
    if state.next_index > start_index
        && !(state.next_index - start_index).is_multiple_of(checkpoint_every)
    {
        writer.sync()?;
        write_state_atomic(checkpoint_path, &state)?;
    }
    writer.finish()?;
    if state.next_index == refs.len() {
        validate_legacy_import(refs, archive_path, options.max_body_bytes)?;
        state.complete = true;
        state.validated = true;
        write_state_atomic(checkpoint_path, &state)?;
    }
    report.next_index = state.next_index;
    report.complete = state.complete;
    report.validated = state.validated;
    Ok(report)
}

pub fn validate_legacy_import(
    refs: &[LegacyBodyRef],
    archive_path: impl AsRef<Path>,
    max_body_bytes: u64,
) -> Result<()> {
    let mut archive = ArchiveReader::open(archive_path)?;
    for reference in refs {
        let source = File::open(&reference.path)?;
        let source_digest = digest_reader(GzDecoder::new(BufReader::new(source)), max_body_bytes)?;
        let mut archive_digest = Sha256::new();
        let mut sink = DigestWriter(&mut archive_digest);
        archive.copy_body_to(
            &reference.trace_id,
            &reference.body_kind,
            max_body_bytes,
            &mut sink,
        )?;
        if source_digest != hex(&archive_digest.finalize()) {
            return Err(LarError::Corrupt(format!(
                "migrated body differs from original: {}/{}",
                reference.trace_id, reference.body_kind
            )));
        }
    }
    Ok(())
}

/// Discover current Alex gzip bodies without opening or deleting them.
pub fn discover_legacy_bodies(root: impl AsRef<Path>) -> Result<Vec<LegacyBodyRef>> {
    let mut pending = vec![root.as_ref().to_path_buf()];
    let mut refs = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(stem) = name.strip_suffix(".gz") else {
                continue;
            };
            let Some((trace_id, body_kind)) = stem.split_once('.') else {
                continue;
            };
            if trace_id.is_empty() || body_kind.is_empty() {
                continue;
            }
            refs.push(LegacyBodyRef {
                trace_id: trace_id.to_owned(),
                body_kind: body_kind.to_owned(),
                path: entry.path(),
            });
        }
    }
    refs.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(refs)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureExportReport {
    pub bodies: u64,
    pub sanitized_bodies: u64,
}

/// Export a replayable archive containing only selected bodies. JSON secrets
/// are redacted recursively; raw non-JSON payloads are copied byte-for-byte.
pub fn export_sanitized_fixture(
    source: impl AsRef<Path>,
    output: impl AsRef<Path>,
    selections: &[BodyKey],
    max_body_bytes: u64,
) -> Result<FixtureExportReport> {
    let output = output.as_ref();
    let mut reader = ArchiveReader::open(source)?;
    let mut writer = ArchiveWriter::open_with_limit(output, max_body_bytes)?;
    if !writer.is_empty() {
        return Err(LarError::Conflict {
            trace_id: "fixture".into(),
            body_kind: "output archive is not empty".into(),
        });
    }
    let mut report = FixtureExportReport::default();
    for key in selections {
        let metadata = reader.metadata(&key.trace_id, &key.body_kind)?;
        let body = reader.read_body(&key.trace_id, &key.body_kind, max_body_bytes)?;
        let (sanitized, changed) = sanitize_body(&body)?;
        writer.append_reader_with_sanitized(
            &key.trace_id,
            &key.body_kind,
            &sanitized[..],
            metadata.sse_timing,
            true,
        )?;
        report.bodies += 1;
        report.sanitized_bodies += u64::from(changed);
    }
    writer.finish()?;
    let mut fixture = ArchiveReader::open(output)?;
    fixture.verify(max_body_bytes)?;
    Ok(report)
}

pub fn sanitize_body(body: &[u8]) -> Result<(Vec<u8>, bool)> {
    let (output, changed) = if let Ok(mut value) = serde_json::from_slice::<Value>(body) {
        let changed = sanitize_json(&mut value);
        (serde_json::to_vec(&value)?, changed)
    } else if let Some(result) = sanitize_sse(body)? {
        result
    } else {
        return Err(LarError::UnsafeFixture(
            "body is neither JSON nor strictly framed JSON SSE".into(),
        ));
    };
    if contains_secret_pattern(&output) {
        return Err(LarError::UnsafeFixture(
            "a credential-like value remains after structured redaction".into(),
        ));
    }
    Ok((output, changed))
}

fn sanitize_sse(body: &[u8]) -> Result<Option<(Vec<u8>, bool)>> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Ok(None);
    };
    let mut output = String::with_capacity(text.len());
    let mut saw_data = false;
    let mut changed = false;
    for line_with_end in text.split_inclusive('\n') {
        let has_newline = line_with_end.ends_with('\n');
        let line = line_with_end
            .strip_suffix('\n')
            .unwrap_or(line_with_end)
            .strip_suffix('\r')
            .unwrap_or_else(|| line_with_end.strip_suffix('\n').unwrap_or(line_with_end));
        if let Some(payload) = line.strip_prefix("data:") {
            saw_data = true;
            let payload = payload.trim_start();
            if payload == "[DONE]" {
                output.push_str("data: [DONE]");
            } else {
                let mut value: Value = serde_json::from_str(payload).map_err(|_| {
                    LarError::UnsafeFixture("SSE data field is not a JSON object".into())
                })?;
                changed |= sanitize_json(&mut value);
                output.push_str("data: ");
                output.push_str(&serde_json::to_string(&value)?);
            }
        } else if line.is_empty()
            || line.starts_with(':')
            || line.starts_with("event:")
            || line.starts_with("id:")
            || line.starts_with("retry:")
        {
            output.push_str(line);
        } else {
            return Ok(None);
        }
        if has_newline {
            output.push('\n');
        }
    }
    Ok(saw_data.then(|| (output.into_bytes(), changed)))
}

fn contains_secret_pattern(body: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(body).to_ascii_lowercase();
    [
        "bearer ",
        "sk-",
        "xoxb-",
        "xoxp-",
        "ghp_",
        "github_pat_",
        "-----begin private key-----",
    ]
    .iter()
    .any(|pattern| lower.contains(pattern))
}

fn sanitize_json(value: &mut Value) -> bool {
    const SECRET_KEYS: &[&str] = &[
        "authorization",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "token",
        "cookie",
        "set-cookie",
        "x-api-key",
        "x-goog-api-key",
        "proxy-authorization",
        "client_secret",
        "password",
        "secret",
    ];
    match value {
        Value::Object(map) => {
            let mut changed = false;
            for (key, value) in map {
                if SECRET_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                    *value = Value::String("[REDACTED]".into());
                    changed = true;
                } else {
                    changed |= sanitize_json(value);
                }
            }
            changed
        }
        Value::Array(values) => {
            let mut changed = false;
            for value in values {
                changed |= sanitize_json(value);
            }
            changed
        }
        _ => false,
    }
}

struct ScanResult {
    entries: Vec<IndexEntry>,
    records_end: u64,
}

fn scan_records(file: &mut File, file_len: u64) -> Result<ScanResult> {
    let mut offset = FILE_HEADER_LEN;
    let mut entries = Vec::new();
    while offset < file_len {
        let remaining = file_len - offset;
        if remaining < RECORD_HEADER_LEN {
            break;
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut raw = [0u8; RECORD_HEADER_LEN as usize];
        file.read_exact(&mut raw)?;
        if &raw[..4] == INDEX_MAGIC {
            // The index and footer are derived data. A crash while writing
            // either leaves this marker after the complete record stream; a
            // writer can safely discard it and rebuild from record headers.
            break;
        }
        if &raw[..4] != RECORD_MAGIC {
            return Err(LarError::Corrupt(format!(
                "unexpected bytes at record offset {offset}"
            )));
        }
        let header = decode_record_header(&raw)?;
        let record_len = RECORD_HEADER_LEN
            .checked_add(header.metadata_len)
            .and_then(|len| len.checked_add(header.payload_len))
            .ok_or_else(|| LarError::Corrupt("record length overflow".into()))?;
        if record_len > remaining {
            break;
        }
        let entry = IndexEntry {
            key: [0; 32],
            record_offset: offset,
            record_len,
        };
        let metadata = read_record_metadata(file, entry)?;
        verify_record_crc(file, entry, header)?;
        entries.push(IndexEntry {
            key: index_key(&metadata.trace_id, &metadata.body_kind),
            ..entry
        });
        if entries.len() as u64 > MAX_INDEX_ENTRIES {
            return Err(LarError::Limit(format!(
                "recovery exceeds {MAX_INDEX_ENTRIES} records"
            )));
        }
        offset += record_len;
    }
    entries.sort_unstable_by_key(|entry| entry.key);
    for pair in entries.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(LarError::Corrupt(
                "duplicate trace/body key during recovery".into(),
            ));
        }
    }
    Ok(ScanResult {
        entries,
        records_end: offset,
    })
}

fn write_file_header(file: &mut File) -> Result<()> {
    let mut header = [0u8; FILE_HEADER_LEN as usize];
    header[..4].copy_from_slice(FILE_MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(FILE_HEADER_LEN as u16).to_le_bytes());
    file.write_all(&header)?;
    Ok(())
}

fn validate_file_header(file: &mut File) -> Result<()> {
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0u8; FILE_HEADER_LEN as usize];
    file.read_exact(&mut header)
        .map_err(|_| LarError::Corrupt("truncated LAR file header".into()))?;
    if &header[..4] != FILE_MAGIC {
        return Err(LarError::Corrupt("missing LAR1 magic".into()));
    }
    if u16::from_le_bytes([header[4], header[5]]) != VERSION {
        return Err(LarError::Corrupt("unsupported LAR version".into()));
    }
    if u16::from_le_bytes([header[6], header[7]]) as u64 != FILE_HEADER_LEN {
        return Err(LarError::Corrupt("invalid LAR header length".into()));
    }
    Ok(())
}

fn encode_record_header(
    metadata_len: u64,
    payload_len: u64,
    metadata_crc: u32,
    payload_crc: u32,
) -> [u8; RECORD_HEADER_LEN as usize] {
    let mut raw = [0u8; RECORD_HEADER_LEN as usize];
    raw[..4].copy_from_slice(RECORD_MAGIC);
    raw[4..6].copy_from_slice(&VERSION.to_le_bytes());
    raw[6] = RECORD_TYPE_BODY;
    raw[8..12].copy_from_slice(&(metadata_len as u32).to_le_bytes());
    raw[12..20].copy_from_slice(&payload_len.to_le_bytes());
    raw[20..24].copy_from_slice(&metadata_crc.to_le_bytes());
    raw[24..28].copy_from_slice(&payload_crc.to_le_bytes());
    let header_crc = crc32fast::hash(&raw[..28]);
    raw[28..32].copy_from_slice(&header_crc.to_le_bytes());
    raw
}

fn decode_record_header(raw: &[u8; RECORD_HEADER_LEN as usize]) -> Result<RecordHeader> {
    if &raw[..4] != RECORD_MAGIC {
        return Err(LarError::Corrupt("invalid record magic".into()));
    }
    if u16::from_le_bytes([raw[4], raw[5]]) != VERSION || raw[6] != RECORD_TYPE_BODY {
        return Err(LarError::Corrupt(
            "unsupported record version or type".into(),
        ));
    }
    let expected_crc = u32::from_le_bytes(raw[28..32].try_into().unwrap());
    if crc32fast::hash(&raw[..28]) != expected_crc {
        return Err(LarError::Corrupt("record header checksum mismatch".into()));
    }
    let metadata_len = u32::from_le_bytes(raw[8..12].try_into().unwrap()) as u64;
    if metadata_len > MAX_METADATA_BYTES {
        return Err(LarError::Limit(format!(
            "record metadata exceeds {MAX_METADATA_BYTES} bytes"
        )));
    }
    Ok(RecordHeader {
        metadata_len,
        payload_len: u64::from_le_bytes(raw[12..20].try_into().unwrap()),
        metadata_crc: u32::from_le_bytes(raw[20..24].try_into().unwrap()),
        payload_crc: u32::from_le_bytes(raw[24..28].try_into().unwrap()),
    })
}

fn read_record_header(file: &mut File, offset: u64) -> Result<RecordHeader> {
    file.seek(SeekFrom::Start(offset))?;
    let mut raw = [0u8; RECORD_HEADER_LEN as usize];
    file.read_exact(&mut raw)?;
    decode_record_header(&raw)
}

fn read_record_metadata(file: &mut File, entry: IndexEntry) -> Result<BodyMetadata> {
    let header = read_record_header(file, entry.record_offset)?;
    let expected_len = RECORD_HEADER_LEN + header.metadata_len + header.payload_len;
    if entry.record_len != expected_len {
        return Err(LarError::Corrupt("index record length mismatch".into()));
    }
    let mut bytes = vec![0u8; header.metadata_len as usize];
    file.read_exact(&mut bytes)?;
    if crc32fast::hash(&bytes) != header.metadata_crc {
        return Err(LarError::Corrupt(
            "record metadata checksum mismatch".into(),
        ));
    }
    let metadata: BodyMetadata = serde_json::from_slice(&bytes)?;
    if metadata.trace_id.is_empty() || metadata.body_kind.is_empty() {
        return Err(LarError::Corrupt("empty trace or body kind".into()));
    }
    Ok(metadata)
}

fn stream_record(
    file: &mut File,
    entry: IndexEntry,
    max_bytes: u64,
    output: &mut impl Write,
) -> Result<BodyMetadata> {
    let metadata = read_record_metadata(file, entry)?;
    let header = read_record_header(file, entry.record_offset)?;
    if header.payload_len > max_bytes {
        return Err(LarError::Limit(format!(
            "body is {} bytes; requested maximum is {max_bytes}",
            header.payload_len
        )));
    }
    file.seek(SeekFrom::Start(
        entry.record_offset + RECORD_HEADER_LEN + header.metadata_len,
    ))?;
    let mut remaining = header.payload_len;
    let mut crc = Crc32::new();
    let mut sha = Sha256::new();
    let mut buffer = [0u8; IO_CHUNK];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..chunk])?;
        crc.update(&buffer[..chunk]);
        sha.update(&buffer[..chunk]);
        output.write_all(&buffer[..chunk])?;
        remaining -= chunk as u64;
    }
    if crc.finalize() != header.payload_crc {
        return Err(LarError::Corrupt("record payload checksum mismatch".into()));
    }
    if hex(&sha.finalize()) != metadata.sha256 {
        return Err(LarError::Corrupt("record payload SHA-256 mismatch".into()));
    }
    Ok(metadata)
}

fn verify_record_crc(file: &mut File, entry: IndexEntry, header: RecordHeader) -> Result<()> {
    stream_record(file, entry, header.payload_len, &mut io::sink()).map(|_| ())
}

fn encode_index_entry(entry: IndexEntry) -> [u8; INDEX_ENTRY_LEN as usize] {
    let mut raw = [0u8; INDEX_ENTRY_LEN as usize];
    raw[..32].copy_from_slice(&entry.key);
    raw[32..40].copy_from_slice(&entry.record_offset.to_le_bytes());
    raw[40..48].copy_from_slice(&entry.record_len.to_le_bytes());
    raw
}

fn encode_index_header(count: u64) -> [u8; INDEX_HEADER_LEN as usize] {
    let mut raw = [0u8; INDEX_HEADER_LEN as usize];
    raw[..4].copy_from_slice(INDEX_MAGIC);
    raw[4..6].copy_from_slice(&VERSION.to_le_bytes());
    raw[6..8].copy_from_slice(&(INDEX_HEADER_LEN as u16).to_le_bytes());
    raw[8..16].copy_from_slice(&count.to_le_bytes());
    let checksum = crc32fast::hash(&raw[..16]);
    raw[16..20].copy_from_slice(&checksum.to_le_bytes());
    raw
}

fn validate_index_header(file: &mut File, offset: u64, expected_count: u64) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut raw = [0u8; INDEX_HEADER_LEN as usize];
    file.read_exact(&mut raw)?;
    if &raw[..4] != INDEX_MAGIC
        || u16::from_le_bytes([raw[4], raw[5]]) != VERSION
        || u16::from_le_bytes([raw[6], raw[7]]) as u64 != INDEX_HEADER_LEN
        || u64::from_le_bytes(raw[8..16].try_into().unwrap()) != expected_count
        || u32::from_le_bytes(raw[16..20].try_into().unwrap()) != crc32fast::hash(&raw[..16])
    {
        return Err(LarError::Corrupt("invalid footer index header".into()));
    }
    Ok(())
}

fn decode_index_entry(raw: &[u8; INDEX_ENTRY_LEN as usize]) -> IndexEntry {
    IndexEntry {
        key: raw[..32].try_into().unwrap(),
        record_offset: u64::from_le_bytes(raw[32..40].try_into().unwrap()),
        record_len: u64::from_le_bytes(raw[40..48].try_into().unwrap()),
    }
}

fn read_index_entry(file: &mut File, index_offset: u64, position: u64) -> Result<IndexEntry> {
    let offset = index_offset
        .checked_add(
            position
                .checked_mul(INDEX_ENTRY_LEN)
                .ok_or_else(|| LarError::Corrupt("index offset multiplication overflow".into()))?,
        )
        .ok_or_else(|| LarError::Corrupt("index offset overflow".into()))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut raw = [0u8; INDEX_ENTRY_LEN as usize];
    file.read_exact(&mut raw)?;
    Ok(decode_index_entry(&raw))
}

fn read_all_index_entries(file: &mut File, footer: Footer) -> Result<Vec<IndexEntry>> {
    if footer.index_count > MAX_INDEX_ENTRIES {
        return Err(LarError::Limit(format!(
            "index exceeds {MAX_INDEX_ENTRIES} records"
        )));
    }
    let mut entries = Vec::with_capacity(footer.index_count as usize);
    let mut previous: Option<[u8; 32]> = None;
    for position in 0..footer.index_count {
        let entry = read_index_entry(file, footer.index_offset + INDEX_HEADER_LEN, position)?;
        if previous.is_some_and(|key| key >= entry.key) {
            return Err(LarError::Corrupt(
                "footer index is not strictly sorted".into(),
            ));
        }
        if entry.record_offset < FILE_HEADER_LEN
            || entry
                .record_offset
                .checked_add(entry.record_len)
                .is_none_or(|end| end > footer.records_end)
        {
            return Err(LarError::Corrupt(
                "footer index points outside records".into(),
            ));
        }
        previous = Some(entry.key);
        entries.push(entry);
    }
    Ok(entries)
}

fn encode_footer(footer: Footer, index_crc: u32) -> [u8; FOOTER_LEN as usize] {
    let mut raw = [0u8; FOOTER_LEN as usize];
    raw[..4].copy_from_slice(FOOTER_MAGIC);
    raw[4..6].copy_from_slice(&VERSION.to_le_bytes());
    raw[6..8].copy_from_slice(&(FOOTER_LEN as u16).to_le_bytes());
    raw[8..16].copy_from_slice(&footer.index_offset.to_le_bytes());
    raw[16..24].copy_from_slice(&footer.index_count.to_le_bytes());
    raw[24..32].copy_from_slice(&footer.records_end.to_le_bytes());
    raw[32..36].copy_from_slice(&index_crc.to_le_bytes());
    let footer_crc = crc32fast::hash(&raw[..44]);
    raw[44..48].copy_from_slice(&footer_crc.to_le_bytes());
    raw
}

fn read_footer(file: &mut File, file_len: u64) -> Result<Option<Footer>> {
    if file_len < FILE_HEADER_LEN + FOOTER_LEN {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(file_len - FOOTER_LEN))?;
    let mut raw = [0u8; FOOTER_LEN as usize];
    file.read_exact(&mut raw)?;
    if &raw[..4] != FOOTER_MAGIC {
        return Ok(None);
    }
    if u16::from_le_bytes([raw[4], raw[5]]) != VERSION
        || u16::from_le_bytes([raw[6], raw[7]]) as u64 != FOOTER_LEN
    {
        return Err(LarError::Corrupt(
            "unsupported footer version or size".into(),
        ));
    }
    let expected_crc = u32::from_le_bytes(raw[44..48].try_into().unwrap());
    if crc32fast::hash(&raw[..44]) != expected_crc {
        return Err(LarError::Corrupt("footer checksum mismatch".into()));
    }
    let footer = Footer {
        index_offset: u64::from_le_bytes(raw[8..16].try_into().unwrap()),
        index_count: u64::from_le_bytes(raw[16..24].try_into().unwrap()),
        records_end: u64::from_le_bytes(raw[24..32].try_into().unwrap()),
    };
    if footer.index_count > MAX_INDEX_ENTRIES {
        return Err(LarError::Limit(format!(
            "index exceeds {MAX_INDEX_ENTRIES} records"
        )));
    }
    let index_len = footer
        .index_count
        .checked_mul(INDEX_ENTRY_LEN)
        .ok_or_else(|| LarError::Corrupt("index length overflow".into()))?;
    if footer.records_end != footer.index_offset
        || footer
            .index_offset
            .checked_add(INDEX_HEADER_LEN)
            .and_then(|end| end.checked_add(index_len))
            .and_then(|end| end.checked_add(FOOTER_LEN))
            != Some(file_len)
    {
        return Err(LarError::Corrupt("footer index geometry is invalid".into()));
    }
    validate_index_header(file, footer.index_offset, footer.index_count)?;
    let expected_index_crc = u32::from_le_bytes(raw[32..36].try_into().unwrap());
    file.seek(SeekFrom::Start(footer.index_offset + INDEX_HEADER_LEN))?;
    let mut crc = Crc32::new();
    let mut remaining = index_len;
    let mut buffer = [0u8; IO_CHUNK];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..chunk])?;
        crc.update(&buffer[..chunk]);
        remaining -= chunk as u64;
    }
    if crc.finalize() != expected_index_crc {
        return Err(LarError::Corrupt("footer index checksum mismatch".into()));
    }
    Ok(Some(footer))
}

fn index_key(trace_id: &str, body_kind: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(trace_id.as_bytes());
    digest.update([0]);
    digest.update(body_kind.as_bytes());
    digest.finalize().into()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex(&digest.finalize())
}

fn digest_reader(mut reader: impl Read, max_bytes: u64) -> Result<String> {
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; IO_CHUNK];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > max_bytes {
            return Err(LarError::Limit(format!(
                "body exceeds configured maximum of {max_bytes} bytes"
            )));
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

struct DigestWriter<'a>(&'a mut Sha256);

impl Write for DigestWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn refs_digest(refs: &[LegacyBodyRef]) -> String {
    let mut digest = Sha256::new();
    for reference in refs {
        digest.update(reference.trace_id.as_bytes());
        digest.update([0]);
        digest.update(reference.body_kind.as_bytes());
        digest.update([0]);
        digest.update(reference.path.to_string_lossy().as_bytes());
        digest.update([0xff]);
    }
    hex(&digest.finalize())
}

fn write_state_atomic(path: &Path, state: &ImportState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temporary_path(path, "checkpoint");
    let result = (|| {
        let mut file = BufWriter::new(File::create(&temp)?);
        serde_json::to_writer_pretty(&mut file, state)?;
        file.flush()?;
        file.get_ref().sync_all()?;
        fs::rename(&temp, path)?;
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn temporary_path(path: &Path, purpose: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("archive");
    path.with_file_name(format!(
        ".{name}.{purpose}.{}.{}",
        std::process::id(),
        counter
    ))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0xf) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use tempfile::tempdir;

    fn gzip(path: &Path, bytes: &[u8]) {
        let mut encoder = GzEncoder::new(File::create(path).unwrap(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap().sync_all().unwrap();
    }

    #[test]
    fn random_access_reads_only_requested_trace_and_kind() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("day.lar");
        let mut writer = ArchiveWriter::open(&path).unwrap();
        writer
            .append_body("trace-a", "request", b"request-a", Vec::new())
            .unwrap();
        writer
            .append_body(
                "trace-a",
                "response",
                b"response-a",
                vec![SseEvent {
                    byte_offset: 4,
                    ms_delta: 12,
                }],
            )
            .unwrap();
        writer
            .append_body("trace-b", "request", b"request-b", Vec::new())
            .unwrap();
        writer.finish().unwrap();

        let mut reader = ArchiveReader::open(&path).unwrap();
        assert_eq!(reader.resident_index_bytes(), 0);
        assert_eq!(
            reader.read_body("trace-a", "response", 32).unwrap(),
            b"response-a"
        );
        assert_eq!(
            reader
                .metadata("trace-a", "response")
                .unwrap()
                .sse_timing
                .len(),
            1
        );
        assert!(matches!(
            reader.read_body("trace-a", "request", 2),
            Err(LarError::Limit(_))
        ));
    }

    #[test]
    fn interrupted_tail_is_recovered_and_truncated_before_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("crash.lar");
        let mut writer = ArchiveWriter::open(&path).unwrap();
        writer
            .append_body("trace-a", "request", b"complete", Vec::new())
            .unwrap();
        writer.finish().unwrap();

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let len = file.metadata().unwrap().len();
        let footer = read_footer(&mut file, len).unwrap().unwrap();
        file.set_len(footer.records_end).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        let torn = encode_record_header(100, 1000, 1, 2);
        file.write_all(&torn).unwrap();
        file.write_all(b"partial").unwrap();
        file.sync_all().unwrap();
        drop(file);

        let mut writer = ArchiveWriter::open(&path).unwrap();
        assert_eq!(writer.len(), 1);
        writer
            .append_body("trace-b", "response", b"after-recovery", Vec::new())
            .unwrap();
        writer.finish().unwrap();
        let mut reader = ArchiveReader::open(&path).unwrap();
        assert_eq!(reader.len(), 2);
        assert_eq!(
            reader.read_body("trace-a", "request", 100).unwrap(),
            b"complete"
        );
        assert_eq!(
            reader.read_body("trace-b", "response", 100).unwrap(),
            b"after-recovery"
        );
    }

    #[test]
    fn interrupted_footer_index_is_rebuilt_from_complete_records() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("index-crash.lar");
        let mut writer = ArchiveWriter::open(&path).unwrap();
        writer
            .append_body("trace-a", "request", b"one", Vec::new())
            .unwrap();
        writer
            .append_body("trace-b", "response", b"two", Vec::new())
            .unwrap();
        writer.finish().unwrap();

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let len = file.metadata().unwrap().len();
        let footer = read_footer(&mut file, len).unwrap().unwrap();
        // Preserve the index marker and only part of the first entry, exactly
        // as a power loss during close could do.
        file.set_len(footer.records_end + INDEX_HEADER_LEN + 10)
            .unwrap();
        file.sync_all().unwrap();
        drop(file);

        let writer = ArchiveWriter::open(&path).unwrap();
        assert_eq!(writer.len(), 2);
        writer.finish().unwrap();
        let mut reader = ArchiveReader::open(&path).unwrap();
        assert_eq!(reader.len(), 2);
        assert_eq!(reader.read_body("trace-b", "response", 10).unwrap(), b"two");
    }

    #[test]
    fn detects_payload_and_index_corruption() {
        let dir = tempdir().unwrap();
        let payload_path = dir.path().join("payload.lar");
        let mut writer = ArchiveWriter::open(&payload_path).unwrap();
        writer
            .append_body("trace", "response", b"abcdef", Vec::new())
            .unwrap();
        writer.finish().unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&payload_path)
            .unwrap();
        let len = file.metadata().unwrap().len();
        let footer = read_footer(&mut file, len).unwrap().unwrap();
        let entry = read_index_entry(&mut file, footer.index_offset + INDEX_HEADER_LEN, 0).unwrap();
        let header = read_record_header(&mut file, entry.record_offset).unwrap();
        let payload_offset = entry.record_offset + RECORD_HEADER_LEN + header.metadata_len;
        file.seek(SeekFrom::Start(payload_offset + 2)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut reader = ArchiveReader::open(&payload_path).unwrap();
        assert!(matches!(
            reader.read_body("trace", "response", 100),
            Err(LarError::Corrupt(_))
        ));

        let index_path = dir.path().join("index.lar");
        let mut writer = ArchiveWriter::open(&index_path).unwrap();
        writer
            .append_body("x", "request", b"x", Vec::new())
            .unwrap();
        writer.finish().unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&index_path)
            .unwrap();
        let len = file.metadata().unwrap().len();
        file.seek(SeekFrom::Start(len - FOOTER_LEN - 1)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(matches!(
            ArchiveReader::open(index_path),
            Err(LarError::Corrupt(_))
        ));
    }

    #[test]
    fn migration_resumes_idempotently_and_preserves_originals_until_validation() {
        let dir = tempdir().unwrap();
        let bodies = dir.path().join("bodies/2026-07-21");
        fs::create_dir_all(&bodies).unwrap();
        gzip(&bodies.join("a.request.json.gz"), br#"{"prompt":"one"}"#);
        gzip(&bodies.join("a.response.body.gz"), br#"{"answer":"two"}"#);
        gzip(&bodies.join("b.request.json.gz"), br#"{"prompt":"three"}"#);
        let refs = discover_legacy_bodies(dir.path().join("bodies")).unwrap();
        assert_eq!(refs.len(), 3);
        let archive = dir.path().join("migrated.lar");
        let checkpoint = dir.path().join("migrate.json");

        let first = import_legacy(
            &refs,
            &archive,
            &checkpoint,
            ImportOptions {
                max_entries_this_run: Some(1),
                ..ImportOptions::default()
            },
        )
        .unwrap();
        assert_eq!(first.imported, 1);
        assert!(!first.complete);
        assert!(refs.iter().all(|reference| reference.path.exists()));

        let second = import_legacy(&refs, &archive, &checkpoint, ImportOptions::default()).unwrap();
        assert_eq!(second.imported, 2);
        assert!(second.complete && second.validated && second.originals_preserved);
        assert!(refs.iter().all(|reference| reference.path.exists()));

        let third = import_legacy(&refs, &archive, &checkpoint, ImportOptions::default()).unwrap();
        assert_eq!(third.imported, 0);
        assert!(third.complete && third.validated);
        let reader = ArchiveReader::open(&archive).unwrap();
        assert_eq!(reader.len(), 3);
    }

    #[test]
    fn sanitized_fixture_reopens_and_replays_without_credentials() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source.lar");
        let output = dir.path().join("fixture.lar");
        let mut writer = ArchiveWriter::open(&source).unwrap();
        writer
            .append_body(
                "demo",
                "request",
                br#"{"model":"fable-5","authorization":"Bearer secret","nested":{"api_key":"sk-secret"}}"#,
                Vec::new(),
            )
            .unwrap();
        writer
            .append_body("demo", "response", br#"{"answer":"hello"}"#, Vec::new())
            .unwrap();
        writer.finish().unwrap();

        let report = export_sanitized_fixture(
            &source,
            &output,
            &[
                BodyKey::new("demo", "request"),
                BodyKey::new("demo", "response"),
            ],
            1024,
        )
        .unwrap();
        assert_eq!(report.bodies, 2);
        assert_eq!(report.sanitized_bodies, 1);
        let mut fixture = ArchiveReader::open(&output).unwrap();
        let request = fixture.read_body("demo", "request", 1024).unwrap();
        let request = String::from_utf8(request).unwrap();
        assert!(!request.contains("secret"));
        assert!(request.contains("[REDACTED]"));
        assert!(fixture.metadata("demo", "request").unwrap().sanitized);
        assert_eq!(
            fixture.read_body("demo", "response", 1024).unwrap(),
            br#"{"answer":"hello"}"#
        );
    }

    #[test]
    fn fixture_sanitizer_redacts_json_sse_and_rejects_opaque_secrets() {
        let sse = b"event: message\ndata: {\"type\":\"delta\",\"client_secret\":\"hidden\"}\n\ndata: [DONE]\n";
        let (sanitized, changed) = sanitize_body(sse).unwrap();
        let sanitized = String::from_utf8(sanitized).unwrap();
        assert!(changed);
        assert!(sanitized.contains("[REDACTED]"));
        assert!(!sanitized.contains("hidden"));

        let opaque = b"raw provider frame with Authorization: Bearer top-secret";
        assert!(matches!(
            sanitize_body(opaque),
            Err(LarError::UnsafeFixture(_))
        ));
        let embedded = br#"{"message":"copy sk-live-secret into the tool"}"#;
        assert!(matches!(
            sanitize_body(embedded),
            Err(LarError::UnsafeFixture(_))
        ));
    }

    #[test]
    fn large_footer_index_remains_lazy_and_random_accessible() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("large.lar");
        let mut writer = ArchiveWriter::open(&path).unwrap();
        const COUNT: usize = 55_000;
        for index in 0..COUNT {
            writer
                .append_body(&format!("trace-{index:05}"), "request", &[], Vec::new())
                .unwrap();
        }
        writer.finish().unwrap();

        let mut reader = ArchiveReader::open(&path).unwrap();
        assert_eq!(reader.len(), COUNT as u64);
        assert_eq!(reader.resident_index_bytes(), 0);
        assert_eq!(reader.read_body("trace-54321", "request", 1).unwrap(), b"");
        assert_eq!(reader.list(10_000, 25).unwrap().len(), 25);
        assert_eq!(reader.resident_index_bytes(), 0);
    }

    #[test]
    fn oversized_recovery_index_is_rejected_before_allocation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("oversized-index.lar");
        let mut file = File::create(&path).unwrap();
        write_file_header(&mut file).unwrap();
        file.write_all(&encode_footer(
            Footer {
                index_offset: FILE_HEADER_LEN,
                index_count: MAX_INDEX_ENTRIES + 1,
                records_end: FILE_HEADER_LEN,
            },
            0,
        ))
        .unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(matches!(ArchiveReader::open(path), Err(LarError::Limit(_))));
    }
}
