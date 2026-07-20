//! Stable file identity, availability, detach, and validated reattach.
//!
//! Catalog paths may change; file UUID plus a validated whole-file digest may
//! not. Detach never moves or deletes bytes. Reattach validates the candidate
//! as a clean sealed LAR file and compares its UUID, role, header, length, and
//! digest before one transaction changes the catalog path and availability.

use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use alex_lar::{ArchiveReader, FileRole, Limits, RecoveryStatus};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::Store;

const IDENTITY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS lar_file_identities (
  file_uuid        TEXT PRIMARY KEY,
  hash_algorithm   TEXT NOT NULL CHECK (hash_algorithm = 'blake3'),
  file_hash        BLOB NOT NULL,
  size_bytes       INTEGER NOT NULL CHECK (size_bytes >= 0),
  source           TEXT NOT NULL,
  validated_at_ms  INTEGER NOT NULL,
  FOREIGN KEY (file_uuid) REFERENCES lar_files(file_uuid)
);
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LarArchiveAvailability {
    Online,
    ArchivedOffline,
    ArchivedMissing,
    Retired,
}

impl LarArchiveAvailability {
    pub fn code(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::ArchivedOffline => "archived_offline",
            Self::ArchivedMissing => "archived_missing",
            Self::Retired => "retired",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LarArchiveFileStatus {
    pub file_uuid: String,
    pub archive_set_uuid: String,
    pub role: String,
    pub catalog_path: String,
    pub resolved_path: String,
    pub catalog_state: String,
    pub availability: LarArchiveAvailability,
    pub exists: bool,
    pub identity_validated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LarArchiveDetachReport {
    pub already_offline: bool,
    pub file: LarArchiveFileStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct LarArchiveReattachReport {
    pub file_uuid: String,
    pub catalog_path: String,
    pub source_size: u64,
    pub source_blake3: String,
    pub already_attached: bool,
    pub relocated: bool,
    pub file: LarArchiveFileStatus,
}

#[derive(Clone, Debug)]
pub struct LarArchiveReattachOptions {
    pub limits: Limits,
}

impl Default for LarArchiveReattachOptions {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LarArchiveUnavailableError {
    pub availability: LarArchiveAvailability,
    pub file_uuid: String,
    pub path: String,
}

impl LarArchiveUnavailableError {
    pub fn code(&self) -> &'static str {
        self.availability.code()
    }

    pub(crate) fn offline(file_uuid: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            availability: LarArchiveAvailability::ArchivedOffline,
            file_uuid: file_uuid.into(),
            path: path.into(),
        }
    }

    pub(crate) fn missing(file_uuid: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            availability: LarArchiveAvailability::ArchivedMissing,
            file_uuid: file_uuid.into(),
            path: path.into(),
        }
    }
}

impl fmt::Display for LarArchiveUnavailableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.availability {
            LarArchiveAvailability::ArchivedOffline => write!(
                formatter,
                "LAR archive {} is archived offline at {}; reattach it to restore body reads",
                self.file_uuid, self.path
            ),
            LarArchiveAvailability::ArchivedMissing => write!(
                formatter,
                "LAR archive {} is missing from {}; locate and reattach it to restore body reads",
                self.file_uuid, self.path
            ),
            other => write!(
                formatter,
                "LAR archive {} is unavailable ({}) at {}",
                self.file_uuid,
                other.code(),
                self.path
            ),
        }
    }
}

impl std::error::Error for LarArchiveUnavailableError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LarFileIdentity {
    pub size: u64,
    pub blake3: [u8; 32],
}

#[derive(Clone)]
struct CatalogFile {
    file_uuid: String,
    archive_set_uuid: String,
    role: String,
    path: String,
    state: String,
    container_major: u16,
    container_minor: u16,
    required_feature_bits: u64,
    optional_feature_bits: u64,
}

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(IDENTITY_SCHEMA)?;
    ensure_column(conn, "lar_checkpoints", "frame_offset", "INTEGER")?;
    ensure_column(conn, "lar_checkpoints", "frame_length", "INTEGER")?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|value| value == column) {
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
    }
    Ok(())
}

pub(crate) fn resolved_catalog_path(data_dir: &Path, catalog_path: &str) -> PathBuf {
    let path = PathBuf::from(catalog_path);
    if path.is_absolute() {
        path
    } else {
        data_dir.join(path)
    }
}

pub(crate) fn compute_lar_file_identity(path: &Path) -> Result<LarFileIdentity> {
    let mut file =
        File::open(path).with_context(|| format!("opening LAR file {}", path.display()))?;
    hash_file(&mut file)
}

pub(crate) fn record_lar_file_identity(
    conn: &Connection,
    file_uuid: &str,
    identity: &LarFileIdentity,
    source: &str,
    validated_at_ms: i64,
) -> Result<()> {
    let size = i64::try_from(identity.size).context("LAR file size exceeds SQLite range")?;
    conn.execute(
        "INSERT INTO lar_file_identities
           (file_uuid, hash_algorithm, file_hash, size_bytes, source, validated_at_ms)
         VALUES (?1, 'blake3', ?2, ?3, ?4, ?5)
         ON CONFLICT(file_uuid) DO NOTHING",
        params![
            file_uuid,
            identity.blake3.as_slice(),
            size,
            source,
            validated_at_ms,
        ],
    )?;
    let stored: (Vec<u8>, i64) = conn.query_row(
        "SELECT file_hash, size_bytes FROM lar_file_identities WHERE file_uuid=?1",
        [file_uuid],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if stored.0.as_slice() != identity.blake3 || stored.1 != size {
        bail!("LAR file UUID is already bound to different immutable archive bytes");
    }
    Ok(())
}

impl Store {
    pub fn lar_archive_file_status(&self, file_uuid: &str) -> Result<Option<LarArchiveFileStatus>> {
        let row: Option<(String, String, String, String, bool)> = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT f.archive_set_uuid, f.role, f.path, f.state,
                        i.file_uuid IS NOT NULL
                 FROM lar_files f LEFT JOIN lar_file_identities i USING(file_uuid)
                 WHERE f.file_uuid=?1",
                [file_uuid],
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
            .optional()?
        };
        row.map(
            |(archive_set_uuid, role, path, state, identity_validated)| {
                archive_status(
                    &self.data_dir,
                    file_uuid.to_string(),
                    archive_set_uuid,
                    role,
                    path,
                    state,
                    identity_validated,
                )
            },
        )
        .transpose()
    }

    pub fn lar_archive_file_statuses(&self) -> Result<Vec<LarArchiveFileStatus>> {
        let rows: Vec<(String, String, String, String, String, bool)> = {
            let conn = self.conn.lock().unwrap();
            let mut statement = conn.prepare(
                "SELECT f.file_uuid, f.archive_set_uuid, f.role, f.path, f.state,
                        i.file_uuid IS NOT NULL
                 FROM lar_files f LEFT JOIN lar_file_identities i USING(file_uuid)
                 ORDER BY f.created_at_ms, f.file_uuid",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        rows.into_iter()
            .map(
                |(file_uuid, archive_set_uuid, role, path, state, identity_validated)| {
                    archive_status(
                        &self.data_dir,
                        file_uuid,
                        archive_set_uuid,
                        role,
                        path,
                        state,
                        identity_validated,
                    )
                },
            )
            .collect()
    }

    /// Mark an immutable sealed archive offline without moving or deleting it.
    /// A legacy sealed row without a digest is upgraded only from its current
    /// online catalog path. A replacement can never establish expected bytes.
    pub fn detach_lar_archive(&self, file_uuid: &str) -> Result<LarArchiveDetachReport> {
        let initial = load_catalog_file(self, file_uuid)?;
        if initial.state == "sealed" {
            self.ensure_online_file_identity(&initial, &Limits::default())?;
        }
        let already_offline;
        {
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (archive_set_uuid, state): (String, String) = tx
                .query_row(
                    "SELECT archive_set_uuid, state FROM lar_files WHERE file_uuid=?1",
                    [file_uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .with_context(|| format!("locating LAR archive file {file_uuid}"))?;
            already_offline = state == "offline";
            match state.as_str() {
                "sealed" => {
                    let has_identity: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM lar_file_identities WHERE file_uuid=?1)",
                        [file_uuid],
                        |row| row.get(0),
                    )?;
                    if !has_identity {
                        bail!("sealed LAR archive has no validated whole-file identity");
                    }
                    let now = chrono::Utc::now().timestamp_millis();
                    tx.execute(
                        "UPDATE lar_files SET state='offline' WHERE file_uuid=?1",
                        [file_uuid],
                    )?;
                    tx.execute(
                        "UPDATE lar_archive_sets SET
                           state=CASE WHEN NOT EXISTS (
                             SELECT 1 FROM lar_files
                             WHERE archive_set_uuid=?1 AND state IN ('active','sealed','repairing')
                           ) THEN 'offline' ELSE state END,
                           updated_at_ms=?2, catalog_revision=catalog_revision+1
                         WHERE archive_set_uuid=?1",
                        params![archive_set_uuid, now],
                    )?;
                }
                "offline" => {}
                "active" => bail!(
                    "LAR archive {file_uuid} is an active writer; seal or rotate it before detach"
                ),
                "repairing" => {
                    bail!("LAR archive {file_uuid} is being repaired and cannot be detached")
                }
                "retired" => bail!("LAR archive {file_uuid} is retired and cannot be detached"),
                other => bail!("LAR archive {file_uuid} has unsupported catalog state {other}"),
            }
            tx.commit()?;
        }
        let file = self
            .lar_archive_file_status(file_uuid)?
            .context("detached LAR archive disappeared from the catalog")?;
        Ok(LarArchiveDetachReport {
            already_offline,
            file,
        })
    }

    /// Validate and attach an immutable sealed LAR file at a new location.
    pub fn reattach_lar_archive(
        &self,
        file_uuid: &str,
        path: impl AsRef<Path>,
        options: &LarArchiveReattachOptions,
    ) -> Result<LarArchiveReattachReport> {
        let catalog = load_catalog_file(self, file_uuid)?;
        if !matches!(catalog.state.as_str(), "offline" | "sealed") {
            bail!(
                "LAR archive {file_uuid} cannot be reattached from catalog state {}",
                catalog.state
            );
        }
        let expected = self.ensure_online_file_identity(&catalog, &options.limits)?;
        let candidate_path = path.as_ref().canonicalize().with_context(|| {
            format!(
                "resolving LAR reattach candidate {}",
                path.as_ref().display()
            )
        })?;
        let actual = validate_sealed_candidate(&candidate_path, &catalog, &options.limits)?;
        if actual != expected {
            bail!("reattach candidate differs from the cataloged immutable LAR file identity");
        }
        let catalog_path = safe_catalog_path(&self.data_dir, &candidate_path)?;
        let previous_path = catalog.path.clone();
        {
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: (String, String, String) = tx.query_row(
                "SELECT archive_set_uuid, path, state FROM lar_files WHERE file_uuid=?1",
                [file_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if current.0 != catalog.archive_set_uuid
                || current.1 != catalog.path
                || current.2 != catalog.state
            {
                bail!("LAR catalog changed while reattach candidate was being validated");
            }
            let stored = load_identity(&tx, file_uuid)?
                .context("validated LAR file identity disappeared before reattach")?;
            if stored != expected {
                bail!("LAR file identity changed while reattach candidate was being validated");
            }
            let occupied: Option<String> = tx
                .query_row(
                    "SELECT file_uuid FROM lar_files WHERE path=?1 AND file_uuid!=?2 LIMIT 1",
                    params![catalog_path, file_uuid],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(occupied) = occupied {
                bail!("archive path is already attached as LAR file {occupied}");
            }
            if current.1 != catalog_path || current.2 != "sealed" {
                let now = chrono::Utc::now().timestamp_millis();
                tx.execute(
                    "UPDATE lar_files SET path=?2, state='sealed', size_bytes=?3
                     WHERE file_uuid=?1",
                    params![
                        file_uuid,
                        catalog_path,
                        i64::try_from(actual.size).context("LAR file size exceeds SQLite range")?,
                    ],
                )?;
                tx.execute(
                    "UPDATE lar_archive_sets SET
                       state=CASE WHEN EXISTS (
                         SELECT 1 FROM lar_files
                         WHERE archive_set_uuid=?1 AND state='active'
                       ) THEN 'active' ELSE 'sealed' END,
                       updated_at_ms=?2, catalog_revision=catalog_revision+1
                     WHERE archive_set_uuid=?1",
                    params![catalog.archive_set_uuid, now],
                )?;
            }
            tx.commit()?;
        }
        let file = self
            .lar_archive_file_status(file_uuid)?
            .context("reattached LAR archive disappeared from the catalog")?;
        Ok(LarArchiveReattachReport {
            file_uuid: file_uuid.to_string(),
            catalog_path: file.catalog_path.clone(),
            source_size: actual.size,
            source_blake3: hex(&actual.blake3),
            already_attached: previous_path == file.catalog_path,
            relocated: previous_path != file.catalog_path,
            file,
        })
    }

    fn ensure_online_file_identity(
        &self,
        catalog: &CatalogFile,
        limits: &Limits,
    ) -> Result<LarFileIdentity> {
        {
            let conn = self.conn.lock().unwrap();
            if let Some(identity) = load_identity(&conn, &catalog.file_uuid)? {
                return Ok(identity);
            }
        }
        if catalog.state != "sealed" {
            bail!(
                "LAR archive {} has no prior whole-file identity; restore its original online path before reattach",
                catalog.file_uuid
            );
        }
        let original_path = resolved_catalog_path(&self.data_dir, &catalog.path);
        if !original_path.is_file() {
            bail!(
                "cannot establish LAR file identity because the original catalog path is missing: {}",
                original_path.display()
            );
        }
        let identity = validate_sealed_candidate(&original_path, catalog, limits)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: (String, String) = tx.query_row(
            "SELECT path, state FROM lar_files WHERE file_uuid=?1",
            [&catalog.file_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if current != (catalog.path.clone(), "sealed".to_string()) {
            bail!("LAR catalog changed while its original file identity was being established");
        }
        record_lar_file_identity(
            &tx,
            &catalog.file_uuid,
            &identity,
            "online_upgrade",
            chrono::Utc::now().timestamp_millis(),
        )?;
        tx.commit()?;
        Ok(identity)
    }
}

fn load_catalog_file(store: &Store, file_uuid: &str) -> Result<CatalogFile> {
    let conn = store.conn.lock().unwrap();
    conn.query_row(
        "SELECT file_uuid, archive_set_uuid, role, path, state, container_major,
                container_minor, required_feature_bits, optional_feature_bits
         FROM lar_files WHERE file_uuid=?1",
        [file_uuid],
        |row| {
            Ok(CatalogFile {
                file_uuid: row.get(0)?,
                archive_set_uuid: row.get(1)?,
                role: row.get(2)?,
                path: row.get(3)?,
                state: row.get(4)?,
                container_major: row.get(5)?,
                container_minor: row.get(6)?,
                required_feature_bits: row.get(7)?,
                optional_feature_bits: row.get(8)?,
            })
        },
    )
    .with_context(|| format!("locating LAR archive file {file_uuid}"))
}

fn load_identity(conn: &Connection, file_uuid: &str) -> Result<Option<LarFileIdentity>> {
    conn.query_row(
        "SELECT size_bytes, file_hash FROM lar_file_identities
         WHERE file_uuid=?1 AND hash_algorithm='blake3'",
        [file_uuid],
        |row| {
            let size: i64 = row.get(0)?;
            let hash: Vec<u8> = row.get(1)?;
            let blake3: [u8; 32] = hash.try_into().map_err(|value: Vec<u8>| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Blob,
                    format!("expected 32-byte BLAKE3 hash, got {}", value.len()).into(),
                )
            })?;
            let size = u64::try_from(size).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
            Ok(LarFileIdentity { size, blake3 })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn validate_sealed_candidate(
    path: &Path,
    catalog: &CatalogFile,
    limits: &Limits,
) -> Result<LarFileIdentity> {
    let mut file =
        File::open(path).with_context(|| format!("opening LAR file {}", path.display()))?;
    let before = hash_file(&mut file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut reader = ArchiveReader::open(&mut file, limits.clone())
        .map_err(anyhow::Error::new)
        .with_context(|| format!("opening sealed LAR candidate {}", path.display()))?;
    if !reader.is_sealed() || reader.recovery_status() != RecoveryStatus::Clean {
        bail!("LAR reattach candidate is not a clean sealed archive");
    }
    let header = reader.header();
    if hex(&header.file_uuid) != catalog.file_uuid
        || role_name(header.file_role) != catalog.role
        || header.container_major != catalog.container_major
        || header.container_minor != catalog.container_minor
        || header.required_feature_bits != catalog.required_feature_bits
        || header.optional_feature_bits != catalog.optional_feature_bits
    {
        bail!("LAR reattach candidate header does not match its cataloged file identity");
    }
    let descriptors: Vec<_> = reader.chunk_records().collect();
    let local: HashSet<_> = descriptors.iter().map(|value| value.hash).collect();
    for descriptor in &descriptors {
        reader
            .read_chunk(&descriptor.hash)
            .map_err(anyhow::Error::new)
            .context("validating sealed LAR candidate chunk")?;
    }
    let local_manifests: Vec<_> = reader
        .manifest_ids()
        .filter(|id| {
            reader.manifest(id).is_some_and(|manifest| {
                manifest
                    .chunks
                    .iter()
                    .all(|reference| local.contains(&reference.chunk_hash))
            })
        })
        .copied()
        .collect();
    for manifest in local_manifests {
        reader
            .read_body(&manifest)
            .map_err(anyhow::Error::new)
            .context("validating sealed LAR candidate body")?;
    }
    drop(reader);
    file.seek(SeekFrom::Start(0))?;
    let after = hash_file(&mut file)?;
    if after != before {
        bail!("LAR candidate changed while it was being validated");
    }
    let mut current = File::open(path)?;
    let current = hash_file(&mut current)?;
    if current != before {
        bail!("LAR candidate path changed while it was being validated");
    }
    Ok(before)
}

fn hash_file(file: &mut File) -> Result<LarFileIdentity> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .context("LAR file size overflow")?;
    }
    Ok(LarFileIdentity {
        size,
        blake3: *hasher.finalize().as_bytes(),
    })
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
        .context("LAR archive path is not valid UTF-8")
}

fn archive_status(
    data_dir: &Path,
    file_uuid: String,
    archive_set_uuid: String,
    role: String,
    catalog_path: String,
    catalog_state: String,
    identity_validated: bool,
) -> Result<LarArchiveFileStatus> {
    let resolved = resolved_catalog_path(data_dir, &catalog_path);
    let exists = resolved
        .try_exists()
        .with_context(|| format!("checking LAR archive at {}", resolved.display()))?;
    let availability = match catalog_state.as_str() {
        "offline" => LarArchiveAvailability::ArchivedOffline,
        "retired" => LarArchiveAvailability::Retired,
        _ if !exists => LarArchiveAvailability::ArchivedMissing,
        _ => LarArchiveAvailability::Online,
    };
    Ok(LarArchiveFileStatus {
        file_uuid,
        archive_set_uuid,
        role,
        catalog_path,
        resolved_path: resolved.to_string_lossy().into_owned(),
        catalog_state,
        availability,
        exists,
        identity_validated,
    })
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}
