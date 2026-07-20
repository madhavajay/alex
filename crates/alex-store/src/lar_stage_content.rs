//! Bounded actual-content views for one trace's capture-ordered stages.
//!
//! Stages reference deduplicated body/header tables in the response. A shared
//! manifest or header block is read, budgeted, and serialized once even when
//! retries reference it repeatedly.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::path::PathBuf;
use std::str::FromStr;

use alex_lar::{
    read_chunk_record_at, BodyManifest, ChunkHash, ChunkRecordDescriptor, ChunkRef, HashAlgorithm,
    Limits, ManifestId,
};
use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use crate::{
    lar_archive_ops::resolved_catalog_path, LarArchiveAvailability, LarArchiveUnavailableError,
    LarArtifactBatchRead, LarArtifactLocation, LarArtifactReadRequest, Store,
};

pub const DEFAULT_STAGE_CONTENT_LIMIT: usize = 64;
pub const MAX_STAGE_CONTENT_LIMIT: usize = 256;
pub const DEFAULT_STAGE_CONTENT_BODY_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_STAGE_CONTENT_BODY_BYTES: u64 = 16 * 1024 * 1024;
pub const DEFAULT_STAGE_CONTENT_HEADER_BYTES: u64 = 256 * 1024;
pub const MAX_STAGE_CONTENT_HEADER_BYTES: u64 = 2 * 1024 * 1024;
const MAX_STAGE_CONTENT_CURSOR_ID_BYTES: usize = 512;
const CATALOG_QUERY_BATCH_SIZE: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentOptions {
    pub stage_limit: usize,
    pub body_byte_budget: u64,
    pub header_byte_budget: u64,
    pub after_capture_sequence: Option<u64>,
    pub after_stage_id: Option<String>,
}

impl Default for LarStageContentOptions {
    fn default() -> Self {
        Self {
            stage_limit: DEFAULT_STAGE_CONTENT_LIMIT,
            body_byte_budget: DEFAULT_STAGE_CONTENT_BODY_BYTES,
            header_byte_budget: DEFAULT_STAGE_CONTENT_HEADER_BYTES,
            after_capture_sequence: None,
            after_stage_id: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentError {
    code: &'static str,
    message: String,
}

impl LarStageContentError {
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

impl fmt::Display for LarStageContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LarStageContentError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentHeaderAtom {
    pub ordinal: u64,
    pub original_name: Vec<u8>,
    pub value: Vec<u8>,
    pub flags: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentHeaderBlock {
    pub content_id: String,
    pub block_id: Option<String>,
    pub state: String,
    pub fidelity: Option<String>,
    pub total_atoms: Option<u64>,
    pub total_bytes: Option<u64>,
    pub atoms: Vec<LarStageContentHeaderAtom>,
    pub error_kind: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageArtifactContent {
    pub content_id: String,
    pub manifest_id: Option<String>,
    pub artifact_kind: Option<String>,
    pub state: String,
    pub fidelity: String,
    pub total_bytes: Option<u64>,
    pub media_type: Option<Vec<u8>>,
    pub content_encoding: Option<Vec<u8>>,
    pub bytes: Option<Vec<u8>>,
    pub error_kind: Option<String>,
    pub message: Option<String>,
    pub archive_file_uuid: Option<String>,
    pub archive_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentRecord {
    pub stage_id: String,
    pub capture_sequence: u64,
    pub kind: String,
    pub attempt_number: Option<u64>,
    pub wall_time_ns: Option<u64>,
    pub monotonic_delta_ns: Option<u64>,
    pub fidelity: String,
    pub request_headers_ref: Option<String>,
    pub request_headers_content_id: Option<String>,
    pub request_body_manifest_ref: Option<String>,
    pub request_body_content_id: Option<String>,
    pub response_headers_ref: Option<String>,
    pub response_headers_content_id: Option<String>,
    pub response_body_manifest_ref: Option<String>,
    pub response_body_content_id: Option<String>,
    pub trailers_ref: Option<String>,
    pub trailers_content_id: Option<String>,
    pub stream_index_ref: Option<String>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentCursor {
    pub capture_sequence: u64,
    pub stage_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarStageContentPage {
    pub trace_id: String,
    pub total_stages: u64,
    pub stages_truncated: bool,
    pub has_more: bool,
    pub next_cursor: Option<LarStageContentCursor>,
    pub stage_limit: usize,
    pub body_byte_budget: u64,
    pub body_bytes_loaded: u64,
    pub header_byte_budget: u64,
    pub header_bytes_loaded: u64,
    pub stages: Vec<LarStageContentRecord>,
    pub header_blocks: Vec<LarStageContentHeaderBlock>,
    pub bodies: Vec<LarStageArtifactContent>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum BodySource {
    Manifest(String),
    Legacy(String),
    Missing(String),
    Error(String, String),
}

#[derive(Clone)]
struct ManifestPlan {
    id: String,
    manifest: BodyManifest,
}

#[derive(Clone)]
struct ManifestMetadata {
    total_length: u64,
    media_type: Option<Vec<u8>>,
    content_encoding: Option<Vec<u8>>,
}

struct ManifestAssembly {
    parsed_id: ManifestId,
    total_length: u64,
    whole_body_hash: ChunkHash,
    media_type: Option<Vec<u8>>,
    content_encoding: Option<Vec<u8>>,
    chunks: Vec<ChunkRef>,
}

#[derive(Clone)]
struct ChunkLocation {
    path: PathBuf,
    file_uuid: String,
    file_state: String,
    descriptor: ChunkRecordDescriptor,
}

#[derive(Clone)]
enum ChunkLoad {
    Bytes(Vec<u8>),
    ArchiveUnavailable(LarArchiveUnavailableError),
    Error(String),
}

impl Store {
    pub fn lar_stage_content_page(
        &self,
        trace_id: &str,
        options: &LarStageContentOptions,
    ) -> Result<LarStageContentPage> {
        validate_options(options)?;
        let (
            mut total_stages,
            mut stages,
            legacy_request_headers,
            legacy_response_headers,
            legacy_upstream_path,
            legacy_response_path,
            last_upstream_request,
        ) = {
            let conn = self.conn.lock().unwrap();
            let trace_exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM traces WHERE id=?1)",
                [trace_id],
                |row| row.get(0),
            )?;
            if !trace_exists {
                return Err(LarStageContentError::new(
                    "stage_content_trace_not_found",
                    format!("unknown trace '{trace_id}'"),
                )
                .into());
            }
            let total: u64 = conn.query_row(
                "SELECT COUNT(*) FROM lar_stage_records WHERE trace_id=?1",
                [trace_id],
                |row| row.get(0),
            )?;
            let mut statement = conn.prepare(
                "SELECT stage_id, capture_sequence, kind, attempt_number,
                        wall_time_ns, monotonic_delta_ns, request_headers_ref,
                        request_body_manifest_ref, response_headers_ref,
                        response_body_manifest_ref, trailers_ref, stream_index_ref,
                        fidelity
                   FROM lar_stage_records WHERE trace_id=?1
                    AND (?2 IS NULL OR capture_sequence > ?2
                         OR (capture_sequence = ?2 AND stage_id > ?3))
                  ORDER BY capture_sequence, stage_id LIMIT ?4",
            )?;
            let stages = statement
                .query_map(
                    params![
                        trace_id,
                        options.after_capture_sequence,
                        options.after_stage_id.as_deref(),
                        options.stage_limit.saturating_add(1) as u64
                    ],
                    |row| {
                        Ok(LarStageContentRecord {
                            stage_id: row.get(0)?,
                            capture_sequence: row.get(1)?,
                            kind: row.get(2)?,
                            attempt_number: row.get(3)?,
                            wall_time_ns: row.get(4)?,
                            monotonic_delta_ns: row.get(5)?,
                            request_headers_ref: row.get(6)?,
                            request_headers_content_id: None,
                            request_body_manifest_ref: row.get(7)?,
                            request_body_content_id: None,
                            response_headers_ref: row.get(8)?,
                            response_headers_content_id: None,
                            response_body_manifest_ref: row.get(9)?,
                            response_body_content_id: None,
                            trailers_ref: row.get(10)?,
                            trailers_content_id: None,
                            stream_index_ref: row.get(11)?,
                            fidelity: row.get(12)?,
                            limitations: Vec::new(),
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let headers = conn
                .query_row(
                    "SELECT req_headers_json, resp_headers_json,
                            upstream_req_body_path, resp_body_path
                       FROM traces WHERE id=?1",
                    [trace_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?
                .unwrap_or((None, None, None, None));
            let last_upstream_request = conn
                .query_row(
                    "SELECT stage_id FROM lar_stage_records
                      WHERE trace_id=?1 AND kind='upstream_request'
                      ORDER BY capture_sequence DESC, stage_id DESC LIMIT 1",
                    [trace_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            (
                total,
                stages,
                headers.0,
                headers.1,
                headers.2,
                headers.3,
                last_upstream_request,
            )
        };

        if total_stages == 0 {
            let upstream_exists = legacy_upstream_path.is_some()
                || artifact_is_captured(self, trace_id, "upstream_request");
            let response_exists = legacy_response_path.is_some()
                || legacy_response_headers.is_some()
                || artifact_is_captured(self, trace_id, "client_response");
            let mut legacy_stages =
                synthesize_legacy_stages(trace_id, upstream_exists, response_exists);
            total_stages = legacy_stages.len() as u64;
            legacy_stages.retain(|stage| stage_is_after_options(stage, options));
            legacy_stages.truncate(options.stage_limit.saturating_add(1));
            stages = legacy_stages;
        }

        let has_more = stages.len() > options.stage_limit;
        if has_more {
            stages.pop();
        }
        let next_cursor = has_more.then(|| {
            let last = stages
                .last()
                .expect("a non-zero page limit has a last stage");
            LarStageContentCursor {
                capture_sequence: last.capture_sequence,
                stage_id: last.stage_id.clone(),
            }
        });

        let mut header_order = Vec::<String>::new();
        let mut seen_headers = HashSet::<String>::new();
        for stage in &mut stages {
            register_header_ref(
                &stage.request_headers_ref,
                &mut stage.request_headers_content_id,
                &mut header_order,
                &mut seen_headers,
            );
            register_header_ref(
                &stage.response_headers_ref,
                &mut stage.response_headers_content_id,
                &mut header_order,
                &mut seen_headers,
            );
            register_header_ref(
                &stage.trailers_ref,
                &mut stage.trailers_content_id,
                &mut header_order,
                &mut seen_headers,
            );
        }
        register_legacy_headers(
            &mut stages,
            legacy_request_headers.as_deref(),
            legacy_response_headers.as_deref(),
            &mut header_order,
            &mut seen_headers,
        );
        let (header_blocks, header_bytes_loaded) = self.load_stage_header_blocks(
            &header_order,
            legacy_request_headers.as_deref(),
            legacy_response_headers.as_deref(),
            options.header_byte_budget,
        )?;

        let mut body_order = Vec::<BodySource>::new();
        let mut body_ids = HashMap::<BodySource, String>::new();
        let last_upstream_request = last_upstream_request.or_else(|| {
            stages
                .iter()
                .rfind(|stage| stage.kind == "upstream_request")
                .map(|stage| stage.stage_id.clone())
        });
        for stage in &mut stages {
            stage.request_body_content_id = resolve_body_content(
                self,
                trace_id,
                stage.request_body_manifest_ref.as_deref(),
                fallback_request_artifact(stage, last_upstream_request.as_deref()),
                &mut body_order,
                &mut body_ids,
            );
            stage.response_body_content_id = resolve_body_content(
                self,
                trace_id,
                stage.response_body_manifest_ref.as_deref(),
                fallback_response_artifact(stage),
                &mut body_order,
                &mut body_ids,
            );
        }
        let (bodies, body_bytes_loaded) =
            self.load_stage_bodies(trace_id, &body_order, &body_ids, options.body_byte_budget)?;

        Ok(LarStageContentPage {
            trace_id: trace_id.into(),
            total_stages,
            stages_truncated: has_more,
            has_more,
            next_cursor,
            stage_limit: options.stage_limit,
            body_byte_budget: options.body_byte_budget,
            body_bytes_loaded,
            header_byte_budget: options.header_byte_budget,
            header_bytes_loaded,
            stages,
            header_blocks,
            bodies,
        })
    }

    fn load_stage_header_blocks(
        &self,
        order: &[String],
        legacy_request: Option<&str>,
        legacy_response: Option<&str>,
        budget: u64,
    ) -> Result<(Vec<LarStageContentHeaderBlock>, u64)> {
        #[derive(Clone)]
        struct HeaderMetadata {
            fidelity: String,
            atom_count: u64,
            joined_atom_count: u64,
            total_bytes: u64,
            block_id: Option<String>,
        }
        enum HeaderDecision {
            Load(String),
            Complete(LarStageContentHeaderBlock),
        }

        let mut metadata = HashMap::<String, HeaderMetadata>::new();
        let mut atoms_by_id = HashMap::<String, Vec<LarStageContentHeaderAtom>>::new();
        let mut early_errors = HashMap::<String, String>::new();
        {
            let conn = self.conn.lock().unwrap();
            let ids = order
                .iter()
                .filter(|id| !id.starts_with("legacy:"))
                .map(String::as_str)
                .collect::<Vec<_>>();
            for batch in catalog_query_batches(&ids) {
                let sql = format!(
                    "SELECT b.block_id, COALESCE(b.fidelity_detail, b.fidelity), b.atom_count,
                            COUNT(h.atom_id),
                            COALESCE(SUM(LENGTH(h.original_name_bytes) + LENGTH(h.value_bytes)), 0)
                       FROM lar_header_blocks b
                       LEFT JOIN lar_header_block_atoms a ON a.block_id=b.block_id
                       LEFT JOIN lar_header_atoms h ON h.atom_id=a.atom_id
                      WHERE b.block_id IN ({})
                      GROUP BY b.block_id, COALESCE(b.fidelity_detail, b.fidelity), b.atom_count",
                    sql_placeholders(batch.len())
                );
                let mut statement = conn.prepare(&sql)?;
                let rows =
                    statement.query_map(rusqlite::params_from_iter(batch.iter()), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, u64>(2)?,
                            row.get::<_, u64>(3)?,
                            row.get::<_, u64>(4)?,
                        ))
                    })?;
                for row in rows {
                    let (id, fidelity, atom_count, joined_atom_count, total_bytes) = row?;
                    metadata.insert(
                        id.clone(),
                        HeaderMetadata {
                            fidelity,
                            atom_count,
                            joined_atom_count,
                            total_bytes,
                            block_id: Some(id),
                        },
                    );
                }
            }
        }
        for (id, raw) in [
            ("legacy:request-headers", legacy_request),
            ("legacy:response-headers", legacy_response),
        ] {
            if !order.iter().any(|value| value == id) {
                continue;
            }
            if let Some(raw) = raw {
                match legacy_header_atoms(raw) {
                    Ok(atoms) => {
                        let total_bytes = atoms.iter().try_fold(0u64, |sum, atom| {
                            sum.checked_add(atom.original_name.len() as u64)
                                .and_then(|value| value.checked_add(atom.value.len() as u64))
                        });
                        if let Some(total_bytes) = total_bytes {
                            metadata.insert(
                                id.into(),
                                HeaderMetadata {
                                    fidelity: "legacy_normalized".into(),
                                    atom_count: atoms.len() as u64,
                                    joined_atom_count: atoms.len() as u64,
                                    total_bytes,
                                    block_id: None,
                                },
                            );
                            atoms_by_id.insert(id.into(), atoms);
                        } else {
                            early_errors.insert(id.into(), "header byte count overflow".into());
                        }
                    }
                    Err(error) => {
                        early_errors.insert(id.into(), error.to_string());
                    }
                }
            }
        }

        let mut remaining = budget;
        let mut decisions = Vec::with_capacity(order.len());
        let mut selected_lar = Vec::<String>::new();
        for id in order {
            if let Some(message) = early_errors.remove(id) {
                decisions.push(HeaderDecision::Complete(LarStageContentHeaderBlock {
                    content_id: id.clone(),
                    block_id: None,
                    state: "error".into(),
                    fidelity: Some("legacy_normalized".into()),
                    total_atoms: None,
                    total_bytes: None,
                    atoms: Vec::new(),
                    error_kind: Some("legacy_headers_invalid".into()),
                    message: Some(message),
                }));
                continue;
            }
            let Some(info) = metadata.get(id) else {
                decisions.push(HeaderDecision::Complete(LarStageContentHeaderBlock {
                    content_id: id.clone(),
                    block_id: (!id.starts_with("legacy:")).then(|| id.clone()),
                    state: "missing".into(),
                    fidelity: None,
                    total_atoms: None,
                    total_bytes: None,
                    atoms: Vec::new(),
                    error_kind: Some("header_block_missing".into()),
                    message: Some("captured header block is absent from the catalog".into()),
                }));
                continue;
            };
            if info.atom_count != info.joined_atom_count {
                decisions.push(HeaderDecision::Complete(LarStageContentHeaderBlock {
                    content_id: id.clone(),
                    block_id: info.block_id.clone(),
                    state: "error".into(),
                    fidelity: Some(info.fidelity.clone()),
                    total_atoms: Some(info.atom_count),
                    total_bytes: Some(info.total_bytes),
                    atoms: Vec::new(),
                    error_kind: Some("header_atom_count_mismatch".into()),
                    message: Some("header atom catalog is incomplete".into()),
                }));
                continue;
            }
            if info.total_bytes > remaining {
                decisions.push(HeaderDecision::Complete(LarStageContentHeaderBlock {
                    content_id: id.clone(),
                    block_id: info.block_id.clone(),
                    state: "truncated".into(),
                    fidelity: Some(info.fidelity.clone()),
                    total_atoms: Some(info.atom_count),
                    total_bytes: Some(info.total_bytes),
                    atoms: Vec::new(),
                    error_kind: Some("header_byte_budget".into()),
                    message: Some(format!(
                        "header block is {} bytes; {remaining} bytes remain",
                        info.total_bytes
                    )),
                }));
                continue;
            }
            remaining -= info.total_bytes;
            if info.block_id.is_some() {
                selected_lar.push(id.clone());
            }
            decisions.push(HeaderDecision::Load(id.clone()));
        }

        {
            let conn = self.conn.lock().unwrap();
            for batch in catalog_query_batches(&selected_lar) {
                let sql = format!(
                    "SELECT a.block_id, a.ordinal, h.original_name_bytes, h.value_bytes, h.flags
                       FROM lar_header_block_atoms a
                       JOIN lar_header_atoms h ON h.atom_id=a.atom_id
                      WHERE a.block_id IN ({})
                      ORDER BY a.block_id, a.ordinal",
                    sql_placeholders(batch.len())
                );
                let mut statement = conn.prepare(&sql)?;
                let rows =
                    statement.query_map(rusqlite::params_from_iter(batch.iter()), |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            LarStageContentHeaderAtom {
                                ordinal: row.get(1)?,
                                original_name: row.get(2)?,
                                value: row.get(3)?,
                                flags: row.get(4)?,
                            },
                        ))
                    })?;
                for row in rows {
                    let (id, atom) = row?;
                    atoms_by_id.entry(id).or_default().push(atom);
                }
            }
        }

        let mut loaded = 0u64;
        let mut result = Vec::with_capacity(decisions.len());
        for decision in decisions {
            match decision {
                HeaderDecision::Complete(outcome) => result.push(outcome),
                HeaderDecision::Load(id) => {
                    let info = metadata.get(&id).expect("selected header metadata exists");
                    let atoms = atoms_by_id.remove(&id).unwrap_or_default();
                    let actual_bytes = atoms.iter().try_fold(0u64, |sum, atom| {
                        sum.checked_add(atom.original_name.len() as u64)
                            .and_then(|value| value.checked_add(atom.value.len() as u64))
                    });
                    if atoms.len() as u64 != info.atom_count
                        || actual_bytes != Some(info.total_bytes)
                    {
                        result.push(header_error(
                            &id,
                            &info.fidelity,
                            "header_atom_count_mismatch",
                            "loaded header atoms do not match aggregate catalog metadata",
                        ));
                        continue;
                    }
                    loaded += info.total_bytes;
                    result.push(LarStageContentHeaderBlock {
                        content_id: id,
                        block_id: info.block_id.clone(),
                        state: "available".into(),
                        fidelity: Some(info.fidelity.clone()),
                        total_atoms: Some(info.atom_count),
                        total_bytes: Some(info.total_bytes),
                        atoms,
                        error_kind: None,
                        message: None,
                    });
                }
            }
        }
        Ok((result, loaded))
    }

    fn load_stage_bodies(
        &self,
        trace_id: &str,
        order: &[BodySource],
        ids: &HashMap<BodySource, String>,
        budget: u64,
    ) -> Result<(Vec<LarStageArtifactContent>, u64)> {
        let manifest_metadata = self.stage_manifest_metadata(order)?;
        let mut remaining = budget;
        let mut selected_manifests = Vec::new();
        let mut result = HashMap::<String, LarStageArtifactContent>::new();
        let mut loaded = 0u64;
        for source in order {
            let content_id = ids
                .get(source)
                .expect("registered body source has an id")
                .clone();
            match source {
                BodySource::Manifest(manifest_id) => match manifest_metadata.get(manifest_id) {
                    Some(metadata) if metadata.total_length <= remaining => {
                        remaining -= metadata.total_length;
                        selected_manifests.push(manifest_id.clone());
                    }
                    Some(metadata) => {
                        let mut body = truncated_body(
                            &content_id,
                            Some(manifest_id),
                            None,
                            Some(metadata.total_length),
                            remaining,
                        );
                        apply_manifest_metadata(&mut body, metadata);
                        result.insert(content_id.clone(), body);
                    }
                    None => {
                        result.insert(
                            content_id.clone(),
                            body_error(
                                &content_id,
                                Some(manifest_id),
                                None,
                                "missing",
                                "manifest_missing",
                                "captured body manifest is absent from the catalog",
                            ),
                        );
                    }
                },
                BodySource::Legacy(artifact_kind) => {
                    let request = LarArtifactReadRequest::new("trace", trace_id, artifact_kind);
                    let outcome = self
                        .read_lar_or_legacy_artifact_batch_bounded(&[request], remaining)
                        .into_iter()
                        .next()
                        .unwrap_or(LarArtifactBatchRead::Missing);
                    let body = body_from_batch(&content_id, artifact_kind, outcome);
                    if let Some(bytes) = body.bytes.as_ref() {
                        remaining = remaining.saturating_sub(bytes.len() as u64);
                        loaded += bytes.len() as u64;
                    }
                    result.insert(content_id.clone(), body);
                }
                BodySource::Missing(artifact_kind) => {
                    result.insert(
                        content_id.clone(),
                        body_error(
                            &content_id,
                            None,
                            Some(artifact_kind),
                            "missing",
                            "artifact_missing",
                            "no captured body is available for this stage",
                        ),
                    );
                }
                BodySource::Error(kind, message) => {
                    result.insert(
                        content_id.clone(),
                        body_error(&content_id, None, None, "error", kind, message),
                    );
                }
            }
        }

        for (manifest_id, read) in self.read_stage_manifest_batch(&selected_manifests)? {
            let content_id = ids
                .get(&BodySource::Manifest(manifest_id.clone()))
                .expect("selected manifest is registered")
                .clone();
            let metadata = manifest_metadata.get(&manifest_id);
            let total = metadata.map(|value| value.total_length);
            let mut body = match read {
                LarArtifactBatchRead::Read(bytes) => {
                    loaded += bytes.len() as u64;
                    LarStageArtifactContent {
                        content_id: content_id.clone(),
                        manifest_id: Some(manifest_id),
                        artifact_kind: None,
                        state: "available".into(),
                        fidelity: "captured".into(),
                        total_bytes: Some(bytes.len() as u64),
                        media_type: None,
                        content_encoding: None,
                        bytes: Some(bytes),
                        error_kind: None,
                        message: None,
                        archive_file_uuid: None,
                        archive_path: None,
                    }
                }
                LarArtifactBatchRead::ArchiveUnavailable(error) => {
                    unavailable_body(&content_id, Some(&manifest_id), None, total, error)
                }
                LarArtifactBatchRead::Error { kind, detail } => body_error(
                    &content_id,
                    Some(&manifest_id),
                    None,
                    "error",
                    &kind,
                    &detail,
                ),
                LarArtifactBatchRead::Missing => body_error(
                    &content_id,
                    Some(&manifest_id),
                    None,
                    "missing",
                    "manifest_missing",
                    "captured body manifest is absent from the catalog",
                ),
                LarArtifactBatchRead::Truncated { .. } => {
                    unreachable!("budgeting happened before reads")
                }
            };
            if let Some(metadata) = metadata {
                apply_manifest_metadata(&mut body, metadata);
            }
            result.insert(content_id, body);
        }
        let ordered = order
            .iter()
            .map(|source| {
                let id = ids.get(source).expect("body source id exists");
                result.remove(id).expect("every body source has an outcome")
            })
            .collect();
        Ok((ordered, loaded))
    }

    fn stage_manifest_metadata(
        &self,
        order: &[BodySource],
    ) -> Result<HashMap<String, ManifestMetadata>> {
        let conn = self.conn.lock().unwrap();
        let mut metadata = HashMap::new();
        let ids = order
            .iter()
            .filter_map(|source| match source {
                BodySource::Manifest(id) => Some(id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for batch in catalog_query_batches(&ids) {
            let sql = format!(
                "SELECT manifest_id, total_length, media_type, content_encoding
                   FROM lar_manifests
                  WHERE state='ready' AND manifest_id IN ({})",
                sql_placeholders(batch.len())
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(batch.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    ManifestMetadata {
                        total_length: row.get(1)?,
                        media_type: row.get::<_, Option<String>>(2)?.map(String::into_bytes),
                        content_encoding: row.get::<_, Option<String>>(3)?.map(String::into_bytes),
                    },
                ))
            })?;
            for row in rows {
                let (id, value) = row?;
                metadata.insert(id, value);
            }
        }
        Ok(metadata)
    }

    /// Reconstruct a unique manifest set with one open file handle per pack
    /// and one decoded copy per shared chunk.
    fn read_stage_manifest_batch(
        &self,
        manifest_ids: &[String],
    ) -> Result<Vec<(String, LarArtifactBatchRead)>> {
        let (plans, locations) = self.stage_manifest_plans(manifest_ids)?;
        let limits = Limits::default();
        let mut files = HashMap::<PathBuf, File>::new();
        let mut chunks = HashMap::<ChunkHash, ChunkLoad>::new();
        let mut output = Vec::with_capacity(manifest_ids.len());
        for id in manifest_ids {
            let Some(plan) = plans.get(id) else {
                output.push((id.clone(), LarArtifactBatchRead::Missing));
                continue;
            };
            let read = reconstruct_plan(plan, &locations, &mut files, &mut chunks, &limits);
            output.push((id.clone(), read));
        }
        Ok(output)
    }

    fn stage_manifest_plans(
        &self,
        manifest_ids: &[String],
    ) -> Result<(
        HashMap<String, ManifestPlan>,
        HashMap<ChunkHash, ChunkLocation>,
    )> {
        let conn = self.conn.lock().unwrap();
        let mut plans = HashMap::new();
        let mut locations = HashMap::new();
        let valid_ids = manifest_ids
            .iter()
            .filter_map(|id| {
                ManifestId::from_str(id)
                    .ok()
                    .map(|parsed| (id.as_str(), parsed))
            })
            .collect::<Vec<_>>();
        let mut assemblies = HashMap::<String, ManifestAssembly>::new();
        for batch in catalog_query_batches(&valid_ids) {
            let sql = format!(
                "SELECT manifest_id, total_length, whole_body_hash, media_type, content_encoding
                   FROM lar_manifests
                  WHERE hash_algorithm='blake3' AND state='ready'
                    AND manifest_id IN ({})",
                sql_placeholders(batch.len())
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(batch.iter().map(|(id, _)| id)),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )?;
            for row in rows {
                let (id, total_length, whole_hash, media_type, content_encoding) = row?;
                let Some((_, parsed_id)) = valid_ids.iter().find(|(value, _)| *value == id) else {
                    continue;
                };
                let Ok(digest) = <Vec<u8> as TryInto<[u8; 32]>>::try_into(whole_hash) else {
                    continue;
                };
                assemblies.insert(
                    id,
                    ManifestAssembly {
                        parsed_id: *parsed_id,
                        total_length,
                        whole_body_hash: ChunkHash {
                            algorithm: HashAlgorithm::Blake3,
                            digest,
                        },
                        media_type: media_type.map(String::into_bytes),
                        content_encoding: content_encoding.map(String::into_bytes),
                        chunks: Vec::new(),
                    },
                );
            }
        }

        let assembly_ids = assemblies.keys().cloned().collect::<Vec<_>>();
        for batch in catalog_query_batches(&assembly_ids) {
            let sql = format!(
                "SELECT manifest_id, chunk_hash, logical_offset, chunk_offset, length
                   FROM lar_manifest_chunks
                  WHERE manifest_id IN ({})
                  ORDER BY manifest_id, ordinal",
                sql_placeholders(batch.len())
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(batch.iter()), |row| {
                let bytes: Vec<u8> = row.get(1)?;
                let digest: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Blob,
                        format!("invalid BLAKE3 digest length {}", bytes.len()).into(),
                    )
                })?;
                Ok((
                    row.get::<_, String>(0)?,
                    ChunkRef {
                        chunk_hash: ChunkHash {
                            algorithm: HashAlgorithm::Blake3,
                            digest,
                        },
                        logical_offset: row.get(2)?,
                        chunk_offset: row.get(3)?,
                        length: row.get(4)?,
                    },
                ))
            })?;
            for row in rows {
                let (id, reference) = row?;
                if let Some(assembly) = assemblies.get_mut(&id) {
                    assembly.chunks.push(reference);
                }
            }
        }

        let mut needed_chunks = HashSet::<ChunkHash>::new();
        for (id, assembly) in assemblies {
            let manifest = body_manifest_from_catalog(
                assembly.total_length,
                assembly.whole_body_hash,
                assembly.media_type,
                assembly.content_encoding,
                assembly.chunks,
            );
            if manifest.id != assembly.parsed_id {
                continue;
            }
            needed_chunks.extend(manifest.chunks.iter().map(|reference| reference.chunk_hash));
            plans.insert(id.clone(), ManifestPlan { id, manifest });
        }

        let chunk_digests = needed_chunks
            .iter()
            .map(|hash| hash.digest.to_vec())
            .collect::<Vec<_>>();
        for batch in catalog_query_batches(&chunk_digests) {
            let sql = format!(
                "SELECT c.chunk_hash, f.path, f.file_uuid, f.state, c.page_offset,
                        c.uncompressed_length, c.compressed_length
                   FROM lar_chunks c JOIN lar_files f ON f.file_uuid=c.file_uuid
                  WHERE c.hash_algorithm='blake3' AND c.state='ready'
                    AND c.chunk_hash IN ({})",
                sql_placeholders(batch.len())
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(batch.iter()), |row| {
                let bytes: Vec<u8> = row.get(0)?;
                let digest: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Blob,
                        format!("invalid BLAKE3 digest length {}", bytes.len()).into(),
                    )
                })?;
                let hash = ChunkHash {
                    algorithm: HashAlgorithm::Blake3,
                    digest,
                };
                Ok((
                    hash,
                    ChunkLocation {
                        path: resolved_catalog_path(&self.data_dir, &row.get::<_, String>(1)?),
                        file_uuid: row.get(2)?,
                        file_state: row.get(3)?,
                        descriptor: ChunkRecordDescriptor {
                            hash,
                            frame_offset: row.get(4)?,
                            uncompressed_length: row.get(5)?,
                            compressed_length: row.get(6)?,
                        },
                    },
                ))
            })?;
            for row in rows {
                let (hash, location) = row?;
                locations.insert(hash, location);
            }
        }
        Ok((plans, locations))
    }
}

fn catalog_query_batches<T>(values: &[T]) -> std::slice::Chunks<'_, T> {
    values.chunks(CATALOG_QUERY_BATCH_SIZE)
}

fn sql_placeholders(count: usize) -> String {
    debug_assert!(count > 0 && count <= CATALOG_QUERY_BATCH_SIZE);
    (1..=count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn body_manifest_from_catalog(
    total_length: u64,
    whole_body_hash: ChunkHash,
    media_type: Option<Vec<u8>>,
    content_encoding: Option<Vec<u8>>,
    chunks: Vec<ChunkRef>,
) -> BodyManifest {
    BodyManifest::new(
        total_length,
        whole_body_hash,
        media_type,
        content_encoding,
        chunks,
    )
}

fn validate_options(options: &LarStageContentOptions) -> Result<()> {
    if options.stage_limit == 0 || options.stage_limit > MAX_STAGE_CONTENT_LIMIT {
        return Err(LarStageContentError::new(
            "stage_content_invalid_request",
            format!("stage_limit must be between 1 and {MAX_STAGE_CONTENT_LIMIT}"),
        )
        .into());
    }
    if options.body_byte_budget > MAX_STAGE_CONTENT_BODY_BYTES {
        return Err(LarStageContentError::new(
            "stage_content_invalid_request",
            format!("body_byte_budget must not exceed {MAX_STAGE_CONTENT_BODY_BYTES}"),
        )
        .into());
    }
    if options.header_byte_budget > MAX_STAGE_CONTENT_HEADER_BYTES {
        return Err(LarStageContentError::new(
            "stage_content_invalid_request",
            format!("header_byte_budget must not exceed {MAX_STAGE_CONTENT_HEADER_BYTES}"),
        )
        .into());
    }
    if options.after_capture_sequence.is_some() != options.after_stage_id.is_some() {
        return Err(LarStageContentError::new(
            "stage_content_invalid_request",
            "after_capture_sequence and after_stage_id must be provided together",
        )
        .into());
    }
    if options
        .after_stage_id
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > MAX_STAGE_CONTENT_CURSOR_ID_BYTES)
    {
        return Err(LarStageContentError::new(
            "stage_content_invalid_request",
            format!("after_stage_id must contain 1 to {MAX_STAGE_CONTENT_CURSOR_ID_BYTES} bytes"),
        )
        .into());
    }
    Ok(())
}

fn artifact_is_captured(store: &Store, trace_id: &str, artifact_kind: &str) -> bool {
    matches!(
        store.lar_artifact_location("trace", trace_id, artifact_kind, None),
        Ok(Some(_))
    )
}

fn synthesize_legacy_stages(
    trace_id: &str,
    upstream_exists: bool,
    response_exists: bool,
) -> Vec<LarStageContentRecord> {
    let limitations = vec![
        "legacy capture does not preserve exact per-stage timing".into(),
        "legacy header JSON does not preserve wire ordering or duplicate fields".into(),
        "legacy capture does not preserve per-attempt upstream headers".into(),
    ];
    let mut kinds = vec!["client_request"];
    if upstream_exists {
        kinds.push("upstream_request");
    }
    if response_exists {
        kinds.push("client_response");
    }
    kinds
        .into_iter()
        .enumerate()
        .map(|(capture_sequence, kind)| LarStageContentRecord {
            stage_id: format!(
                "legacy-{}",
                blake3::hash(format!("{trace_id}\0{kind}").as_bytes()).to_hex()
            ),
            capture_sequence: capture_sequence as u64,
            kind: kind.into(),
            attempt_number: None,
            wall_time_ns: None,
            monotonic_delta_ns: None,
            fidelity: "legacy".into(),
            request_headers_ref: None,
            request_headers_content_id: None,
            request_body_manifest_ref: None,
            request_body_content_id: None,
            response_headers_ref: None,
            response_headers_content_id: None,
            response_body_manifest_ref: None,
            response_body_content_id: None,
            trailers_ref: None,
            trailers_content_id: None,
            stream_index_ref: None,
            limitations: limitations.clone(),
        })
        .collect()
}

fn stage_is_after_options(stage: &LarStageContentRecord, options: &LarStageContentOptions) -> bool {
    cursor_is_after(stage.capture_sequence, &stage.stage_id, options)
}

fn cursor_is_after(
    capture_sequence: u64,
    candidate_stage_id: &str,
    options: &LarStageContentOptions,
) -> bool {
    match (
        options.after_capture_sequence,
        options.after_stage_id.as_deref(),
    ) {
        (Some(sequence), Some(cursor_stage_id)) => {
            capture_sequence > sequence
                || (capture_sequence == sequence && candidate_stage_id > cursor_stage_id)
        }
        _ => true,
    }
}

fn register_header_ref(
    reference: &Option<String>,
    content_id: &mut Option<String>,
    order: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    let Some(reference) = reference else { return };
    *content_id = Some(reference.clone());
    if seen.insert(reference.clone()) {
        order.push(reference.clone());
    }
}

fn register_legacy_headers(
    stages: &mut [LarStageContentRecord],
    request: Option<&str>,
    response: Option<&str>,
    order: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for stage in stages {
        let legacy = match stage.kind.as_str() {
            "client_request" if stage.request_headers_content_id.is_none() && request.is_some() => {
                Some((
                    &mut stage.request_headers_content_id,
                    "legacy:request-headers",
                ))
            }
            "client_response"
                if stage.response_headers_content_id.is_none() && response.is_some() =>
            {
                Some((
                    &mut stage.response_headers_content_id,
                    "legacy:response-headers",
                ))
            }
            _ => None,
        };
        if let Some((target, id)) = legacy {
            *target = Some(id.into());
            if seen.insert(id.into()) {
                order.push(id.into());
            }
        }
    }
}

fn legacy_header_atoms(raw: &str) -> Result<Vec<LarStageContentHeaderAtom>> {
    let value: serde_json::Value = serde_json::from_str(raw).context("decoding legacy headers")?;
    let object = value
        .as_object()
        .context("legacy headers are not a JSON object")?;
    let mut values = object
        .iter()
        .map(|(name, value)| {
            let value = match value {
                serde_json::Value::String(value) => value.clone(),
                other => other.to_string(),
            };
            (name.clone(), value)
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.0.to_lowercase().cmp(&right.0.to_lowercase()));
    Ok(values
        .into_iter()
        .enumerate()
        .map(|(ordinal, (name, value))| LarStageContentHeaderAtom {
            ordinal: ordinal as u64,
            original_name: name.into_bytes(),
            value: value.into_bytes(),
            flags: 0,
        })
        .collect())
}

fn header_error(id: &str, fidelity: &str, kind: &str, message: &str) -> LarStageContentHeaderBlock {
    LarStageContentHeaderBlock {
        content_id: id.into(),
        block_id: (!id.starts_with("legacy:")).then(|| id.into()),
        state: "error".into(),
        fidelity: Some(fidelity.into()),
        total_atoms: None,
        total_bytes: None,
        atoms: Vec::new(),
        error_kind: Some(kind.into()),
        message: Some(message.into()),
    }
}

fn fallback_request_artifact<'a>(
    stage: &'a LarStageContentRecord,
    last_upstream_request: Option<&str>,
) -> Option<&'a str> {
    match stage.kind.as_str() {
        "client_request" => Some("client_request"),
        "upstream_request" if Some(stage.stage_id.as_str()) == last_upstream_request => {
            Some("upstream_request")
        }
        _ => None,
    }
}

fn fallback_response_artifact(stage: &LarStageContentRecord) -> Option<&str> {
    match stage.kind.as_str() {
        "client_response" => Some("client_response"),
        _ => None,
    }
}

fn resolve_body_content(
    store: &Store,
    trace_id: &str,
    manifest_id: Option<&str>,
    fallback_artifact: Option<&str>,
    order: &mut Vec<BodySource>,
    ids: &mut HashMap<BodySource, String>,
) -> Option<String> {
    let source = if let Some(manifest_id) = manifest_id {
        BodySource::Manifest(manifest_id.into())
    } else if let Some(artifact_kind) = fallback_artifact {
        match store.lar_artifact_location("trace", trace_id, artifact_kind, None) {
            Ok(Some(LarArtifactLocation::Lar { manifest_id, .. })) => {
                BodySource::Manifest(manifest_id)
            }
            Ok(Some(LarArtifactLocation::Legacy { .. })) => {
                BodySource::Legacy(artifact_kind.into())
            }
            Ok(Some(LarArtifactLocation::Unavailable { error, .. })) => {
                BodySource::Error(error.kind, error.detail)
            }
            Ok(None) => BodySource::Missing(artifact_kind.into()),
            Err(error) => BodySource::Error("catalog".into(), format!("{error:#}")),
        }
    } else {
        return None;
    };
    if let Some(existing) = ids.get(&source) {
        return Some(existing.clone());
    }
    let id = match &source {
        BodySource::Manifest(value) => value.clone(),
        BodySource::Legacy(kind) => format!("legacy:{kind}"),
        BodySource::Missing(kind) => format!("missing:{kind}"),
        BodySource::Error(kind, message) => {
            format!(
                "error:{}",
                blake3::hash(format!("{kind}\0{message}").as_bytes()).to_hex()
            )
        }
    };
    ids.insert(source.clone(), id.clone());
    order.push(source);
    Some(id)
}

fn body_from_batch(
    content_id: &str,
    artifact_kind: &str,
    outcome: LarArtifactBatchRead,
) -> LarStageArtifactContent {
    match outcome {
        LarArtifactBatchRead::Read(bytes) => LarStageArtifactContent {
            content_id: content_id.into(),
            manifest_id: None,
            artifact_kind: Some(artifact_kind.into()),
            state: "available".into(),
            fidelity: "legacy".into(),
            total_bytes: Some(bytes.len() as u64),
            media_type: None,
            content_encoding: None,
            bytes: Some(bytes),
            error_kind: None,
            message: None,
            archive_file_uuid: None,
            archive_path: None,
        },
        LarArtifactBatchRead::Missing => body_error(
            content_id,
            None,
            Some(artifact_kind),
            "missing",
            "artifact_missing",
            "legacy body is missing",
        ),
        LarArtifactBatchRead::Truncated {
            total_length,
            budget_remaining,
        } => truncated_body(
            content_id,
            None,
            Some(artifact_kind),
            total_length,
            budget_remaining,
        ),
        LarArtifactBatchRead::Error { kind, detail } => body_error(
            content_id,
            None,
            Some(artifact_kind),
            "error",
            &kind,
            &detail,
        ),
        LarArtifactBatchRead::ArchiveUnavailable(error) => {
            unavailable_body(content_id, None, Some(artifact_kind), None, error)
        }
    }
}

fn truncated_body(
    content_id: &str,
    manifest_id: Option<&String>,
    artifact_kind: Option<&str>,
    total: Option<u64>,
    remaining: u64,
) -> LarStageArtifactContent {
    LarStageArtifactContent {
        content_id: content_id.into(),
        manifest_id: manifest_id.cloned(),
        artifact_kind: artifact_kind.map(str::to_owned),
        state: "truncated".into(),
        fidelity: if manifest_id.is_some() {
            "captured"
        } else {
            "legacy"
        }
        .into(),
        total_bytes: total,
        media_type: None,
        content_encoding: None,
        bytes: None,
        error_kind: Some("body_byte_budget".into()),
        message: Some(format!(
            "body exceeds the {remaining} remaining byte budget"
        )),
        archive_file_uuid: None,
        archive_path: None,
    }
}

fn body_error(
    content_id: &str,
    manifest_id: Option<&String>,
    artifact_kind: Option<&str>,
    state: &str,
    kind: &str,
    message: &str,
) -> LarStageArtifactContent {
    LarStageArtifactContent {
        content_id: content_id.into(),
        manifest_id: manifest_id.cloned(),
        artifact_kind: artifact_kind.map(str::to_owned),
        state: state.into(),
        fidelity: if manifest_id.is_some() {
            "captured"
        } else {
            "legacy"
        }
        .into(),
        total_bytes: None,
        media_type: None,
        content_encoding: None,
        bytes: None,
        error_kind: Some(kind.into()),
        message: Some(message.into()),
        archive_file_uuid: None,
        archive_path: None,
    }
}

fn unavailable_body(
    content_id: &str,
    manifest_id: Option<&String>,
    artifact_kind: Option<&str>,
    total: Option<u64>,
    error: LarArchiveUnavailableError,
) -> LarStageArtifactContent {
    LarStageArtifactContent {
        content_id: content_id.into(),
        manifest_id: manifest_id.cloned(),
        artifact_kind: artifact_kind.map(str::to_owned),
        state: error.code().into(),
        fidelity: if manifest_id.is_some() {
            "captured"
        } else {
            "legacy"
        }
        .into(),
        total_bytes: total,
        media_type: None,
        content_encoding: None,
        bytes: None,
        error_kind: Some(error.code().into()),
        message: Some(error.to_string()),
        archive_file_uuid: Some(error.file_uuid),
        archive_path: Some(error.path),
    }
}

fn apply_manifest_metadata(body: &mut LarStageArtifactContent, metadata: &ManifestMetadata) {
    body.media_type = metadata.media_type.clone();
    body.content_encoding = metadata.content_encoding.clone();
}

fn reconstruct_plan(
    plan: &ManifestPlan,
    locations: &HashMap<ChunkHash, ChunkLocation>,
    files: &mut HashMap<PathBuf, File>,
    chunks: &mut HashMap<ChunkHash, ChunkLoad>,
    limits: &Limits,
) -> LarArtifactBatchRead {
    if let Err(error) = plan.manifest.validate() {
        return LarArtifactBatchRead::Error {
            kind: "manifest_invalid".into(),
            detail: error.to_string(),
        };
    }
    let capacity = match usize::try_from(plan.manifest.total_length) {
        Ok(value) => value,
        Err(error) => {
            return LarArtifactBatchRead::Error {
                kind: "manifest_invalid".into(),
                detail: error.to_string(),
            }
        }
    };
    let mut body = Vec::with_capacity(capacity);
    for reference in &plan.manifest.chunks {
        if !chunks.contains_key(&reference.chunk_hash) {
            let loaded = match locations.get(&reference.chunk_hash) {
                None => ChunkLoad::Error("catalog chunk is missing".into()),
                Some(location) if !matches!(location.file_state.as_str(), "active" | "sealed") => {
                    ChunkLoad::ArchiveUnavailable(LarArchiveUnavailableError {
                        availability: LarArchiveAvailability::ArchivedOffline,
                        file_uuid: location.file_uuid.clone(),
                        path: location.path.to_string_lossy().into_owned(),
                    })
                }
                Some(location) => {
                    let open_error = if !files.contains_key(&location.path) {
                        match File::open(&location.path) {
                            Ok(file) => {
                                files.insert(location.path.clone(), file);
                                None
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                Some(ChunkLoad::ArchiveUnavailable(LarArchiveUnavailableError {
                                    availability: LarArchiveAvailability::ArchivedMissing,
                                    file_uuid: location.file_uuid.clone(),
                                    path: location.path.to_string_lossy().into_owned(),
                                }))
                            }
                            Err(error) => Some(ChunkLoad::Error(format!(
                                "opening {} failed: {error}",
                                location.path.display()
                            ))),
                        }
                    } else {
                        None
                    };
                    match open_error {
                        Some(error) => error,
                        None => match read_chunk_record_at(
                            files.get_mut(&location.path).expect("file was inserted"),
                            &location.descriptor,
                            limits,
                        ) {
                            Ok(bytes) => ChunkLoad::Bytes(bytes),
                            Err(error) => ChunkLoad::Error(error.to_string()),
                        },
                    }
                }
            };
            chunks.insert(reference.chunk_hash, loaded);
        }
        let bytes = match chunks
            .get(&reference.chunk_hash)
            .expect("chunk outcome was inserted")
        {
            ChunkLoad::Bytes(bytes) => bytes,
            ChunkLoad::ArchiveUnavailable(error) => {
                return LarArtifactBatchRead::ArchiveUnavailable(error.clone())
            }
            ChunkLoad::Error(detail) => {
                return LarArtifactBatchRead::Error {
                    kind: "archive_read".into(),
                    detail: detail.clone(),
                }
            }
        };
        let start = match usize::try_from(reference.chunk_offset) {
            Ok(value) => value,
            Err(error) => {
                return LarArtifactBatchRead::Error {
                    kind: "manifest_invalid".into(),
                    detail: error.to_string(),
                }
            }
        };
        let end = match reference
            .chunk_offset
            .checked_add(reference.length)
            .and_then(|value| usize::try_from(value).ok())
        {
            Some(value) => value,
            None => {
                return LarArtifactBatchRead::Error {
                    kind: "manifest_invalid".into(),
                    detail: "chunk range overflow".into(),
                }
            }
        };
        let Some(range) = bytes.get(start..end) else {
            return LarArtifactBatchRead::Error {
                kind: "manifest_invalid".into(),
                detail: "manifest range exceeds decoded chunk".into(),
            };
        };
        body.extend_from_slice(range);
    }
    if body.len() as u64 != plan.manifest.total_length
        || ChunkHash::blake3(&body) != plan.manifest.whole_body_hash
    {
        return LarArtifactBatchRead::Error {
            kind: "manifest_hash_mismatch".into(),
            detail: format!(
                "reconstructed manifest {} failed length/hash validation",
                plan.id
            ),
        };
    }
    LarArtifactBatchRead::Read(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_queries_are_bounded_and_cursor_ties_use_stage_id() {
        let values = (0..(CATALOG_QUERY_BATCH_SIZE * 2 + 1)).collect::<Vec<_>>();
        assert_eq!(
            catalog_query_batches(&values)
                .map(<[_]>::len)
                .collect::<Vec<_>>(),
            [CATALOG_QUERY_BATCH_SIZE, CATALOG_QUERY_BATCH_SIZE, 1]
        );
        assert_eq!(sql_placeholders(3), "?1,?2,?3");

        let options = LarStageContentOptions {
            after_capture_sequence: Some(7),
            after_stage_id: Some("middle".into()),
            ..Default::default()
        };
        assert!(!cursor_is_after(6, "z", &options));
        assert!(!cursor_is_after(7, "before", &options));
        assert!(!cursor_is_after(7, "middle", &options));
        assert!(cursor_is_after(7, "z", &options));
        assert!(cursor_is_after(8, "a", &options));
    }

    #[test]
    fn catalog_manifest_identity_preserves_media_type_and_content_encoding() {
        let bytes = b"body";
        let hash = ChunkHash::blake3(bytes);
        let chunks = vec![ChunkRef {
            chunk_hash: hash,
            logical_offset: 0,
            chunk_offset: 0,
            length: bytes.len() as u64,
        }];
        let expected = BodyManifest::new(
            bytes.len() as u64,
            hash,
            Some(b"application/json".to_vec()),
            Some(b"gzip".to_vec()),
            chunks.clone(),
        );
        let reconstructed = body_manifest_from_catalog(
            bytes.len() as u64,
            hash,
            Some("application/json".to_string().into_bytes()),
            Some("gzip".to_string().into_bytes()),
            chunks,
        );
        assert_eq!(reconstructed.id, expected.id);
        assert_eq!(
            reconstructed.media_type.as_deref(),
            Some(b"application/json".as_slice())
        );
        assert_eq!(
            reconstructed.content_encoding.as_deref(),
            Some(b"gzip".as_slice())
        );
    }
}
