//! Transitive, self-contained export of already-captured LAR exchanges.
//!
//! This path copies normalized records and reconstructs each referenced body
//! exactly once into the destination writer. It is deliberately separate from
//! interchange exporters: a `.lar` export must not collapse retries, Dario
//! stages, upstream responses, stream timing, or conversation graph records
//! into the three columns available in the legacy `traces` table.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;

use alex_lar::{
    ArchiveReader, ArchiveWriter, ArtifactRangeRef, ConversationEntry, ConversationEntryId,
    Exchange, ExchangeMetadataData, Generation, GenerationId, HeaderAtom, HeaderBlockId,
    HeaderFidelity, Limits, ManifestId, ParsedFrame, Stage, StageData, StreamIndex, StreamIndexId,
    StreamRead, TurnView,
};
use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use crate::{
    archive_body_path, lar_archive_ops::resolved_catalog_path, sqlite_row_json, Store,
    BACKUP_TRACE_COLS, BODY_TMP_COUNTER,
};

/// Stable cursor for deterministic, bounded trace-export selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarExportTraceCursor {
    pub ts_request_ms: i64,
    pub trace_id: String,
    pub max_rowid: i64,
}

/// One canonical manifest descriptor. Bytes are streamed separately by ID.
///
/// Interchange callers consume one trace at a time, so this never retains
/// bodies belonging to other selected traces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarInterchangeBody {
    pub manifest_id: String,
    pub total_length: u64,
    pub whole_body_blake3: String,
    pub media_type: Option<Vec<u8>>,
    pub content_encoding: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarInterchangeHeaderBlock {
    pub block_id: String,
    pub fidelity: &'static str,
    pub atoms: Vec<HeaderAtom>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarInterchangeStream {
    pub stream_index_id: String,
    pub raw_body_manifest_id: String,
    pub reads: Vec<StreamRead>,
    pub frames: Vec<ParsedFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarInterchangeStage {
    pub stage_id: String,
    pub record_id: String,
    pub ordinal: u64,
    pub capture_sequence: u64,
    pub kind: String,
    pub data: StageData,
    pub tool_id: Option<String>,
    pub tool_phase: Option<String>,
    pub supplement_trace_id: Option<String>,
    pub supplement_exchange_id: Option<String>,
}

/// Exact transport graph for one cataloged LAR exchange.
///
/// IDs remain content-addressed references. Header atoms preserve order,
/// duplicates, original name bytes, and capture flags; stages preserve every
/// attempt/tool event; stream indexes retain read/frame timing and address the
/// same single-copy manifest represented in `bodies`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarInterchangeTrace {
    pub exchange_id: String,
    pub trace_id: Vec<u8>,
    pub session_id: Option<Vec<u8>>,
    pub run_id: Option<Vec<u8>>,
    pub parent_trace_id: Option<Vec<u8>>,
    pub capture_sequence: u64,
    pub wall_time_ns: u64,
    pub monotonic_delta_ns: Option<u64>,
    pub clock_id: Option<Vec<u8>>,
    pub metadata: Option<ExchangeMetadataData>,
    pub stages: Vec<LarInterchangeStage>,
    pub header_blocks: Vec<LarInterchangeHeaderBlock>,
    pub bodies: Vec<LarInterchangeBody>,
    pub streams: Vec<LarInterchangeStream>,
}

#[derive(Clone, Debug)]
struct CatalogInterchangeStage {
    occurrence_id: String,
    capture_sequence: u64,
    file_uuid: Option<String>,
    record_id: Option<String>,
    tool_id: Option<String>,
    tool_phase: Option<String>,
    supplement_trace_id: Option<String>,
    supplement_wall_time_ns: Option<u64>,
    supplement_exchange_id: Option<String>,
    references: CatalogStageReferences,
}

#[derive(Clone, Debug)]
struct CatalogStageReferences {
    request_headers_ref: Option<String>,
    request_body_manifest_ref: Option<String>,
    response_headers_ref: Option<String>,
    response_body_manifest_ref: Option<String>,
    trailers_ref: Option<String>,
    stream_index_ref: Option<String>,
}

impl Store {
    pub fn lar_trace_has_canonical_exchange(&self, trace_id: &str) -> Result<bool> {
        Ok(trace_archive_source(self, trace_id)?.is_some())
    }

    /// Read one deterministic page of trace metadata for an interchange
    /// export. The cursor order is stable across pages and does not hold the
    /// SQLite lock while callers reconstruct archive content.
    pub fn lar_export_trace_rows_page(
        &self,
        trace_id: Option<&str>,
        session_id: Option<&str>,
        after: Option<&LarExportTraceCursor>,
        through: &LarExportTraceCursor,
        limit: usize,
    ) -> Result<Vec<Value>> {
        if limit == 0 || limit > 1_024 {
            bail!("LAR export trace page limit must be between 1 and 1024");
        }
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM traces
              WHERE (?1 IS NULL OR id=?1)
                AND (?2 IS NULL OR session_id=?2)
                AND (?3 IS NULL OR ts_request_ms > ?3
                     OR (ts_request_ms=?3 AND id>?4))
                AND (ts_request_ms < ?5 OR (ts_request_ms=?5 AND id<=?6))
                AND rowid<=?7
              ORDER BY ts_request_ms, id LIMIT ?8",
            BACKUP_TRACE_COLS.join(", ")
        );
        let (after_ms, after_id) = after
            .map(|cursor| (Some(cursor.ts_request_ms), Some(cursor.trace_id.as_str())))
            .unwrap_or((None, None));
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                trace_id,
                session_id,
                after_ms,
                after_id,
                through.ts_request_ms,
                through.trace_id,
                through.max_rowid,
                limit as u64
            ],
            |row| sqlite_row_json(row, BACKUP_TRACE_COLS),
        )?;
        let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(conn);
        for row in &mut rows {
            for column in ["req_body_path", "upstream_req_body_path", "resp_body_path"] {
                archive_body_path(&self.data_dir, &mut row[column]);
            }
        }
        Ok(rows)
    }

    /// Freeze the inclusive high-water mark used by both export passes.
    pub fn lar_export_trace_upper_bound(
        &self,
        trace_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Option<LarExportTraceCursor>> {
        let conn = self.conn.lock().unwrap();
        let max_rowid: Option<i64> = conn.query_row(
            "SELECT MAX(rowid) FROM traces
              WHERE (?1 IS NULL OR id=?1) AND (?2 IS NULL OR session_id=?2)",
            params![trace_id, session_id],
            |row| row.get(0),
        )?;
        let Some(max_rowid) = max_rowid else {
            return Ok(None);
        };
        conn.query_row(
            "SELECT ts_request_ms, id FROM traces
              WHERE (?1 IS NULL OR id=?1) AND (?2 IS NULL OR session_id=?2)
                AND rowid<=?3
              ORDER BY ts_request_ms DESC, id DESC LIMIT 1",
            params![trace_id, session_id, max_rowid],
            |row| {
                Ok(LarExportTraceCursor {
                    ts_request_ms: row.get(0)?,
                    trace_id: row.get(1)?,
                    max_rowid,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Load the exact canonical exchange/stage graph for one trace.
    ///
    /// `None` means the trace is genuinely legacy-only. Once a trace has a
    /// cataloged exchange, missing/offline/corrupt archive data is an error and
    /// is never silently replaced by the legacy projection.
    pub fn lar_interchange_trace(&self, trace_id: &str) -> Result<Option<LarInterchangeTrace>> {
        let source_location = trace_archive_source(self, trace_id)?;
        let Some((file_uuid, catalog_path, state)) = source_location else {
            return Ok(None);
        };
        if !matches!(state.as_str(), "active" | "sealed") {
            bail!("LAR exchange {trace_id} is in offline archive {file_uuid}");
        }
        let path = resolved_catalog_path(&self.data_dir, &catalog_path);
        let file = File::open(&path)
            .with_context(|| format!("opening LAR exchange archive {}", path.display()))?;
        let source = ArchiveReader::open(file, Limits::default())
            .map_err(anyhow::Error::new)
            .with_context(|| format!("reading LAR exchange archive {file_uuid}"))?;
        let exchange = source
            .exchange_by_trace(trace_id.as_bytes())
            .cloned()
            .with_context(|| {
                format!("cataloged trace {trace_id} has no exchange in archive {file_uuid}")
            })?;

        let metadata = source
            .exchange_metadata(&exchange.id)
            .map(|record| record.data.clone());
        let stage_rows = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT s.stage_id, s.capture_sequence, s.file_uuid, s.record_id,
                        t.tool_id, t.phase, t.supplement_trace_id, t.wall_time_ns,
                        t.exchange_id, s.request_headers_ref,
                        s.request_body_manifest_ref, s.response_headers_ref,
                        s.response_body_manifest_ref, s.trailers_ref,
                        s.stream_index_ref
                   FROM lar_stage_records s
                   LEFT JOIN lar_timeline_supplements t ON t.stage_id=s.stage_id
                  WHERE s.trace_id=?1
                  ORDER BY s.capture_sequence, s.stage_id",
            )?;
            let rows = statement
                .query_map([trace_id], |row| {
                    Ok(CatalogInterchangeStage {
                        occurrence_id: row.get(0)?,
                        capture_sequence: row.get(1)?,
                        file_uuid: row.get(2)?,
                        record_id: row.get(3)?,
                        tool_id: row.get(4)?,
                        tool_phase: row.get(5)?,
                        supplement_trace_id: row.get(6)?,
                        supplement_wall_time_ns: row.get(7)?,
                        supplement_exchange_id: row.get(8)?,
                        references: CatalogStageReferences {
                            request_headers_ref: row.get(9)?,
                            request_body_manifest_ref: row.get(10)?,
                            response_headers_ref: row.get(11)?,
                            response_body_manifest_ref: row.get(12)?,
                            trailers_ref: row.get(13)?,
                            stream_index_ref: row.get(14)?,
                        },
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let stage_rows = if stage_rows.is_empty() {
            exchange
                .data
                .stages
                .iter()
                .enumerate()
                .map(|(ordinal, id)| {
                    let stage = source.stage(id);
                    CatalogInterchangeStage {
                        occurrence_id: id.to_string(),
                        capture_sequence: ordinal as u64,
                        file_uuid: Some(file_uuid.clone()),
                        record_id: Some(id.to_string()),
                        tool_id: None,
                        tool_phase: None,
                        supplement_trace_id: None,
                        supplement_wall_time_ns: None,
                        supplement_exchange_id: None,
                        references: CatalogStageReferences {
                            request_headers_ref: stage
                                .and_then(|stage| stage.data.request_headers_ref)
                                .map(|id| id.to_string()),
                            request_body_manifest_ref: stage
                                .and_then(|stage| stage.data.request_body_manifest_ref)
                                .map(|id| id.to_string()),
                            response_headers_ref: stage
                                .and_then(|stage| stage.data.response_headers_ref)
                                .map(|id| id.to_string()),
                            response_body_manifest_ref: stage
                                .and_then(|stage| stage.data.response_body_manifest_ref)
                                .map(|id| id.to_string()),
                            trailers_ref: stage
                                .and_then(|stage| stage.data.trailers_ref)
                                .map(|id| id.to_string()),
                            stream_index_ref: stage
                                .and_then(|stage| stage.data.stream_index_ref)
                                .map(|id| id.to_string()),
                        },
                    }
                })
                .collect::<Vec<_>>()
        } else {
            stage_rows
        };
        let (mut base_rows, mut stage_rows): (Vec<_>, Vec<_>) = stage_rows
            .into_iter()
            .partition(|row| row.tool_id.is_none());
        let mut ordered_rows = Vec::with_capacity(base_rows.len() + stage_rows.len());
        for stage_id in &exchange.data.stages {
            let wanted = stage_id.to_string();
            if let Some(index) = base_rows
                .iter()
                .position(|row| row.record_id.as_deref() == Some(wanted.as_str()))
            {
                ordered_rows.push(base_rows.remove(index));
            }
        }
        base_rows.sort_by(|left, right| {
            left.capture_sequence
                .cmp(&right.capture_sequence)
                .then_with(|| left.occurrence_id.cmp(&right.occurrence_id))
        });
        ordered_rows.extend(base_rows);
        stage_rows.sort_by(|left, right| {
            left.supplement_wall_time_ns
                .cmp(&right.supplement_wall_time_ns)
                .then_with(|| left.capture_sequence.cmp(&right.capture_sequence))
                .then_with(|| {
                    phase_order(left.tool_phase.as_deref())
                        .cmp(&phase_order(right.tool_phase.as_deref()))
                })
                .then_with(|| left.supplement_trace_id.cmp(&right.supplement_trace_id))
        });
        ordered_rows.extend(stage_rows);
        let stage_rows = ordered_rows;

        let mut readers = HashMap::new();
        readers.insert(file_uuid.clone(), source);
        let mut stages = Vec::with_capacity(stage_rows.len());
        let mut header_blocks = Vec::new();
        let mut seen_headers = HashSet::new();
        let mut manifest_ids = Vec::new();
        let mut seen_manifests = HashSet::new();
        let mut streams = Vec::new();
        let mut seen_streams = HashSet::new();

        for (ordinal, catalog_stage) in stage_rows.into_iter().enumerate() {
            let stage_file_uuid = catalog_stage
                .file_uuid
                .clone()
                .unwrap_or_else(|| file_uuid.clone());
            if !readers.contains_key(&stage_file_uuid) {
                let (stage_path, stage_state) = {
                    let conn = self.conn.lock().unwrap();
                    conn.query_row(
                        "SELECT path, state FROM lar_files WHERE file_uuid=?1",
                        [&stage_file_uuid],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .with_context(|| {
                        format!("locating stage archive {stage_file_uuid} for trace {trace_id}")
                    })?
                };
                if !matches!(stage_state.as_str(), "active" | "sealed") {
                    bail!("LAR stage for {trace_id} is in offline archive {stage_file_uuid}");
                }
                let stage_path = resolved_catalog_path(&self.data_dir, &stage_path);
                let reader = ArchiveReader::open(
                    File::open(&stage_path).with_context(|| {
                        format!("opening LAR stage archive {}", stage_path.display())
                    })?,
                    Limits::default(),
                )
                .map_err(anyhow::Error::new)
                .with_context(|| format!("reading LAR stage archive {stage_file_uuid}"))?;
                readers.insert(stage_file_uuid.clone(), reader);
            }
            let reader = readers
                .get(&stage_file_uuid)
                .expect("stage archive reader was inserted");
            let record_id = catalog_stage
                .record_id
                .clone()
                .unwrap_or_else(|| catalog_stage.occurrence_id.clone());
            let parsed_stage_id = alex_lar::StageId(parse_content_id(&record_id, "stage")?);
            let stage = reader.stage(&parsed_stage_id).cloned().with_context(|| {
                format!("exchange {trace_id} is missing stage record {record_id}")
            })?;
            // Catalog references are the retention visibility authority. The
            // immutable stage record can still contain a body reference after
            // bodies-only pruning and before repack; never resurrect it here.
            let mut stage_data = stage.data;
            overlay_catalog_stage_references(&mut stage_data, &catalog_stage.references)?;
            for id in [
                stage_data.request_headers_ref,
                stage_data.response_headers_ref,
                stage_data.trailers_ref,
            ]
            .into_iter()
            .flatten()
            {
                if seen_headers.insert(id) {
                    let block = reader.header_block(&id).with_context(|| {
                        format!("exchange {trace_id} references missing header block {id}")
                    })?;
                    header_blocks.push(LarInterchangeHeaderBlock {
                        block_id: id.to_string(),
                        fidelity: header_fidelity_name(block.fidelity),
                        atoms: block.atoms.clone(),
                    });
                }
            }
            for id in [
                stage_data.request_body_manifest_ref,
                stage_data.response_body_manifest_ref,
            ]
            .into_iter()
            .flatten()
            {
                if seen_manifests.insert(id) {
                    manifest_ids.push(id);
                }
            }
            if let Some(id) = stage_data.stream_index_ref {
                if seen_streams.insert(id) {
                    let stream = reader.stream_index(&id).cloned().with_context(|| {
                        format!("exchange {trace_id} references missing stream index {id}")
                    })?;
                    if seen_manifests.insert(stream.raw_body_manifest_id) {
                        manifest_ids.push(stream.raw_body_manifest_id);
                    }
                    streams.push(LarInterchangeStream {
                        stream_index_id: id.to_string(),
                        raw_body_manifest_id: stream.raw_body_manifest_id.to_string(),
                        reads: stream.reads,
                        frames: stream.frames,
                    });
                }
            }
            stages.push(LarInterchangeStage {
                stage_id: catalog_stage.occurrence_id,
                record_id,
                ordinal: ordinal as u64,
                capture_sequence: catalog_stage.capture_sequence,
                kind: stage_kind_name(stage_data.kind),
                data: stage_data,
                tool_id: catalog_stage.tool_id,
                tool_phase: catalog_stage.tool_phase,
                supplement_trace_id: catalog_stage.supplement_trace_id,
                supplement_exchange_id: catalog_stage.supplement_exchange_id,
            });
        }

        let mut bodies = Vec::with_capacity(manifest_ids.len());
        for id in manifest_ids {
            bodies.push(read_interchange_manifest(self, id)?);
        }

        Ok(Some(LarInterchangeTrace {
            exchange_id: exchange.id.to_string(),
            trace_id: exchange.data.trace_id,
            session_id: exchange.data.session_id,
            run_id: exchange.data.run_id,
            parent_trace_id: exchange.data.parent_trace_id,
            capture_sequence: exchange.data.capture_sequence,
            wall_time_ns: exchange.data.wall_time_ns,
            monotonic_delta_ns: exchange.data.monotonic_delta_ns,
            clock_id: exchange.data.clock_id,
            metadata,
            stages,
            header_blocks,
            bodies,
            streams,
        }))
    }

    /// Append the exact record closure for one cataloged LAR exchange.
    ///
    /// Returns `false` only for a genuinely legacy trace with no stage records;
    /// callers may then synthesize a declared legacy-fidelity exchange. Any
    /// cataloged but unavailable/inconsistent archive is an error rather than a
    /// silent downgrade.
    pub fn append_exact_trace_to_standalone<W: Read + Write + Seek>(
        &self,
        destination: &mut ArchiveWriter<W>,
        trace_id: &str,
    ) -> Result<bool> {
        let source = trace_archive_source(self, trace_id)?;
        let Some((file_uuid, catalog_path, state)) = source else {
            return Ok(false);
        };
        if !matches!(state.as_str(), "active" | "sealed") {
            bail!("LAR exchange {trace_id} is in offline archive {file_uuid}");
        }
        let path = resolved_catalog_path(&self.data_dir, &catalog_path);
        let file = File::open(&path)
            .with_context(|| format!("opening LAR exchange archive {}", path.display()))?;
        let mut source = ArchiveReader::open(file, Limits::default())
            .map_err(anyhow::Error::new)
            .with_context(|| format!("reading LAR exchange archive {file_uuid}"))?;
        let exchange = source
            .exchange_by_trace(trace_id.as_bytes())
            .cloned()
            .with_context(|| {
                format!("cataloged trace {trace_id} has no exchange in archive {file_uuid}")
            })?;

        let catalog_stage_references = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT record_id, request_headers_ref, request_body_manifest_ref,
                        response_headers_ref, response_body_manifest_ref,
                        trailers_ref, stream_index_ref
                   FROM lar_stage_records
                  WHERE (trace_id=?1 OR stage_id IN (
                           SELECT stage_id FROM lar_timeline_supplements
                            WHERE supplement_trace_id=?1))
                    AND record_id IS NOT NULL",
            )?;
            let references = statement
                .query_map([trace_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        CatalogStageReferences {
                            request_headers_ref: row.get(1)?,
                            request_body_manifest_ref: row.get(2)?,
                            response_headers_ref: row.get(3)?,
                            response_body_manifest_ref: row.get(4)?,
                            trailers_ref: row.get(5)?,
                            stream_index_ref: row.get(6)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?;
            references
        };

        let mut manifests = HashMap::<ManifestId, ManifestId>::new();
        let mut headers = HashMap::<HeaderBlockId, HeaderBlockId>::new();
        let mut streams = HashMap::<StreamIndexId, StreamIndexId>::new();
        let mut destination_stages = Vec::with_capacity(exchange.data.stages.len());
        for source_stage_id in &exchange.data.stages {
            let source_stage = source.stage(source_stage_id).cloned().with_context(|| {
                format!("exchange {trace_id} is missing stage {source_stage_id}")
            })?;
            let mut data = source_stage.data.clone();
            if let Some(references) = catalog_stage_references.get(&source_stage_id.to_string()) {
                overlay_catalog_stage_references(&mut data, references)?;
            }
            data.request_headers_ref =
                copy_header(destination, &source, &mut headers, data.request_headers_ref)?;
            data.response_headers_ref = copy_header(
                destination,
                &mut source,
                &mut headers,
                data.response_headers_ref,
            )?;
            data.trailers_ref = copy_header(destination, &source, &mut headers, data.trailers_ref)?;
            data.request_body_manifest_ref = copy_manifest(
                self,
                destination,
                &mut source,
                &mut manifests,
                data.request_body_manifest_ref,
            )?;
            data.response_body_manifest_ref = copy_manifest(
                self,
                destination,
                &mut source,
                &mut manifests,
                data.response_body_manifest_ref,
            )?;
            data.stream_index_ref = copy_stream(
                self,
                destination,
                &mut source,
                &mut manifests,
                &mut streams,
                data.stream_index_ref,
            )?;
            let destination_id = destination
                .append_stage(Stage::new(data))
                .map_err(anyhow::Error::new)?;
            destination_stages.push(destination_id);
        }

        let mut exchange_data = exchange.data.clone();
        exchange_data.stages = destination_stages;
        let destination_exchange = Exchange::new(exchange_data);
        if let Some(metadata) = source.exchange_metadata(&exchange.id) {
            destination
                .append_exchange_with_metadata(destination_exchange, metadata.data.clone())
                .map_err(anyhow::Error::new)?;
        } else {
            let metadata = self
                .get_trace(trace_id)?
                .as_ref()
                .map(exchange_metadata_from_trace_json)
                .unwrap_or_default();
            destination
                .append_exchange_with_metadata(destination_exchange, metadata)
                .map_err(anyhow::Error::new)?;
        }

        if let Some(closure) = self.lar_conversation_archive_closure(trace_id)? {
            let mut entry_ids = HashMap::<ConversationEntryId, ConversationEntryId>::new();
            for entry in closure.entries {
                let source_id = entry.id;
                let raw_ranges = entry
                    .data
                    .raw_ranges
                    .iter()
                    .map(|range| {
                        let manifest_id = copy_manifest(
                            self,
                            destination,
                            &mut source,
                            &mut manifests,
                            Some(range.manifest_id),
                        )?
                        .expect("a present conversation manifest remains present");
                        Ok(ArtifactRangeRef {
                            manifest_id,
                            byte_offset: range.byte_offset,
                            byte_length: range.byte_length,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut data = entry.data;
                data.raw_ranges = raw_ranges;
                let destination_id = destination
                    .append_conversation_entry(ConversationEntry::new(data))
                    .map_err(anyhow::Error::new)?;
                entry_ids.insert(source_id, destination_id);
            }
            let mut generation_ids = HashMap::<GenerationId, GenerationId>::new();
            for generation in closure.generations {
                let source_id = generation.id;
                let mut data = generation.data;
                data.parent_generation_id = data
                    .parent_generation_id
                    .map(|id| {
                        generation_ids.get(&id).copied().with_context(|| {
                            format!("conversation generation {source_id} precedes its parent")
                        })
                    })
                    .transpose()?;
                data.entries = data
                    .entries
                    .iter()
                    .map(|id| {
                        entry_ids.get(id).copied().with_context(|| {
                            format!("conversation generation {source_id} is missing entry {id}")
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let destination_id = destination
                    .append_generation(Generation::new(data))
                    .map_err(anyhow::Error::new)?;
                generation_ids.insert(source_id, destination_id);
            }
            let mut data = closure.turn.data;
            data.generation_id = generation_ids
                .get(&data.generation_id)
                .copied()
                .context("conversation turn is missing its generation")?;
            data.response_entry_refs = data
                .response_entry_refs
                .iter()
                .map(|id| {
                    entry_ids.get(id).copied().with_context(|| {
                        format!("conversation turn is missing response entry {id}")
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            destination
                .append_turn_view(TurnView::new(data))
                .map_err(anyhow::Error::new)?;
        } else if let Some(turn) = source.turn_view_by_trace(trace_id.as_bytes()).cloned() {
            let mut entries = HashMap::<ConversationEntryId, ConversationEntryId>::new();
            let mut generations = HashMap::<GenerationId, GenerationId>::new();
            let mut visiting = HashSet::<GenerationId>::new();
            let generation_id = copy_generation(
                self,
                destination,
                &mut source,
                &mut manifests,
                &mut entries,
                &mut generations,
                &mut visiting,
                turn.data.generation_id,
            )?;
            let response_entry_refs = turn
                .data
                .response_entry_refs
                .iter()
                .map(|id| {
                    copy_conversation_entry(
                        self,
                        destination,
                        &mut source,
                        &mut manifests,
                        &mut entries,
                        *id,
                    )
                })
                .collect::<Result<Vec<_>>>()?;
            let mut data = turn.data;
            data.generation_id = generation_id;
            data.response_entry_refs = response_entry_refs;
            destination
                .append_turn_view(TurnView::new(data))
                .map_err(anyhow::Error::new)?;
        }
        let supplements = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT supplement_trace_id FROM lar_timeline_supplements
                  WHERE parent_trace_id=?1 OR display_trace_id=?1
                  ORDER BY wall_time_ns, capture_sequence, supplement_trace_id",
            )?;
            let rows = statement
                .query_map([trace_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for supplement_trace_id in supplements {
            if supplement_trace_id != trace_id {
                self.append_exact_trace_to_standalone(destination, &supplement_trace_id)?;
            }
        }
        Ok(true)
    }
}

fn trace_archive_source(store: &Store, trace_id: &str) -> Result<Option<(String, String, String)>> {
    let conn = store.conn.lock().unwrap();
    let exchange_source = conn
        .query_row(
            "SELECT f.file_uuid, f.path, f.state
               FROM lar_exchange_records e
               JOIN lar_files f ON f.file_uuid=e.file_uuid
              WHERE e.trace_id=?1",
            [trace_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if exchange_source.is_some() {
        return Ok(exchange_source);
    }
    // Compatibility with catalogs written before explicit trace-to-exchange
    // ownership was introduced.
    Ok(conn
        .query_row(
            "SELECT f.file_uuid, f.path, f.state
               FROM lar_stage_records s
               JOIN lar_files f ON f.file_uuid=s.file_uuid
              WHERE s.trace_id=?1
              ORDER BY s.capture_sequence, s.stage_id LIMIT 1",
            [trace_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?)
}

fn phase_order(phase: Option<&str>) -> u8 {
    match phase {
        Some("start") => 0,
        Some("arguments") => 1,
        Some("end") => 2,
        Some("result") => 3,
        _ => 4,
    }
}

fn read_interchange_manifest(store: &Store, id: ManifestId) -> Result<LarInterchangeBody> {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT total_length, whole_body_hash, media_type, content_encoding
           FROM lar_manifests WHERE manifest_id=?1 AND state='ready'",
        [id.to_string()],
        |row| {
            let digest = row.get::<_, Vec<u8>>(1)?;
            Ok(LarInterchangeBody {
                manifest_id: id.to_string(),
                total_length: row.get(0)?,
                whole_body_blake3: hex_digest(&digest),
                media_type: row.get::<_, Option<String>>(2)?.map(String::into_bytes),
                content_encoding: row.get::<_, Option<String>>(3)?.map(String::into_bytes),
            })
        },
    )
    .optional()?
    .with_context(|| format!("source catalog is missing manifest {id}"))
}

fn parse_content_id(value: &str, kind: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        bail!("{kind} ID must contain exactly 64 hexadecimal characters");
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).context("content ID is not ASCII")?;
        output[index] = u8::from_str_radix(text, 16)
            .with_context(|| format!("{kind} ID contains non-hexadecimal bytes"))?;
    }
    Ok(output)
}

fn parse_optional_header_id(value: Option<&str>, kind: &str) -> Result<Option<HeaderBlockId>> {
    value
        .map(|value| parse_content_id(value, kind).map(HeaderBlockId))
        .transpose()
}

fn parse_optional_manifest_id(value: Option<&str>, kind: &str) -> Result<Option<ManifestId>> {
    value
        .map(|value| parse_content_id(value, kind).map(ManifestId))
        .transpose()
}

fn parse_optional_stream_id(value: Option<&str>, kind: &str) -> Result<Option<StreamIndexId>> {
    value
        .map(|value| parse_content_id(value, kind).map(StreamIndexId))
        .transpose()
}

fn overlay_catalog_stage_references(
    data: &mut StageData,
    references: &CatalogStageReferences,
) -> Result<()> {
    data.request_headers_ref = parse_optional_header_id(
        references.request_headers_ref.as_deref(),
        "request header block",
    )?;
    data.request_body_manifest_ref = parse_optional_manifest_id(
        references.request_body_manifest_ref.as_deref(),
        "request body manifest",
    )?;
    data.response_headers_ref = parse_optional_header_id(
        references.response_headers_ref.as_deref(),
        "response header block",
    )?;
    data.response_body_manifest_ref = parse_optional_manifest_id(
        references.response_body_manifest_ref.as_deref(),
        "response body manifest",
    )?;
    data.trailers_ref =
        parse_optional_header_id(references.trailers_ref.as_deref(), "trailer header block")?;
    data.stream_index_ref =
        parse_optional_stream_id(references.stream_index_ref.as_deref(), "stream index")?;
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn header_fidelity_name(value: HeaderFidelity) -> &'static str {
    match value {
        HeaderFidelity::Exact => "exact",
        HeaderFidelity::LegacyOrderUnknown => "legacy_order_unknown",
        HeaderFidelity::LegacyCasingUnknown => "legacy_casing_unknown",
        HeaderFidelity::LegacyOrderAndCasingUnknown => "legacy_order_and_casing_unknown",
    }
}

fn stage_kind_name(value: alex_lar::StageKind) -> String {
    match value {
        alex_lar::StageKind::ClientRequest => "client_request".into(),
        alex_lar::StageKind::NormalizedRequest => "normalized_request".into(),
        alex_lar::StageKind::RouterDecision => "router_decision".into(),
        alex_lar::StageKind::RetryDecision => "retry_decision".into(),
        alex_lar::StageKind::FailoverDecision => "failover_decision".into(),
        alex_lar::StageKind::UpstreamRequest => "upstream_request".into(),
        alex_lar::StageKind::UpstreamResponse => "upstream_response".into(),
        alex_lar::StageKind::UpstreamFailure => "upstream_failure".into(),
        alex_lar::StageKind::ClientResponse => "client_response".into(),
        alex_lar::StageKind::ClientTrailers => "client_trailers".into(),
        alex_lar::StageKind::ToolCall => "tool_call".into(),
        alex_lar::StageKind::ToolResult => "tool_result".into(),
        alex_lar::StageKind::AuthRefresh => "auth_refresh".into(),
        alex_lar::StageKind::AccountRouting => "account_routing".into(),
        alex_lar::StageKind::DarioRequest => "dario_request".into(),
        alex_lar::StageKind::DarioResponse => "dario_response".into(),
        alex_lar::StageKind::InjectedResponse => "injected_response".into(),
        alex_lar::StageKind::Cancellation => "cancellation".into(),
        alex_lar::StageKind::Unknown(code) => format!("unknown_{code}"),
    }
}

fn copy_manifest<W: Read + Write + Seek, R: Read + Seek>(
    store: &Store,
    destination: &mut ArchiveWriter<W>,
    source: &mut ArchiveReader<R>,
    copied: &mut HashMap<ManifestId, ManifestId>,
    source_id: Option<ManifestId>,
) -> Result<Option<ManifestId>> {
    let Some(source_id) = source_id else {
        return Ok(None);
    };
    if let Some(id) = copied.get(&source_id) {
        return Ok(Some(*id));
    }
    // Spool one logical body through a file so both the source verifier and
    // destination chunker operate in fixed-size windows. This avoids retaining
    // even one arbitrarily large artifact in memory.
    let source_metadata = if let Some(manifest) = source.manifest(&source_id) {
        (
            manifest.media_type.clone(),
            manifest.content_encoding.clone(),
        )
    } else {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT media_type, content_encoding FROM lar_manifests
              WHERE manifest_id=?1 AND state='ready'",
            [source_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?.map(String::into_bytes),
                    row.get::<_, Option<String>>(1)?.map(String::into_bytes),
                ))
            },
        )
        .optional()?
        .with_context(|| format!("source catalog is missing manifest {source_id}"))?
    };
    let cataloged = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM lar_manifests WHERE manifest_id=?1 AND state='ready')",
            [source_id.to_string()],
            |row| row.get::<_, bool>(0),
        )?
    };
    let temp_root = store.data_dir.join("lar");
    fs::create_dir_all(&temp_root)?;
    let counter = BODY_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_path = temp_root.join(format!(
        ".standalone-manifest-{}-{counter}.tmp",
        std::process::id()
    ));
    let result = (|| -> Result<ManifestId> {
        let mut temp = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temp_path)
            .with_context(|| format!("creating export spool {}", temp_path.display()))?;
        if cataloged {
            store
                .write_lar_manifest_body(&source_id.to_string(), &mut temp)
                .with_context(|| format!("streaming exported LAR manifest {source_id}"))?;
        } else {
            source
                .write_body(&source_id, &mut temp)
                .map_err(anyhow::Error::new)
                .with_context(|| format!("streaming source LAR manifest {source_id}"))?;
        }
        temp.seek(SeekFrom::Start(0))?;
        destination
            .append_reader_with_metadata(temp, source_metadata.0, source_metadata.1)
            .map_err(anyhow::Error::new)
    })();
    let _ = fs::remove_file(&temp_path);
    let destination_id = result?;
    copied.insert(source_id, destination_id);
    Ok(Some(destination_id))
}

fn copy_header<W: Read + Write + Seek>(
    destination: &mut ArchiveWriter<W>,
    source: &ArchiveReader<File>,
    copied: &mut HashMap<HeaderBlockId, HeaderBlockId>,
    source_id: Option<HeaderBlockId>,
) -> Result<Option<HeaderBlockId>> {
    let Some(source_id) = source_id else {
        return Ok(None);
    };
    if let Some(id) = copied.get(&source_id) {
        return Ok(Some(*id));
    }
    let block = source
        .header_block(&source_id)
        .cloned()
        .with_context(|| format!("exported stage references missing header block {source_id}"))?;
    let destination_id = destination
        .append_header_block(block)
        .map_err(anyhow::Error::new)?;
    copied.insert(source_id, destination_id);
    Ok(Some(destination_id))
}

fn copy_stream<W: Read + Write + Seek>(
    store: &Store,
    destination: &mut ArchiveWriter<W>,
    source: &mut ArchiveReader<File>,
    manifests: &mut HashMap<ManifestId, ManifestId>,
    copied: &mut HashMap<StreamIndexId, StreamIndexId>,
    source_id: Option<StreamIndexId>,
) -> Result<Option<StreamIndexId>> {
    let Some(source_id) = source_id else {
        return Ok(None);
    };
    if let Some(id) = copied.get(&source_id) {
        return Ok(Some(*id));
    }
    let index = source
        .stream_index(&source_id)
        .cloned()
        .with_context(|| format!("exported stage references missing stream index {source_id}"))?;
    let manifest_id = copy_manifest(
        store,
        destination,
        source,
        manifests,
        Some(index.raw_body_manifest_id),
    )?
    .expect("a present stream manifest remains present");
    let destination_id = destination
        .append_stream_index(StreamIndex::new(manifest_id, index.reads, index.frames))
        .map_err(anyhow::Error::new)?;
    copied.insert(source_id, destination_id);
    Ok(Some(destination_id))
}

#[allow(clippy::too_many_arguments)]
fn copy_generation<W: Read + Write + Seek>(
    store: &Store,
    destination: &mut ArchiveWriter<W>,
    source: &mut ArchiveReader<File>,
    manifests: &mut HashMap<ManifestId, ManifestId>,
    entries: &mut HashMap<ConversationEntryId, ConversationEntryId>,
    generations: &mut HashMap<GenerationId, GenerationId>,
    visiting: &mut HashSet<GenerationId>,
    source_id: GenerationId,
) -> Result<GenerationId> {
    if let Some(id) = generations.get(&source_id) {
        return Ok(*id);
    }
    if !visiting.insert(source_id) {
        bail!("conversation generation cycle while exporting {source_id}");
    }
    let generation = source
        .generation(&source_id)
        .cloned()
        .with_context(|| format!("turn references missing generation {source_id}"))?;
    let parent_generation_id = generation
        .data
        .parent_generation_id
        .map(|id| {
            copy_generation(
                store,
                destination,
                source,
                manifests,
                entries,
                generations,
                visiting,
                id,
            )
        })
        .transpose()?;
    let destination_entries = generation
        .data
        .entries
        .iter()
        .map(|id| copy_conversation_entry(store, destination, source, manifests, entries, *id))
        .collect::<Result<Vec<_>>>()?;
    let mut data = generation.data;
    data.parent_generation_id = parent_generation_id;
    data.entries = destination_entries;
    let destination_id = destination
        .append_generation(Generation::new(data))
        .map_err(anyhow::Error::new)?;
    visiting.remove(&source_id);
    generations.insert(source_id, destination_id);
    Ok(destination_id)
}

fn copy_conversation_entry<W: Read + Write + Seek>(
    store: &Store,
    destination: &mut ArchiveWriter<W>,
    source: &mut ArchiveReader<File>,
    manifests: &mut HashMap<ManifestId, ManifestId>,
    copied: &mut HashMap<ConversationEntryId, ConversationEntryId>,
    source_id: ConversationEntryId,
) -> Result<ConversationEntryId> {
    if let Some(id) = copied.get(&source_id) {
        return Ok(*id);
    }
    let entry = source
        .conversation_entry(&source_id)
        .cloned()
        .with_context(|| format!("generation references missing entry {source_id}"))?;
    let raw_ranges = entry
        .data
        .raw_ranges
        .iter()
        .map(|range| {
            let manifest_id = copy_manifest(
                store,
                destination,
                source,
                manifests,
                Some(range.manifest_id),
            )?
            .expect("a present conversation manifest remains present");
            Ok(ArtifactRangeRef {
                manifest_id,
                byte_offset: range.byte_offset,
                byte_length: range.byte_length,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut data = entry.data;
    data.raw_ranges = raw_ranges;
    let destination_id = destination
        .append_conversation_entry(ConversationEntry::new(data))
        .map_err(anyhow::Error::new)?;
    copied.insert(source_id, destination_id);
    Ok(destination_id)
}

fn exchange_metadata_from_trace_json(row: &serde_json::Value) -> ExchangeMetadataData {
    let bytes = |name: &str| row[name].as_str().map(str::as_bytes).map(Vec::from);
    let boolean = |name: &str| {
        row[name]
            .as_bool()
            .or_else(|| row[name].as_i64().map(|value| value != 0))
    };
    ExchangeMetadataData {
        ts_request_ms: row["ts_request_ms"].as_i64(),
        ts_response_ms: row["ts_response_ms"].as_i64(),
        harness: bytes("harness"),
        client_format: bytes("client_format"),
        upstream_format: bytes("upstream_format"),
        method: bytes("method"),
        path: bytes("path"),
        streamed: boolean("streamed"),
        status: row["status"].as_i64(),
        cost_usd_bits: row["cost_usd"].as_f64().map(f64::to_bits),
        billing_bucket: bytes("billing_bucket"),
        error_kind: bytes("error_kind"),
        error_code: bytes("error_code"),
        substituted: boolean("substituted").unwrap_or(false),
        original_model: bytes("original_model"),
        served_model: bytes("served_model"),
        substitution_reason: bytes("substitution_reason"),
        injected: boolean("injected").unwrap_or(false),
        fixture_name: bytes("fixture_name"),
        attempts_json: match &row["attempts"] {
            serde_json::Value::Null => None,
            serde_json::Value::String(value) => Some(value.as_bytes().to_vec()),
            value => serde_json::to_vec(value).ok(),
        },
        original_account_id: bytes("original_account_id"),
        served_account_id: bytes("served_account_id"),
        subscription_identity: bytes("subscription_identity"),
        via_dario: boolean("via_dario").unwrap_or(false),
        dario_generation: bytes("dario_generation"),
        tags_json: bytes("tags_json"),
        client_ip: bytes("client_ip"),
        key_fingerprint: bytes("key_fingerprint"),
        reasoning_effort: bytes("reasoning_effort"),
        thinking_budget: row["thinking_budget"].as_i64(),
        input_tokens: row["input_tokens"].as_i64(),
        cached_input_tokens: row["cached_input_tokens"].as_i64(),
        cache_creation_tokens: row["cache_creation_tokens"].as_i64(),
        output_tokens: row["output_tokens"].as_i64(),
        reasoning_tokens: row["reasoning_tokens"].as_i64(),
        unknown_attributes: Vec::new(),
    }
}
