use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alex_lar::{
    read_chunk_record_at, BodyManifest, ChunkHash, ChunkRecordDescriptor, ChunkRef, HashAlgorithm,
    Limits, ManifestId, RawBodyScanner, RawSearchLimits, RawSearchStats,
};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::Store;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LarCatalogGrepMatch {
    pub manifest_id: String,
    pub match_offset: u64,
    pub owner_kind: Option<String>,
    pub owner_id: Option<String>,
    pub artifact_kind: Option<String>,
    pub stage_id: Option<String>,
    pub trace_id: Option<String>,
    pub session_id: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub timestamp_ns: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct LarCatalogGrepReport {
    pub matches: Vec<LarCatalogGrepMatch>,
    pub stats: RawSearchStats,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CatalogAnchor {
    owner_kind: Option<String>,
    owner_id: Option<String>,
    artifact_kind: Option<String>,
    stage_id: Option<String>,
    trace_id: Option<String>,
    session_id: Option<String>,
    timestamp_ms: Option<i64>,
    timestamp_ns: Option<u64>,
}

impl Store {
    /// Search all ready manifests in the configured live catalog. Reverse
    /// references come from SQLite rather than archive event records because
    /// live body packs deliberately contain only chunks; one logical manifest
    /// may also span several rotated packs.
    pub fn grep_lar_catalog_raw(
        &self,
        literal: &[u8],
        result_limit: usize,
        limits: RawSearchLimits,
    ) -> Result<LarCatalogGrepReport> {
        if result_limit == 0 {
            bail!("LAR grep result limit must be greater than zero");
        }
        let conn = self.conn.lock().unwrap();
        let manifest_ids = catalog_manifest_ids(&conn, limits.max_manifests)?;
        let mut scanner = RawBodyScanner::new(literal, limits).map_err(anyhow::Error::new)?;
        let mut readers = HashMap::<PathBuf, File>::new();
        let mut matches = Vec::new();

        for manifest_id in manifest_ids {
            let manifest = load_catalog_manifest(&conn, &manifest_id)?;
            let found = scanner
                .search_manifest(&manifest, |hash| {
                    read_catalog_chunk(&conn, &self.data_dir, &mut readers, hash)
                })
                .with_context(|| format!("searching live LAR manifest {manifest_id}"))?;
            let Some(match_offset) = found else { continue };
            let anchors = catalog_anchors(&conn, &manifest_id, result_limit)?;
            if anchors.is_empty() {
                push_bounded(
                    &mut matches,
                    LarCatalogGrepMatch {
                        manifest_id: manifest_id.clone(),
                        match_offset,
                        owner_kind: None,
                        owner_id: None,
                        artifact_kind: None,
                        stage_id: None,
                        trace_id: None,
                        session_id: None,
                        timestamp_ms: None,
                        timestamp_ns: None,
                    },
                    result_limit,
                )?;
            } else {
                for anchor in anchors {
                    push_bounded(
                        &mut matches,
                        LarCatalogGrepMatch {
                            manifest_id: manifest_id.clone(),
                            match_offset,
                            owner_kind: anchor.owner_kind,
                            owner_id: anchor.owner_id,
                            artifact_kind: anchor.artifact_kind,
                            stage_id: anchor.stage_id,
                            trace_id: anchor.trace_id,
                            session_id: anchor.session_id,
                            timestamp_ms: anchor.timestamp_ms,
                            timestamp_ns: anchor.timestamp_ns,
                        },
                        result_limit,
                    )?;
                }
            }
        }
        matches.sort();
        Ok(LarCatalogGrepReport {
            matches,
            stats: scanner.stats(),
        })
    }
}

fn push_bounded(
    matches: &mut Vec<LarCatalogGrepMatch>,
    value: LarCatalogGrepMatch,
    limit: usize,
) -> Result<()> {
    if matches.len() >= limit {
        bail!(
            "LAR grep result limit exceeded (more than {limit} matches); refine the literal or raise --limit"
        );
    }
    matches.push(value);
    Ok(())
}

fn catalog_manifest_ids(conn: &Connection, max_manifests: u64) -> Result<Vec<String>> {
    let sql_limit = i64::try_from(max_manifests.saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT manifest_id FROM lar_manifests
         WHERE state='ready' ORDER BY manifest_id LIMIT ?1",
    )?;
    let rows = statement.query_map([sql_limit], |row| row.get::<_, String>(0))?;
    let ids = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    if ids.len() as u64 > max_manifests {
        bail!(
            "grep manifests exceeds limit ({} > {max_manifests})",
            ids.len()
        );
    }
    Ok(ids)
}

fn load_catalog_manifest(conn: &Connection, manifest_id: &str) -> Result<BodyManifest> {
    let (length, whole_hash, media_type, content_encoding): (
        i64,
        Vec<u8>,
        Option<String>,
        Option<String>,
    ) = conn.query_row(
        "SELECT total_length, whole_body_hash, media_type, content_encoding FROM lar_manifests
         WHERE manifest_id=?1 AND hash_algorithm='blake3' AND state='ready'",
        [manifest_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let total_length = u64::try_from(length).context("negative catalog manifest length")?;
    let digest: [u8; 32] = whole_hash
        .try_into()
        .map_err(|_| anyhow::anyhow!("catalog manifest has an invalid whole-body digest"))?;
    let mut statement = conn.prepare(
        "SELECT chunk_hash, logical_offset, chunk_offset, length
         FROM lar_manifest_chunks WHERE manifest_id=?1 ORDER BY ordinal",
    )?;
    let rows = statement.query_map([manifest_id], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut references = Vec::new();
    for row in rows {
        let (chunk_hash, logical_offset, chunk_offset, length) = row?;
        references.push(ChunkRef {
            chunk_hash: ChunkHash {
                algorithm: HashAlgorithm::Blake3,
                digest: chunk_hash
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("catalog chunk has an invalid digest"))?,
            },
            logical_offset: u64::try_from(logical_offset)
                .context("negative catalog logical offset")?,
            chunk_offset: u64::try_from(chunk_offset).context("negative catalog chunk offset")?,
            length: u64::try_from(length).context("negative catalog range length")?,
        });
    }
    let manifest = BodyManifest::new(
        total_length,
        ChunkHash {
            algorithm: HashAlgorithm::Blake3,
            digest,
        },
        media_type.map(String::into_bytes),
        content_encoding.map(String::into_bytes),
        references,
    );
    let expected = ManifestId::from_str(manifest_id).map_err(anyhow::Error::new)?;
    if manifest.id != expected {
        bail!("catalog manifest {manifest_id} identity does not match its ranges");
    }
    Ok(manifest)
}

fn read_catalog_chunk(
    conn: &Connection,
    data_dir: &Path,
    readers: &mut HashMap<PathBuf, File>,
    hash: &ChunkHash,
) -> alex_lar::Result<Vec<u8>> {
    let (stored_path, frame_offset, uncompressed_length, compressed_length) = conn
        .query_row(
            "SELECT f.path, c.page_offset, c.uncompressed_length, c.compressed_length
             FROM lar_chunks c
             JOIN lar_files f ON f.file_uuid=c.file_uuid
             WHERE c.hash_algorithm='blake3' AND c.chunk_hash=?1
               AND c.state='ready' AND f.state IN ('active','sealed')",
            params![hash.digest.as_slice()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .map_err(|error| {
            alex_lar::Error::InvalidDetail(format!(
                "live catalog cannot locate chunk {}: {error}",
                hex_digest(&hash.digest)
            ))
        })?;
    let path = resolve_catalog_path(data_dir, &stored_path);
    if !readers.contains_key(&path) {
        let file = File::open(&path).map_err(alex_lar::Error::Io)?;
        readers.insert(path.clone(), file);
    }
    read_chunk_record_at(
        readers
            .get_mut(&path)
            .ok_or_else(|| alex_lar::Error::Missing(path.display().to_string()))?,
        &ChunkRecordDescriptor {
            hash: *hash,
            frame_offset,
            uncompressed_length,
            compressed_length,
        },
        &Limits::default(),
    )
}

fn resolve_catalog_path(data_dir: &Path, stored_path: &str) -> PathBuf {
    let path = Path::new(stored_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        data_dir.join(path)
    }
}

fn catalog_anchors(
    conn: &Connection,
    manifest_id: &str,
    result_limit: usize,
) -> Result<Vec<CatalogAnchor>> {
    let mut anchors = BTreeSet::new();
    let sql_limit = i64::try_from(result_limit.saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT owner_kind, owner_id, artifact_kind, NULLIF(stage_id, '')
         FROM lar_trace_artifacts WHERE manifest_id=?1
         ORDER BY owner_kind, owner_id, artifact_kind, stage_id LIMIT ?2",
    )?;
    let artifacts = statement.query_map(params![manifest_id, sql_limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let artifacts = artifacts.collect::<rusqlite::Result<Vec<_>>>()?;
    if artifacts.len() > result_limit {
        bail!(
            "LAR grep result limit exceeded (more than {result_limit} reverse references); refine the literal or raise --limit"
        );
    }
    for (owner_kind, owner_id, artifact_kind, stage_id) in artifacts {
        let (trace_id, session_id, timestamp_ms) = match owner_kind.as_str() {
            "trace" => {
                let trace = conn
                    .query_row(
                        "SELECT session_id, ts_request_ms FROM traces WHERE id=?1",
                        [&owner_id],
                        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()?;
                (
                    Some(owner_id.clone()),
                    trace.as_ref().and_then(|row| row.0.clone()),
                    trace.map(|row| row.1),
                )
            }
            "tool_call" => conn
                .query_row(
                    "SELECT trace_id, session_id, ts_start_ms FROM tool_calls WHERE id=?1",
                    [&owner_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()?
                .map(|row| (row.0, Some(row.1), Some(row.2)))
                .unwrap_or((None, None, None)),
            _ => (None, None, None),
        };
        let timestamp_ns = if let Some(stage_id) = stage_id.as_deref() {
            conn.query_row(
                "SELECT wall_time_ns FROM lar_stage_records WHERE stage_id=?1",
                [stage_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten()
            .and_then(|value| u64::try_from(value).ok())
        } else {
            None
        };
        anchors.insert(CatalogAnchor {
            owner_kind: Some(owner_kind),
            owner_id: Some(owner_id),
            artifact_kind: Some(artifact_kind),
            stage_id,
            trace_id,
            session_id,
            timestamp_ms,
            timestamp_ns,
        });
    }

    // Read at most N+1 stage rows independently of artifact rows. Some rows
    // may duplicate an already-published artifact anchor, so limiting by the
    // apparent remaining slots could otherwise hide a later distinct anchor.
    let stage_limit = i64::try_from(result_limit.saturating_add(1)).unwrap_or(i64::MAX);
    let mut statement = conn.prepare(
        "SELECT stage_id, trace_id, kind, wall_time_ns,
                request_body_manifest_ref, response_body_manifest_ref
         FROM lar_stage_records
         WHERE request_body_manifest_ref=?1 OR response_body_manifest_ref=?1
         ORDER BY trace_id, capture_sequence, stage_id LIMIT ?2",
    )?;
    let stages = statement.query_map(params![manifest_id, stage_limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;
    for stage in stages {
        let (stage_id, trace_id, kind, wall_time_ns, request, response) = stage?;
        let trace = conn
            .query_row(
                "SELECT session_id, ts_request_ms FROM traces WHERE id=?1",
                [&trace_id],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        for artifact_kind in [
            (request.as_deref() == Some(manifest_id)).then_some("stage_request_body"),
            (response.as_deref() == Some(manifest_id)).then_some("stage_response_body"),
        ]
        .into_iter()
        .flatten()
        {
            anchors.insert(CatalogAnchor {
                owner_kind: None,
                owner_id: None,
                artifact_kind: Some(format!("{kind}:{artifact_kind}")),
                stage_id: Some(stage_id.clone()),
                trace_id: Some(trace_id.clone()),
                session_id: trace.as_ref().and_then(|row| row.0.clone()),
                timestamp_ms: trace.as_ref().map(|row| row.1),
                timestamp_ns: wall_time_ns.and_then(|value| u64::try_from(value).ok()),
            });
        }
        if anchors.len() > result_limit {
            bail!(
                "LAR grep result limit exceeded (more than {result_limit} reverse references); refine the literal or raise --limit"
            );
        }
    }
    Ok(anchors.into_iter().collect())
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
