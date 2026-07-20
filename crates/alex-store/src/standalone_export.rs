//! Transitive, self-contained export of already-captured LAR exchanges.
//!
//! This path copies normalized records and reconstructs each referenced body
//! exactly once into the destination writer. It is deliberately separate from
//! interchange exporters: a `.lar` export must not collapse retries, Dario
//! stages, upstream responses, stream timing, or conversation graph records
//! into the three columns available in the legacy `traces` table.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, Write};

use alex_lar::{
    ArchiveReader, ArchiveWriter, ArtifactRangeRef, ConversationEntry, ConversationEntryId,
    Exchange, ExchangeMetadataData, Generation, GenerationId, HeaderBlockId, Limits, ManifestId,
    Stage, StreamIndex, StreamIndexId, TurnView,
};
use anyhow::{bail, Context, Result};
use rusqlite::OptionalExtension;

use crate::{lar_archive_ops::resolved_catalog_path, Store};

impl Store {
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
        let source = {
            let conn = self.conn.lock().unwrap();
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
                exchange_source
            } else {
                // Compatibility with catalogs written before explicit
                // trace-to-exchange ownership was introduced.
                conn.query_row(
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
                .optional()?
            }
        };
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

        let mut manifests = HashMap::<ManifestId, ManifestId>::new();
        let mut headers = HashMap::<HeaderBlockId, HeaderBlockId>::new();
        let mut streams = HashMap::<StreamIndexId, StreamIndexId>::new();
        let mut destination_stages = Vec::with_capacity(exchange.data.stages.len());
        for source_stage_id in &exchange.data.stages {
            let source_stage = source.stage(source_stage_id).cloned().with_context(|| {
                format!("exchange {trace_id} is missing stage {source_stage_id}")
            })?;
            let mut data = source_stage.data.clone();
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
        Ok(true)
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
    // Read one logical body at a time. The destination writer immediately
    // chunks/compresses it, so memory is bounded by the largest single artifact
    // rather than by the selected session/corpus.
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
    let bytes = if cataloged {
        store
            .read_lar_manifest_body(&source_id.to_string())
            .with_context(|| format!("reconstructing exported LAR manifest {source_id}"))?
    } else {
        source
            .read_body(&source_id)
            .map_err(anyhow::Error::new)
            .with_context(|| format!("reconstructing source LAR manifest {source_id}"))?
    };
    let destination_id = destination
        .append_body_with_metadata(&bytes, source_metadata.0, source_metadata.1)
        .map_err(anyhow::Error::new)?;
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
