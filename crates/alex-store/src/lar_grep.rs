use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alex_lar::{
    read_chunk_record_at, ArchiveReader, BodyManifest, ChunkHash, ChunkRecordDescriptor, ChunkRef,
    ExchangeId, HashAlgorithm, HeaderBlock, Limits, ManifestId, RawBodyScanner, RawSearchLimits,
    RawSearchStats,
};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::lar_archive_ops::resolved_catalog_path;
use crate::live_body_store::{sensitive_header_name, LAR_HEADER_FLAG_REDACTED};
use crate::Store;

const MAX_OPEN_GREP_PACKS: usize = 32;
const MAX_RECORD_GREP_EXCHANGES: usize = 100_000;
const MAX_RECORD_GREP_STAGES: u64 = 1_000_000;
const MAX_RECORD_GREP_HEADER_ATOMS: u64 = 2_000_000;
const MAX_RECORD_GREP_FIELD_BYTES: u64 = 256 * 1024 * 1024;

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

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct LarRecordGrepMatch {
    pub category: String,
    pub field: String,
    pub match_offset: u64,
    pub header_ordinal: Option<u64>,
    pub stage_id: Option<String>,
    pub trace_id: String,
    pub session_id: Option<String>,
    pub timestamp_ns: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LarRecordGrepCoverage {
    pub category: String,
    pub status: String,
    pub records_scanned: u64,
    pub bytes_scanned: u64,
    pub values_skipped: u64,
    pub missing_records: u64,
    pub excluded_fields: Vec<String>,
    pub details: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarRecordGrepReport {
    pub matches: Vec<LarRecordGrepMatch>,
    pub coverage: Vec<LarRecordGrepCoverage>,
}

impl LarRecordGrepCoverage {
    fn new(category: &str, excluded_fields: &[&str]) -> Self {
        Self {
            category: category.into(),
            status: if excluded_fields.is_empty() {
                "searched".into()
            } else {
                "searched_with_exclusions".into()
            },
            records_scanned: 0,
            bytes_scanned: 0,
            values_skipped: 0,
            missing_records: 0,
            excluded_fields: excluded_fields
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            details: Vec::new(),
        }
    }

    fn merge(&mut self, other: Self) {
        self.records_scanned = self.records_scanned.saturating_add(other.records_scanned);
        self.bytes_scanned = self.bytes_scanned.saturating_add(other.bytes_scanned);
        self.values_skipped = self.values_skipped.saturating_add(other.values_skipped);
        self.missing_records = self.missing_records.saturating_add(other.missing_records);
        self.excluded_fields.extend(other.excluded_fields);
        self.excluded_fields.sort();
        self.excluded_fields.dedup();
        self.details.extend(other.details);
        self.details.sort();
        self.details.dedup();
        if self.missing_records > 0 || self.status == "partial" || other.status == "partial" {
            self.status = "partial".into();
        } else if !self.excluded_fields.is_empty() {
            self.status = "searched_with_exclusions".into();
        }
    }
}

fn record_grep_coverage() -> Vec<LarRecordGrepCoverage> {
    vec![
        LarRecordGrepCoverage::new("ordered_headers", &[]),
        LarRecordGrepCoverage::new("ordered_trailers", &[]),
        LarRecordGrepCoverage::new(
            "stage_metadata",
            &["account_id", "cost", "status", "timing", "token_usage"],
        ),
        LarRecordGrepCoverage::new(
            "exchange_metadata",
            &[
                "attempts_json",
                "client_ip",
                "key_fingerprint",
                "original_account_id",
                "path",
                "served_account_id",
                "subscription_identity",
                "tags_json",
                "unknown_attributes",
            ],
        ),
    ]
}

fn coverage_mut<'a>(
    coverage: &'a mut [LarRecordGrepCoverage],
    category: &str,
) -> &'a mut LarRecordGrepCoverage {
    coverage
        .iter_mut()
        .find(|value| value.category == category)
        .expect("fixed record grep coverage category exists")
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
        let mut readers = CatalogFileReaders::new(MAX_OPEN_GREP_PACKS);
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

    /// Search privacy-filtered canonical records reachable from live catalog
    /// exchanges. Unlike raw body grep, this never scans arbitrary SQLite text
    /// columns or unreferenced bytes in a pack.
    pub fn grep_lar_catalog_records(
        &self,
        literal: &[u8],
        result_limit: usize,
    ) -> Result<LarRecordGrepReport> {
        if literal.is_empty() {
            bail!("LAR record grep literal must not be empty");
        }
        if result_limit == 0 {
            bail!("LAR grep result limit must be greater than zero");
        }
        let (sources, offline_count) = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT e.trace_id, f.path
                   FROM lar_exchange_records e
                   JOIN lar_files f ON f.file_uuid=e.file_uuid
                  WHERE f.state IN ('active','sealed')
                  ORDER BY f.path, e.capture_sequence, e.exchange_id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.len() > MAX_RECORD_GREP_EXCHANGES {
                bail!(
                    "record grep exchanges exceeds limit ({} > {MAX_RECORD_GREP_EXCHANGES})",
                    rows.len()
                );
            }
            let mut grouped = HashMap::<String, Vec<String>>::new();
            for (trace_id, path) in rows {
                grouped.entry(path).or_default().push(trace_id);
            }
            let offline_count = conn.query_row(
                "SELECT COUNT(*) FROM lar_exchange_records e
                   JOIN lar_files f ON f.file_uuid=e.file_uuid
                  WHERE f.state NOT IN ('active','sealed')",
                [],
                |row| row.get::<_, u64>(0),
            )?;
            (grouped, offline_count)
        };

        let mut matches = Vec::new();
        let mut coverage = record_grep_coverage();
        let mut ordered_sources = sources.into_iter().collect::<Vec<_>>();
        ordered_sources.sort_by(|left, right| left.0.cmp(&right.0));
        for (stored_path, trace_ids) in ordered_sources {
            let path = resolved_catalog_path(&self.data_dir, &stored_path);
            let file = File::open(&path)
                .with_context(|| format!("opening live LAR record source {}", path.display()))?;
            let reader = ArchiveReader::open(file, Limits::default())
                .map_err(anyhow::Error::new)
                .with_context(|| format!("reading live LAR record source {}", path.display()))?;
            let ids = trace_ids
                .iter()
                .map(|trace_id| {
                    reader
                        .exchange_by_trace(trace_id.as_bytes())
                        .map(|exchange| exchange.id)
                        .with_context(|| {
                            format!("cataloged trace {trace_id} has no canonical exchange")
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            let report =
                grep_lar_archive_records_for_exchanges(&reader, ids.iter(), literal, result_limit)?;
            append_record_matches(&mut matches, report.matches, result_limit)?;
            merge_coverage(&mut coverage, report.coverage);
        }
        if offline_count > 0 {
            for item in &mut coverage {
                item.status = "partial".into();
                item.details.push(format!(
                    "{offline_count} catalog exchange(s) are in offline archives and were not searched"
                ));
            }
        }
        matches.sort();
        Ok(LarRecordGrepReport { matches, coverage })
    }
}

/// Search safe canonical fields reachable from every exchange in one opened
/// archive. Sensitive header values are skipped even when a foreign archive
/// failed to set Alex's redaction flag.
pub fn grep_lar_archive_records<R: Read + Seek>(
    reader: &ArchiveReader<R>,
    literal: &[u8],
    result_limit: usize,
) -> Result<LarRecordGrepReport> {
    let mut ids = reader.exchange_ids().copied().collect::<Vec<_>>();
    ids.sort_by_key(|id| id.0);
    if ids.len() > MAX_RECORD_GREP_EXCHANGES {
        bail!(
            "record grep exchanges exceeds limit ({} > {MAX_RECORD_GREP_EXCHANGES})",
            ids.len()
        );
    }
    grep_lar_archive_records_for_exchanges(reader, ids.iter(), literal, result_limit)
}

fn grep_lar_archive_records_for_exchanges<'a, R, I>(
    reader: &ArchiveReader<R>,
    exchange_ids: I,
    literal: &[u8],
    result_limit: usize,
) -> Result<LarRecordGrepReport>
where
    R: Read + Seek,
    I: IntoIterator<Item = &'a ExchangeId>,
{
    if literal.is_empty() {
        bail!("LAR record grep literal must not be empty");
    }
    if result_limit == 0 {
        bail!("LAR grep result limit must be greater than zero");
    }
    let mut matches = Vec::new();
    let mut coverage = record_grep_coverage();
    let mut stages_scanned = 0u64;
    let mut header_atoms_scanned = 0u64;
    for exchange_id in exchange_ids {
        let Some(exchange) = reader.exchange(exchange_id) else {
            let item = coverage_mut(&mut coverage, "exchange_metadata");
            item.missing_records = item.missing_records.saturating_add(1);
            item.status = "partial".into();
            continue;
        };
        let trace_id = String::from_utf8_lossy(&exchange.data.trace_id).into_owned();
        let session_id = exchange
            .data
            .session_id
            .as_deref()
            .map(String::from_utf8_lossy)
            .map(|value| value.into_owned());
        for (field, value) in [
            ("trace_id", Some(exchange.data.trace_id.as_slice())),
            ("session_id", exchange.data.session_id.as_deref()),
            ("run_id", exchange.data.run_id.as_deref()),
            ("parent_trace_id", exchange.data.parent_trace_id.as_deref()),
            ("clock_id", exchange.data.clock_id.as_deref()),
        ] {
            if let Some(value) = value {
                scan_record_field(
                    &mut matches,
                    &mut coverage,
                    literal,
                    "exchange_metadata",
                    field,
                    value,
                    None,
                    None,
                    &trace_id,
                    session_id.as_deref(),
                    exchange.data.wall_time_ns,
                    result_limit,
                )?;
            }
        }
        if let Some(metadata) = reader.exchange_metadata(exchange_id) {
            let data = &metadata.data;
            for (field, value) in [
                ("harness", data.harness.as_deref()),
                ("client_format", data.client_format.as_deref()),
                ("upstream_format", data.upstream_format.as_deref()),
                ("method", data.method.as_deref()),
                ("billing_bucket", data.billing_bucket.as_deref()),
                ("error_kind", data.error_kind.as_deref()),
                ("error_code", data.error_code.as_deref()),
                ("original_model", data.original_model.as_deref()),
                ("served_model", data.served_model.as_deref()),
                ("substitution_reason", data.substitution_reason.as_deref()),
                ("fixture_name", data.fixture_name.as_deref()),
                ("reasoning_effort", data.reasoning_effort.as_deref()),
            ] {
                if let Some(value) = value {
                    scan_record_field(
                        &mut matches,
                        &mut coverage,
                        literal,
                        "exchange_metadata",
                        field,
                        value,
                        None,
                        None,
                        &trace_id,
                        session_id.as_deref(),
                        exchange.data.wall_time_ns,
                        result_limit,
                    )?;
                }
            }
        } else {
            let item = coverage_mut(&mut coverage, "exchange_metadata");
            item.missing_records = item.missing_records.saturating_add(1);
            item.status = "partial".into();
            item.details
                .push("an exchange has no optional metadata companion record".into());
        }

        for stage_id in &exchange.data.stages {
            stages_scanned = stages_scanned.saturating_add(1);
            if stages_scanned > MAX_RECORD_GREP_STAGES {
                bail!(
                    "record grep stages exceeds limit ({stages_scanned} > {MAX_RECORD_GREP_STAGES})"
                );
            }
            let Some(stage) = reader.stage(stage_id) else {
                let item = coverage_mut(&mut coverage, "stage_metadata");
                item.missing_records = item.missing_records.saturating_add(1);
                item.status = "partial".into();
                continue;
            };
            let stage_id = stage.id.to_string();
            let kind = format!("{:?}", stage.data.kind);
            scan_record_field(
                &mut matches,
                &mut coverage,
                literal,
                "stage_metadata",
                "kind",
                kind.as_bytes(),
                None,
                Some(&stage_id),
                &trace_id,
                session_id.as_deref(),
                stage.data.wall_time_ns,
                result_limit,
            )?;
            for (field, value) in [
                ("provider", stage.data.provider.as_deref()),
                ("requested_model", stage.data.requested_model.as_deref()),
                ("routed_model", stage.data.routed_model.as_deref()),
                ("routing_reason", stage.data.routing_reason.as_deref()),
                ("error_class", stage.data.error_class.as_deref()),
                ("error_message", stage.data.error_message.as_deref()),
            ] {
                if let Some(value) = value {
                    scan_record_field(
                        &mut matches,
                        &mut coverage,
                        literal,
                        "stage_metadata",
                        field,
                        value,
                        None,
                        Some(&stage_id),
                        &trace_id,
                        session_id.as_deref(),
                        stage.data.wall_time_ns,
                        result_limit,
                    )?;
                }
            }
            for (header_id, category, field) in [
                (
                    stage.data.request_headers_ref,
                    "ordered_headers",
                    "request_headers",
                ),
                (
                    stage.data.response_headers_ref,
                    "ordered_headers",
                    "response_headers",
                ),
                (stage.data.trailers_ref, "ordered_trailers", "trailers"),
            ] {
                let Some(header_id) = header_id else { continue };
                let Some(block) = reader.header_block(&header_id) else {
                    let item = coverage_mut(&mut coverage, category);
                    item.missing_records = item.missing_records.saturating_add(1);
                    item.status = "partial".into();
                    continue;
                };
                scan_header_block(
                    block,
                    &mut matches,
                    &mut coverage,
                    &mut header_atoms_scanned,
                    literal,
                    category,
                    field,
                    &stage_id,
                    &trace_id,
                    session_id.as_deref(),
                    stage.data.wall_time_ns,
                    result_limit,
                )?;
            }
        }
    }
    for item in &mut coverage {
        item.details.sort();
        item.details.dedup();
    }
    matches.sort();
    Ok(LarRecordGrepReport { matches, coverage })
}

#[allow(clippy::too_many_arguments)]
fn scan_header_block(
    block: &HeaderBlock,
    matches: &mut Vec<LarRecordGrepMatch>,
    coverage: &mut [LarRecordGrepCoverage],
    header_atoms_scanned: &mut u64,
    literal: &[u8],
    category: &str,
    field: &str,
    stage_id: &str,
    trace_id: &str,
    session_id: Option<&str>,
    timestamp_ns: u64,
    result_limit: usize,
) -> Result<()> {
    for (ordinal, atom) in block.atoms.iter().enumerate() {
        *header_atoms_scanned = header_atoms_scanned.saturating_add(1);
        if *header_atoms_scanned > MAX_RECORD_GREP_HEADER_ATOMS {
            bail!(
                "record grep header atoms exceeds limit ({} > {MAX_RECORD_GREP_HEADER_ATOMS})",
                *header_atoms_scanned
            );
        }
        let ordinal = u64::try_from(ordinal).context("header ordinal exceeds u64")?;
        scan_record_field(
            matches,
            coverage,
            literal,
            category,
            &format!("{field}.name"),
            &atom.original_name,
            Some(ordinal),
            Some(stage_id),
            trace_id,
            session_id,
            timestamp_ns,
            result_limit,
        )?;
        if atom.flags & LAR_HEADER_FLAG_REDACTED != 0 || sensitive_header_name(&atom.original_name)
        {
            coverage_mut(coverage, category).values_skipped += 1;
            continue;
        }
        scan_record_field(
            matches,
            coverage,
            literal,
            category,
            &format!("{field}.value"),
            &atom.value,
            Some(ordinal),
            Some(stage_id),
            trace_id,
            session_id,
            timestamp_ns,
            result_limit,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn scan_record_field(
    matches: &mut Vec<LarRecordGrepMatch>,
    coverage: &mut [LarRecordGrepCoverage],
    literal: &[u8],
    category: &str,
    field: &str,
    bytes: &[u8],
    header_ordinal: Option<u64>,
    stage_id: Option<&str>,
    trace_id: &str,
    session_id: Option<&str>,
    timestamp_ns: u64,
    result_limit: usize,
) -> Result<()> {
    let item = coverage_mut(coverage, category);
    item.records_scanned = item.records_scanned.saturating_add(1);
    item.bytes_scanned = item.bytes_scanned.saturating_add(bytes.len() as u64);
    let total = coverage.iter().map(|item| item.bytes_scanned).sum::<u64>();
    if total > MAX_RECORD_GREP_FIELD_BYTES {
        bail!(
            "record grep canonical field bytes exceeds limit ({total} > {MAX_RECORD_GREP_FIELD_BYTES})"
        );
    }
    let Some(offset) = bytes
        .windows(literal.len())
        .position(|window| window == literal)
    else {
        return Ok(());
    };
    push_record_match(
        matches,
        LarRecordGrepMatch {
            category: category.into(),
            field: field.into(),
            match_offset: offset as u64,
            header_ordinal,
            stage_id: stage_id.map(str::to_owned),
            trace_id: trace_id.into(),
            session_id: session_id.map(str::to_owned),
            timestamp_ns,
        },
        result_limit,
    )
}

fn push_record_match(
    matches: &mut Vec<LarRecordGrepMatch>,
    value: LarRecordGrepMatch,
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

fn append_record_matches(
    matches: &mut Vec<LarRecordGrepMatch>,
    values: Vec<LarRecordGrepMatch>,
    limit: usize,
) -> Result<()> {
    for value in values {
        push_record_match(matches, value, limit)?;
    }
    Ok(())
}

fn merge_coverage(coverage: &mut [LarRecordGrepCoverage], values: Vec<LarRecordGrepCoverage>) {
    for value in values {
        coverage_mut(coverage, &value.category).merge(value);
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

pub(crate) fn load_catalog_manifest(conn: &Connection, manifest_id: &str) -> Result<BodyManifest> {
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
    readers: &mut CatalogFileReaders,
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
    read_chunk_record_at(
        readers.file(&path)?,
        &ChunkRecordDescriptor {
            hash: *hash,
            frame_offset,
            uncompressed_length,
            compressed_length,
        },
        &Limits::default(),
    )
}

struct CatalogFileReader {
    file: File,
    last_used: u64,
}

/// Keeps live grep from accumulating one file descriptor for every rotated
/// pack in a large catalog. Chunk bytes themselves are cached/spilled by
/// RawBodyScanner; reopening an evicted pack does not decompress a chunk twice.
struct CatalogFileReaders {
    entries: HashMap<PathBuf, CatalogFileReader>,
    max_open: usize,
    use_clock: u64,
}

impl CatalogFileReaders {
    fn new(max_open: usize) -> Self {
        Self {
            entries: HashMap::new(),
            max_open: max_open.max(1),
            use_clock: 0,
        }
    }

    fn file(&mut self, path: &Path) -> alex_lar::Result<&mut File> {
        self.use_clock = self.use_clock.saturating_add(1);
        if self.entries.contains_key(path) {
            let entry = self
                .entries
                .get_mut(path)
                .ok_or_else(|| alex_lar::Error::Missing(path.display().to_string()))?;
            entry.last_used = self.use_clock;
            return Ok(&mut entry.file);
        }
        if self.entries.len() >= self.max_open {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(path, _)| path.clone())
                .ok_or(alex_lar::Error::Invalid(
                    "live grep file cache could not select an eviction",
                ))?;
            self.entries.remove(&oldest);
        }
        let file = File::open(path).map_err(alex_lar::Error::Io)?;
        self.entries.insert(
            path.to_path_buf(),
            CatalogFileReader {
                file,
                last_used: self.use_clock,
            },
        );
        Ok(&mut self
            .entries
            .get_mut(path)
            .ok_or_else(|| alex_lar::Error::Missing(path.display().to_string()))?
            .file)
    }
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
