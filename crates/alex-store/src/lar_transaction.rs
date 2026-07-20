//! Bounded, byte-authoritative export of one canonical HTTP/LLM transaction.
//!
//! The wire representation is RFC 7464 JSON Text Sequences. Metadata records
//! are ordinary JSON values; artifact bytes are emitted in independently
//! decodable base64 pieces. Bodies remain content-addressed and are emitted
//! once per referenced manifest, never once per stage occurrence.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::str::FromStr;

use alex_lar::{
    read_chunk_record_at, ArchiveReader, ChunkHash, ChunkRecordDescriptor, ExchangeMetadataData,
    HeaderBlockId, HeaderFidelity, Limits, ManifestId, Stage, StageData, StageKind, StreamIndexId,
    TokenUsage,
};
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    lar_archive_ops::resolved_catalog_path, lar_tool_timeline::parse_tool_supplement,
    sqlite_row_json, LarInterchangeTrace, Store, BACKUP_TRACE_COLS,
};

pub const LAR_TRANSACTION_FORMAT: &str = "alex-lar-transaction-json-seq";
pub const LAR_TRANSACTION_VERSION: u64 = 1;
pub const LAR_TRANSACTION_ARTIFACT_PIECE_BYTES: usize = 48 * 1024;
const MAX_TRANSACTION_STAGES: usize = 100_000;
const MAX_TRANSACTION_ARTIFACTS: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LarTransactionExportReport {
    pub format: &'static str,
    pub version: u64,
    pub trace_id: String,
    pub fidelity: String,
    pub exchanges: u64,
    pub exchange_metadata_present: bool,
    pub stages: u64,
    pub header_blocks: u64,
    pub artifacts: u64,
    pub artifact_bytes: u64,
    pub stream_indexes: u64,
    pub output_bytes: u64,
    pub max_source_chunk_bytes: u64,
    pub output_piece_bytes: usize,
    pub limitations: Vec<&'static str>,
}

impl Store {
    /// Stream the base exchange and every canonical late supplement directly
    /// from their cataloged packs. Body bytes are resolved by manifest/chunk
    /// identity and never materialized as a whole artifact.
    pub fn write_lar_transaction<W: Write>(
        &self,
        trace_id: &str,
        output: W,
    ) -> Result<Option<LarTransactionExportReport>> {
        let Some(exchange) = self.lar_interchange_trace(trace_id)? else {
            return Ok(None);
        };
        write_catalog_transaction(self, std::slice::from_ref(&exchange), trace_id, output).map(Some)
    }

    /// Stream a deliberately lossy projection of a genuinely legacy trace.
    ///
    /// The legacy body sources are hashed once and then streamed directly into
    /// bounded JSON-sequence pieces. No whole body and no temporary standalone
    /// archive is materialized. Bodies with the same BLAKE3 identity are
    /// emitted once even when several synthesized stages reference them.
    pub fn write_legacy_transaction<W: Write>(
        &self,
        trace_id: &str,
        output: W,
    ) -> Result<Option<LarTransactionExportReport>> {
        let row = {
            let conn = self.conn.lock().unwrap();
            let sql = format!(
                "SELECT {} FROM traces WHERE id=?1",
                BACKUP_TRACE_COLS.join(", ")
            );
            conn.query_row(&sql, [trace_id], |row| {
                sqlite_row_json(row, BACKUP_TRACE_COLS)
            })
            .optional()?
        };
        let Some(row) = row else {
            return Ok(None);
        };
        write_legacy_store_transaction(self, trace_id, &row, output).map(Some)
    }
}

fn write_catalog_transaction<W: Write>(
    store: &Store,
    timeline: &[LarInterchangeTrace],
    trace_id: &str,
    output: W,
) -> Result<LarTransactionExportReport> {
    if timeline.len() != 1 {
        bail!("catalog transaction projection requires exactly one flattened timeline");
    }
    let timeline_view = &timeline[0];
    let total_stages = timeline
        .iter()
        .map(|exchange| exchange.stages.len())
        .sum::<usize>();
    if total_stages > MAX_TRANSACTION_STAGES {
        bail!("transaction stage count exceeds limit ({total_stages} > {MAX_TRANSACTION_STAGES})");
    }
    let limitations = vec![
        "application-level HTTP/LLM replay only; TCP, TLS, HTTP/2 frame, and connection timing were not captured",
        "no HTTP or provider framing is invented; stream replay uses only observed reads or recorded parsed-frame ranges",
        "headers contain the redacted bytes retained at capture time; secrets removed at capture cannot be reconstructed",
    ];
    let mut writer = JsonSequenceWriter::new(output);
    writer.record(&json!({
        "type": "format",
        "format": LAR_TRANSACTION_FORMAT,
        "version": LAR_TRANSACTION_VERSION,
        "trace_id": trace_id,
        "fidelity": "canonical",
        "byte_encoding": "utf8-string-or-base64-object",
        "artifact_piece_bytes": LAR_TRANSACTION_ARTIFACT_PIECE_BYTES,
        "limitations": limitations,
    }))?;

    let mut emitted_headers = HashSet::<String>::new();
    let mut emitted_streams = HashSet::<String>::new();
    let mut artifact_ids = HashSet::<ManifestId>::new();
    let mut artifact_order = Vec::<ManifestId>::new();
    let mut exchange_ordinals = HashMap::<String, usize>::new();
    exchange_ordinals.insert(timeline_view.exchange_id.clone(), 0);
    for stage in &timeline_view.stages {
        if stage.supplement_trace_id.is_some() && stage.supplement_exchange_id.is_none() {
            bail!("catalog supplement stage is missing its source exchange content ID");
        }
        if let Some(exchange_id) = &stage.supplement_exchange_id {
            if !exchange_ordinals.contains_key(exchange_id) {
                let ordinal = exchange_ordinals.len();
                exchange_ordinals.insert(exchange_id.clone(), ordinal);
            }
        }
    }
    let source_exchange_count = exchange_ordinals.len();
    writer.record(&interchange_timeline_json(timeline_view))?;
    let mut global_stage_ordinal = 0usize;
    let mut exchange_stage_ordinals = HashMap::<String, usize>::new();
    for exchange in timeline {
        for stage in &exchange.stages {
            let source_exchange_id = stage
                .supplement_exchange_id
                .as_deref()
                .unwrap_or(&exchange.exchange_id);
            let source_exchange_ordinal = *exchange_ordinals
                .get(source_exchange_id)
                .context("transaction stage source exchange has no timeline ordinal")?;
            let ordinal_within_exchange = exchange_stage_ordinals
                .entry(source_exchange_id.to_owned())
                .or_default();
            let occurrence_id =
                transaction_occurrence_id(source_exchange_id, *ordinal_within_exchange);
            let mut stage_record = stage_json(
                global_stage_ordinal,
                source_exchange_ordinal,
                *ordinal_within_exchange,
                source_exchange_id,
                &occurrence_id,
                &stage.record_id,
                &stage.data,
            );
            *ordinal_within_exchange += 1;
            let stage_object = stage_record
                .as_object_mut()
                .expect("stage records are JSON objects");
            stage_object.insert("capture_sequence".into(), stage.capture_sequence.into());
            stage_object.insert(
                "tool_id".into(),
                stage
                    .tool_id
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            stage_object.insert(
                "tool_phase".into(),
                stage
                    .tool_phase
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            stage_object.insert(
                "supplement_trace_id".into(),
                stage
                    .supplement_trace_id
                    .as_deref()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            writer.record(&stage_record)?;
            global_stage_ordinal += 1;
            for (field, header_id) in [
                ("request_headers", stage.data.request_headers_ref),
                ("response_headers", stage.data.response_headers_ref),
                ("trailers", stage.data.trailers_ref),
            ] {
                let Some(header_id) = header_id else {
                    continue;
                };
                let header_id = header_id.to_string();
                if !emitted_headers.insert(header_id.clone()) {
                    continue;
                }
                let block = exchange
                    .header_blocks
                    .iter()
                    .find(|block| block.block_id == header_id)
                    .with_context(|| {
                        format!(
                            "stage {} references missing header block {header_id}",
                            stage.stage_id
                        )
                    })?;
                writer.record(&json!({
                    "type": "header_block",
                    "content_id": header_id,
                    "first_reference": {"stage_id": stage.record_id, "field": field},
                    "fidelity": block.fidelity,
                    "atoms": block.atoms.iter().enumerate().map(|(ordinal, atom)| json!({
                        "ordinal": ordinal,
                        "name": bytes_json(&atom.original_name),
                        "value": bytes_json(&atom.value),
                        "flags": atom.flags,
                    })).collect::<Vec<_>>(),
                }))?;
            }
            for manifest_id in [
                stage.data.request_body_manifest_ref,
                stage.data.response_body_manifest_ref,
            ]
            .into_iter()
            .flatten()
            {
                register_artifact(manifest_id, &mut artifact_ids, &mut artifact_order)?;
            }
            if let Some(stream_id) = stage.data.stream_index_ref {
                let stream_id = stream_id.to_string();
                if emitted_streams.insert(stream_id.clone()) {
                    let stream = exchange
                        .streams
                        .iter()
                        .find(|stream| stream.stream_index_id == stream_id)
                        .with_context(|| {
                            format!(
                                "stage {} references missing stream index {stream_id}",
                                stage.stage_id
                            )
                        })?;
                    let raw_manifest = ManifestId::from_str(&stream.raw_body_manifest_id)
                        .map_err(anyhow::Error::new)?;
                    register_artifact(raw_manifest, &mut artifact_ids, &mut artifact_order)?;
                    writer.record(&json!({
                        "type": "stream_index",
                        "content_id": stream_id,
                        "stage_id": stage.record_id,
                        "raw_body_content_id": stream.raw_body_manifest_id,
                        "observed_reads": stream.reads.iter().map(|read| json!({
                            "byte_offset": read.byte_offset,
                            "byte_length": read.byte_length,
                            "delta_from_first_byte_ns": read.delta_from_first_byte_ns,
                        })).collect::<Vec<_>>(),
                        "parsed_frames": stream.frames.iter().map(|frame| json!({
                            "byte_offset": frame.byte_offset,
                            "byte_length": frame.byte_length,
                            "delta_from_first_byte_ns": frame.delta_from_first_byte_ns,
                            "parser": format!("{:?}", frame.parser),
                            "frame_kind": format!("{:?}", frame.frame_kind),
                        })).collect::<Vec<_>>(),
                    }))?;
                }
            }
        }
    }

    let (artifact_bytes, max_source_chunk_bytes) =
        write_catalog_artifacts(store, &mut writer, &artifact_order)?;
    let output_bytes_before_end = writer.bytes_written;
    writer.record(&json!({
        "type": "end",
        "trace_id": trace_id,
        "exchanges": source_exchange_count,
        "stages": total_stages,
        "header_blocks": emitted_headers.len(),
        "artifacts": artifact_order.len(),
        "artifact_bytes": artifact_bytes,
        "stream_indexes": emitted_streams.len(),
        "bytes_before_end_record": output_bytes_before_end,
        "complete": true,
    }))?;
    Ok(LarTransactionExportReport {
        format: LAR_TRANSACTION_FORMAT,
        version: LAR_TRANSACTION_VERSION,
        trace_id: trace_id.into(),
        fidelity: "canonical".into(),
        exchanges: source_exchange_count as u64,
        exchange_metadata_present: timeline.iter().all(|exchange| exchange.metadata.is_some()),
        stages: total_stages as u64,
        header_blocks: emitted_headers.len() as u64,
        artifacts: artifact_order.len() as u64,
        artifact_bytes,
        stream_indexes: emitted_streams.len() as u64,
        output_bytes: writer.bytes_written,
        max_source_chunk_bytes,
        output_piece_bytes: LAR_TRANSACTION_ARTIFACT_PIECE_BYTES,
        limitations,
    })
}

fn write_catalog_artifacts<W: Write>(
    store: &Store,
    writer: &mut JsonSequenceWriter<W>,
    artifact_order: &[ManifestId],
) -> Result<(u64, u64)> {
    let mut artifact_bytes = 0u64;
    let mut max_source_chunk_bytes = 0u64;
    for manifest_id in artifact_order {
        let manifest = {
            let conn = store.conn.lock().unwrap();
            crate::lar_grep::load_catalog_manifest(&conn, &manifest_id.to_string())?
        };
        writer.record(&json!({
            "type": "artifact_start",
            "content_id": manifest.id.to_string(),
            "total_length": manifest.total_length,
            "whole_body_hash": hex(&manifest.whole_body_hash.digest),
            "media_type": manifest.media_type.as_deref().map(bytes_json),
            "content_encoding": manifest.content_encoding.as_deref().map(bytes_json),
            "range_count": manifest.chunks.len(),
        }))?;
        let mut body_hasher = blake3::Hasher::new();
        let mut reconstructed = 0u64;
        for (ordinal, reference) in manifest.chunks.iter().enumerate() {
            writer.record(&json!({
                "type": "artifact_range",
                "content_id": manifest.id.to_string(),
                "ordinal": ordinal,
                "chunk_hash": hex(&reference.chunk_hash.digest),
                "chunk_offset": reference.chunk_offset,
                "logical_offset": reference.logical_offset,
                "length": reference.length,
            }))?;
            let chunk = catalog_chunk(store, &reference.chunk_hash)?;
            max_source_chunk_bytes = max_source_chunk_bytes.max(chunk.len() as u64);
            let start = usize::try_from(reference.chunk_offset)
                .context("transaction chunk offset exceeds address space")?;
            let end = usize::try_from(
                reference
                    .chunk_offset
                    .checked_add(reference.length)
                    .context("transaction chunk range overflow")?,
            )
            .context("transaction chunk end exceeds address space")?;
            let range = chunk
                .get(start..end)
                .context("transaction manifest range exceeds catalog chunk")?;
            for (piece_index, piece) in range
                .chunks(LAR_TRANSACTION_ARTIFACT_PIECE_BYTES)
                .enumerate()
            {
                let piece_offset = (piece_index * LAR_TRANSACTION_ARTIFACT_PIECE_BYTES) as u64;
                writer.record(&json!({
                    "type": "artifact_bytes",
                    "content_id": manifest.id.to_string(),
                    "logical_offset": reference.logical_offset + piece_offset,
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(piece),
                }))?;
                body_hasher.update(piece);
                reconstructed = reconstructed.saturating_add(piece.len() as u64);
            }
        }
        if reconstructed != manifest.total_length
            || body_hasher.finalize().as_bytes() != &manifest.whole_body_hash.digest
        {
            bail!("transaction artifact {manifest_id} failed body identity verification");
        }
        artifact_bytes = artifact_bytes.saturating_add(reconstructed);
        writer.record(&json!({
            "type": "artifact_end",
            "content_id": manifest.id.to_string(),
            "total_length": reconstructed,
            "verified": true,
        }))?;
    }
    Ok((artifact_bytes, max_source_chunk_bytes))
}

fn catalog_chunk(store: &Store, hash: &ChunkHash) -> Result<Vec<u8>> {
    let (stored_path, frame_offset, uncompressed_length, compressed_length) = {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT f.path, c.page_offset, c.uncompressed_length, c.compressed_length
               FROM lar_chunks c JOIN lar_files f ON f.file_uuid=c.file_uuid
              WHERE c.hash_algorithm='blake3' AND c.chunk_hash=?1
                AND c.state='ready' AND f.state IN ('active','sealed')",
            [hash.digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )?
    };
    let path = resolved_catalog_path(&store.data_dir, &stored_path);
    let mut file = File::open(&path)
        .with_context(|| format!("opening transaction chunk source {}", path.display()))?;
    read_chunk_record_at(
        &mut file,
        &ChunkRecordDescriptor {
            hash: *hash,
            frame_offset,
            uncompressed_length,
            compressed_length,
        },
        &Limits::default(),
    )
    .map_err(anyhow::Error::new)
}

#[derive(Clone, Debug)]
struct LegacyArtifact {
    source_kind: &'static str,
    content_id: String,
    total_length: u64,
    whole_body_hash: String,
}

fn write_legacy_store_transaction<W: Write>(
    store: &Store,
    trace_id: &str,
    row: &Value,
    output: W,
) -> Result<LarTransactionExportReport> {
    let limitations = vec![
        "application-level HTTP/LLM replay only; TCP, TLS, HTTP/2 frame, and connection timing were not captured",
        "legacy stage order was synthesized from three body slots rather than captured attempts",
        "legacy header order, original casing, duplicate fields, trailers, and stream timing may be unavailable",
        "legacy content IDs are synthesized from exact retained body/header bytes rather than original capture records",
        "headers contain the redacted bytes retained at capture time; secrets removed at capture cannot be reconstructed",
    ];
    let mut slots = HashMap::<&'static str, Option<String>>::new();
    let mut artifacts = Vec::<LegacyArtifact>::new();
    let mut artifact_indexes = HashMap::<String, usize>::new();
    for source_kind in ["client_request", "upstream_request", "client_response"] {
        let mut identity = HashingCounter::default();
        if !store.write_lar_or_legacy_artifact(
            "trace",
            trace_id,
            source_kind,
            None,
            &mut identity,
        )? {
            slots.insert(source_kind, None);
            continue;
        }
        let whole_body_hash = identity.hasher.finalize().to_hex().to_string();
        let content_id = format!("legacy-body-blake3:{whole_body_hash}");
        if let Some(index) = artifact_indexes.get(&content_id).copied() {
            if artifacts[index].total_length != identity.length {
                bail!("legacy body identity collision has inconsistent lengths");
            }
        } else {
            artifact_indexes.insert(content_id.clone(), artifacts.len());
            artifacts.push(LegacyArtifact {
                source_kind,
                content_id: content_id.clone(),
                total_length: identity.length,
                whole_body_hash,
            });
        }
        slots.insert(source_kind, Some(content_id));
    }

    let request_headers = legacy_header_atoms(row.get("req_headers_json"));
    let response_headers = legacy_header_atoms(row.get("resp_headers_json"));
    let request_header_id = legacy_header_id(&request_headers);
    let response_header_id = legacy_header_id(&response_headers);
    let mut header_blocks = Vec::<(String, Vec<(Vec<u8>, Vec<u8>)>, &'static str)>::new();
    if let Some(id) = &request_header_id {
        header_blocks.push((id.clone(), request_headers, "request_headers"));
    }
    if let Some(id) = &response_header_id {
        if !header_blocks.iter().any(|value| value.0 == *id) {
            header_blocks.push((id.clone(), response_headers, "response_headers"));
        }
    }

    let request_time = row["ts_request_ms"].as_i64().unwrap_or_default().max(0) as u64 * 1_000_000;
    let response_time = row["ts_response_ms"]
        .as_i64()
        .unwrap_or(row["ts_request_ms"].as_i64().unwrap_or_default())
        .max(0) as u64
        * 1_000_000;
    let status = row["status"]
        .as_i64()
        .and_then(|value| u16::try_from(value).ok());
    let request_body_id = slots.get("client_request").cloned().flatten();
    let upstream_body_id = slots.get("upstream_request").cloned().flatten();
    let response_body_id = slots.get("client_response").cloned().flatten();

    let mut stages = Vec::<(String, StageData, Option<String>, Option<String>)>::new();
    let mut client_request = StageData::new(StageKind::ClientRequest, request_time);
    client_request.requested_model = legacy_row_bytes(row, "requested_model");
    stages.push((
        legacy_record_id("stage-client-request", trace_id.as_bytes()),
        client_request,
        request_header_id.clone(),
        request_body_id,
    ));

    let mut routing = StageData::new(StageKind::RouterDecision, request_time);
    routing.provider = legacy_row_bytes(row, "upstream_provider");
    routing.requested_model = legacy_row_bytes(row, "requested_model");
    routing.routed_model = legacy_row_bytes(row, "routed_model");
    routing.account_id = legacy_row_bytes(row, "account_id");
    routing.routing_reason = legacy_row_bytes(row, "substitution_reason");
    stages.push((
        legacy_record_id("stage-router-decision", trace_id.as_bytes()),
        routing,
        None,
        None,
    ));

    let mut upstream_request = StageData::new(StageKind::UpstreamRequest, request_time);
    upstream_request.attempt_number = Some(1);
    upstream_request.provider = legacy_row_bytes(row, "upstream_provider");
    upstream_request.requested_model = legacy_row_bytes(row, "requested_model");
    upstream_request.routed_model = legacy_row_bytes(row, "routed_model");
    stages.push((
        legacy_record_id("stage-upstream-request", trace_id.as_bytes()),
        upstream_request,
        None,
        upstream_body_id,
    ));

    let mut upstream_response = StageData::new(StageKind::UpstreamResponse, response_time);
    upstream_response.attempt_number = Some(1);
    upstream_response.provider = legacy_row_bytes(row, "upstream_provider");
    upstream_response.status_code = status;
    stages.push((
        legacy_record_id("stage-upstream-response", trace_id.as_bytes()),
        upstream_response,
        None,
        response_body_id.clone(),
    ));

    let mut client_response = StageData::new(StageKind::ClientResponse, response_time);
    client_response.status_code = status;
    client_response.usage = Some(TokenUsage {
        input_tokens: legacy_row_u64(row, "input_tokens"),
        output_tokens: legacy_row_u64(row, "output_tokens"),
        cached_tokens: legacy_row_u64(row, "cached_input_tokens"),
        reasoning_tokens: legacy_row_u64(row, "reasoning_tokens"),
    });
    client_response.error_class = legacy_row_bytes(row, "error_class");
    client_response.error_message = legacy_row_bytes(row, "error");
    if let Some(cost) = row["cost_usd"].as_f64() {
        client_response.cost_nanos = Some((cost.max(0.0) * 1_000_000_000.0) as u64);
        client_response.cost_currency = Some(b"USD".to_vec());
    }
    stages.push((
        legacy_record_id("stage-client-response", trace_id.as_bytes()),
        client_response,
        response_header_id.clone(),
        response_body_id,
    ));

    let exchange_id = legacy_record_id("exchange", trace_id.as_bytes());
    let mut writer = JsonSequenceWriter::new(output);
    writer.record(&json!({
        "type": "format",
        "format": LAR_TRANSACTION_FORMAT,
        "version": LAR_TRANSACTION_VERSION,
        "trace_id": trace_id,
        "fidelity": "synthesized_legacy",
        "byte_encoding": "utf8-string-or-base64-object",
        "artifact_piece_bytes": LAR_TRANSACTION_ARTIFACT_PIECE_BYTES,
        "limitations": limitations,
    }))?;
    writer.record(&json!({
        "type": "transaction_timeline",
        "base_exchange_content_id": exchange_id,
        "trace_id": bytes_json(trace_id.as_bytes()),
        "session_id": legacy_row_bytes(row, "session_id").as_deref().map(bytes_json),
        "run_id": legacy_row_bytes(row, "run_id").as_deref().map(bytes_json),
        "parent_trace_id": Value::Null,
        "capture_sequence": 0,
        "wall_time_ns": request_time,
        "monotonic_delta_ns": Value::Null,
        "clock_id": Value::Null,
        "ordered_stage_content_ids": stages.iter().map(|stage| stage.0.as_str()).collect::<Vec<_>>(),
        "supplements": [],
        "metadata": legacy_exchange_metadata_json(row),
    }))?;
    for (ordinal, (stage_id, data, header_id, body_id)) in stages.iter().enumerate() {
        let occurrence_id = transaction_occurrence_id(&exchange_id, ordinal);
        let mut record = stage_json(
            ordinal,
            0,
            ordinal,
            &exchange_id,
            &occurrence_id,
            stage_id,
            data,
        );
        let object = record.as_object_mut().expect("stage record is an object");
        match data.kind {
            StageKind::ClientRequest | StageKind::UpstreamRequest => {
                object.insert(
                    "request_headers_content_id".into(),
                    header_id.as_deref().map(Value::from).unwrap_or(Value::Null),
                );
                object.insert(
                    "request_body_content_id".into(),
                    body_id.as_deref().map(Value::from).unwrap_or(Value::Null),
                );
            }
            StageKind::UpstreamResponse | StageKind::ClientResponse => {
                object.insert(
                    "response_headers_content_id".into(),
                    header_id.as_deref().map(Value::from).unwrap_or(Value::Null),
                );
                object.insert(
                    "response_body_content_id".into(),
                    body_id.as_deref().map(Value::from).unwrap_or(Value::Null),
                );
            }
            _ => {}
        }
        writer.record(&record)?;
    }
    for (content_id, atoms, field) in &header_blocks {
        let first_stage_id = if *field == "request_headers" {
            &stages[0].0
        } else {
            &stages[4].0
        };
        writer.record(&json!({
            "type": "header_block",
            "content_id": content_id,
            "first_reference": {"stage_id": first_stage_id, "field": field},
            "fidelity": "legacy_order_and_casing_unknown",
            "atoms": atoms.iter().enumerate().map(|(ordinal, (name, value))| json!({
                "ordinal": ordinal,
                "name": bytes_json(name),
                "value": bytes_json(value),
                "flags": 0,
            })).collect::<Vec<_>>(),
        }))?;
    }

    let mut artifact_bytes = 0_u64;
    for artifact in &artifacts {
        writer.record(&json!({
            "type": "artifact_start",
            "content_id": artifact.content_id,
            "total_length": artifact.total_length,
            "whole_body_hash": artifact.whole_body_hash,
            "media_type": Value::Null,
            "content_encoding": Value::Null,
            "range_count": 1,
        }))?;
        writer.record(&json!({
            "type": "artifact_range",
            "content_id": artifact.content_id,
            "ordinal": 0,
            "chunk_hash": artifact.whole_body_hash,
            "chunk_offset": 0,
            "logical_offset": 0,
            "length": artifact.total_length,
            "fidelity": "synthesized_legacy_whole_body_range",
        }))?;
        let mut piece_writer = ArtifactPieceWriter::new(&mut writer, &artifact.content_id);
        if !store.write_lar_or_legacy_artifact(
            "trace",
            trace_id,
            artifact.source_kind,
            None,
            &mut piece_writer,
        )? {
            bail!(
                "legacy artifact {} disappeared during export",
                artifact.source_kind
            );
        }
        let (written, hash) = piece_writer.finish()?;
        if written != artifact.total_length || hash != artifact.whole_body_hash {
            bail!(
                "legacy artifact {} changed during export",
                artifact.source_kind
            );
        }
        artifact_bytes = artifact_bytes.saturating_add(written);
        writer.record(&json!({
            "type": "artifact_end",
            "content_id": artifact.content_id,
            "total_length": written,
            "verified": true,
        }))?;
    }
    let bytes_before_end = writer.bytes_written;
    writer.record(&json!({
        "type": "end",
        "trace_id": trace_id,
        "exchanges": 1,
        "stages": stages.len(),
        "header_blocks": header_blocks.len(),
        "artifacts": artifacts.len(),
        "artifact_bytes": artifact_bytes,
        "stream_indexes": 0,
        "bytes_before_end_record": bytes_before_end,
        "complete": true,
    }))?;
    Ok(LarTransactionExportReport {
        format: LAR_TRANSACTION_FORMAT,
        version: LAR_TRANSACTION_VERSION,
        trace_id: trace_id.into(),
        fidelity: "synthesized_legacy".into(),
        exchanges: 1,
        exchange_metadata_present: true,
        stages: stages.len() as u64,
        header_blocks: header_blocks.len() as u64,
        artifacts: artifacts.len() as u64,
        artifact_bytes,
        stream_indexes: 0,
        output_bytes: writer.bytes_written,
        max_source_chunk_bytes: 0,
        output_piece_bytes: LAR_TRANSACTION_ARTIFACT_PIECE_BYTES,
        limitations,
    })
}

#[derive(Default)]
struct HashingCounter {
    hasher: blake3::Hasher,
    length: u64,
}

impl Write for HashingCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.hasher.update(bytes);
        self.length = self.length.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct ArtifactPieceWriter<'a, W> {
    writer: &'a mut JsonSequenceWriter<W>,
    content_id: &'a str,
    pending: Vec<u8>,
    logical_offset: u64,
    hasher: blake3::Hasher,
}

impl<'a, W: Write> ArtifactPieceWriter<'a, W> {
    fn new(writer: &'a mut JsonSequenceWriter<W>, content_id: &'a str) -> Self {
        Self {
            writer,
            content_id,
            pending: Vec::with_capacity(LAR_TRANSACTION_ARTIFACT_PIECE_BYTES),
            logical_offset: 0,
            hasher: blake3::Hasher::new(),
        }
    }

    fn emit_pending(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.writer.record(&json!({
            "type": "artifact_bytes",
            "content_id": self.content_id,
            "logical_offset": self.logical_offset,
            "data_base64": base64::engine::general_purpose::STANDARD.encode(&self.pending),
        }))?;
        self.logical_offset = self
            .logical_offset
            .saturating_add(self.pending.len() as u64);
        self.pending.clear();
        Ok(())
    }

    fn finish(mut self) -> Result<(u64, String)> {
        self.emit_pending()?;
        Ok((
            self.logical_offset,
            self.hasher.finalize().to_hex().to_string(),
        ))
    }
}

impl<W: Write> Write for ArtifactPieceWriter<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let original_length = bytes.len();
        self.hasher.update(bytes);
        while !bytes.is_empty() {
            let remaining = LAR_TRANSACTION_ARTIFACT_PIECE_BYTES - self.pending.len();
            let take = remaining.min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == LAR_TRANSACTION_ARTIFACT_PIECE_BYTES {
                self.emit_pending()
                    .map_err(|error| io::Error::other(format!("{error:#}")))?;
            }
        }
        Ok(original_length)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn legacy_header_atoms(value: Option<&Value>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let parsed = match value {
        Some(Value::String(raw)) => serde_json::from_str(raw).ok(),
        Some(value @ (Value::Array(_) | Value::Object(_))) => Some(value.clone()),
        _ => None,
    };
    let mut atoms = Vec::new();
    match parsed {
        Some(Value::Object(values)) => {
            for (name, value) in values {
                match value {
                    Value::Array(items) => {
                        for item in items {
                            if let Some(value) = legacy_header_value(&item) {
                                atoms.push((name.as_bytes().to_vec(), value));
                            }
                        }
                    }
                    value => {
                        if let Some(value) = legacy_header_value(&value) {
                            atoms.push((name.as_bytes().to_vec(), value));
                        }
                    }
                }
            }
        }
        Some(Value::Array(values)) => {
            for value in values {
                if let Value::Array(pair) = value {
                    if pair.len() == 2 {
                        if let (Some(name), Some(value)) =
                            (pair[0].as_str(), legacy_header_value(&pair[1]))
                        {
                            atoms.push((name.as_bytes().to_vec(), value));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    atoms
}

fn legacy_header_value(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::String(value) => Some(value.as_bytes().to_vec()),
        Value::Bool(_) | Value::Number(_) => Some(value.to_string().into_bytes()),
        _ => None,
    }
}

fn legacy_header_id(atoms: &[(Vec<u8>, Vec<u8>)]) -> Option<String> {
    if atoms.is_empty() {
        return None;
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"alex-legacy-header-block-v1\0");
    for (name, value) in atoms {
        hasher.update(&(name.len() as u64).to_le_bytes());
        hasher.update(name);
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value);
    }
    Some(format!(
        "legacy-header-blake3:{}",
        hasher.finalize().to_hex()
    ))
}

fn legacy_record_id(domain: &str, bytes: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"alex-legacy-transaction-v1\0");
    hasher.update(domain.as_bytes());
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    format!("legacy-{domain}-blake3:{}", hasher.finalize().to_hex())
}

fn legacy_row_bytes(row: &Value, name: &str) -> Option<Vec<u8>> {
    row[name].as_str().map(|value| value.as_bytes().to_vec())
}

fn legacy_row_u64(row: &Value, name: &str) -> u64 {
    row[name]
        .as_u64()
        .or_else(|| {
            row[name]
                .as_i64()
                .and_then(|value| u64::try_from(value).ok())
        })
        .unwrap_or_default()
}

fn legacy_row_bool(row: &Value, name: &str) -> Option<bool> {
    row[name]
        .as_bool()
        .or_else(|| row[name].as_i64().map(|value| value != 0))
}

fn legacy_exchange_metadata_json(row: &Value) -> Value {
    let bytes = |name: &str| legacy_row_bytes(row, name).as_deref().map(bytes_json);
    json!({
        "ts_request_ms": row["ts_request_ms"].as_i64(),
        "ts_response_ms": row["ts_response_ms"].as_i64(),
        "harness": bytes("harness"),
        "client_format": bytes("client_format"),
        "upstream_format": bytes("upstream_format"),
        "method": bytes("method"),
        "path": bytes("path"),
        "streamed": legacy_row_bool(row, "streamed"),
        "status": row["status"].as_i64(),
        "cost_usd_f64_bits": row["cost_usd"].as_f64().map(f64::to_bits),
        "billing_bucket": bytes("billing_bucket"),
        "error_kind": bytes("error_kind"),
        "error_code": bytes("error_code"),
        "substituted": legacy_row_bool(row, "substituted").unwrap_or(false),
        "original_model": bytes("original_model"),
        "served_model": bytes("served_model"),
        "substitution_reason": bytes("substitution_reason"),
        "injected": legacy_row_bool(row, "injected").unwrap_or(false),
        "fixture_name": bytes("fixture_name"),
        "attempts_json": bytes("attempts"),
        "original_account_id": bytes("original_account_id"),
        "served_account_id": bytes("served_account_id"),
        "subscription_identity": bytes("subscription_identity"),
        "via_dario": legacy_row_bool(row, "via_dario").unwrap_or(false),
        "dario_generation": bytes("dario_generation"),
        "tags_json": bytes("tags_json"),
        "client_ip": bytes("client_ip"),
        "key_fingerprint": bytes("key_fingerprint"),
        "reasoning_effort": bytes("reasoning_effort"),
        "thinking_budget": row["thinking_budget"].as_i64(),
        "input_tokens": row["input_tokens"].as_i64(),
        "cached_input_tokens": row["cached_input_tokens"].as_i64(),
        "cache_creation_tokens": row["cache_creation_tokens"].as_i64(),
        "output_tokens": row["output_tokens"].as_i64(),
        "reasoning_tokens": row["reasoning_tokens"].as_i64(),
        "unknown_attributes": [],
    })
}

/// Stream one trace from a clean standalone/sealed archive. The archive is the
/// only source of truth; missing referenced canonical records are fatal.
pub fn write_archive_transaction<R: Read + Seek, W: Write>(
    source: &mut ArchiveReader<R>,
    trace_id: &str,
    output: W,
) -> Result<LarTransactionExportReport> {
    write_transaction(source, trace_id, "canonical", output)
}

/// Used only after the caller has deliberately synthesized a canonical graph
/// from legacy rows. Keeping the fidelity argument constrained here prevents a
/// legacy transaction from being mislabeled as captured canonical evidence.
pub fn write_synthesized_legacy_transaction<R: Read + Seek, W: Write>(
    source: &mut ArchiveReader<R>,
    trace_id: &str,
    output: W,
) -> Result<LarTransactionExportReport> {
    write_transaction(source, trace_id, "synthesized_legacy", output)
}

#[derive(Clone)]
struct ArchiveStageProjection {
    stage: Stage,
    exchange_content_id: String,
    exchange_ordinal: usize,
    ordinal_within_exchange: usize,
    capture_sequence: u64,
    tool_id: Option<String>,
    tool_phase: Option<String>,
    supplement_trace_id: Option<String>,
}

fn archive_stage_projection<R: Read + Seek>(
    source: &ArchiveReader<R>,
    exchanges: &[alex_lar::Exchange],
) -> Result<Vec<ArchiveStageProjection>> {
    let mut output = Vec::new();
    for (exchange_ordinal, exchange) in exchanges.iter().enumerate() {
        let exchange_stages = exchange
            .data
            .stages
            .iter()
            .map(|stage_id| {
                source
                    .stage(stage_id)
                    .cloned()
                    .with_context(|| format!("transaction exchange is missing stage {stage_id}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let supplement = if exchange_ordinal == 0 {
            None
        } else {
            parse_tool_supplement(exchange, &exchange_stages)?
        };
        for (stage_ordinal, stage) in exchange_stages.into_iter().enumerate() {
            output.push(ArchiveStageProjection {
                exchange_content_id: exchange.id.to_string(),
                exchange_ordinal,
                ordinal_within_exchange: stage_ordinal,
                capture_sequence: if supplement.is_some() {
                    exchange
                        .data
                        .capture_sequence
                        .saturating_add(stage_ordinal as u64)
                } else {
                    stage_ordinal as u64
                },
                tool_id: supplement
                    .as_ref()
                    .map(|value| value.provenance.tool_id.clone()),
                tool_phase: supplement
                    .as_ref()
                    .map(|value| value.provenance.phase.clone()),
                supplement_trace_id: supplement
                    .as_ref()
                    .map(|value| value.supplement_trace_id.clone()),
                stage,
            });
        }
    }
    Ok(output)
}

fn write_transaction<R: Read + Seek, W: Write>(
    source: &mut ArchiveReader<R>,
    trace_id: &str,
    fidelity: &str,
    output: W,
) -> Result<LarTransactionExportReport> {
    let exchanges =
        crate::lar_tool_timeline::canonical_exchange_timeline(source, trace_id.as_bytes())?
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
    if exchanges.is_empty() {
        bail!("trace {trace_id} has no canonical exchange");
    }
    let stage_projection = archive_stage_projection(source, &exchanges)?;
    let total_stages = stage_projection.len();
    if total_stages > MAX_TRANSACTION_STAGES {
        bail!(
            "transaction stage count exceeds limit ({} > {MAX_TRANSACTION_STAGES})",
            total_stages
        );
    }
    let exchange_metadata_present = source.exchange_metadata(&exchanges[0].id).is_some();
    let mut limitations = vec![
        "application-level HTTP/LLM replay only; TCP, TLS, HTTP/2 frame, and connection timing were not captured",
        "no HTTP or provider framing is invented; stream replay uses only observed reads or recorded parsed-frame ranges",
        "headers contain the redacted bytes retained at capture time; secrets removed at capture cannot be reconstructed",
    ];
    if fidelity == "synthesized_legacy" {
        limitations.extend([
            "legacy stage order was synthesized from three body slots rather than captured attempts",
            "legacy header order, duplicate fields, trailers, and stream timing may be unavailable",
        ]);
    }
    let mut writer = JsonSequenceWriter::new(output);
    writer.record(&json!({
        "type": "format",
        "format": LAR_TRANSACTION_FORMAT,
        "version": LAR_TRANSACTION_VERSION,
        "trace_id": trace_id,
        "fidelity": fidelity,
        "byte_encoding": "utf8-string-or-base64-object",
        "artifact_piece_bytes": LAR_TRANSACTION_ARTIFACT_PIECE_BYTES,
        "limitations": limitations,
    }))?;
    let mut emitted_headers = HashSet::<HeaderBlockId>::new();
    let mut emitted_streams = HashSet::<StreamIndexId>::new();
    let mut artifact_order = Vec::<ManifestId>::new();
    let mut artifact_ids = HashSet::<ManifestId>::new();
    writer.record(&archive_timeline_json(
        &exchanges[0],
        source
            .exchange_metadata(&exchanges[0].id)
            .map(|value| &value.data),
        &stage_projection,
    ))?;
    for (global_stage_ordinal, projected) in stage_projection.iter().enumerate() {
        let stage = &projected.stage;
        let occurrence_id = transaction_occurrence_id(
            &projected.exchange_content_id,
            projected.ordinal_within_exchange,
        );
        let mut stage_record = stage_json(
            global_stage_ordinal,
            projected.exchange_ordinal,
            projected.ordinal_within_exchange,
            &projected.exchange_content_id,
            &occurrence_id,
            &stage.id.to_string(),
            &stage.data,
        );
        let stage_object = stage_record
            .as_object_mut()
            .expect("stage records are JSON objects");
        stage_object.insert("capture_sequence".into(), projected.capture_sequence.into());
        stage_object.insert(
            "tool_id".into(),
            projected
                .tool_id
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        stage_object.insert(
            "tool_phase".into(),
            projected
                .tool_phase
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        stage_object.insert(
            "supplement_trace_id".into(),
            projected
                .supplement_trace_id
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        writer.record(&stage_record)?;
        for (field, header_id) in [
            ("request_headers", stage.data.request_headers_ref),
            ("response_headers", stage.data.response_headers_ref),
            ("trailers", stage.data.trailers_ref),
        ] {
            let Some(header_id) = header_id else {
                continue;
            };
            if !emitted_headers.insert(header_id) {
                continue;
            }
            let block = source.header_block(&header_id).with_context(|| {
                format!(
                    "stage {} references missing header block {header_id}",
                    stage.id
                )
            })?;
            writer.record(&json!({
            "type": "header_block",
            "content_id": header_id.to_string(),
            "first_reference": {"stage_id": stage.id.to_string(), "field": field},
            "fidelity": header_fidelity_name(block.fidelity),
            "atoms": block.atoms.iter().enumerate().map(|(ordinal, atom)| json!({
                "ordinal": ordinal,
                "name": bytes_json(&atom.original_name),
                "value": bytes_json(&atom.value),
                "flags": atom.flags,
            })).collect::<Vec<_>>(),
            }))?;
        }
        for manifest_id in [
            stage.data.request_body_manifest_ref,
            stage.data.response_body_manifest_ref,
        ]
        .into_iter()
        .flatten()
        {
            register_artifact(manifest_id, &mut artifact_ids, &mut artifact_order)?;
        }
        if let Some(stream_id) = stage.data.stream_index_ref {
            if emitted_streams.insert(stream_id) {
                let stream = source.stream_index(&stream_id).with_context(|| {
                    format!(
                        "stage {} references missing stream index {stream_id}",
                        stage.id
                    )
                })?;
                register_artifact(
                    stream.raw_body_manifest_id,
                    &mut artifact_ids,
                    &mut artifact_order,
                )?;
                writer.record(&json!({
                "type": "stream_index",
                "content_id": stream_id.to_string(),
                "stage_id": stage.id.to_string(),
                "raw_body_content_id": stream.raw_body_manifest_id.to_string(),
                "observed_reads": stream.reads.iter().map(|read| json!({
                    "byte_offset": read.byte_offset,
                    "byte_length": read.byte_length,
                    "delta_from_first_byte_ns": read.delta_from_first_byte_ns,
                })).collect::<Vec<_>>(),
                "parsed_frames": stream.frames.iter().map(|frame| json!({
                    "byte_offset": frame.byte_offset,
                    "byte_length": frame.byte_length,
                    "delta_from_first_byte_ns": frame.delta_from_first_byte_ns,
                    "parser": format!("{:?}", frame.parser),
                    "frame_kind": format!("{:?}", frame.frame_kind),
                })).collect::<Vec<_>>(),
                }))?;
            }
        }
    }

    let mut artifact_bytes = 0u64;
    let mut max_source_chunk_bytes = 0u64;
    for manifest_id in &artifact_order {
        let manifest = source_manifest(source, *manifest_id)?;
        writer.record(&json!({
            "type": "artifact_start",
            "content_id": manifest.id.to_string(),
            "total_length": manifest.total_length,
            "whole_body_hash": hex(&manifest.whole_body_hash.digest),
            "media_type": manifest.media_type.as_deref().map(bytes_json),
            "content_encoding": manifest.content_encoding.as_deref().map(bytes_json),
            "range_count": manifest.chunks.len(),
        }))?;
        let mut body_hasher = blake3::Hasher::new();
        let mut reconstructed = 0u64;
        for (ordinal, reference) in manifest.chunks.iter().enumerate() {
            writer.record(&json!({
                "type": "artifact_range",
                "content_id": manifest.id.to_string(),
                "ordinal": ordinal,
                "chunk_hash": hex(&reference.chunk_hash.digest),
                "chunk_offset": reference.chunk_offset,
                "logical_offset": reference.logical_offset,
                "length": reference.length,
            }))?;
            let chunk = source_chunk(source, &reference.chunk_hash)?;
            max_source_chunk_bytes = max_source_chunk_bytes.max(chunk.len() as u64);
            let start = usize::try_from(reference.chunk_offset)
                .context("transaction chunk offset exceeds address space")?;
            let end = usize::try_from(
                reference
                    .chunk_offset
                    .checked_add(reference.length)
                    .context("transaction chunk range overflow")?,
            )
            .context("transaction chunk end exceeds address space")?;
            let range = chunk
                .get(start..end)
                .context("transaction manifest range exceeds source chunk")?;
            for (piece_index, piece) in range
                .chunks(LAR_TRANSACTION_ARTIFACT_PIECE_BYTES)
                .enumerate()
            {
                let piece_offset = (piece_index * LAR_TRANSACTION_ARTIFACT_PIECE_BYTES) as u64;
                writer.record(&json!({
                    "type": "artifact_bytes",
                    "content_id": manifest.id.to_string(),
                    "logical_offset": reference.logical_offset + piece_offset,
                    "data_base64": base64::engine::general_purpose::STANDARD.encode(piece),
                }))?;
                body_hasher.update(piece);
                reconstructed = reconstructed.saturating_add(piece.len() as u64);
            }
        }
        if reconstructed != manifest.total_length
            || body_hasher.finalize().as_bytes() != &manifest.whole_body_hash.digest
        {
            bail!("transaction artifact {manifest_id} failed body identity verification");
        }
        artifact_bytes = artifact_bytes.saturating_add(reconstructed);
        writer.record(&json!({
            "type": "artifact_end",
            "content_id": manifest.id.to_string(),
            "total_length": reconstructed,
            "verified": true,
        }))?;
    }

    let output_bytes_before_end = writer.bytes_written;
    writer.record(&json!({
        "type": "end",
        "trace_id": trace_id,
        "exchanges": exchanges.len(),
        "stages": total_stages,
        "header_blocks": emitted_headers.len(),
        "artifacts": artifact_order.len(),
        "artifact_bytes": artifact_bytes,
        "stream_indexes": emitted_streams.len(),
        "bytes_before_end_record": output_bytes_before_end,
        "complete": true,
    }))?;
    Ok(LarTransactionExportReport {
        format: LAR_TRANSACTION_FORMAT,
        version: LAR_TRANSACTION_VERSION,
        trace_id: trace_id.into(),
        fidelity: fidelity.into(),
        exchanges: exchanges.len() as u64,
        exchange_metadata_present,
        stages: total_stages as u64,
        header_blocks: emitted_headers.len() as u64,
        artifacts: artifact_order.len() as u64,
        artifact_bytes,
        stream_indexes: emitted_streams.len() as u64,
        output_bytes: writer.bytes_written,
        max_source_chunk_bytes,
        output_piece_bytes: LAR_TRANSACTION_ARTIFACT_PIECE_BYTES,
        limitations,
    })
}

fn register_artifact(
    id: ManifestId,
    seen: &mut HashSet<ManifestId>,
    order: &mut Vec<ManifestId>,
) -> Result<()> {
    if seen.insert(id) {
        if order.len() >= MAX_TRANSACTION_ARTIFACTS {
            bail!("transaction artifact count exceeds limit ({MAX_TRANSACTION_ARTIFACTS})");
        }
        order.push(id);
    }
    Ok(())
}

fn source_manifest<R: Read + Seek>(
    source: &ArchiveReader<R>,
    id: ManifestId,
) -> Result<alex_lar::BodyManifest> {
    source
        .manifest(&id)
        .cloned()
        .with_context(|| format!("transaction source is missing manifest {id}"))
}

fn source_chunk<R: Read + Seek>(
    source: &mut ArchiveReader<R>,
    hash: &ChunkHash,
) -> Result<Vec<u8>> {
    source.read_chunk(hash).map_err(anyhow::Error::new)
}

fn archive_timeline_json(
    exchange: &alex_lar::Exchange,
    metadata: Option<&ExchangeMetadataData>,
    stages: &[ArchiveStageProjection],
) -> Value {
    json!({
        "type": "transaction_timeline",
        "base_exchange_content_id": exchange.id.to_string(),
        "trace_id": bytes_json(&exchange.data.trace_id),
        "session_id": exchange.data.session_id.as_deref().map(bytes_json),
        "run_id": exchange.data.run_id.as_deref().map(bytes_json),
        "parent_trace_id": exchange.data.parent_trace_id.as_deref().map(bytes_json),
        "capture_sequence": exchange.data.capture_sequence,
        "wall_time_ns": exchange.data.wall_time_ns,
        "monotonic_delta_ns": exchange.data.monotonic_delta_ns,
        "clock_id": exchange.data.clock_id.as_deref().map(bytes_json),
        "ordered_stage_content_ids": stages.iter().map(|stage| stage.stage.id.to_string()).collect::<Vec<_>>(),
        "supplements": stages.iter().filter_map(|stage| stage.supplement_trace_id.as_deref().map(|trace_id| json!({
            "trace_id": trace_id,
            "exchange_content_id": stage.exchange_content_id,
            "stage_content_id": stage.stage.id.to_string(),
            "tool_id": stage.tool_id,
            "phase": stage.tool_phase,
        }))).collect::<Vec<_>>(),
        "metadata": metadata.map(exchange_metadata_json),
    })
}

fn interchange_timeline_json(exchange: &LarInterchangeTrace) -> Value {
    json!({
        "type": "transaction_timeline",
        "base_exchange_content_id": exchange.exchange_id,
        "trace_id": bytes_json(&exchange.trace_id),
        "session_id": exchange.session_id.as_deref().map(bytes_json),
        "run_id": exchange.run_id.as_deref().map(bytes_json),
        "parent_trace_id": exchange.parent_trace_id.as_deref().map(bytes_json),
        "capture_sequence": exchange.capture_sequence,
        "wall_time_ns": exchange.wall_time_ns,
        "monotonic_delta_ns": exchange.monotonic_delta_ns,
        "clock_id": exchange.clock_id.as_deref().map(bytes_json),
        "ordered_stage_content_ids": exchange.stages.iter().map(|stage| stage.record_id.as_str()).collect::<Vec<_>>(),
        "supplements": exchange.stages.iter().filter_map(|stage| stage.supplement_trace_id.as_deref().map(|trace_id| json!({
            "trace_id": trace_id,
            "exchange_content_id": stage.supplement_exchange_id,
            "stage_content_id": stage.record_id,
            "tool_id": stage.tool_id,
            "phase": stage.tool_phase,
        }))).collect::<Vec<_>>(),
        "metadata": exchange.metadata.as_ref().map(exchange_metadata_json),
    })
}

fn exchange_metadata_json(data: &ExchangeMetadataData) -> Value {
    json!({
        "ts_request_ms": data.ts_request_ms,
        "ts_response_ms": data.ts_response_ms,
        "harness": data.harness.as_deref().map(bytes_json),
        "client_format": data.client_format.as_deref().map(bytes_json),
        "upstream_format": data.upstream_format.as_deref().map(bytes_json),
        "method": data.method.as_deref().map(bytes_json),
        "path": data.path.as_deref().map(bytes_json),
        "streamed": data.streamed,
        "status": data.status,
        "cost_usd_f64_bits": data.cost_usd_bits,
        "billing_bucket": data.billing_bucket.as_deref().map(bytes_json),
        "error_kind": data.error_kind.as_deref().map(bytes_json),
        "error_code": data.error_code.as_deref().map(bytes_json),
        "substituted": data.substituted,
        "original_model": data.original_model.as_deref().map(bytes_json),
        "served_model": data.served_model.as_deref().map(bytes_json),
        "substitution_reason": data.substitution_reason.as_deref().map(bytes_json),
        "injected": data.injected,
        "fixture_name": data.fixture_name.as_deref().map(bytes_json),
        "attempts_json": data.attempts_json.as_deref().map(bytes_json),
        "original_account_id": data.original_account_id.as_deref().map(bytes_json),
        "served_account_id": data.served_account_id.as_deref().map(bytes_json),
        "subscription_identity": data.subscription_identity.as_deref().map(bytes_json),
        "via_dario": data.via_dario,
        "dario_generation": data.dario_generation.as_deref().map(bytes_json),
        "tags_json": data.tags_json.as_deref().map(bytes_json),
        "client_ip": data.client_ip.as_deref().map(bytes_json),
        "key_fingerprint": data.key_fingerprint.as_deref().map(bytes_json),
        "reasoning_effort": data.reasoning_effort.as_deref().map(bytes_json),
        "thinking_budget": data.thinking_budget,
        "input_tokens": data.input_tokens,
        "cached_input_tokens": data.cached_input_tokens,
        "cache_creation_tokens": data.cache_creation_tokens,
        "output_tokens": data.output_tokens,
        "reasoning_tokens": data.reasoning_tokens,
        "unknown_attributes": data.unknown_attributes.iter().map(|attribute| json!({
            "key": bytes_json(&attribute.key),
            "value": bytes_json(&attribute.value),
        })).collect::<Vec<_>>(),
    })
}

fn stage_json(
    timeline_ordinal: usize,
    exchange_ordinal: usize,
    ordinal_within_exchange: usize,
    exchange_content_id: &str,
    occurrence_id: &str,
    content_id: &str,
    data: &StageData,
) -> Value {
    json!({
        "type": "stage",
        "timeline_ordinal": timeline_ordinal,
        "exchange_ordinal": exchange_ordinal,
        "ordinal_within_exchange": ordinal_within_exchange,
        "exchange_content_id": exchange_content_id,
        "occurrence_id": occurrence_id,
        "content_id": content_id,
        "kind": format!("{:?}", data.kind),
        "attempt_number": data.attempt_number,
        "wall_time_ns": data.wall_time_ns,
        "monotonic_delta_ns": data.monotonic_delta_ns,
        "first_byte_delta_ns": data.first_byte_delta_ns,
        "last_byte_delta_ns": data.last_byte_delta_ns,
        "request_headers_content_id": data.request_headers_ref.map(|id| id.to_string()),
        "request_body_content_id": data.request_body_manifest_ref.map(|id| id.to_string()),
        "response_headers_content_id": data.response_headers_ref.map(|id| id.to_string()),
        "response_body_content_id": data.response_body_manifest_ref.map(|id| id.to_string()),
        "trailers_content_id": data.trailers_ref.map(|id| id.to_string()),
        "stream_index_content_id": data.stream_index_ref.map(|id| id.to_string()),
        "provider": data.provider.as_deref().map(bytes_json),
        "requested_model": data.requested_model.as_deref().map(bytes_json),
        "routed_model": data.routed_model.as_deref().map(bytes_json),
        "account_id": data.account_id.as_deref().map(bytes_json),
        "routing_reason": data.routing_reason.as_deref().map(bytes_json),
        "status_code": data.status_code,
        "usage": data.usage.as_ref().map(|usage| json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cached_tokens": usage.cached_tokens,
            "reasoning_tokens": usage.reasoning_tokens,
        })),
        "cost_nanos": data.cost_nanos,
        "cost_currency": data.cost_currency.as_deref().map(bytes_json),
        "error_class": data.error_class.as_deref().map(bytes_json),
        "error_message": data.error_message.as_deref().map(bytes_json),
    })
}

fn transaction_occurrence_id(exchange_content_id: &str, ordinal: usize) -> String {
    format!("{exchange_content_id}#stage-{ordinal}")
}

fn bytes_json(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => Value::String(text.into()),
        Err(_) => json!({
            "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            "length": bytes.len(),
        }),
    }
}

fn header_fidelity_name(value: HeaderFidelity) -> &'static str {
    match value {
        HeaderFidelity::Exact => "exact",
        HeaderFidelity::LegacyOrderUnknown => "legacy_order_unknown",
        HeaderFidelity::LegacyCasingUnknown => "legacy_casing_unknown",
        HeaderFidelity::LegacyOrderAndCasingUnknown => "legacy_order_and_casing_unknown",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct JsonSequenceWriter<W> {
    output: W,
    bytes_written: u64,
}

impl<W: Write> JsonSequenceWriter<W> {
    fn new(output: W) -> Self {
        Self {
            output,
            bytes_written: 0,
        }
    }

    fn record<T: Serialize>(&mut self, value: &T) -> Result<()> {
        self.output.write_all(&[0x1e])?;
        self.bytes_written = self.bytes_written.saturating_add(1);
        let mut counting = CountingWriter {
            output: &mut self.output,
            written: 0,
        };
        serde_json::to_writer(&mut counting, value)?;
        self.bytes_written = self.bytes_written.saturating_add(counting.written);
        self.output.write_all(b"\n")?;
        self.bytes_written = self.bytes_written.saturating_add(1);
        Ok(())
    }
}

struct CountingWriter<'a, W> {
    output: &'a mut W,
    written: u64,
}

impl<W: Write> Write for CountingWriter<'_, W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.output.write(buffer)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}
