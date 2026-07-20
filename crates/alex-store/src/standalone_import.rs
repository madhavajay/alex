//! Transactional attachment of immutable, sealed standalone LAR archives.
//!
//! Import never rewrites or copies the source archive. The archive is fully
//! opened and every chunk/body is read back before one SQLite transaction
//! publishes its file, content, header, stage, artifact, trace, and session
//! anchors. Repeating the operation repairs missing catalog rows and otherwise
//! reuses content IDs without appending body bytes.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

use alex_lar::{
    ArchiveReader, ArtifactRangeRef, BodyManifest, ChunkRecordDescriptor, ConversationEntry,
    ConversationEntryId, ConversationEntryKind, ConversationRole, Exchange, ExchangeMetadataData,
    FileHeader, FileRole, Generation, GenerationData, GenerationId, GenerationReason, HeaderAtom,
    HeaderBlock, HeaderBlockId, HeaderFidelity, Limits, ManifestId, RecoveryStatus, Stage,
    StageKind, TurnView, TurnViewData, REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS,
};
use anyhow::{bail, Context, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use crate::{
    lar_archive_ops::{record_lar_file_identity, LarFileIdentity},
    Store,
};

#[derive(Clone, Debug)]
pub struct LarStandaloneImportOptions {
    pub limits: Limits,
    /// Standalone CLI imports normally materialize minimal trace rows so the
    /// imported archive is immediately discoverable. Backup restore disables
    /// this: its complete SQLite rows are published only after the archive has
    /// passed validation and been attached.
    pub insert_trace_rows: bool,
}

impl Default for LarStandaloneImportOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            insert_trace_rows: true,
        }
    }
}

/// A body edge carried by the versioned trace-backup envelope. Standalone LAR
/// exchange records cover trace artifacts; this side table supplies the
/// equivalent owner edge for tool arguments and results.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub struct LarBackupArtifactRef {
    pub owner_kind: String,
    pub owner_id: String,
    pub artifact_kind: String,
    #[serde(default)]
    pub stage_id: String,
    pub blake3: String,
    pub total_length: u64,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarStandaloneImportReport {
    pub archive_set_uuid: String,
    pub file_uuid: String,
    pub catalog_path: String,
    pub source_size: u64,
    pub source_blake3: String,
    pub already_attached: bool,
    pub relocated: bool,
    pub chunks: u64,
    pub chunks_reused: u64,
    pub manifests: u64,
    pub manifests_reused: u64,
    pub header_blocks: u64,
    pub stages: u64,
    pub stages_indexed: u64,
    pub stream_indexes: u64,
    pub exchanges: u64,
    pub traces_inserted: u64,
    pub conversation_entries: u64,
    pub conversation_generations: u64,
    pub conversation_turn_views: u64,
}

struct ValidatedStage {
    trace_id: String,
    sequence: u64,
    stage: Stage,
}

struct ValidatedExchange {
    exchange: Exchange,
    metadata: Option<ExchangeMetadataData>,
    trace_id: String,
    session_id: Option<String>,
    run_id: Option<String>,
    request_headers_json: Option<String>,
    response_headers_json: Option<String>,
    stages: Vec<Stage>,
}

struct ValidatedTurnView {
    turn: TurnView,
    trace_id: String,
    session_id: String,
}

struct ValidatedArchive {
    header: FileHeader,
    catalog_path: String,
    source_size: u64,
    source_hash: [u8; 32],
    chunks: Vec<ChunkRecordDescriptor>,
    manifests: Vec<BodyManifest>,
    header_blocks: Vec<HeaderBlock>,
    stages: Vec<ValidatedStage>,
    stream_indexes: usize,
    exchanges: Vec<ValidatedExchange>,
    conversation_entries: Vec<ConversationEntry>,
    conversation_generations: Vec<Generation>,
    conversation_turn_views: Vec<ValidatedTurnView>,
}

impl Store {
    /// Validate and attach one immutable standalone archive. Passing a new path
    /// for an already known file UUID performs a validated relocation/reattach.
    pub fn import_sealed_lar_archive(
        &self,
        path: impl AsRef<Path>,
        options: &LarStandaloneImportOptions,
    ) -> Result<LarStandaloneImportReport> {
        let archive = validate_archive(&self.data_dir, path.as_ref(), &options.limits)?;
        self.publish_validated_archive(archive, options.insert_trace_rows)
    }

    fn publish_validated_archive(
        &self,
        archive: ValidatedArchive,
        insert_trace_rows: bool,
    ) -> Result<LarStandaloneImportReport> {
        let file_uuid = hex(&archive.header.file_uuid);
        let archive_set_uuid = format!("standalone-{file_uuid}");
        let source_hash_text = hex(&archive.source_hash);
        let archive_description =
            format!("attached standalone LAR archive blake3:{source_hash_text}");
        let created_at_ms = ns_to_ms(archive.header.created_at_ns, "archive creation time")?;
        let source_size = to_i64(archive.source_size, "archive size")?;
        let now = chrono::Utc::now().timestamp_millis();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing_file: Option<(String, String, String)> = tx
            .query_row(
                "SELECT archive_set_uuid, role, path FROM lar_files WHERE file_uuid=?1",
                [&file_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((stored_set, role, _)) = &existing_file {
            if stored_set != &archive_set_uuid || role != "standalone" {
                bail!("LAR file UUID is already attached with a different archive identity");
            }
        }

        // A first attachment must never graft authoritative LAR artifacts onto
        // an unrelated legacy or already-cataloged trace row. Reattaching the
        // same immutable file is idempotent (and is also how relocation works).
        if insert_trace_rows && existing_file.is_none() {
            for value in &archive.exchanges {
                if tx
                    .query_row(
                        "SELECT 1 FROM traces WHERE id=?1",
                        [&value.trace_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some()
                {
                    bail!(
                        "trace {} already exists; refusing to replace it with a standalone archive",
                        value.trace_id
                    );
                }
            }
        }
        if existing_file.is_some() {
            for value in &archive.exchanges {
                let stored: Option<(String, String, i64)> = tx
                    .query_row(
                        "SELECT exchange_id, file_uuid,
                                (SELECT COUNT(*) FROM lar_stage_records s
                                  WHERE s.trace_id=e.trace_id)
                           FROM lar_exchange_records e WHERE trace_id=?1",
                        [&value.trace_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()?;
                let expected_stage_count =
                    to_i64(value.stages.len() as u64, "exchange stage count")?;
                if stored.as_ref()
                    != Some(&(
                        value.exchange.id.to_string(),
                        file_uuid.clone(),
                        expected_stage_count,
                    ))
                {
                    bail!(
                        "trace {} has an incompatible existing standalone exchange catalog",
                        value.trace_id
                    );
                }
            }
        }
        let existing_description: Option<String> = tx
            .query_row(
                "SELECT description FROM lar_archive_sets WHERE archive_set_uuid=?1",
                [&archive_set_uuid],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if existing_description
            .as_deref()
            .is_some_and(|description| description != archive_description)
        {
            bail!("LAR file UUID is already attached to different immutable archive bytes");
        }
        let occupied: Option<String> = tx
            .query_row(
                "SELECT file_uuid FROM lar_files WHERE path=?1 AND file_uuid!=?2 LIMIT 1",
                params![archive.catalog_path, file_uuid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(occupied) = occupied {
            bail!("archive path is already attached as LAR file {occupied}");
        }

        tx.execute(
            "INSERT INTO lar_archive_sets
               (archive_set_uuid, created_at_ms, updated_at_ms, state, description)
             VALUES (?1, ?2, ?3, 'sealed', ?4)
             ON CONFLICT(archive_set_uuid) DO UPDATE SET
               updated_at_ms=excluded.updated_at_ms, state='sealed'",
            params![archive_set_uuid, created_at_ms, now, archive_description],
        )?;
        tx.execute(
            "INSERT INTO lar_files
               (file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, required_feature_bits, optional_feature_bits,
                created_at_ms, sealed_at_ms, size_bytes)
             VALUES (?1, ?2, 'standalone', ?3, 'sealed', ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(file_uuid) DO UPDATE SET
               path=excluded.path, state='sealed', container_major=excluded.container_major,
               container_minor=excluded.container_minor,
               required_feature_bits=excluded.required_feature_bits,
               optional_feature_bits=excluded.optional_feature_bits,
               sealed_at_ms=excluded.sealed_at_ms, size_bytes=excluded.size_bytes",
            params![
                file_uuid,
                archive_set_uuid,
                archive.catalog_path,
                archive.header.container_major,
                archive.header.container_minor,
                archive.header.required_feature_bits,
                archive.header.optional_feature_bits,
                created_at_ms,
                now,
                source_size,
            ],
        )?;
        record_lar_file_identity(
            &tx,
            &file_uuid,
            &LarFileIdentity {
                size: archive.source_size,
                blake3: archive.source_hash,
            },
            "standalone_import",
            now,
        )?;

        let mut manifest_map = HashMap::<ManifestId, String>::new();
        let mut body_map =
            HashMap::<([u8; 32], u64, Option<Vec<u8>>, Option<Vec<u8>>), String>::new();
        let mut new_manifests = Vec::new();
        let mut manifests_reused = 0u64;
        for manifest in &archive.manifests {
            let identity = (
                manifest.whole_body_hash.digest,
                manifest.total_length,
                manifest.media_type.clone(),
                manifest.content_encoding.clone(),
            );
            let existing = find_catalog_manifest(&tx, manifest)?;
            let (catalog_id, reused) = if let Some(catalog_id) = existing {
                (catalog_id, true)
            } else if let Some(catalog_id) = body_map.get(&identity) {
                (catalog_id.clone(), true)
            } else {
                let catalog_id = manifest.id.to_string();
                body_map.insert(identity, catalog_id.clone());
                new_manifests.push(manifest);
                (catalog_id, false)
            };
            manifest_map.insert(manifest.id, catalog_id);
            if reused {
                manifests_reused += 1;
            }
        }

        // Only catalog chunks needed by genuinely new logical bodies. A
        // standalone archive may chunk an already-known body differently; in
        // that case its immutable source remains attached, but its redundant
        // physical chunks do not become canonical catalog content.
        let required_chunks: HashSet<[u8; 32]> = new_manifests
            .iter()
            .flat_map(|manifest| {
                manifest
                    .chunks
                    .iter()
                    .map(|reference| reference.chunk_hash.digest)
            })
            .collect();
        let mut inserted_chunks = 0u64;
        for descriptor in &archive.chunks {
            if required_chunks.contains(&descriptor.hash.digest)
                && insert_chunk(&tx, &file_uuid, *descriptor, now)?
            {
                inserted_chunks += 1;
            }
        }
        for required in &required_chunks {
            if !archive
                .chunks
                .iter()
                .any(|descriptor| descriptor.hash.digest == *required)
            {
                bail!("standalone manifest references a chunk absent from its archive");
            }
        }
        let chunks_reused = (archive.chunks.len() as u64).saturating_sub(inserted_chunks);

        for manifest in new_manifests {
            insert_manifest(&tx, &file_uuid, manifest, now)?;
        }

        for block in &archive.header_blocks {
            attach_header_block(&tx, &file_uuid, block, now)?;
        }

        let mut stages_indexed = 0u64;
        let mut stage_occurrences = HashMap::<(String, u64), String>::new();
        for value in &archive.stages {
            let (stage_id, inserted) = attach_stage(&tx, &file_uuid, value, &manifest_map)?;
            stages_indexed += inserted as u64;
            if stage_occurrences
                .insert((value.trace_id.clone(), value.sequence), stage_id)
                .is_some()
            {
                bail!("standalone archive contains a duplicate trace stage occurrence");
            }
        }

        let mut traces_inserted = 0u64;
        for value in &archive.exchanges {
            traces_inserted += attach_exchange(
                &tx,
                &file_uuid,
                value,
                &manifest_map,
                &stage_occurrences,
                &source_hash_text,
                now,
                insert_trace_rows,
            )? as u64;
        }
        let (entries, generations, turns) = remap_conversation_graph(
            &archive.conversation_entries,
            &archive.conversation_generations,
            &archive.conversation_turn_views,
            &manifest_map,
        )?;
        for entry in &entries {
            attach_conversation_entry(&tx, entry, now)?;
        }
        for generation in &generations {
            attach_conversation_generation(&tx, generation, now)?;
        }
        for turn in &turns {
            attach_conversation_turn(&tx, turn, now)?;
        }
        tx.commit()?;

        let previous_path = existing_file.as_ref().map(|(_, _, path)| path.as_str());
        Ok(LarStandaloneImportReport {
            archive_set_uuid,
            file_uuid,
            catalog_path: archive.catalog_path.clone(),
            source_size: archive.source_size,
            source_blake3: source_hash_text,
            already_attached: previous_path == Some(archive.catalog_path.as_str()),
            relocated: previous_path.is_some_and(|path| path != archive.catalog_path),
            chunks: archive.chunks.len() as u64,
            chunks_reused,
            manifests: archive.manifests.len() as u64,
            manifests_reused,
            header_blocks: archive.header_blocks.len() as u64,
            stages: archive.stages.len() as u64,
            stages_indexed,
            stream_indexes: archive.stream_indexes as u64,
            exchanges: archive.exchanges.len() as u64,
            traces_inserted,
            conversation_entries: archive.conversation_entries.len() as u64,
            conversation_generations: archive.conversation_generations.len() as u64,
            conversation_turn_views: archive.conversation_turn_views.len() as u64,
        })
    }

    /// Publish tool/body ownership edges after a backup's complete SQLite rows
    /// have been restored. Content is resolved by hash and length because an
    /// existing catalog may deduplicate the imported archive to another local
    /// manifest ID.
    pub fn attach_validated_backup_artifacts(
        &self,
        artifacts: &[LarBackupArtifactRef],
        source_fingerprint: &str,
    ) -> Result<u64> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut inserted = 0u64;
        for artifact in artifacts {
            if artifact.owner_kind != "tool_call" {
                bail!(
                    "backup artifact owner kind is not supported: {}",
                    artifact.owner_kind
                );
            }
            if !matches!(
                artifact.artifact_kind.as_str(),
                "tool_arguments" | "tool_result"
            ) {
                bail!(
                    "backup tool artifact kind is not supported: {}",
                    artifact.artifact_kind
                );
            }
            let owner_exists = tx
                .query_row(
                    "SELECT 1 FROM tool_calls WHERE id=?1",
                    [&artifact.owner_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !owner_exists {
                bail!("backup artifact owner is missing: {}", artifact.owner_id);
            }
            let digest = decode_blake3(&artifact.blake3)?;
            let manifest_id: String = tx
                .query_row(
                    "SELECT manifest_id FROM lar_manifests
                     WHERE hash_algorithm='blake3' AND whole_body_hash=?1
                       AND total_length=?2 AND state='ready'",
                    params![
                        digest.as_slice(),
                        to_i64(artifact.total_length, "backup artifact length")?
                    ],
                    |row| row.get(0),
                )
                .optional()?
                .with_context(|| {
                    format!(
                        "validated backup body is absent from the LAR catalog: {} {}",
                        artifact.owner_id, artifact.artifact_kind
                    )
                })?;
            let changed = tx.execute(
                "INSERT INTO lar_trace_artifacts
                   (owner_kind, owner_id, artifact_kind, stage_id, manifest_id,
                    source_fingerprint, fidelity, validation_state, validated_at_ms)
                 VALUES ('tool_call', ?1, ?2, ?3, ?4, ?5,
                         'standalone_validated', 'validated', ?6)
                 ON CONFLICT(owner_kind, owner_id, artifact_kind, stage_id) DO NOTHING",
                params![
                    artifact.owner_id,
                    artifact.artifact_kind,
                    artifact.stage_id,
                    manifest_id,
                    source_fingerprint,
                    now,
                ],
            )?;
            let stored: Option<String> = tx.query_row(
                "SELECT manifest_id FROM lar_trace_artifacts
                 WHERE owner_kind='tool_call' AND owner_id=?1
                   AND artifact_kind=?2 AND stage_id=?3",
                params![artifact.owner_id, artifact.artifact_kind, artifact.stage_id],
                |row| row.get(0),
            )?;
            if stored.as_deref() != Some(manifest_id.as_str()) {
                bail!(
                    "tool artifact {} {}/{} conflicts with restored backup content",
                    artifact.owner_id,
                    artifact.artifact_kind,
                    artifact.stage_id
                );
            }
            inserted += changed as u64;
        }
        tx.commit()?;
        Ok(inserted)
    }
}

fn decode_blake3(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        bail!("invalid BLAKE3 digest length in backup artifact");
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair)?;
        output[index] = u8::from_str_radix(text, 16)
            .with_context(|| "invalid BLAKE3 digest in backup artifact")?;
    }
    Ok(output)
}

fn validate_archive(data_dir: &Path, path: &Path, limits: &Limits) -> Result<ValidatedArchive> {
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("resolving standalone LAR archive {}", path.display()))?;
    if !canonical_path.is_file() {
        bail!("standalone LAR source is not a regular file");
    }
    let catalog_path = safe_catalog_path(data_dir, &canonical_path)?;
    let mut file = File::open(&canonical_path).with_context(|| {
        format!(
            "opening standalone LAR archive {}",
            canonical_path.display()
        )
    })?;
    let (source_size, source_hash) = hash_file(&mut file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut reader = ArchiveReader::open(&mut file, limits.clone())
        .map_err(anyhow::Error::new)
        .context("opening standalone LAR archive")?;
    if reader.header().file_role != FileRole::Standalone {
        bail!("LAR archive must have the standalone file role");
    }
    if reader.header().required_feature_bits & REQUIRED_FEATURE_ARCHIVE_SET_BODY_REFS != 0 {
        bail!("standalone LAR archive must not require external archive-set body references");
    }
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        bail!("standalone LAR archive must have a clean, validated sealed footer");
    }
    let header = reader.header().clone();

    let mut chunks: Vec<_> = reader.chunk_records().collect();
    chunks.sort_by_key(|value| value.hash.digest);
    for descriptor in &chunks {
        reader
            .read_chunk(&descriptor.hash)
            .map_err(anyhow::Error::new)
            .with_context(|| format!("validating standalone chunk {:?}", descriptor.hash))?;
    }

    let mut manifests: Vec<_> = reader
        .manifest_ids()
        .filter_map(|id| reader.manifest(id).cloned())
        .collect();
    manifests.sort_by_key(|value| value.id.0);
    for manifest in &manifests {
        validate_optional_text(manifest.media_type.as_deref(), "manifest media type")?;
        validate_optional_text(
            manifest.content_encoding.as_deref(),
            "manifest content encoding",
        )?;
        reader
            .write_body(&manifest.id, std::io::sink())
            .map_err(anyhow::Error::new)
            .with_context(|| format!("validating standalone manifest {}", manifest.id))?;
    }

    let mut header_blocks: Vec<_> = reader
        .header_block_ids()
        .filter_map(|id| reader.header_block(id).cloned())
        .collect();
    header_blocks.sort_by_key(|value| value.id.0);

    let mut exchanges: Vec<_> = reader
        .exchange_ids()
        .filter_map(|id| reader.exchange(id).cloned())
        .collect();
    exchanges.sort_by(|left, right| {
        (&left.data.trace_id, left.data.capture_sequence, left.id.0).cmp(&(
            &right.data.trace_id,
            right.data.capture_sequence,
            right.id.0,
        ))
    });
    let mut validated_exchanges = Vec::with_capacity(exchanges.len());
    let mut stages = Vec::new();
    for exchange in exchanges {
        let trace_id = identifier(&exchange.data.trace_id, "trace ID")?;
        let session_id = optional_identifier(exchange.data.session_id.as_deref(), "session ID")?;
        let run_id = optional_identifier(exchange.data.run_id.as_deref(), "run ID")?;
        optional_identifier(exchange.data.parent_trace_id.as_deref(), "parent trace ID")?;
        optional_identifier(exchange.data.clock_id.as_deref(), "clock ID")?;
        let mut exchange_stages = Vec::with_capacity(exchange.data.stages.len());
        for (sequence, stage_id) in exchange.data.stages.iter().enumerate() {
            let stage = reader
                .stage(stage_id)
                .cloned()
                .with_context(|| format!("exchange {trace_id} is missing stage {stage_id}"))?;
            validate_stage_text(&stage)?;
            // Stage records are content-addressed and may legitimately be
            // shared by exchanges or repeated within one exchange. Catalog
            // every occurrence; the SQLite stage key is occurrence-scoped.
            stages.push(ValidatedStage {
                trace_id: trace_id.clone(),
                sequence: sequence as u64,
                stage: stage.clone(),
            });
            exchange_stages.push(stage);
        }
        let request_headers_json = exchange_stages
            .iter()
            .find(|stage| stage.data.kind == StageKind::ClientRequest)
            .and_then(|stage| stage.data.request_headers_ref)
            .map(|id| header_block_json(&reader, id))
            .transpose()?
            .flatten();
        let response_headers_json = exchange_stages
            .iter()
            .rev()
            .find(|stage| stage.data.kind == StageKind::ClientResponse)
            .and_then(|stage| stage.data.response_headers_ref)
            .map(|id| header_block_json(&reader, id))
            .transpose()?
            .flatten();
        let metadata = reader
            .exchange_metadata(&exchange.id)
            .map(|value| value.data.clone());
        validated_exchanges.push(ValidatedExchange {
            exchange,
            metadata,
            trace_id,
            session_id,
            run_id,
            request_headers_json,
            response_headers_json,
            stages: exchange_stages,
        });
    }
    let stream_indexes = reader.stream_index_count();
    let mut conversation_entries = reader
        .conversation_entry_ids()
        .filter_map(|id| reader.conversation_entry(id).cloned())
        .collect::<Vec<_>>();
    conversation_entries.sort_by_key(|entry| entry.id.0);
    let mut conversation_generations = reader
        .generation_ids()
        .filter_map(|id| reader.generation(id).cloned())
        .collect::<Vec<_>>();
    conversation_generations.sort_by_key(|generation| generation.id.0);
    let mut conversation_turn_views = reader
        .turn_view_ids()
        .filter_map(|id| reader.turn_view(id).cloned())
        .map(|turn| {
            let trace_id = identifier(&turn.data.trace_id, "conversation turn trace ID")?;
            let exchange = reader
                .exchange_by_trace(&turn.data.trace_id)
                .with_context(|| format!("conversation turn {trace_id} has no exchange"))?;
            let session_id = exchange
                .data
                .session_id
                .as_deref()
                .map(|value| identifier(value, "conversation turn session ID"))
                .transpose()?
                .with_context(|| format!("conversation turn {trace_id} has no session ID"))?;
            Ok(ValidatedTurnView {
                turn,
                trace_id,
                session_id,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    conversation_turn_views.sort_by(|left, right| left.trace_id.cmp(&right.trace_id));
    drop(reader);

    // Reopen by the canonical catalog path after validation. A replacement or
    // mutation during the import must not make the catalog point at bytes that
    // differ from the handle we validated.
    let mut current = File::open(&canonical_path)?;
    let (current_size, current_hash) = hash_file(&mut current)?;
    if current_size != source_size || current_hash != source_hash {
        bail!("standalone LAR source changed while it was being validated");
    }

    Ok(ValidatedArchive {
        header,
        catalog_path,
        source_size,
        source_hash,
        chunks,
        manifests,
        header_blocks,
        stages,
        stream_indexes,
        exchanges: validated_exchanges,
        conversation_entries,
        conversation_generations,
        conversation_turn_views,
    })
}

/// SQLite keeps legacy header JSON as a compatibility projection for clients
/// that have not moved to stage/header-block reads yet. The ordered block in
/// the archive remains authoritative; an array of pairs preserves duplicates
/// and order whenever both name and value are representable as UTF-8.
fn header_block_json<R: Read + Seek>(
    reader: &ArchiveReader<R>,
    id: HeaderBlockId,
) -> Result<Option<String>> {
    let block = reader
        .header_block(&id)
        .with_context(|| format!("exchange references missing header block {id}"))?;
    let mut values = Vec::with_capacity(block.atoms.len());
    for atom in &block.atoms {
        let Ok(name) = std::str::from_utf8(&atom.original_name) else {
            return Ok(None);
        };
        let Ok(value) = std::str::from_utf8(&atom.value) else {
            return Ok(None);
        };
        values.push([name, value]);
    }
    serde_json::to_string(&values).map(Some).map_err(Into::into)
}

fn safe_catalog_path(data_dir: &Path, archive: &Path) -> Result<String> {
    let root = data_dir
        .canonicalize()
        .with_context(|| format!("resolving Alex data directory {}", data_dir.display()))?;
    let stored = match archive.strip_prefix(&root) {
        Ok(relative)
            if !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))) =>
        {
            relative
        }
        _ => archive,
    };
    stored
        .to_str()
        .map(str::to_owned)
        .context("standalone LAR path is not valid UTF-8")
}

fn hash_file(file: &mut File) -> Result<(u64, [u8; 32])> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut length = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        length = length
            .checked_add(read as u64)
            .context("standalone LAR size overflow")?;
    }
    Ok((length, *hasher.finalize().as_bytes()))
}

fn identifier(value: &[u8], field: &str) -> Result<String> {
    String::from_utf8(value.to_vec()).with_context(|| format!("{field} is not valid UTF-8"))
}

fn optional_identifier(value: Option<&[u8]>, field: &str) -> Result<Option<String>> {
    value.map(|value| identifier(value, field)).transpose()
}

fn validate_optional_text(value: Option<&[u8]>, field: &str) -> Result<()> {
    if let Some(value) = value {
        std::str::from_utf8(value).with_context(|| format!("{field} is not valid UTF-8"))?;
    }
    Ok(())
}

fn validate_stage_text(stage: &Stage) -> Result<()> {
    for (field, value) in [
        ("provider", stage.data.provider.as_deref()),
        ("requested model", stage.data.requested_model.as_deref()),
        ("routed model", stage.data.routed_model.as_deref()),
        ("account ID", stage.data.account_id.as_deref()),
        ("routing reason", stage.data.routing_reason.as_deref()),
        ("cost currency", stage.data.cost_currency.as_deref()),
        ("error class", stage.data.error_class.as_deref()),
        ("error message", stage.data.error_message.as_deref()),
    ] {
        validate_optional_text(value, field)?;
    }
    Ok(())
}

fn to_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).with_context(|| format!("{field} exceeds SQLite's integer range"))
}

fn ns_to_ms(value: u64, field: &str) -> Result<i64> {
    to_i64(value / 1_000_000, field)
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn insert_chunk(
    tx: &Transaction<'_>,
    file_uuid: &str,
    descriptor: ChunkRecordDescriptor,
    created_at_ms: i64,
) -> Result<bool> {
    let inserted = tx.execute(
        "INSERT INTO lar_chunks
           (hash_algorithm, chunk_hash, uncompressed_length, compression,
            compressed_length, file_uuid, record_id, page_offset, record_offset,
            checksum, created_at_ms, state)
         VALUES ('blake3', ?1, ?2, 'zstd', ?3, ?4, ?5, ?6, ?6, ?1, ?7, 'ready')
         ON CONFLICT(hash_algorithm, chunk_hash) DO NOTHING",
        params![
            descriptor.hash.digest.as_slice(),
            to_i64(descriptor.uncompressed_length, "chunk length")?,
            to_i64(descriptor.compressed_length, "compressed chunk length")?,
            file_uuid,
            format!("chunk:{}", hex(&descriptor.hash.digest)),
            to_i64(descriptor.frame_offset, "chunk frame offset")?,
            created_at_ms,
        ],
    )?;
    let stored: (i64, String) = tx.query_row(
        "SELECT uncompressed_length, state FROM lar_chunks
         WHERE hash_algorithm='blake3' AND chunk_hash=?1",
        [descriptor.hash.digest.as_slice()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored.0 != to_i64(descriptor.uncompressed_length, "chunk length")? || stored.1 != "ready" {
        bail!("catalog chunk hash is bound to incompatible content or state");
    }
    Ok(inserted == 1)
}

fn find_catalog_manifest(tx: &Transaction<'_>, manifest: &BodyManifest) -> Result<Option<String>> {
    let total_length = to_i64(manifest.total_length, "manifest length")?;
    let media_type = manifest
        .media_type
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()?;
    let content_encoding = manifest
        .content_encoding
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()?;
    let manifest_id = manifest.id.to_string();
    let by_id: Option<(i64, Vec<u8>, Option<String>, Option<String>, String)> = tx
        .query_row(
            "SELECT total_length, whole_body_hash, media_type, content_encoding, state
               FROM lar_manifests WHERE manifest_id=?1",
            [&manifest_id],
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
    if let Some((length, hash, stored_media, stored_encoding, state)) = by_id {
        if length != total_length
            || hash.as_slice() != manifest.whole_body_hash.digest
            || stored_media.as_deref() != media_type
            || stored_encoding.as_deref() != content_encoding
            || state != "ready"
        {
            bail!("standalone manifest ID is already bound to incompatible metadata");
        }
        return Ok(Some(manifest_id));
    }

    let existing: Option<(String, i64, Vec<u8>, String)> = tx
        .query_row(
            "SELECT manifest_id, total_length, whole_body_hash, state
             FROM lar_manifests
             WHERE hash_algorithm='blake3' AND whole_body_hash=?1 AND total_length=?2
               AND media_type IS ?3 AND content_encoding IS ?4
             ORDER BY manifest_id LIMIT 1",
            params![
                manifest.whole_body_hash.digest.as_slice(),
                total_length,
                media_type,
                content_encoding,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    if let Some((id, length, hash, state)) = existing {
        if length != total_length
            || hash.as_slice() != manifest.whole_body_hash.digest
            || state != "ready"
        {
            bail!("catalog body identity is incompatible with standalone manifest");
        }
        return Ok(Some(id));
    }

    Ok(None)
}

fn insert_manifest(
    tx: &Transaction<'_>,
    file_uuid: &str,
    manifest: &BodyManifest,
    created_at_ms: i64,
) -> Result<()> {
    let total_length = to_i64(manifest.total_length, "manifest length")?;
    let media_type = manifest
        .media_type
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()?
        .map(str::to_owned);
    let content_encoding = manifest
        .content_encoding
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()?
        .map(str::to_owned);
    let manifest_id = manifest.id.to_string();
    tx.execute(
        "INSERT INTO lar_manifests
           (manifest_id, total_length, hash_algorithm, whole_body_hash, media_type,
            content_encoding, file_uuid, record_id, created_at_ms, state)
         VALUES (?1, ?2, 'blake3', ?3, ?4, ?5, ?6, ?1, ?7, 'ready')",
        params![
            manifest_id,
            total_length,
            manifest.whole_body_hash.digest.as_slice(),
            media_type,
            content_encoding,
            file_uuid,
            created_at_ms,
        ],
    )?;
    for (ordinal, reference) in manifest.chunks.iter().enumerate() {
        tx.execute(
            "INSERT INTO lar_manifest_chunks
               (manifest_id, ordinal, hash_algorithm, chunk_hash, logical_offset,
                chunk_offset, length)
             VALUES (?1, ?2, 'blake3', ?3, ?4, ?5, ?6)",
            params![
                manifest_id,
                to_i64(ordinal as u64, "manifest chunk ordinal")?,
                reference.chunk_hash.digest.as_slice(),
                to_i64(reference.logical_offset, "manifest logical offset")?,
                to_i64(reference.chunk_offset, "manifest chunk offset")?,
                to_i64(reference.length, "manifest chunk range")?,
            ],
        )?;
    }
    Ok(())
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

fn header_fidelity(value: HeaderFidelity) -> &'static str {
    match value {
        HeaderFidelity::Exact | HeaderFidelity::LegacyCasingUnknown => "observed_ordered",
        HeaderFidelity::LegacyOrderUnknown | HeaderFidelity::LegacyOrderAndCasingUnknown => {
            "legacy_normalized"
        }
    }
}

fn header_fidelity_detail(value: HeaderFidelity) -> &'static str {
    match value {
        HeaderFidelity::Exact => "exact",
        HeaderFidelity::LegacyOrderUnknown => "legacy_order_unknown",
        HeaderFidelity::LegacyCasingUnknown => "legacy_casing_unknown",
        HeaderFidelity::LegacyOrderAndCasingUnknown => "legacy_order_and_casing_unknown",
    }
}

fn attach_header_block(
    tx: &Transaction<'_>,
    file_uuid: &str,
    block: &HeaderBlock,
    created_at_ms: i64,
) -> Result<()> {
    let block_id = block.id.to_string();
    let fidelity = header_fidelity(block.fidelity);
    let fidelity_detail = header_fidelity_detail(block.fidelity);
    tx.execute(
        "INSERT INTO lar_header_blocks
           (block_id, fidelity, fidelity_detail, atom_count, file_uuid, record_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?1, ?6)
         ON CONFLICT(block_id) DO UPDATE SET
           fidelity_detail=COALESCE(lar_header_blocks.fidelity_detail, excluded.fidelity_detail)",
        params![
            block_id,
            fidelity,
            fidelity_detail,
            to_i64(block.atoms.len() as u64, "header atom count")?,
            file_uuid,
            created_at_ms,
        ],
    )?;
    let stored: (String, Option<String>, i64) = tx.query_row(
        "SELECT fidelity, fidelity_detail, atom_count
           FROM lar_header_blocks WHERE block_id=?1",
        [&block_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if stored
        != (
            fidelity.to_string(),
            Some(fidelity_detail.to_string()),
            block.atoms.len() as i64,
        )
    {
        bail!("header block ID is already bound to incompatible metadata");
    }
    for (ordinal, atom) in block.atoms.iter().enumerate() {
        let atom_id = header_atom_id(atom);
        tx.execute(
            "INSERT INTO lar_header_atoms
               (atom_id, original_name_bytes, value_bytes, flags, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(atom_id) DO NOTHING",
            params![
                atom_id,
                atom.original_name,
                atom.value,
                atom.flags,
                created_at_ms,
            ],
        )?;
        let stored: (Vec<u8>, Vec<u8>, i64) = tx.query_row(
            "SELECT original_name_bytes, value_bytes, flags FROM lar_header_atoms
             WHERE atom_id=?1",
            [&atom_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if stored
            != (
                atom.original_name.clone(),
                atom.value.clone(),
                atom.flags as i64,
            )
        {
            bail!("header atom ID is already bound to incompatible bytes");
        }
        tx.execute(
            "INSERT INTO lar_header_block_atoms (block_id, ordinal, atom_id)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(block_id, ordinal) DO NOTHING",
            params![block_id, ordinal as u64, atom_id],
        )?;
        let stored_atom: String = tx.query_row(
            "SELECT atom_id FROM lar_header_block_atoms WHERE block_id=?1 AND ordinal=?2",
            params![block_id, ordinal as u64],
            |row| row.get(0),
        )?;
        if stored_atom != atom_id {
            bail!("header block ordinal is already bound to a different atom");
        }
    }
    Ok(())
}

fn stage_kind_name(kind: StageKind) -> String {
    match kind {
        StageKind::ClientRequest => "client_request".into(),
        StageKind::NormalizedRequest => "normalized_request".into(),
        StageKind::RouterDecision => "router_decision".into(),
        StageKind::RetryDecision => "retry_decision".into(),
        StageKind::FailoverDecision => "failover_decision".into(),
        StageKind::UpstreamRequest => "upstream_request".into(),
        StageKind::UpstreamResponse => "upstream_response".into(),
        StageKind::UpstreamFailure => "upstream_failure".into(),
        StageKind::ClientResponse => "client_response".into(),
        StageKind::ClientTrailers => "client_trailers".into(),
        StageKind::ToolCall => "tool_call".into(),
        StageKind::ToolResult => "tool_result".into(),
        StageKind::AuthRefresh => "auth_refresh".into(),
        StageKind::AccountRouting => "account_routing".into(),
        StageKind::DarioRequest => "dario_request".into(),
        StageKind::DarioResponse => "dario_response".into(),
        StageKind::InjectedResponse => "injected_response".into(),
        StageKind::Cancellation => "cancellation".into(),
        StageKind::Unknown(code) => format!("unknown:{code}"),
    }
}

fn mapped_manifest(
    value: Option<ManifestId>,
    manifests: &HashMap<ManifestId, String>,
) -> Result<Option<String>> {
    value
        .map(|id| {
            manifests
                .get(&id)
                .cloned()
                .with_context(|| format!("standalone stage references uncataloged manifest {id}"))
        })
        .transpose()
}

fn attach_stage(
    tx: &Transaction<'_>,
    file_uuid: &str,
    value: &ValidatedStage,
    manifests: &HashMap<ManifestId, String>,
) -> Result<(String, bool)> {
    let data = &value.stage.data;
    let request_body = mapped_manifest(data.request_body_manifest_ref, manifests)?;
    let response_body = mapped_manifest(data.response_body_manifest_ref, manifests)?;
    let mut local_data = data.clone();
    local_data.request_body_manifest_ref = request_body
        .as_deref()
        .map(str::parse)
        .transpose()
        .context("mapped request manifest ID is invalid")?;
    local_data.response_body_manifest_ref = response_body
        .as_deref()
        .map(str::parse)
        .transpose()
        .context("mapped response manifest ID is invalid")?;
    let local_content_id = Stage::new(local_data).id.to_string();
    let existing_occurrence: Option<String> = tx
        .query_row(
            "SELECT stage_id FROM lar_stage_records
              WHERE trace_id=?1 AND capture_sequence=?2",
            params![
                value.trace_id,
                to_i64(value.sequence, "stage capture sequence")?
            ],
            |row| row.get(0),
        )
        .optional()?;
    let content_id_in_use = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM lar_stage_records WHERE stage_id=?1)",
        [&local_content_id],
        |row| row.get::<_, bool>(0),
    )?;
    let stage_id = existing_occurrence.unwrap_or_else(|| {
        if content_id_in_use {
            stage_occurrence_id(
                file_uuid,
                &value.trace_id,
                value.sequence,
                &local_content_id,
            )
        } else {
            local_content_id.clone()
        }
    });
    let capture_sequence = to_i64(value.sequence, "stage capture sequence")?;
    let wall_time_ns = to_i64(data.wall_time_ns, "stage wall time")?;
    let monotonic_delta_ns = data
        .monotonic_delta_ns
        .map(|value| to_i64(value, "stage monotonic time"))
        .transpose()?;
    let request_headers = data.request_headers_ref.map(|id| id.to_string());
    let response_headers = data.response_headers_ref.map(|id| id.to_string());
    let trailers = data.trailers_ref.map(|id| id.to_string());
    let stream_index = data.stream_index_ref.map(|id| id.to_string());
    let inserted = tx.execute(
        "INSERT INTO lar_stage_records
           (stage_id, trace_id, capture_sequence, kind, attempt_number,
            wall_time_ns, monotonic_delta_ns, request_headers_ref,
            request_body_manifest_ref, response_headers_ref,
            response_body_manifest_ref, trailers_ref, stream_index_ref,
            file_uuid, record_id, fidelity)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, 'captured')
         ON CONFLICT(stage_id) DO NOTHING",
        params![
            stage_id,
            value.trace_id,
            capture_sequence,
            stage_kind_name(data.kind),
            data.attempt_number,
            wall_time_ns,
            monotonic_delta_ns,
            request_headers,
            request_body,
            response_headers,
            response_body,
            trailers,
            stream_index,
            file_uuid,
            local_content_id,
        ],
    )?;
    let stored = tx.query_row(
        "SELECT trace_id, capture_sequence, kind, attempt_number, wall_time_ns,
                monotonic_delta_ns, request_headers_ref, request_body_manifest_ref,
                response_headers_ref, response_body_manifest_ref, trailers_ref,
                stream_index_ref, record_id, fidelity
         FROM lar_stage_records WHERE stage_id=?1",
        [&stage_id],
        |row| {
            Ok(StoredStage {
                trace_id: row.get(0)?,
                capture_sequence: row.get(1)?,
                kind: row.get(2)?,
                attempt_number: row.get(3)?,
                wall_time_ns: row.get(4)?,
                monotonic_delta_ns: row.get(5)?,
                request_headers: row.get(6)?,
                request_body: row.get(7)?,
                response_headers: row.get(8)?,
                response_body: row.get(9)?,
                trailers: row.get(10)?,
                stream_index: row.get(11)?,
                record_id: row.get(12)?,
                fidelity: row.get(13)?,
            })
        },
    )?;
    let expected = StoredStage {
        trace_id: value.trace_id.clone(),
        capture_sequence,
        kind: stage_kind_name(data.kind),
        attempt_number: data.attempt_number.map(i64::from),
        wall_time_ns: Some(wall_time_ns),
        monotonic_delta_ns,
        request_headers,
        request_body,
        response_headers,
        response_body,
        trailers,
        stream_index,
        record_id: Some(local_content_id),
        fidelity: "captured".to_string(),
    };
    if stored != expected {
        bail!("standalone stage ID is already bound to incompatible catalog metadata");
    }
    Ok((stage_id, inserted == 1))
}

fn stage_occurrence_id(
    file_uuid: &str,
    trace_id: &str,
    sequence: u64,
    local_content_id: &str,
) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(b"alex-lar-stage-occurrence-v1\0");
    for value in [
        file_uuid.as_bytes(),
        trace_id.as_bytes(),
        local_content_id.as_bytes(),
    ] {
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value);
    }
    hash.update(&sequence.to_le_bytes());
    hash.finalize().to_hex().to_string()
}

#[derive(Debug, PartialEq, Eq)]
struct StoredStage {
    trace_id: String,
    capture_sequence: i64,
    kind: String,
    attempt_number: Option<i64>,
    wall_time_ns: Option<i64>,
    monotonic_delta_ns: Option<i64>,
    request_headers: Option<String>,
    request_body: Option<String>,
    response_headers: Option<String>,
    response_body: Option<String>,
    trailers: Option<String>,
    stream_index: Option<String>,
    record_id: Option<String>,
    fidelity: String,
}

#[derive(Default)]
struct MaterializedTrace {
    ts_request_ms: i64,
    ts_response_ms: Option<i64>,
    harness: Option<String>,
    client_format: Option<String>,
    upstream_provider: Option<String>,
    upstream_format: Option<String>,
    requested_model: Option<String>,
    routed_model: Option<String>,
    method: Option<String>,
    path: Option<String>,
    status: Option<i64>,
    streamed: Option<bool>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_creation_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cost_usd: Option<f64>,
    billing_bucket: Option<String>,
    error: Option<String>,
    account_id: Option<String>,
    error_kind: Option<String>,
    error_code: Option<String>,
    error_class: Option<String>,
    substituted: bool,
    original_model: Option<String>,
    served_model: Option<String>,
    substitution_reason: Option<String>,
    attempts: Option<String>,
    injected: bool,
    fixture_name: Option<String>,
    original_account_id: Option<String>,
    served_account_id: Option<String>,
    tags: Option<String>,
    client_ip: Option<String>,
    key_fingerprint: Option<String>,
    reasoning_effort: Option<String>,
    thinking_budget: Option<i64>,
    subscription_identity: Option<String>,
    via_dario: bool,
    dario_generation: Option<String>,
}

fn materialized_trace(value: &ValidatedExchange) -> Result<MaterializedTrace> {
    let metadata = value.metadata.as_ref();
    let router = value.stages.iter().rev().find(|stage| {
        matches!(
            stage.data.kind,
            StageKind::RouterDecision
                | StageKind::AccountRouting
                | StageKind::RetryDecision
                | StageKind::FailoverDecision
        )
    });
    let client_request = value
        .stages
        .iter()
        .find(|stage| stage.data.kind == StageKind::ClientRequest);
    let provider_stage = value.stages.iter().rev().find(|stage| {
        stage.data.provider.is_some()
            || stage.data.requested_model.is_some()
            || stage.data.routed_model.is_some()
            || stage.data.account_id.is_some()
    });
    let response = value.stages.iter().rev().find(|stage| {
        matches!(
            stage.data.kind,
            StageKind::ClientResponse
                | StageKind::InjectedResponse
                | StageKind::DarioResponse
                | StageKind::UpstreamResponse
                | StageKind::UpstreamFailure
        )
    });
    let usage = response.and_then(|stage| stage.data.usage.as_ref());
    let normalized_cost = response.and_then(|stage| {
        (stage.data.cost_currency.as_deref() == Some(b"USD"))
            .then_some(stage.data.cost_nanos)
            .flatten()
            .map(|nanos| nanos as f64 / 1_000_000_000.0)
    });
    let request_ms = metadata
        .and_then(|value| value.ts_request_ms)
        .unwrap_or(ns_to_ms(
            value.exchange.data.wall_time_ns,
            "exchange wall time",
        )?);
    let response_ms = metadata.and_then(|value| value.ts_response_ms).or(value
        .stages
        .iter()
        .map(|stage| stage.data.wall_time_ns)
        .max()
        .map(|value| ns_to_ms(value, "exchange response time"))
        .transpose()?);

    Ok(MaterializedTrace {
        ts_request_ms: request_ms,
        ts_response_ms: response_ms,
        harness: metadata.and_then(|value| metadata_string(value.harness.as_deref())),
        client_format: metadata.and_then(|value| metadata_string(value.client_format.as_deref())),
        upstream_provider: stage_string(router, |data| data.provider.as_deref())
            .or_else(|| stage_string(provider_stage, |data| data.provider.as_deref())),
        upstream_format: metadata
            .and_then(|value| metadata_string(value.upstream_format.as_deref())),
        requested_model: stage_string(client_request, |data| data.requested_model.as_deref())
            .or_else(|| stage_string(router, |data| data.requested_model.as_deref()))
            .or_else(|| stage_string(provider_stage, |data| data.requested_model.as_deref())),
        routed_model: stage_string(router, |data| data.routed_model.as_deref())
            .or_else(|| stage_string(provider_stage, |data| data.routed_model.as_deref())),
        method: metadata.and_then(|value| metadata_string(value.method.as_deref())),
        path: metadata.and_then(|value| metadata_string(value.path.as_deref())),
        status: metadata
            .and_then(|value| value.status)
            .or_else(|| response.and_then(|stage| stage.data.status_code.map(i64::from))),
        streamed: metadata.and_then(|value| value.streamed),
        input_tokens: metadata
            .and_then(|value| value.input_tokens)
            .or(usage.map(|value| u64_to_i64_saturating(value.input_tokens))),
        cached_input_tokens: metadata
            .and_then(|value| value.cached_input_tokens)
            .or(usage.map(|value| u64_to_i64_saturating(value.cached_tokens))),
        cache_creation_tokens: metadata.and_then(|value| value.cache_creation_tokens),
        output_tokens: metadata
            .and_then(|value| value.output_tokens)
            .or(usage.map(|value| u64_to_i64_saturating(value.output_tokens))),
        reasoning_tokens: metadata
            .and_then(|value| value.reasoning_tokens)
            .or(usage.map(|value| u64_to_i64_saturating(value.reasoning_tokens))),
        cost_usd: metadata
            .and_then(|value| value.cost_usd_bits.map(f64::from_bits))
            .or(normalized_cost),
        billing_bucket: metadata.and_then(|value| metadata_string(value.billing_bucket.as_deref())),
        error: response.and_then(|stage| metadata_string(stage.data.error_message.as_deref())),
        account_id: stage_string(router, |data| data.account_id.as_deref())
            .or_else(|| stage_string(provider_stage, |data| data.account_id.as_deref())),
        error_kind: metadata.and_then(|value| metadata_string(value.error_kind.as_deref())),
        error_code: metadata.and_then(|value| metadata_string(value.error_code.as_deref())),
        error_class: response.and_then(|stage| metadata_string(stage.data.error_class.as_deref())),
        substituted: metadata.is_some_and(|value| value.substituted),
        original_model: metadata.and_then(|value| metadata_string(value.original_model.as_deref())),
        served_model: metadata.and_then(|value| metadata_string(value.served_model.as_deref())),
        substitution_reason: metadata
            .and_then(|value| metadata_string(value.substitution_reason.as_deref()))
            .or_else(|| stage_string(router, |data| data.routing_reason.as_deref())),
        attempts: metadata.and_then(|value| metadata_string(value.attempts_json.as_deref())),
        injected: metadata.is_some_and(|value| value.injected),
        fixture_name: metadata.and_then(|value| metadata_string(value.fixture_name.as_deref())),
        original_account_id: metadata
            .and_then(|value| metadata_string(value.original_account_id.as_deref())),
        served_account_id: metadata
            .and_then(|value| metadata_string(value.served_account_id.as_deref())),
        tags: metadata.and_then(|value| metadata_string(value.tags_json.as_deref())),
        client_ip: metadata.and_then(|value| metadata_string(value.client_ip.as_deref())),
        key_fingerprint: metadata
            .and_then(|value| metadata_string(value.key_fingerprint.as_deref())),
        reasoning_effort: metadata
            .and_then(|value| metadata_string(value.reasoning_effort.as_deref())),
        thinking_budget: metadata.and_then(|value| value.thinking_budget),
        subscription_identity: metadata
            .and_then(|value| metadata_string(value.subscription_identity.as_deref())),
        via_dario: metadata.is_some_and(|value| value.via_dario),
        dario_generation: metadata
            .and_then(|value| metadata_string(value.dario_generation.as_deref())),
    })
}

fn metadata_string(value: Option<&[u8]>) -> Option<String> {
    value.and_then(|value| std::str::from_utf8(value).ok().map(str::to_owned))
}

fn stage_string(
    stage: Option<&Stage>,
    field: for<'a> fn(&'a alex_lar::StageData) -> Option<&'a [u8]>,
) -> Option<String> {
    stage.and_then(|stage| metadata_string(field(&stage.data)))
}

fn u64_to_i64_saturating(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn remap_conversation_graph(
    source_entries: &[ConversationEntry],
    source_generations: &[Generation],
    source_turns: &[ValidatedTurnView],
    manifests: &HashMap<ManifestId, String>,
) -> Result<(
    Vec<ConversationEntry>,
    Vec<Generation>,
    Vec<ValidatedTurnView>,
)> {
    let mut entry_ids = HashMap::<ConversationEntryId, ConversationEntryId>::new();
    let mut entries = Vec::with_capacity(source_entries.len());
    for source in source_entries {
        let mut data = source.data.clone();
        data.raw_ranges = source
            .data
            .raw_ranges
            .iter()
            .map(|range| {
                let manifest_id = manifests
                    .get(&range.manifest_id)
                    .with_context(|| {
                        format!(
                            "conversation entry {} references uncataloged manifest {}",
                            source.id, range.manifest_id
                        )
                    })?
                    .parse()
                    .context("mapped conversation manifest ID is invalid")?;
                Ok(ArtifactRangeRef {
                    manifest_id,
                    byte_offset: range.byte_offset,
                    byte_length: range.byte_length,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mapped = ConversationEntry::new(data);
        entry_ids.insert(source.id, mapped.id);
        entries.push(mapped);
    }

    // Parent records are not required to be sorted by ID. Resolve the DAG
    // iteratively so a deep session cannot recurse through the process stack.
    let mut generation_ids = HashMap::<GenerationId, GenerationId>::new();
    let mut generations = Vec::with_capacity(source_generations.len());
    let mut remaining = source_generations.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut deferred = Vec::new();
        for source in remaining {
            let parent_generation_id = match source.data.parent_generation_id {
                Some(parent) => match generation_ids.get(&parent).copied() {
                    Some(parent) => Some(parent),
                    None => {
                        deferred.push(source);
                        continue;
                    }
                },
                None => None,
            };
            let entries_for_generation = source
                .data
                .entries
                .iter()
                .map(|entry| {
                    entry_ids.get(entry).copied().with_context(|| {
                        format!("generation {} references missing entry {entry}", source.id)
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let mapped = Generation::new(GenerationData {
                parent_generation_id,
                entries: entries_for_generation,
                reason: source.data.reason,
            });
            generation_ids.insert(source.id, mapped.id);
            generations.push(mapped);
        }
        if deferred.len() == before {
            bail!("standalone conversation generations have a missing parent or cycle");
        }
        remaining = deferred;
    }

    let mut turns = Vec::with_capacity(source_turns.len());
    for source in source_turns {
        let generation_id = generation_ids
            .get(&source.turn.data.generation_id)
            .copied()
            .with_context(|| {
                format!(
                    "turn {} references missing generation {}",
                    source.turn.id, source.turn.data.generation_id
                )
            })?;
        let response_entry_refs = source
            .turn
            .data
            .response_entry_refs
            .iter()
            .map(|entry| {
                entry_ids.get(entry).copied().with_context(|| {
                    format!(
                        "turn {} references missing response entry {entry}",
                        source.turn.id
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        turns.push(ValidatedTurnView {
            turn: TurnView::new(TurnViewData {
                trace_id: source.turn.data.trace_id.clone(),
                generation_id,
                upto_index: source.turn.data.upto_index,
                response_entry_refs,
            }),
            trace_id: source.trace_id.clone(),
            session_id: source.session_id.clone(),
        });
    }

    Ok((entries, generations, turns))
}

fn attach_conversation_entry(
    tx: &Transaction<'_>,
    entry: &ConversationEntry,
    created_at_ms: i64,
) -> Result<()> {
    let entry_id = entry.id.to_string();
    let role = conversation_role_name(entry.data.role);
    let kind = conversation_kind_name(entry.data.kind);
    tx.execute(
        "INSERT INTO lar_conversation_entries
           (entry_id, semantic_schema, role, kind, name, tool_call_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(entry_id) DO NOTHING",
        params![
            entry_id,
            i64::from(entry.data.semantic_schema),
            role,
            kind,
            entry.data.name,
            entry.data.tool_call_id,
            created_at_ms,
        ],
    )?;
    let stored: (i64, String, String, Option<Vec<u8>>, Option<Vec<u8>>) = tx.query_row(
        "SELECT semantic_schema, role, kind, name, tool_call_id
           FROM lar_conversation_entries WHERE entry_id=?1",
        [&entry_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    if stored
        != (
            i64::from(entry.data.semantic_schema),
            role,
            kind,
            entry.data.name.clone(),
            entry.data.tool_call_id.clone(),
        )
    {
        bail!("standalone conversation entry ID conflicts with existing semantics");
    }

    let expected_ranges = entry
        .data
        .raw_ranges
        .iter()
        .enumerate()
        .map(|(ordinal, range)| {
            Ok((
                to_i64(ordinal as u64, "conversation range ordinal")?,
                range.manifest_id.to_string(),
                to_i64(range.byte_offset, "conversation range offset")?,
                to_i64(range.byte_length, "conversation range length")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    for (ordinal, manifest_id, byte_offset, byte_length) in &expected_ranges {
        tx.execute(
            "INSERT INTO lar_conversation_entry_ranges
               (entry_id, ordinal, manifest_id, byte_offset, byte_length)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(entry_id, ordinal) DO NOTHING",
            params![entry_id, ordinal, manifest_id, byte_offset, byte_length],
        )?;
    }
    let mut statement = tx.prepare(
        "SELECT ordinal, manifest_id, byte_offset, byte_length
           FROM lar_conversation_entry_ranges WHERE entry_id=?1 ORDER BY ordinal",
    )?;
    let stored_ranges = statement
        .query_map([&entry_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<(i64, String, i64, i64)>>>()?;
    if stored_ranges != expected_ranges {
        bail!("standalone conversation entry ID conflicts with existing ranges");
    }
    Ok(())
}

fn attach_conversation_generation(
    tx: &Transaction<'_>,
    generation: &Generation,
    created_at_ms: i64,
) -> Result<()> {
    let generation_id = generation.id.to_string();
    let parent = generation
        .data
        .parent_generation_id
        .map(|value| value.to_string());
    let reason = generation_reason_name(generation.data.reason);
    tx.execute(
        "INSERT INTO lar_conversation_generations
           (generation_id, parent_generation_id, reason, created_at_ms)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT(generation_id) DO NOTHING",
        params![generation_id, parent, reason, created_at_ms],
    )?;
    let stored: (Option<String>, String) = tx.query_row(
        "SELECT parent_generation_id, reason FROM lar_conversation_generations
         WHERE generation_id=?1",
        [&generation_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored != (parent, reason) {
        bail!("standalone conversation generation ID conflicts with existing metadata");
    }
    let expected = generation
        .data
        .entries
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for (ordinal, entry_id) in expected.iter().enumerate() {
        tx.execute(
            "INSERT INTO lar_conversation_generation_entries
               (generation_id, ordinal, entry_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(generation_id, ordinal) DO NOTHING",
            params![
                generation_id,
                to_i64(ordinal as u64, "conversation generation ordinal")?,
                entry_id
            ],
        )?;
    }
    if load_conversation_ids(
        tx,
        "lar_conversation_generation_entries",
        "generation_id",
        "entry_id",
        &generation_id,
    )? != expected
    {
        bail!("standalone conversation generation ID conflicts with existing entries");
    }
    Ok(())
}

fn attach_conversation_turn(
    tx: &Transaction<'_>,
    value: &ValidatedTurnView,
    created_at_ms: i64,
) -> Result<()> {
    let turn_id = value.turn.id.to_string();
    let generation_id = value.turn.data.generation_id.to_string();
    let upto_index = to_i64(value.turn.data.upto_index, "conversation turn upto index")?;
    tx.execute(
        "INSERT INTO lar_conversation_turn_views
           (turn_view_id, trace_id, session_id, generation_id, upto_index, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(turn_view_id) DO NOTHING",
        params![
            turn_id,
            value.trace_id,
            value.session_id,
            generation_id,
            upto_index,
            created_at_ms,
        ],
    )?;
    let stored: (String, String, String, i64) = tx.query_row(
        "SELECT trace_id, session_id, generation_id, upto_index
           FROM lar_conversation_turn_views WHERE turn_view_id=?1",
        [&turn_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    if stored
        != (
            value.trace_id.clone(),
            value.session_id.clone(),
            generation_id.clone(),
            upto_index,
        )
    {
        bail!("standalone conversation turn ID conflicts with existing metadata");
    }
    let expected = value
        .turn
        .data
        .response_entry_refs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for (ordinal, entry_id) in expected.iter().enumerate() {
        tx.execute(
            "INSERT INTO lar_conversation_turn_responses
               (turn_view_id, ordinal, entry_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(turn_view_id, ordinal) DO NOTHING",
            params![
                turn_id,
                to_i64(ordinal as u64, "conversation response ordinal")?,
                entry_id
            ],
        )?;
    }
    if load_conversation_ids(
        tx,
        "lar_conversation_turn_responses",
        "turn_view_id",
        "entry_id",
        &turn_id,
    )? != expected
    {
        bail!("standalone conversation turn ID conflicts with existing responses");
    }

    // Associate the complete ancestor chain with the imported session. This
    // keeps compaction/branch history pageable even when the selected export
    // contains only a descendant turn.
    let mut current = Some(generation_id);
    let mut visited = HashSet::new();
    while let Some(generation_id) = current {
        if !visited.insert(generation_id.clone()) {
            bail!("standalone conversation generation ancestry contains a cycle");
        }
        tx.execute(
            "INSERT INTO lar_conversation_session_generations
             (session_id, generation_id, evidence_source, evidence_kind,
                evidence_id, created_at_ms)
             VALUES (?1, ?2, 'import', 'standalone_archive', ?3, ?4)
             ON CONFLICT(session_id, generation_id) DO NOTHING",
            params![value.session_id, generation_id, turn_id, created_at_ms],
        )?;
        current = tx
            .query_row(
                "SELECT parent_generation_id FROM lar_conversation_generations
                 WHERE generation_id=?1",
                [&generation_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
    }
    Ok(())
}

fn load_conversation_ids(
    tx: &Transaction<'_>,
    table: &str,
    owner_column: &str,
    value_column: &str,
    owner_id: &str,
) -> Result<Vec<String>> {
    let sql =
        format!("SELECT {value_column} FROM {table} WHERE {owner_column}=?1 ORDER BY ordinal");
    let mut statement = tx.prepare(&sql)?;
    let values = statement
        .query_map([owner_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(anyhow::Error::new)?;
    Ok(values)
}

fn conversation_role_name(value: ConversationRole) -> String {
    match value {
        ConversationRole::Opaque => "opaque".into(),
        ConversationRole::System => "system".into(),
        ConversationRole::User => "user".into(),
        ConversationRole::Assistant => "assistant".into(),
        ConversationRole::Tool => "tool".into(),
        ConversationRole::Unknown(code) => format!("unknown:{code}"),
    }
}

fn conversation_kind_name(value: ConversationEntryKind) -> String {
    match value {
        ConversationEntryKind::Opaque => "opaque".into(),
        ConversationEntryKind::Message => "message".into(),
        ConversationEntryKind::ToolCall => "tool_call".into(),
        ConversationEntryKind::ToolResult => "tool_result".into(),
        ConversationEntryKind::Summary => "summary".into(),
        ConversationEntryKind::Unknown(code) => format!("unknown:{code}"),
    }
}

fn generation_reason_name(value: GenerationReason) -> String {
    match value {
        GenerationReason::Initial => "initial".into(),
        GenerationReason::Append => "append".into(),
        GenerationReason::Compaction => "compaction".into(),
        GenerationReason::Branch => "branch".into(),
        GenerationReason::Mutation => "mutation".into(),
        GenerationReason::Import => "import".into(),
        GenerationReason::Unknown(code) => format!("unknown:{code}"),
    }
}

fn attach_exchange(
    tx: &Transaction<'_>,
    file_uuid: &str,
    value: &ValidatedExchange,
    manifests: &HashMap<ManifestId, String>,
    stage_occurrences: &HashMap<(String, u64), String>,
    source_fingerprint: &str,
    validated_at_ms: i64,
    insert_trace_rows: bool,
) -> Result<bool> {
    crate::live_body_store::catalog_capture_exchange(
        tx,
        file_uuid,
        &value.trace_id,
        &value.exchange.id.to_string(),
        value.exchange.data.capture_sequence,
        value.stages.len() as u64,
        "standalone_validated",
    )?;
    let existing: Option<(Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT session_id, run_id FROM traces WHERE id=?1",
            [&value.trace_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((session_id, run_id)) = &existing {
        if session_id
            .as_ref()
            .zip(value.session_id.as_ref())
            .is_some_and(|(left, right)| left != right)
            || run_id
                .as_ref()
                .zip(value.run_id.as_ref())
                .is_some_and(|(left, right)| left != right)
        {
            bail!(
                "trace {} already has conflicting session/run anchors",
                value.trace_id
            );
        }
    }
    let materialized = materialized_trace(value)?;
    let inserted = if insert_trace_rows {
        tx.execute(
            "INSERT INTO traces (
               id, ts_request_ms, ts_response_ms, session_id, harness, client_format,
               upstream_provider, upstream_format, requested_model, routed_model,
               method, path, status, streamed,
               input_tokens, cached_input_tokens, cache_creation_tokens, output_tokens,
               reasoning_tokens, cost_usd, billing_bucket,
               req_body_path, upstream_req_body_path, resp_body_path,
               req_headers_json, resp_headers_json, error, account_id,
               error_kind, error_code, error_class,
               substituted, original_model, served_model, substitution_reason, attempts,
               injected, fixture_name, original_account_id, served_account_id,
               run_id, tags_json, client_ip, key_fingerprint, reasoning_effort,
               thinking_budget, subscription_identity, via_dario, dario_generation
             ) VALUES (
               ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,
               ?18,?19,?20,?21,NULL,NULL,NULL,?22,?23,?24,?25,?26,?27,?28,?29,
               ?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46
             )
             ON CONFLICT(id) DO UPDATE SET
               session_id=COALESCE(traces.session_id, excluded.session_id),
               run_id=COALESCE(traces.run_id, excluded.run_id)",
            params![
                value.trace_id,
                materialized.ts_request_ms,
                materialized.ts_response_ms,
                value.session_id,
                materialized.harness,
                materialized.client_format,
                materialized.upstream_provider,
                materialized.upstream_format,
                materialized.requested_model,
                materialized.routed_model,
                materialized.method,
                materialized.path,
                materialized.status,
                materialized.streamed.map(i64::from),
                materialized.input_tokens,
                materialized.cached_input_tokens,
                materialized.cache_creation_tokens,
                materialized.output_tokens,
                materialized.reasoning_tokens,
                materialized.cost_usd,
                materialized.billing_bucket,
                value.request_headers_json,
                value.response_headers_json,
                materialized.error,
                materialized.account_id,
                materialized.error_kind,
                materialized.error_code,
                materialized.error_class,
                i64::from(materialized.substituted),
                materialized.original_model,
                materialized.served_model,
                materialized.substitution_reason,
                materialized.attempts,
                i64::from(materialized.injected),
                materialized.fixture_name,
                materialized.original_account_id,
                materialized.served_account_id,
                value.run_id,
                materialized.tags,
                materialized.client_ip,
                materialized.key_fingerprint,
                materialized.reasoning_effort,
                materialized.thinking_budget,
                materialized.subscription_identity,
                i64::from(materialized.via_dario),
                materialized.dario_generation,
            ],
        )?
    } else {
        0
    };

    if let Some(session_id) = value.session_id.as_deref() {
        tx.execute(
            "INSERT INTO lar_session_revisions (session_id, revision, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET
               revision=MAX(lar_session_revisions.revision, excluded.revision),
               updated_at_ms=excluded.updated_at_ms",
            params![
                session_id,
                value.exchange.data.capture_sequence.max(1),
                validated_at_ms,
            ],
        )?;
    }

    let last_upstream_request = value
        .stages
        .iter()
        .rposition(|stage| stage.data.kind == StageKind::UpstreamRequest);
    let last_upstream_response = value
        .stages
        .iter()
        .rposition(|stage| stage.data.kind == StageKind::UpstreamResponse);
    for (sequence, stage) in value.stages.iter().enumerate() {
        let stage_id = stage_occurrences
            .get(&(value.trace_id.clone(), sequence as u64))
            .with_context(|| {
                format!(
                    "trace {} is missing catalog stage occurrence {sequence}",
                    value.trace_id
                )
            })?
            .clone();
        let data = &stage.data;
        if let Some(manifest_id) = mapped_manifest(data.request_body_manifest_ref, manifests)? {
            attach_artifact(
                tx,
                &value.trace_id,
                "request_body",
                &stage_id,
                Some(&manifest_id),
                None,
                source_fingerprint,
                "standalone_validated",
                validated_at_ms,
            )?;
            let canonical = match data.kind {
                StageKind::ClientRequest => Some("client_request"),
                StageKind::UpstreamRequest if Some(sequence) == last_upstream_request => {
                    Some("upstream_request")
                }
                StageKind::DarioRequest => Some("dario_upstream_request"),
                _ => None,
            };
            if let Some(kind) = canonical {
                attach_artifact(
                    tx,
                    &value.trace_id,
                    kind,
                    "",
                    Some(&manifest_id),
                    None,
                    source_fingerprint,
                    "standalone_validated",
                    validated_at_ms,
                )?;
            }
        }
        if let Some(manifest_id) = mapped_manifest(data.response_body_manifest_ref, manifests)? {
            attach_artifact(
                tx,
                &value.trace_id,
                "response_body",
                &stage_id,
                Some(&manifest_id),
                None,
                source_fingerprint,
                "standalone_validated",
                validated_at_ms,
            )?;
            let canonical = match data.kind {
                StageKind::UpstreamResponse if Some(sequence) == last_upstream_response => {
                    Some("upstream_response")
                }
                StageKind::ClientResponse => Some("client_response"),
                StageKind::DarioResponse => Some("dario_upstream_response"),
                _ => None,
            };
            if let Some(kind) = canonical {
                attach_artifact(
                    tx,
                    &value.trace_id,
                    kind,
                    "",
                    Some(&manifest_id),
                    None,
                    source_fingerprint,
                    "standalone_validated",
                    validated_at_ms,
                )?;
            }
        }
        for (kind, block_id) in [
            ("request_headers", data.request_headers_ref),
            ("response_headers", data.response_headers_ref),
            ("trailers", data.trailers_ref),
        ] {
            if let Some(block_id) = block_id {
                attach_artifact(
                    tx,
                    &value.trace_id,
                    kind,
                    &stage_id,
                    None,
                    Some(&block_id.to_string()),
                    source_fingerprint,
                    "standalone_headers",
                    validated_at_ms,
                )?;
            }
        }
    }
    Ok(existing.is_none() && inserted == 1)
}

#[allow(clippy::too_many_arguments)]
fn attach_artifact(
    tx: &Transaction<'_>,
    trace_id: &str,
    artifact_kind: &str,
    stage_id: &str,
    manifest_id: Option<&str>,
    header_block_id: Option<&str>,
    source_fingerprint: &str,
    fidelity: &str,
    validated_at_ms: i64,
) -> Result<()> {
    let existing: Option<(Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT manifest_id, header_block_id FROM lar_trace_artifacts
             WHERE owner_kind='trace' AND owner_id=?1 AND artifact_kind=?2 AND stage_id=?3",
            params![trace_id, artifact_kind, stage_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing
            != (
                manifest_id.map(str::to_owned),
                header_block_id.map(str::to_owned),
            )
        {
            bail!(
                "trace {trace_id} artifact {artifact_kind}/{stage_id} conflicts with attached content"
            );
        }
        return Ok(());
    }
    tx.execute(
        "INSERT INTO lar_trace_artifacts
           (owner_kind, owner_id, artifact_kind, stage_id, manifest_id,
            header_block_id, source_fingerprint, fidelity, validation_state,
            validated_at_ms)
         VALUES ('trace', ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'validated', ?8)",
        params![
            trace_id,
            artifact_kind,
            stage_id,
            manifest_id,
            header_block_id,
            source_fingerprint,
            fidelity,
            validated_at_ms,
        ],
    )?;
    Ok(())
}
