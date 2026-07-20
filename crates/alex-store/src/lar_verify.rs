//! Read-only verification of the live LAR catalog and every published body.
//!
//! Verification deliberately goes through `ArchiveReader`, the same reader
//! used by mixed-mode serving. It never changes migration state or legacy
//! source files.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;

use alex_lar::{ArchiveReader, FileRole, Limits, ManifestId, RecoveryStatus};
use anyhow::Result;
use serde::Serialize;

use crate::Store;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LarMigrationVerificationIssue {
    pub scope: String,
    pub id: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LarMigrationVerificationReport {
    pub report_schema: String,
    pub valid: bool,
    pub files_checked: u64,
    pub manifests_checked: u64,
    pub artifacts_checked: u64,
    pub migrated_items_checked: u64,
    pub bytes_reconstructed: u64,
    pub issues: Vec<LarMigrationVerificationIssue>,
    pub checksum_algorithm: String,
    pub report_checksum: String,
}

#[derive(Serialize)]
struct LarMigrationVerificationChecksumPayload<'a> {
    report_schema: &'a str,
    valid: bool,
    files_checked: u64,
    manifests_checked: u64,
    artifacts_checked: u64,
    migrated_items_checked: u64,
    bytes_reconstructed: u64,
    issues: &'a [LarMigrationVerificationIssue],
}

impl LarMigrationVerificationReport {
    pub const SCHEMA: &'static str = "alex-lar-migration-verification-v1";
    pub const CHECKSUM_ALGORITHM: &'static str = "blake3";

    fn checksum_payload(&self) -> LarMigrationVerificationChecksumPayload<'_> {
        LarMigrationVerificationChecksumPayload {
            report_schema: &self.report_schema,
            valid: self.valid,
            files_checked: self.files_checked,
            manifests_checked: self.manifests_checked,
            artifacts_checked: self.artifacts_checked,
            migrated_items_checked: self.migrated_items_checked,
            bytes_reconstructed: self.bytes_reconstructed,
            issues: &self.issues,
        }
    }

    fn computed_checksum(&self) -> String {
        let canonical = serde_json::to_vec(&self.checksum_payload())
            .expect("migration verification checksum payload is always serializable");
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"alex-lar-migration-verification-v1\0");
        hasher.update(&canonical);
        hasher.finalize().to_hex().to_string()
    }

    fn seal_checksum(&mut self) {
        self.checksum_algorithm = Self::CHECKSUM_ALGORITHM.into();
        self.report_checksum = self.computed_checksum();
    }

    /// Recompute the canonical report checksum after transport or archival.
    /// This is tamper-evidence, not an authenticity signature.
    pub fn checksum_matches(&self) -> bool {
        self.report_schema == Self::SCHEMA
            && self.checksum_algorithm == Self::CHECKSUM_ALGORITHM
            && self.report_checksum == self.computed_checksum()
    }
}

#[derive(Debug)]
struct CatalogFile {
    uuid: String,
    path: String,
    role: String,
    container_major: i64,
    container_minor: i64,
    size_bytes: Option<i64>,
}

#[derive(Debug, Clone)]
struct CatalogManifest {
    id: String,
    total_length: i64,
    hash_algorithm: String,
    whole_body_hash: Vec<u8>,
    file_uuid: Option<String>,
    record_id: Option<String>,
}

#[derive(Debug)]
struct MigratedItem {
    id: String,
    artifact_kind: String,
    destination_manifest_id: Option<String>,
    destination_exchange_id: Option<String>,
    destination_file_uuid: Option<String>,
    metadata_stage_count: i64,
    metadata_header_count: i64,
    source_length: Option<i64>,
    source_hash_algorithm: Option<String>,
    source_hash: Option<Vec<u8>>,
}

#[derive(Default)]
struct HashingSink {
    bytes: u64,
    hasher: blake3::Hasher,
}

impl Write for HashingSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn issue(
    issues: &mut Vec<LarMigrationVerificationIssue>,
    scope: &str,
    id: impl Into<String>,
    kind: &str,
    detail: impl Into<String>,
) {
    issues.push(LarMigrationVerificationIssue {
        scope: scope.into(),
        id: id.into(),
        kind: kind.into(),
        detail: detail.into(),
    });
}

fn role_name(role: FileRole) -> &'static str {
    match role {
        FileRole::BodyPack => "body-pack",
        FileRole::EventLog => "event-log",
        FileRole::Standalone => "standalone",
        FileRole::SearchPack => "search-pack",
        FileRole::Dictionary => "dictionary",
    }
}

fn parse_file_uuid(value: &str) -> Option<[u8; 16]> {
    let compact = value
        .chars()
        .filter(|value| *value != '-')
        .collect::<String>();
    if compact.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn resolve_path(data_dir: &std::path::Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        data_dir.join(path)
    }
}

impl Store {
    /// Verify every active/sealed catalog file, ready manifest, published
    /// artifact pointer, and validated migrated item without mutating state.
    pub fn verify_lar_migration(&self) -> Result<LarMigrationVerificationReport> {
        let (files, manifests, header_block_ids, artifacts, migrated_items) = {
            let conn = self.conn.lock().unwrap();
            let mut file_statement = conn.prepare(
                "SELECT file_uuid, path, role, container_major,
                        container_minor, size_bytes
                   FROM lar_files WHERE state IN ('active','sealed') ORDER BY file_uuid",
            )?;
            let files = file_statement
                .query_map([], |row| {
                    Ok(CatalogFile {
                        uuid: row.get(0)?,
                        path: row.get(1)?,
                        role: row.get(2)?,
                        container_major: row.get(3)?,
                        container_minor: row.get(4)?,
                        size_bytes: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut manifest_statement = conn.prepare(
                "SELECT manifest_id, total_length, hash_algorithm, whole_body_hash,
                        file_uuid, record_id
                   FROM lar_manifests WHERE state='ready' ORDER BY manifest_id",
            )?;
            let manifests = manifest_statement
                .query_map([], |row| {
                    Ok(CatalogManifest {
                        id: row.get(0)?,
                        total_length: row.get(1)?,
                        hash_algorithm: row.get(2)?,
                        whole_body_hash: row.get(3)?,
                        file_uuid: row.get(4)?,
                        record_id: row.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut header_statement =
                conn.prepare("SELECT block_id FROM lar_header_blocks ORDER BY block_id")?;
            let header_block_ids = header_statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;

            let mut artifact_statement = conn.prepare(
                "SELECT owner_kind || ':' || owner_id || ':' || artifact_kind || ':' || stage_id,
                        manifest_id, header_block_id
                   FROM lar_trace_artifacts
                  WHERE validation_state='validated'
                  ORDER BY owner_kind, owner_id, artifact_kind, stage_id",
            )?;
            let artifacts = artifact_statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let mut item_statement = conn.prepare(
                "SELECT item_id, artifact_kind, destination_manifest_id,
                        destination_exchange_id, destination_file_uuid,
                        metadata_stage_count, metadata_header_count, source_length,
                        source_hash_algorithm, source_hash
                   FROM lar_migration_items
                  WHERE state='migrated' AND validation_state='validated'
                  ORDER BY item_id",
            )?;
            let items = item_statement
                .query_map([], |row| {
                    Ok(MigratedItem {
                        id: row.get(0)?,
                        artifact_kind: row.get(1)?,
                        destination_manifest_id: row.get(2)?,
                        destination_exchange_id: row.get(3)?,
                        destination_file_uuid: row.get(4)?,
                        metadata_stage_count: row.get(5)?,
                        metadata_header_count: row.get(6)?,
                        source_length: row.get(7)?,
                        source_hash_algorithm: row.get(8)?,
                        source_hash: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            (files, manifests, header_block_ids, artifacts, items)
        };

        let mut report = LarMigrationVerificationReport {
            report_schema: LarMigrationVerificationReport::SCHEMA.into(),
            valid: true,
            files_checked: 0,
            manifests_checked: 0,
            artifacts_checked: artifacts.len() as u64,
            migrated_items_checked: migrated_items.len() as u64,
            bytes_reconstructed: 0,
            issues: Vec::new(),
            checksum_algorithm: LarMigrationVerificationReport::CHECKSUM_ALGORITHM.into(),
            report_checksum: String::new(),
        };
        let manifests_by_file = manifests.iter().fold(
            HashMap::<&str, Vec<&CatalogManifest>>::new(),
            |mut grouped, manifest| {
                if let Some(file_uuid) = manifest.file_uuid.as_deref() {
                    grouped.entry(file_uuid).or_default().push(manifest);
                }
                grouped
            },
        );
        let manifest_by_id = manifests
            .iter()
            .map(|manifest| (manifest.id.as_str(), manifest))
            .collect::<HashMap<_, _>>();
        let mut reconstructed = HashMap::<String, (u64, Vec<u8>)>::new();
        let mut exchange_records = HashMap::<(String, String), (usize, usize, bool)>::new();
        let mut known_files = HashSet::new();

        for catalog_file in files {
            report.files_checked += 1;
            known_files.insert(catalog_file.uuid.clone());
            let path = resolve_path(&self.data_dir, &catalog_file.path);
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    issue(
                        &mut report.issues,
                        "file",
                        &catalog_file.uuid,
                        "unreadable",
                        format!("{}: {error}", path.display()),
                    );
                    continue;
                }
            };
            if let Some(size) = catalog_file.size_bytes {
                if size < 0 || size as u64 != metadata.len() {
                    issue(
                        &mut report.issues,
                        "file",
                        &catalog_file.uuid,
                        "size_mismatch",
                        format!(
                            "catalog size {size}, physical size {} ({})",
                            metadata.len(),
                            path.display()
                        ),
                    );
                }
            }
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(error) => {
                    issue(
                        &mut report.issues,
                        "file",
                        &catalog_file.uuid,
                        "unreadable",
                        format!("{}: {error}", path.display()),
                    );
                    continue;
                }
            };
            let mut reader = match ArchiveReader::open(file, Limits::default()) {
                Ok(reader) => reader,
                Err(error) => {
                    issue(
                        &mut report.issues,
                        "file",
                        &catalog_file.uuid,
                        "invalid_archive",
                        error.to_string(),
                    );
                    continue;
                }
            };
            if reader.recovery_status() != RecoveryStatus::Clean {
                issue(
                    &mut report.issues,
                    "file",
                    &catalog_file.uuid,
                    "truncated_tail",
                    format!("{:?}", reader.recovery_status()),
                );
            }
            let header = reader.header();
            match parse_file_uuid(&catalog_file.uuid) {
                Some(expected) if expected != header.file_uuid => issue(
                    &mut report.issues,
                    "file",
                    &catalog_file.uuid,
                    "uuid_mismatch",
                    "catalog UUID does not match archive header",
                ),
                None => issue(
                    &mut report.issues,
                    "file",
                    &catalog_file.uuid,
                    "invalid_catalog_uuid",
                    "file UUID must be an RFC 4122 or compact hexadecimal identifier",
                ),
                _ => {}
            }
            if catalog_file.role != role_name(header.file_role) {
                issue(
                    &mut report.issues,
                    "file",
                    &catalog_file.uuid,
                    "role_mismatch",
                    format!(
                        "catalog role {}, archive role {}",
                        catalog_file.role,
                        role_name(header.file_role)
                    ),
                );
            }
            if catalog_file.container_major != i64::from(header.container_major)
                || catalog_file.container_minor != i64::from(header.container_minor)
            {
                issue(
                    &mut report.issues,
                    "file",
                    &catalog_file.uuid,
                    "version_mismatch",
                    format!(
                        "catalog {}.{}, archive {}.{}",
                        catalog_file.container_major,
                        catalog_file.container_minor,
                        header.container_major,
                        header.container_minor
                    ),
                );
            }

            for catalog_manifest in manifests_by_file
                .get(catalog_file.uuid.as_str())
                .into_iter()
                .flatten()
            {
                report.manifests_checked += 1;
                let id = match ManifestId::from_str(&catalog_manifest.id) {
                    Ok(id) => id,
                    Err(error) => {
                        issue(
                            &mut report.issues,
                            "manifest",
                            &catalog_manifest.id,
                            "invalid_id",
                            error.to_string(),
                        );
                        continue;
                    }
                };
                if catalog_manifest.record_id.as_deref() != Some(catalog_manifest.id.as_str()) {
                    issue(
                        &mut report.issues,
                        "manifest",
                        &catalog_manifest.id,
                        "record_id_mismatch",
                        "catalog record_id does not match manifest_id",
                    );
                }
                let Some(stored) = reader.manifest(&id).cloned() else {
                    issue(
                        &mut report.issues,
                        "manifest",
                        &catalog_manifest.id,
                        "missing_record",
                        format!("not found in file {}", catalog_file.uuid),
                    );
                    continue;
                };
                if catalog_manifest.total_length < 0
                    || catalog_manifest.total_length as u64 != stored.total_length
                    || catalog_manifest.hash_algorithm != "blake3"
                    || catalog_manifest.whole_body_hash.as_slice()
                        != stored.whole_body_hash.digest.as_slice()
                {
                    issue(
                        &mut report.issues,
                        "manifest",
                        &catalog_manifest.id,
                        "metadata_mismatch",
                        "catalog length or hash differs from the archive manifest",
                    );
                }
                let mut sink = HashingSink::default();
                match reader.write_body(&id, &mut sink) {
                    Ok(written) => {
                        let hash = sink.hasher.finalize().as_bytes().to_vec();
                        report.bytes_reconstructed =
                            report.bytes_reconstructed.saturating_add(written);
                        reconstructed.insert(catalog_manifest.id.clone(), (written, hash));
                    }
                    Err(error) => issue(
                        &mut report.issues,
                        "manifest",
                        &catalog_manifest.id,
                        "reconstruction_failed",
                        error.to_string(),
                    ),
                }
            }
            for exchange_id in reader.exchange_ids() {
                let Some(exchange) = reader.exchange(exchange_id) else {
                    continue;
                };
                let mut header_refs = 0usize;
                for stage_id in &exchange.data.stages {
                    if let Some(stage) = reader.stage(stage_id) {
                        header_refs = header_refs
                            .saturating_add(usize::from(stage.data.request_headers_ref.is_some()))
                            .saturating_add(usize::from(stage.data.response_headers_ref.is_some()))
                            .saturating_add(usize::from(stage.data.trailers_ref.is_some()));
                    }
                }
                exchange_records.insert(
                    (catalog_file.uuid.clone(), exchange_id.to_string()),
                    (
                        exchange.data.stages.len(),
                        header_refs,
                        reader.exchange_metadata(exchange_id).is_some(),
                    ),
                );
            }
        }

        for manifest in &manifests {
            let Some(file_uuid) = manifest.file_uuid.as_deref() else {
                issue(
                    &mut report.issues,
                    "manifest",
                    &manifest.id,
                    "missing_file_pointer",
                    "ready manifest has no file_uuid",
                );
                continue;
            };
            if !known_files.contains(file_uuid) {
                issue(
                    &mut report.issues,
                    "manifest",
                    &manifest.id,
                    "unavailable_file",
                    format!("file {file_uuid} is absent or not active/sealed"),
                );
            }
        }

        for (artifact_id, manifest_id, header_block_id) in artifacts {
            if let Some(ref manifest_id) = manifest_id {
                if !manifest_by_id.contains_key(manifest_id.as_str()) {
                    issue(
                        &mut report.issues,
                        "artifact",
                        &artifact_id,
                        "invalid_manifest_pointer",
                        format!("ready manifest {manifest_id} is missing"),
                    );
                } else if !reconstructed.contains_key(manifest_id) {
                    issue(
                        &mut report.issues,
                        "artifact",
                        &artifact_id,
                        "unverified_manifest_pointer",
                        format!("manifest {manifest_id} could not be reconstructed"),
                    );
                }
            }
            if let Some(header_block_id) = header_block_id {
                if !header_block_ids.contains(&header_block_id) {
                    issue(
                        &mut report.issues,
                        "artifact",
                        &artifact_id,
                        "invalid_header_pointer",
                        format!("header block {header_block_id} is missing"),
                    );
                }
            } else if manifest_id.is_none() {
                issue(
                    &mut report.issues,
                    "artifact",
                    &artifact_id,
                    "empty_pointer",
                    "artifact points to neither a manifest nor a header block",
                );
            }
        }

        for item in migrated_items {
            if item.artifact_kind == "exchange_metadata" {
                let Some(exchange_id) = item.destination_exchange_id.as_deref() else {
                    issue(
                        &mut report.issues,
                        "migration_item",
                        &item.id,
                        "missing_exchange_destination",
                        "validated metadata item has no destination exchange",
                    );
                    continue;
                };
                let Some(file_uuid) = item.destination_file_uuid.as_deref() else {
                    issue(
                        &mut report.issues,
                        "migration_item",
                        &item.id,
                        "missing_exchange_file",
                        "validated metadata item has no destination file",
                    );
                    continue;
                };
                let Some((stage_count, header_count, has_metadata)) =
                    exchange_records.get(&(file_uuid.to_string(), exchange_id.to_string()))
                else {
                    issue(
                        &mut report.issues,
                        "migration_item",
                        &item.id,
                        "unverified_exchange_destination",
                        format!("exchange {exchange_id} was not found in file {file_uuid}"),
                    );
                    continue;
                };
                if usize::try_from(item.metadata_stage_count).ok() != Some(*stage_count)
                    || usize::try_from(item.metadata_header_count).ok() != Some(*header_count)
                {
                    issue(
                        &mut report.issues,
                        "migration_item",
                        &item.id,
                        "exchange_metadata_mismatch",
                        "catalog stage/header counts differ from the archived exchange",
                    );
                }
                if !has_metadata {
                    issue(
                        &mut report.issues,
                        "migration_item",
                        &item.id,
                        "missing_exchange_metadata",
                        "legacy metadata receipt points to an exchange without its companion record",
                    );
                }
                continue;
            }
            let Some(manifest_id) = item.destination_manifest_id.as_deref() else {
                issue(
                    &mut report.issues,
                    "migration_item",
                    &item.id,
                    "missing_destination",
                    "validated migrated item has no destination manifest",
                );
                continue;
            };
            let Some((length, hash)) = reconstructed.get(manifest_id) else {
                issue(
                    &mut report.issues,
                    "migration_item",
                    &item.id,
                    "unverified_destination",
                    format!("destination manifest {manifest_id} was not reconstructed"),
                );
                continue;
            };
            if item
                .source_length
                .and_then(|value| u64::try_from(value).ok())
                != Some(*length)
                || item.source_hash_algorithm.as_deref() != Some("blake3")
                || item.source_hash.as_deref() != Some(hash.as_slice())
            {
                issue(
                    &mut report.issues,
                    "migration_item",
                    &item.id,
                    "source_hash_mismatch",
                    "reconstructed body differs from the source length or hash recorded at migration",
                );
            }
        }

        report.valid = report.issues.is_empty();
        report.seal_checksum();
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::LarMigrationVerificationReport;
    use crate::{LarLegacyImportOptions, Store};
    use alex_core::TraceRecord;

    fn temp_store(name: &str) -> Store {
        let root = std::env::temp_dir().join(format!(
            "alex-store-lar-verify-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Store::open(root).unwrap()
    }

    #[test]
    fn imported_artifact_verifies_and_tampering_is_reported_without_mutation() {
        let store = temp_store("roundtrip");
        let mut trace = TraceRecord {
            id: "trace-verify".into(),
            ts_request_ms: 1,
            ..TraceRecord::default()
        };
        trace.req_body_path = Some(
            store
                .write_body("trace-verify", "request", br#"{"message":"hello"}"#)
                .unwrap(),
        );
        store.insert_trace(&trace).unwrap();
        let imported = store
            .run_lar_legacy_import(&LarLegacyImportOptions::default())
            .unwrap();
        assert_eq!(imported.migrated, 1);

        let verified = store.verify_lar_migration().unwrap();
        assert!(verified.valid, "{:?}", verified.issues);
        assert_eq!(verified.files_checked, 1);
        assert_eq!(verified.manifests_checked, 1);
        assert_eq!(verified.artifacts_checked, 1);
        assert_eq!(verified.migrated_items_checked, 2);
        assert!(verified.checksum_matches());
        let serialized = serde_json::to_value(&verified).unwrap();
        assert_eq!(
            serialized["report_schema"],
            LarMigrationVerificationReport::SCHEMA
        );
        assert_eq!(serialized["checksum_algorithm"], "blake3");
        assert_eq!(serialized["report_checksum"].as_str().unwrap().len(), 64);

        let mut tampered_report = verified.clone();
        tampered_report.bytes_reconstructed += 1;
        assert!(!tampered_report.checksum_matches());

        let conn = store.conn.lock().unwrap();
        conn.execute("UPDATE lar_manifests SET whole_body_hash=zeroblob(32)", [])
            .unwrap();
        drop(conn);
        let invalid = store.verify_lar_migration().unwrap();
        assert!(!invalid.valid);
        assert!(invalid.checksum_matches());
        assert!(invalid
            .issues
            .iter()
            .any(|value| value.kind == "metadata_mismatch"));
        assert!(store
            .read_lar_or_legacy_artifact("trace", "trace-verify", "client_request", None)
            .unwrap()
            .is_some());
    }

    #[test]
    fn missing_archive_is_an_issue_instead_of_a_destructive_repair() {
        let store = temp_store("missing");
        let mut trace = TraceRecord {
            id: "trace-missing".into(),
            ts_request_ms: 1,
            ..TraceRecord::default()
        };
        trace.req_body_path = Some(
            store
                .write_body("trace-missing", "request", b"body")
                .unwrap(),
        );
        store.insert_trace(&trace).unwrap();
        let imported = store
            .run_lar_legacy_import(&LarLegacyImportOptions::default())
            .unwrap();
        std::fs::remove_file(&imported.file_path).unwrap();
        let report = store.verify_lar_migration().unwrap();
        assert!(!report.valid);
        assert!(report.checksum_matches());
        assert!(report.issues.iter().any(|value| value.kind == "unreadable"));
        assert!(trace
            .req_body_path
            .as_deref()
            .is_some_and(|path| std::path::Path::new(path).exists()));
    }
}
