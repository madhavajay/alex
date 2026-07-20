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
    ArchiveReader, BodyManifest, ChunkRecordDescriptor, Exchange, FileHeader, FileRole, HeaderAtom,
    HeaderBlock, HeaderFidelity, Limits, ManifestId, RecoveryStatus, Stage, StageKind,
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
}

struct ValidatedStage {
    trace_id: String,
    sequence: u64,
    stage: Stage,
}

struct ValidatedExchange {
    exchange: Exchange,
    trace_id: String,
    session_id: Option<String>,
    run_id: Option<String>,
    stages: Vec<Stage>,
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
        let mut body_map = HashMap::<([u8; 32], u64), String>::new();
        let mut new_manifests = Vec::new();
        let mut manifests_reused = 0u64;
        for manifest in &archive.manifests {
            let identity = (manifest.whole_body_hash.digest, manifest.total_length);
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
        for value in &archive.stages {
            stages_indexed += attach_stage(&tx, &file_uuid, value, &manifest_map)? as u64;
        }

        let mut traces_inserted = 0u64;
        for value in &archive.exchanges {
            traces_inserted += attach_exchange(
                &tx,
                &file_uuid,
                value,
                &manifest_map,
                &source_hash_text,
                now,
                insert_trace_rows,
            )? as u64;
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
    if reader.header().required_feature_bits != 0 {
        bail!("standalone LAR archive must not require external archive-set references");
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
            .read_body(&manifest.id)
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
    let mut indexed_stage_ids = HashSet::new();
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
            if indexed_stage_ids.insert(*stage_id) {
                stages.push(ValidatedStage {
                    trace_id: trace_id.clone(),
                    sequence: sequence as u64,
                    stage: stage.clone(),
                });
            }
            exchange_stages.push(stage);
        }
        validated_exchanges.push(ValidatedExchange {
            exchange,
            trace_id,
            session_id,
            run_id,
            stages: exchange_stages,
        });
    }
    let stream_indexes = reader.stream_index_count();
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
    })
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
    let existing: Option<(String, i64, Vec<u8>, String)> = tx
        .query_row(
            "SELECT manifest_id, total_length, whole_body_hash, state
             FROM lar_manifests
             WHERE hash_algorithm='blake3' AND whole_body_hash=?1 AND total_length=?2",
            params![manifest.whole_body_hash.digest.as_slice(), total_length],
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

    let manifest_id = manifest.id.to_string();
    let conflicting: Option<(i64, Vec<u8>, String)> = tx
        .query_row(
            "SELECT total_length, whole_body_hash, state
             FROM lar_manifests WHERE manifest_id=?1",
            [&manifest_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if conflicting.is_some() {
        bail!("standalone manifest ID is already bound to a different body");
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

fn attach_header_block(
    tx: &Transaction<'_>,
    file_uuid: &str,
    block: &HeaderBlock,
    created_at_ms: i64,
) -> Result<()> {
    let block_id = block.id.to_string();
    let fidelity = header_fidelity(block.fidelity);
    tx.execute(
        "INSERT INTO lar_header_blocks
           (block_id, fidelity, atom_count, file_uuid, record_id, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?1, ?5)
         ON CONFLICT(block_id) DO NOTHING",
        params![
            block_id,
            fidelity,
            to_i64(block.atoms.len() as u64, "header atom count")?,
            file_uuid,
            created_at_ms,
        ],
    )?;
    let stored: (String, i64) = tx.query_row(
        "SELECT fidelity, atom_count FROM lar_header_blocks WHERE block_id=?1",
        [&block_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored != (fidelity.to_string(), block.atoms.len() as i64) {
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
) -> Result<bool> {
    let data = &value.stage.data;
    let request_body = mapped_manifest(data.request_body_manifest_ref, manifests)?;
    let response_body = mapped_manifest(data.response_body_manifest_ref, manifests)?;
    let stage_id = value.stage.id.to_string();
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
                 ?13, ?14, ?1, 'captured')
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
        kind: stage_kind_name(data.kind).to_string(),
        attempt_number: data.attempt_number.map(i64::from),
        wall_time_ns: Some(wall_time_ns),
        monotonic_delta_ns,
        request_headers,
        request_body,
        response_headers,
        response_body,
        trailers,
        stream_index,
        record_id: Some(stage_id),
        fidelity: "captured".to_string(),
    };
    if stored != expected {
        bail!("standalone stage ID is already bound to incompatible catalog metadata");
    }
    Ok(inserted == 1)
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

fn attach_exchange(
    tx: &Transaction<'_>,
    _file_uuid: &str,
    value: &ValidatedExchange,
    manifests: &HashMap<ManifestId, String>,
    source_fingerprint: &str,
    validated_at_ms: i64,
    insert_trace_rows: bool,
) -> Result<bool> {
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
    let request_ms = ns_to_ms(value.exchange.data.wall_time_ns, "exchange wall time")?;
    let response_ms = value
        .stages
        .iter()
        .map(|stage| stage.data.wall_time_ns)
        .max()
        .map(|value| ns_to_ms(value, "exchange response time"))
        .transpose()?;
    let inserted = if insert_trace_rows {
        tx.execute(
            "INSERT INTO traces (id, ts_request_ms, ts_response_ms, session_id, run_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               session_id=COALESCE(traces.session_id, excluded.session_id),
               run_id=COALESCE(traces.run_id, excluded.run_id)",
            params![
                value.trace_id,
                request_ms,
                response_ms,
                value.session_id,
                value.run_id,
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
    for (sequence, stage) in value.stages.iter().enumerate() {
        let stage_id = stage.id.to_string();
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
