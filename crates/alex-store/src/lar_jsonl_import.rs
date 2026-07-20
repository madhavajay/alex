//! Streaming import for Alex's versioned JSONL interchange export.
//!
//! A record is validated completely before writes begin. Body bytes use the
//! ordinary live LAR writer and are reconstructed before the trace metadata is
//! published. Consequently an interruption can leave reusable body chunks but
//! never a visible, partially populated trace row.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::{BufRead, BufReader, Read};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    sqlite_row_json, LarBodyArtifact, LarBodyStoreMode, Store, TraceBackupRows, BACKUP_TRACE_COLS,
};

const BODY_PATH_COLUMNS: [&str; 3] = ["req_body_path", "upstream_req_body_path", "resp_body_path"];

/// Hard safety limits for untrusted JSONL input. The importer is line-oriented,
/// so peak input memory is bounded by `max_line_bytes` plus decoded bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LarJsonlImportOptions {
    pub max_input_bytes: u64,
    pub max_line_bytes: usize,
    pub max_records: u64,
    pub max_body_bytes: usize,
    pub max_metadata_bytes: usize,
    pub max_header_count: usize,
    pub max_header_bytes: usize,
    pub max_loss_entries: usize,
}

impl Default for LarJsonlImportOptions {
    fn default() -> Self {
        Self {
            max_input_bytes: 4 * 1024 * 1024 * 1024 * 1024,
            max_line_bytes: 768 * 1024 * 1024,
            max_records: 1_000_000,
            max_body_bytes: 256 * 1024 * 1024,
            max_metadata_bytes: 2 * 1024 * 1024,
            max_header_count: 65_536,
            max_header_bytes: 8 * 1024 * 1024,
            max_loss_entries: 1_024,
        }
    }
}

impl LarJsonlImportOptions {
    fn validate(&self) -> Result<()> {
        if self.max_input_bytes == 0
            || self.max_line_bytes == 0
            || self.max_records == 0
            || self.max_body_bytes == 0
            || self.max_metadata_bytes == 0
            || self.max_header_count == 0
            || self.max_header_bytes == 0
            || self.max_loss_entries == 0
        {
            bail!("JSONL import limits must all be positive");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct LarJsonlImportReport {
    pub format_version: u64,
    pub input_bytes: u64,
    pub records_seen: u64,
    pub traces_imported: u64,
    pub traces_skipped: u64,
    pub bodies_written: u64,
    pub decoded_body_bytes: u64,
    pub source_loss_report: Vec<String>,
    pub header_fidelity_counts: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestLine {
    #[serde(rename = "type")]
    record_type: String,
    version: u64,
    format: String,
    loss_report: Vec<String>,
    #[serde(default)]
    record_schema: Option<String>,
    #[serde(default)]
    canonical_traces: Option<u64>,
    #[serde(default)]
    legacy_traces: Option<u64>,
    #[serde(default)]
    body_part_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceLine {
    #[serde(rename = "type")]
    record_type: String,
    metadata: Value,
    headers: JsonlHeaders,
    artifacts: JsonlArtifacts,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct JsonlHeader {
    name: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonlHeaders {
    request: Vec<JsonlHeader>,
    response: Vec<JsonlHeader>,
    fidelity: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonlArtifacts {
    client_request: Option<JsonlEncodedArtifact>,
    upstream_request: Option<JsonlEncodedArtifact>,
    client_response: Option<JsonlEncodedArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonlEncodedArtifact {
    encoding: String,
    length: u64,
    blake3: String,
    data: String,
}

struct ValidatedTrace {
    trace_id: String,
    metadata: Value,
    headers: JsonlHeaders,
    client_request: Option<Vec<u8>>,
    upstream_request: Option<Vec<u8>>,
    client_response: Option<Vec<u8>>,
}

impl Store {
    /// Import Alex JSONL from any reader. Completed trace records are durable
    /// independently, so rerunning the same stream skips exact prior records
    /// and safely continues after the last successfully published line.
    pub fn import_lar_jsonl<R: Read>(
        &self,
        input: R,
        options: &LarJsonlImportOptions,
    ) -> Result<LarJsonlImportReport> {
        options.validate()?;
        if self.lar_body_store_mode() != LarBodyStoreMode::LarWithFallback {
            bail!("JSONL import requires the lar-with-fallback body-store mode");
        }
        let mut input = BufReader::new(input);
        let mut input_bytes = 0u64;
        let manifest_bytes = read_limited_line(
            &mut input,
            options.max_line_bytes,
            options.max_input_bytes,
            &mut input_bytes,
        )?
        .context("JSONL import is empty; expected an Alex export manifest")?;
        let manifest: ManifestLine = serde_json::from_slice(&manifest_bytes)
            .context("line 1 is not a valid Alex JSONL export manifest")?;
        validate_manifest(&manifest, options)?;
        let mut report = LarJsonlImportReport {
            format_version: manifest.version,
            input_bytes: 0,
            records_seen: 0,
            traces_imported: 0,
            traces_skipped: 0,
            bodies_written: 0,
            decoded_body_bytes: 0,
            source_loss_report: manifest.loss_report,
            header_fidelity_counts: BTreeMap::new(),
        };
        let mut trace_ids = HashSet::new();
        let mut line_number = 1u64;
        while let Some(line) = read_limited_line(
            &mut input,
            options.max_line_bytes,
            options.max_input_bytes,
            &mut input_bytes,
        )? {
            line_number += 1;
            if line.is_empty() {
                bail!("JSONL line {line_number} is empty");
            }
            if report.records_seen >= options.max_records {
                bail!("JSONL import exceeds the configured record limit");
            }
            let parsed: TraceLine = serde_json::from_slice(&line)
                .with_context(|| format!("JSONL line {line_number} has an invalid trace schema"))?;
            let trace = validate_trace(parsed, options)
                .with_context(|| format!("validating JSONL line {line_number}"))?;
            if !trace_ids.insert(trace.trace_id.clone()) {
                bail!(
                    "JSONL line {line_number} duplicates trace ID {}",
                    trace.trace_id
                );
            }
            report.records_seen += 1;
            *report
                .header_fidelity_counts
                .entry(trace.headers.fidelity.clone())
                .or_default() += 1;

            if self.existing_jsonl_trace_matches(&trace)? {
                report.traces_skipped += 1;
                continue;
            }
            let decoded = trace
                .client_request
                .as_ref()
                .into_iter()
                .chain(trace.upstream_request.as_ref())
                .chain(trace.client_response.as_ref())
                .map(|body| body.len() as u64)
                .sum::<u64>();
            let bodies = self.write_jsonl_trace_bodies(&trace)?;
            let written = bodies.iter().filter(|path| path.is_some()).count() as u64;
            self.publish_jsonl_trace(&trace, bodies)?;
            report.traces_imported += 1;
            report.bodies_written += written;
            report.decoded_body_bytes = report.decoded_body_bytes.saturating_add(decoded);
        }
        if report.records_seen == 0 {
            bail!("JSONL export manifest contains no trace records");
        }
        report.input_bytes = input_bytes;
        Ok(report)
    }

    fn existing_jsonl_trace_matches(&self, incoming: &ValidatedTrace) -> Result<bool> {
        let existing = {
            let conn = self.conn.lock().unwrap();
            let sql = format!(
                "SELECT {} FROM traces WHERE id=?1",
                BACKUP_TRACE_COLS.join(", ")
            );
            conn.query_row(&sql, [&incoming.trace_id], |row| {
                sqlite_row_json(row, BACKUP_TRACE_COLS)
            })
            .optional()?
        };
        let Some(mut existing) = existing else {
            return Ok(false);
        };
        remove_body_paths(&mut existing);
        if existing != incoming.metadata {
            bail!(
                "trace {} already exists with conflicting metadata",
                incoming.trace_id
            );
        }
        for (kind, wanted) in [
            ("client_request", incoming.client_request.as_deref()),
            ("upstream_request", incoming.upstream_request.as_deref()),
            ("client_response", incoming.client_response.as_deref()),
        ] {
            let existing =
                self.read_lar_or_legacy_artifact("trace", &incoming.trace_id, kind, None)?;
            if existing.as_deref() != wanted {
                bail!(
                    "trace {} already exists with conflicting {kind} bytes",
                    incoming.trace_id
                );
            }
        }
        Ok(true)
    }

    fn write_jsonl_trace_bodies(&self, trace: &ValidatedTrace) -> Result<[Option<String>; 3]> {
        let mut paths: [Option<String>; 3] = [None, None, None];
        for (index, (kind, legacy_kind, body)) in [
            (
                "client_request",
                "request.json",
                trace.client_request.as_deref(),
            ),
            (
                "upstream_request",
                "upstream-request.json",
                trace.upstream_request.as_deref(),
            ),
            (
                "client_response",
                "response.body",
                trace.client_response.as_deref(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let Some(body) = body else { continue };
            let result = self.write_body_artifact(
                &LarBodyArtifact::trace(&trace.trace_id, kind),
                legacy_kind,
                body,
            )?;
            if let Some(error) = result.lar_error {
                bail!(
                    "writing {kind} for trace {} through LAR failed: {error}",
                    trace.trace_id
                );
            }
            if result.manifest_id.is_none() {
                bail!(
                    "writing {kind} for trace {} did not produce a LAR manifest",
                    trace.trace_id
                );
            }
            let reconstructed = self
                .read_lar_or_legacy_artifact("trace", &trace.trace_id, kind, None)?
                .with_context(|| {
                    format!(
                        "published {kind} for trace {} was not readable",
                        trace.trace_id
                    )
                })?;
            if reconstructed != body {
                bail!(
                    "published {kind} for trace {} failed byte-exact readback",
                    trace.trace_id
                );
            }
            paths[index] = Some(result.legacy_path);
        }
        Ok(paths)
    }

    fn publish_jsonl_trace(
        &self,
        trace: &ValidatedTrace,
        paths: [Option<String>; 3],
    ) -> Result<()> {
        let mut row = trace.metadata.clone();
        let object = row
            .as_object_mut()
            .context("validated JSONL metadata stopped being an object")?;
        for (column, path) in BODY_PATH_COLUMNS.into_iter().zip(paths) {
            object.insert(
                column.to_string(),
                path.map(Value::String).unwrap_or(Value::Null),
            );
        }
        let counts = self.import_trace_backup_rows(&TraceBackupRows {
            traces: vec![row],
            ..TraceBackupRows::default()
        })?;
        if counts.traces_imported == 1 {
            return Ok(());
        }
        if self.existing_jsonl_trace_matches(trace)? {
            return Ok(());
        }
        bail!("trace {} could not be atomically published", trace.trace_id)
    }
}

fn validate_manifest(manifest: &ManifestLine, options: &LarJsonlImportOptions) -> Result<()> {
    if manifest.record_type == "alex.lar.export.manifest"
        && manifest.version == 2
        && manifest.format == "jsonl"
    {
        let schema = manifest.record_schema.as_deref().unwrap_or("unspecified");
        let canonical = manifest.canonical_traces.unwrap_or_default();
        let legacy = manifest.legacy_traces.unwrap_or_default();
        let part_bytes = manifest.body_part_bytes.unwrap_or_default();
        bail!(
            "Alex JSONL v2 schema {schema} is a canonical graph interchange ({canonical} canonical, {legacy} legacy traces; {part_bytes}-byte body parts); this version only imports legacy-compatible JSONL v1 without discarding retries, trailers, streams, or tool links (use a standalone .lar archive for lossless import)"
        );
    }
    if manifest.record_type != "alex.lar.export.manifest"
        || manifest.version != 1
        || manifest.format != "jsonl"
    {
        bail!("unsupported JSONL manifest; expected Alex jsonl version 1");
    }
    if manifest.loss_report.len() > options.max_loss_entries {
        bail!("JSONL loss report exceeds the configured entry limit");
    }
    if manifest
        .loss_report
        .iter()
        .any(|entry| entry.len() > options.max_metadata_bytes)
    {
        bail!("JSONL loss report entry exceeds the configured metadata limit");
    }
    Ok(())
}

fn validate_trace(line: TraceLine, options: &LarJsonlImportOptions) -> Result<ValidatedTrace> {
    if line.record_type != "alex.trace" {
        bail!("record type must be alex.trace");
    }
    validate_metadata(&line.metadata, options)?;
    validate_headers(&line.headers, options)?;
    if metadata_headers(&line.metadata, "req_headers_json")? != line.headers.request {
        bail!("request headers do not match trace metadata req_headers_json");
    }
    if metadata_headers(&line.metadata, "resp_headers_json")? != line.headers.response {
        bail!("response headers do not match trace metadata resp_headers_json");
    }
    let trace_id = line.metadata["id"]
        .as_str()
        .context("trace metadata id must be a string")?
        .to_string();
    let client_request = decode_artifact(line.artifacts.client_request, "client_request", options)?;
    let upstream_request =
        decode_artifact(line.artifacts.upstream_request, "upstream_request", options)?;
    let client_response =
        decode_artifact(line.artifacts.client_response, "client_response", options)?;
    Ok(ValidatedTrace {
        trace_id,
        metadata: line.metadata,
        headers: line.headers,
        client_request,
        upstream_request,
        client_response,
    })
}

fn validate_metadata(metadata: &Value, options: &LarJsonlImportOptions) -> Result<()> {
    let object = metadata
        .as_object()
        .context("trace metadata must be an object")?;
    if serde_json::to_vec(metadata)?.len() > options.max_metadata_bytes {
        bail!("trace metadata exceeds the configured byte limit");
    }
    let expected = BACKUP_TRACE_COLS
        .iter()
        .copied()
        .filter(|column| !BODY_PATH_COLUMNS.contains(column))
        .collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let unknown = actual.difference(&expected).copied().collect::<Vec<_>>();
        bail!("trace metadata schema mismatch; missing={missing:?}, unknown={unknown:?}");
    }
    let id = object["id"]
        .as_str()
        .context("trace metadata id must be a string")?;
    if id.is_empty() || id.len() > 4 * 1024 || id.chars().any(char::is_control) {
        bail!("trace metadata id is empty, too long, or contains control characters");
    }
    require_integer(&object["ts_request_ms"], "ts_request_ms", false)?;
    for name in [
        "ts_response_ms",
        "status",
        "streamed",
        "input_tokens",
        "cached_input_tokens",
        "cache_creation_tokens",
        "output_tokens",
        "reasoning_tokens",
        "substituted",
        "injected",
        "thinking_budget",
        "via_dario",
    ] {
        require_integer(&object[name], name, true)?;
    }
    for name in ["streamed", "substituted", "injected", "via_dario"] {
        if object[name]
            .as_i64()
            .is_some_and(|value| value != 0 && value != 1)
        {
            bail!("trace metadata {name} must be 0, 1, or null");
        }
    }
    if !object["cost_usd"].is_null() && !object["cost_usd"].is_number() {
        bail!("trace metadata cost_usd must be a number or null");
    }
    let numeric = [
        "ts_request_ms",
        "ts_response_ms",
        "status",
        "streamed",
        "input_tokens",
        "cached_input_tokens",
        "cache_creation_tokens",
        "output_tokens",
        "reasoning_tokens",
        "cost_usd",
        "substituted",
        "injected",
        "thinking_budget",
        "via_dario",
    ];
    for (name, value) in object {
        if name == "id" || numeric.contains(&name.as_str()) {
            continue;
        }
        if !value.is_null() && !value.is_string() {
            bail!("trace metadata {name} must be a string or null");
        }
    }
    Ok(())
}

fn require_integer(value: &Value, name: &str, nullable: bool) -> Result<()> {
    if nullable && value.is_null() {
        return Ok(());
    }
    if value.as_i64().is_none() {
        bail!(
            "trace metadata {name} must be an integer{}",
            if nullable { " or null" } else { "" }
        );
    }
    Ok(())
}

fn validate_headers(headers: &JsonlHeaders, options: &LarJsonlImportOptions) -> Result<()> {
    if headers.fidelity != "legacy_order_and_casing_unknown" {
        bail!(
            "unsupported JSONL header fidelity {}; current Alex JSONL exports use legacy_order_and_casing_unknown",
            headers.fidelity
        );
    }
    let count = headers.request.len().saturating_add(headers.response.len());
    if count > options.max_header_count {
        bail!("JSONL headers exceed the configured count limit");
    }
    let bytes =
        headers
            .request
            .iter()
            .chain(&headers.response)
            .try_fold(0usize, |total, header| {
                if header.name.is_empty()
                    || header
                        .name
                        .chars()
                        .any(|value| value == '\r' || value == '\n')
                {
                    bail!("JSONL header name is empty or contains a line break");
                }
                if header
                    .value
                    .chars()
                    .any(|value| value == '\r' || value == '\n')
                {
                    bail!("JSONL header value contains a line break");
                }
                total
                    .checked_add(header.name.len())
                    .and_then(|value| value.checked_add(header.value.len()))
                    .context("JSONL header byte count overflow")
            })?;
    if bytes > options.max_header_bytes {
        bail!("JSONL headers exceed the configured byte limit");
    }
    Ok(())
}

fn metadata_headers(metadata: &Value, field: &str) -> Result<Vec<JsonlHeader>> {
    let raw = match &metadata[field] {
        Value::Null => return Ok(Vec::new()),
        Value::String(raw) => raw,
        _ => bail!("trace metadata {field} must be a JSON string or null"),
    };
    // Existing Alex rows can contain old malformed header JSON. The exporter
    // represents those as an empty normalized list while preserving the raw
    // metadata string, so the importer mirrors that behavior.
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    match parsed {
        Value::Object(values) => {
            for (name, value) in values {
                match value {
                    Value::Array(items) => {
                        for item in items {
                            if let Some(value) = metadata_header_value(&item) {
                                output.push(JsonlHeader {
                                    name: name.clone(),
                                    value,
                                });
                            }
                        }
                    }
                    value => {
                        if let Some(value) = metadata_header_value(&value) {
                            output.push(JsonlHeader { name, value });
                        }
                    }
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                match value {
                    Value::Array(pair) if pair.len() == 2 => {
                        if let (Some(name), Some(value)) =
                            (pair[0].as_str(), metadata_header_value(&pair[1]))
                        {
                            output.push(JsonlHeader {
                                name: name.to_string(),
                                value,
                            });
                        }
                    }
                    Value::Object(pair) => {
                        if let (Some(name), Some(value)) = (
                            pair.get("name").and_then(Value::as_str),
                            pair.get("value").and_then(metadata_header_value),
                        ) {
                            output.push(JsonlHeader {
                                name: name.to_string(),
                                value,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(output)
}

fn metadata_header_value(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn decode_artifact(
    artifact: Option<JsonlEncodedArtifact>,
    name: &str,
    options: &LarJsonlImportOptions,
) -> Result<Option<Vec<u8>>> {
    let Some(artifact) = artifact else {
        return Ok(None);
    };
    if artifact.encoding != "base64" {
        bail!("{name} encoding must be base64");
    }
    if artifact.length > options.max_body_bytes as u64 {
        bail!("{name} exceeds the configured decoded-body limit");
    }
    let estimated = artifact
        .data
        .len()
        .checked_mul(3)
        .with_context(|| format!("{name} base64 length overflow"))?
        / 4;
    if estimated > options.max_body_bytes.saturating_add(2) {
        bail!("{name} base64 data exceeds the configured decoded-body limit");
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(artifact.data.as_bytes())
        .with_context(|| format!("{name} contains invalid base64"))?;
    if bytes.len() as u64 != artifact.length {
        bail!("{name} decoded length does not match its declared length");
    }
    let actual = blake3::hash(&bytes).to_hex().to_string();
    if artifact.blake3.len() != 64
        || !artifact.blake3.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !artifact
            .blake3
            .bytes()
            .all(|byte| !byte.is_ascii_uppercase())
        || artifact.blake3 != actual
    {
        bail!("{name} BLAKE3 hash does not match its decoded bytes");
    }
    Ok(Some(bytes))
}

fn remove_body_paths(metadata: &mut Value) {
    if let Some(object) = metadata.as_object_mut() {
        for column in BODY_PATH_COLUMNS {
            object.remove(column);
        }
    }
}

fn read_limited_line<R: BufRead>(
    reader: &mut R,
    max_line_bytes: usize,
    max_input_bytes: u64,
    input_bytes: &mut u64,
) -> Result<Option<Vec<u8>>> {
    let limit = max_line_bytes
        .checked_add(1)
        .context("JSONL line limit exceeds the address space")?;
    let mut line = Vec::new();
    let read = reader
        .take(limit as u64)
        .read_until(b'\n', &mut line)
        .context("reading JSONL input")?;
    if read == 0 {
        return Ok(None);
    }
    *input_bytes = input_bytes
        .checked_add(read as u64)
        .context("JSONL input byte count overflow")?;
    if *input_bytes > max_input_bytes {
        bail!("JSONL input exceeds the configured total-byte limit");
    }
    if line.len() > max_line_bytes {
        bail!("JSONL line exceeds the configured byte limit");
    }
    if line.last() == Some(&b'\n') {
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
    }
    Ok(Some(line))
}
