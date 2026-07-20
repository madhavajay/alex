//! Bounded, cursor-paged replay views for captured response streams.
//!
//! A page reads only its selected manifest ranges. Absolute observed deltas
//! are returned to the client; the server never sleeps or applies replay speed.

use std::fmt;
use std::fs::File;

use alex_lar::{ArchiveReader, Limits, ManifestId, StreamFrameKind, StreamIndexId, StreamParser};
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;

use crate::{lar_archive_ops::resolved_catalog_path, LarArchiveUnavailableError, Store};

pub const DEFAULT_STREAM_REPLAY_PAGE_LIMIT: usize = 100;
pub const MAX_STREAM_REPLAY_PAGE_LIMIT: usize = 500;
pub const DEFAULT_STREAM_REPLAY_PAGE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_STREAM_REPLAY_PAGE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LarStreamReplaySource {
    ObservedReads,
    ParsedFrames,
}

impl LarStreamReplaySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObservedReads => "observed_reads",
            Self::ParsedFrames => "parsed_frames",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStreamReplayPageOptions {
    pub source: LarStreamReplaySource,
    pub cursor: u64,
    pub limit: usize,
    pub max_page_bytes: u64,
}

impl Default for LarStreamReplayPageOptions {
    fn default() -> Self {
        Self {
            source: LarStreamReplaySource::ObservedReads,
            cursor: 0,
            limit: DEFAULT_STREAM_REPLAY_PAGE_LIMIT,
            max_page_bytes: DEFAULT_STREAM_REPLAY_PAGE_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStreamReplayPageEvent {
    pub index: u64,
    pub byte_offset: u64,
    pub byte_length: u64,
    pub observed_delta_ns: u64,
    pub parser: Option<String>,
    pub frame_kind: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStreamReplayPage {
    pub trace_id: String,
    pub stage_id: String,
    pub stage_kind: String,
    pub source: LarStreamReplaySource,
    pub cursor: u64,
    pub next_cursor: Option<u64>,
    pub total_events: u64,
    pub page_bytes: u64,
    pub stream_index_id: String,
    pub raw_body_manifest_id: String,
    pub archive_file_uuid: String,
    pub archive_state: String,
    pub events: Vec<LarStreamReplayPageEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStreamReplayError {
    code: &'static str,
    message: String,
}

impl LarStreamReplayError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for LarStreamReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LarStreamReplayError {}

#[derive(Clone)]
struct StageReplayRecord {
    stage_kind: String,
    manifest_id: String,
    stream_index_id: String,
    file_uuid: String,
    path: String,
    file_state: String,
}

#[derive(Clone)]
struct ReplayRange {
    byte_offset: u64,
    byte_length: u64,
    observed_delta_ns: u64,
    parser: Option<String>,
    frame_kind: Option<String>,
}

impl Store {
    pub fn lar_stream_replay_page(
        &self,
        trace_id: &str,
        stage_id: &str,
        options: &LarStreamReplayPageOptions,
    ) -> Result<LarStreamReplayPage> {
        validate_options(options)?;
        let stage = self.resolve_replay_stage(trace_id, stage_id)?;
        let stage_path = resolved_catalog_path(&self.data_dir, &stage.path);
        let file = open_available_archive(&stage.file_uuid, &stage.file_state, &stage_path)?;
        let mut reader = ArchiveReader::open(file, Limits::default()).map_err(|error| {
            LarStreamReplayError::new(
                "replay_invalid_archive",
                format!("opening replay archive failed: {error}"),
            )
        })?;
        let stream_id = StreamIndexId(parse_hex_id(&stage.stream_index_id, "stream index")?);
        let manifest_id: ManifestId = stage.manifest_id.parse().map_err(|error| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("stage has invalid response manifest ID: {error}"),
            )
        })?;
        let index = reader.stream_index(&stream_id).cloned().ok_or_else(|| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!(
                    "stage {stage_id} references stream index {} absent from archive {}",
                    stage.stream_index_id, stage.file_uuid
                ),
            )
        })?;
        if index.raw_body_manifest_id != manifest_id {
            return Err(LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("stage {stage_id} stream index references a different raw body"),
            )
            .into());
        }

        let ranges = match options.source {
            LarStreamReplaySource::ObservedReads => index
                .reads
                .iter()
                .map(|read| ReplayRange {
                    byte_offset: read.byte_offset,
                    byte_length: read.byte_length,
                    observed_delta_ns: read.delta_from_first_byte_ns,
                    parser: None,
                    frame_kind: None,
                })
                .collect::<Vec<_>>(),
            LarStreamReplaySource::ParsedFrames => index
                .frames
                .iter()
                .map(|frame| ReplayRange {
                    byte_offset: frame.byte_offset,
                    byte_length: frame.byte_length,
                    observed_delta_ns: frame.delta_from_first_byte_ns,
                    parser: Some(parser_name(frame.parser)),
                    frame_kind: Some(frame_kind_name(frame.frame_kind)),
                })
                .collect::<Vec<_>>(),
        };
        let total_events = ranges.len() as u64;
        if options.cursor > total_events {
            return Err(LarStreamReplayError::new(
                "replay_cursor_out_of_range",
                format!(
                    "replay cursor {} exceeds {total_events} available events",
                    options.cursor
                ),
            )
            .into());
        }
        let start = usize::try_from(options.cursor).map_err(|_| {
            LarStreamReplayError::new(
                "replay_cursor_out_of_range",
                "replay cursor exceeds this platform's address space",
            )
        })?;
        let mut selected = Vec::new();
        let mut page_bytes = 0u64;
        for range in ranges.iter().skip(start).take(options.limit) {
            if range.byte_length > options.max_page_bytes {
                if selected.is_empty() {
                    return Err(LarStreamReplayError::new(
                        "replay_event_too_large",
                        format!(
                            "replay event is {} bytes; page byte limit is {}",
                            range.byte_length, options.max_page_bytes
                        ),
                    )
                    .into());
                }
                break;
            }
            let next_bytes = page_bytes.checked_add(range.byte_length).ok_or_else(|| {
                LarStreamReplayError::new(
                    "replay_invalid_request",
                    "replay page byte count overflow",
                )
            })?;
            if next_bytes > options.max_page_bytes {
                break;
            }
            page_bytes = next_bytes;
            selected.push(range.clone());
        }

        let byte_ranges = selected
            .iter()
            .map(|range| (range.byte_offset, range.byte_length))
            .collect::<Vec<_>>();
        let read_started = std::time::Instant::now();
        let payloads = if byte_ranges.is_empty() {
            Vec::new()
        } else if reader.manifest(&manifest_id).is_some() {
            reader
                .read_body_ranges(&manifest_id, &byte_ranges)
                .map_err(|error| {
                    LarStreamReplayError::new(
                        "replay_invalid_archive",
                        format!("reading replay body ranges failed: {error}"),
                    )
                })?
        } else {
            self.read_lar_manifest_ranges(&stage.manifest_id, &byte_ranges)?
        };
        if !byte_ranges.is_empty() {
            let elapsed = read_started.elapsed();
            let bytes = payloads.iter().fold(0_u64, |total, payload| {
                total.saturating_add(payload.len() as u64)
            });
            self.lar_runtime_metrics.record_read(
                bytes,
                (bytes > 0).then_some(elapsed),
                elapsed,
                false,
            );
        }
        let events = selected
            .into_iter()
            .zip(payloads)
            .enumerate()
            .map(|(offset, (range, bytes))| LarStreamReplayPageEvent {
                index: options.cursor + offset as u64,
                byte_offset: range.byte_offset,
                byte_length: range.byte_length,
                observed_delta_ns: range.observed_delta_ns,
                parser: range.parser,
                frame_kind: range.frame_kind,
                bytes,
            })
            .collect::<Vec<_>>();
        let consumed = events.len() as u64;
        let after = options.cursor + consumed;
        let next_cursor = (after < total_events).then_some(after);
        Ok(LarStreamReplayPage {
            trace_id: trace_id.to_string(),
            stage_id: stage_id.to_string(),
            stage_kind: stage.stage_kind,
            source: options.source,
            cursor: options.cursor,
            next_cursor,
            total_events,
            page_bytes,
            stream_index_id: stage.stream_index_id,
            raw_body_manifest_id: stage.manifest_id,
            archive_file_uuid: stage.file_uuid,
            archive_state: stage.file_state,
            events,
        })
    }

    fn resolve_replay_stage(&self, trace_id: &str, stage_id: &str) -> Result<StageReplayRecord> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT s.kind, s.response_body_manifest_ref, s.stream_index_ref,
                        s.file_uuid, f.path, f.state
                 FROM lar_stage_records s
                 LEFT JOIN lar_files f ON f.file_uuid=s.file_uuid
                 WHERE s.trace_id=?1 AND s.stage_id=?2",
                rusqlite::params![trace_id, stage_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((stage_kind, manifest_id, stream_index_id, file_uuid, path, file_state)) = row
        else {
            return Err(LarStreamReplayError::new(
                "replay_stage_not_found",
                format!("trace {trace_id} has no stage {stage_id}"),
            )
            .into());
        };
        let manifest_id = manifest_id.ok_or_else(|| {
            LarStreamReplayError::new(
                "replay_not_captured",
                format!("stage {stage_id} has no captured response body"),
            )
        })?;
        let stream_index_id = stream_index_id.ok_or_else(|| {
            LarStreamReplayError::new(
                "replay_not_captured",
                format!("stage {stage_id} has no captured stream index"),
            )
        })?;
        let file_uuid = file_uuid.ok_or_else(|| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("stage {stage_id} has no archive file reference"),
            )
        })?;
        let path = path.ok_or_else(|| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("stage {stage_id} archive file is absent from the catalog"),
            )
        })?;
        let file_state = file_state.ok_or_else(|| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("stage {stage_id} archive state is absent from the catalog"),
            )
        })?;
        Ok(StageReplayRecord {
            stage_kind,
            manifest_id,
            stream_index_id,
            file_uuid,
            path,
            file_state,
        })
    }

    fn read_lar_manifest_ranges(
        &self,
        manifest_id: &str,
        ranges: &[(u64, u64)],
    ) -> Result<Vec<Vec<u8>>> {
        if let Some(bytes) = self.read_catalog_manifest_ranges(manifest_id, ranges)? {
            return Ok(bytes);
        }
        let location: Option<(String, String, String)> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT f.path, f.file_uuid, f.state FROM lar_manifests m
                 JOIN lar_files f ON f.file_uuid=m.file_uuid
                 WHERE m.manifest_id=?1 AND m.state='ready'",
                [manifest_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
        };
        let Some((path, file_uuid, file_state)) = location else {
            return Err(LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("replay manifest {manifest_id} is absent from the catalog"),
            )
            .into());
        };
        let path = resolved_catalog_path(&self.data_dir, &path);
        let file = open_available_archive(&file_uuid, &file_state, &path)?;
        let mut reader = ArchiveReader::open(file, Limits::default()).map_err(|error| {
            LarStreamReplayError::new(
                "replay_invalid_archive",
                format!("opening replay body archive failed: {error}"),
            )
        })?;
        let manifest_id: ManifestId = manifest_id.parse().map_err(|error| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("replay manifest ID is invalid: {error}"),
            )
        })?;
        reader
            .read_body_ranges(&manifest_id, ranges)
            .map_err(|error| {
                LarStreamReplayError::new(
                    "replay_invalid_archive",
                    format!("reading replay body ranges failed: {error}"),
                )
                .into()
            })
    }
}

fn validate_options(options: &LarStreamReplayPageOptions) -> Result<()> {
    if options.limit == 0 || options.limit > MAX_STREAM_REPLAY_PAGE_LIMIT {
        return Err(LarStreamReplayError::new(
            "replay_invalid_request",
            format!("replay page limit must be 1..={MAX_STREAM_REPLAY_PAGE_LIMIT}"),
        )
        .into());
    }
    if options.max_page_bytes == 0 || options.max_page_bytes > MAX_STREAM_REPLAY_PAGE_BYTES {
        return Err(LarStreamReplayError::new(
            "replay_invalid_request",
            format!("replay page byte limit must be 1..={MAX_STREAM_REPLAY_PAGE_BYTES}"),
        )
        .into());
    }
    Ok(())
}

fn open_available_archive(file_uuid: &str, state: &str, path: &std::path::Path) -> Result<File> {
    if !matches!(state, "active" | "sealed") {
        return Err(LarArchiveUnavailableError::offline(file_uuid, path.to_string_lossy()).into());
    }
    match File::open(path) {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(LarArchiveUnavailableError::missing(file_uuid, path.to_string_lossy()).into())
        }
        Err(error) => {
            Err(error).with_context(|| format!("opening LAR replay archive {}", path.display()))
        }
    }
}

fn parser_name(parser: StreamParser) -> String {
    match parser {
        StreamParser::Opaque => "opaque".into(),
        StreamParser::Sse => "sse".into(),
        StreamParser::Ndjson => "ndjson".into(),
        StreamParser::Unknown(code) => format!("unknown:{code}"),
    }
}

fn frame_kind_name(kind: StreamFrameKind) -> String {
    match kind {
        StreamFrameKind::Opaque => "opaque".into(),
        StreamFrameKind::SseEvent => "sse_event".into(),
        StreamFrameKind::NdjsonRecord => "ndjson_record".into(),
        StreamFrameKind::Unknown(code) => format!("unknown:{code}"),
    }
}

fn parse_hex_id(value: &str, kind: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        return Err(LarStreamReplayError::new(
            "replay_catalog_invalid",
            format!("stage has invalid {kind} ID length"),
        )
        .into());
    }
    let mut bytes = [0u8; 32];
    for (index, output) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *output = u8::from_str_radix(&value[start..start + 2], 16).map_err(|_| {
            LarStreamReplayError::new(
                "replay_catalog_invalid",
                format!("stage has non-hex {kind} ID"),
            )
        })?;
    }
    Ok(bytes)
}
