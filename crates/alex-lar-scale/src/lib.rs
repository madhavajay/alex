//! Reproducible, public scale verification for LAR V1.
//!
//! The full profile creates 55,000 synthetic traces and 9.4 GB of logical
//! request/response bytes without retaining more than one body in memory.

use alex_lar::{
    export_sanitized_fixture, ArchiveReader, ArchiveWriter, BodyKey, FixtureExportReport,
};
use alex_store::{LarMigrationReport, Store, StoredBodySource, TraceBodyKind, TraceFilter};
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use flate2::{Compression, GzBuilder};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::hint::black_box;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MANIFEST_FILE: &str = "lar-scale-manifest.json";
pub const FULL_TRACE_COUNT: u64 = 55_000;
pub const FULL_LOGICAL_BODY_BYTES: u64 = 9_400_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScaleProfile {
    Ci,
    Full,
}

impl ScaleProfile {
    pub fn spec(self) -> ProfileSpec {
        match self {
            Self::Ci => ProfileSpec {
                profile: self,
                trace_count: 64,
                logical_body_bytes: 8_000_000,
                samples: 20,
                resume_first_entries: 17,
                budgets: BudgetSpec {
                    generation_ms: 60_000,
                    migration_ms: 120_000,
                    archive_verify_ms: 60_000,
                    sqlite_summary_p95_us: 250_000,
                    sqlite_session_summary_p95_us: 500_000,
                    sqlite_filtered_search_p95_us: 250_000,
                    sqlite_trace_get_p95_us: 50_000,
                    lar_random_read_p95_us: 100_000,
                    trace_turn_open_p95_us: 200_000,
                    peak_rss_bytes: 512 * 1024 * 1024,
                },
            },
            Self::Full => ProfileSpec {
                profile: self,
                trace_count: FULL_TRACE_COUNT,
                logical_body_bytes: FULL_LOGICAL_BODY_BYTES,
                samples: 100,
                resume_first_entries: 257,
                budgets: BudgetSpec {
                    generation_ms: 15 * 60 * 1_000,
                    migration_ms: 60 * 60 * 1_000,
                    archive_verify_ms: 20 * 60 * 1_000,
                    sqlite_summary_p95_us: 100_000,
                    sqlite_session_summary_p95_us: 250_000,
                    sqlite_filtered_search_p95_us: 100_000,
                    sqlite_trace_get_p95_us: 25_000,
                    lar_random_read_p95_us: 25_000,
                    trace_turn_open_p95_us: 75_000,
                    peak_rss_bytes: 512 * 1024 * 1024,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProfileSpec {
    pub profile: ScaleProfile,
    pub trace_count: u64,
    pub logical_body_bytes: u64,
    pub samples: usize,
    pub resume_first_entries: usize,
    pub budgets: BudgetSpec,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BudgetSpec {
    pub generation_ms: u64,
    pub migration_ms: u64,
    pub archive_verify_ms: u64,
    pub sqlite_summary_p95_us: u64,
    pub sqlite_session_summary_p95_us: u64,
    pub sqlite_filtered_search_p95_us: u64,
    pub sqlite_trace_get_p95_us: u64,
    pub lar_random_read_p95_us: u64,
    pub trace_turn_open_p95_us: u64,
    pub peak_rss_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CorpusManifest {
    pub schema_version: u32,
    pub profile: ScaleProfile,
    pub seed: u64,
    pub trace_count: u64,
    pub session_count: u64,
    pub bodies_per_trace: u64,
    pub body_count: u64,
    pub logical_body_bytes: u64,
    pub legacy_file_count: u64,
    pub deterministic_date: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MachineMetadata {
    pub os: String,
    pub arch: String,
    pub cpu_model: Option<String>,
    pub logical_cpus: usize,
    pub total_memory_bytes: Option<u64>,
    pub rustc: Option<String>,
    pub git_commit: Option<String>,
    pub package_version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricReport {
    pub name: String,
    pub unit: String,
    pub samples: usize,
    pub min: u64,
    pub p50: u64,
    pub p95: u64,
    pub max: u64,
    pub budget: u64,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MigrationEvidence {
    pub first_batch: LarMigrationReport,
    pub resumed: LarMigrationReport,
    pub partial_run_switched_zero_pointers: bool,
    pub resumed_to_complete_validation: bool,
    pub originals_preserved: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArchiveEvidence {
    pub path: String,
    pub archive_bytes: u64,
    pub record_count: u64,
    pub expected_record_count: u64,
    pub resident_index_bytes: usize,
    pub fully_verified: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScaleReport {
    pub schema_version: u32,
    pub measured_unix_ms: u128,
    pub passed: bool,
    pub profile: ProfileSpec,
    pub machine: MachineMetadata,
    pub corpus: CorpusManifest,
    pub legacy_bytes_on_disk: u64,
    pub sqlite_bytes_on_disk: u64,
    pub generation_ms: Option<u64>,
    pub migration_ms: u64,
    pub archive_verify_ms: u64,
    pub migration: MigrationEvidence,
    pub archive: ArchiveEvidence,
    pub peak_rss_bytes: Option<u64>,
    pub peak_rss_budget_bytes: u64,
    pub peak_rss_passed: Option<bool>,
    pub metrics: Vec<MetricReport>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FableFixtureReport {
    pub schema_version: u32,
    pub archive: String,
    pub bodies: u64,
    pub structurally_redacted_bodies: u64,
    pub all_records_marked_sanitized: bool,
    pub archive_verified: bool,
    pub synthetic_secret_absent: bool,
    pub fable_failure_verified: bool,
    pub sol_reroute_verified: bool,
}

pub fn generate_corpus(root: &Path, profile: ScaleProfile) -> Result<(CorpusManifest, Duration)> {
    ensure_empty_root(root)?;
    let spec = profile.spec();
    if !spec.logical_body_bytes.is_multiple_of(2) {
        bail!("logical body byte target must be even");
    }
    fs::create_dir_all(root)?;
    drop(Store::open(root.to_path_buf())?);
    let body_root = root.join("bodies/2000-01-01");
    fs::create_dir_all(&body_root)?;
    let started = Instant::now();
    let per_file_total = spec.logical_body_bytes / 2;
    let base_file_bytes = per_file_total / spec.trace_count;
    let extra_files = per_file_total % spec.trace_count;

    let database = root.join("alexandria.sqlite3");
    let mut connection = Connection::open(&database)?;
    connection.execute_batch("PRAGMA synchronous=OFF;")?;
    let transaction = connection.transaction()?;
    {
        let mut insert = transaction.prepare_cached(
            "INSERT INTO traces (
               id, ts_request_ms, ts_response_ms, session_id, harness,
               client_format, upstream_provider, upstream_format,
               requested_model, routed_model, method, path, status, streamed,
               input_tokens, output_tokens, req_body_path, resp_body_path,
               run_id, tags_json
             ) VALUES (
               ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
               ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20
             )",
        )?;
        for index in 0..spec.trace_count {
            let trace_id = format!("scale-{index:05}");
            let body_len = base_file_bytes + u64::from(index < extra_files);
            let body = deterministic_body(&trace_id, index, body_len)?;
            let body_path = body_root.join(format!("{trace_id}.turn.json.gz"));
            write_deterministic_gzip(&body_path, &body)?;
            let model = if index % 2 == 0 {
                "gpt-5.6-sol"
            } else {
                "claude-sonnet-4-6"
            };
            let provider = if index % 2 == 0 {
                "openai"
            } else {
                "anthropic"
            };
            let timestamp = 1_700_000_000_000i64 + index as i64;
            insert.execute(params![
                trace_id,
                timestamp,
                timestamp + 25,
                format!("scale-session-{index:05}"),
                "scale-harness",
                "openai_responses",
                provider,
                "openai_responses",
                model,
                model,
                "POST",
                "/v1/responses",
                200i64,
                0i64,
                128i64,
                64i64,
                body_path.to_string_lossy(),
                body_path.to_string_lossy(),
                format!("scale-run-{:03}", index % 100),
                json!({"scale_bucket": index % 32}).to_string(),
            ])?;
        }
    }
    transaction.commit()?;
    let manifest = CorpusManifest {
        schema_version: 1,
        profile,
        seed: 0x4c_41_52_31,
        trace_count: spec.trace_count,
        session_count: spec.trace_count,
        bodies_per_trace: 2,
        body_count: spec.trace_count * 2,
        logical_body_bytes: spec.logical_body_bytes,
        legacy_file_count: spec.trace_count,
        deterministic_date: "2000-01-01".into(),
    };
    write_json(&root.join(MANIFEST_FILE), &manifest)?;
    Ok((manifest, started.elapsed()))
}

pub fn verify_scale(
    root: &Path,
    profile: ScaleProfile,
    generation_duration: Option<Duration>,
    output: &Path,
    enforce: bool,
) -> Result<ScaleReport> {
    let spec = profile.spec();
    let manifest: CorpusManifest = serde_json::from_reader(File::open(root.join(MANIFEST_FILE))?)?;
    validate_manifest(&manifest, &spec)?;
    let (legacy_files, legacy_bytes) = directory_usage(&root.join("bodies"))?;
    if legacy_files != manifest.legacy_file_count {
        bail!(
            "legacy file count {legacy_files} != expected {}",
            manifest.legacy_file_count
        );
    }
    let store = Store::open(root.to_path_buf())?;

    let migration_started = Instant::now();
    let first = store.migrate_legacy_trace_bodies_to_lar(Some(spec.resume_first_entries))?;
    let zero_switch = first.candidates == manifest.body_count
        && first.next_index == spec.resume_first_entries
        && first.pointers_switched == 0
        && !first.complete;
    let resumed = store.migrate_legacy_trace_bodies_to_lar(None)?;
    let migration_duration = migration_started.elapsed();
    let resumed_ok =
        resumed.complete && resumed.validated && resumed.pointers_switched == manifest.body_count;
    let originals_preserved = directory_usage(&root.join("bodies"))?.0 == legacy_files;
    if !zero_switch || !resumed_ok || !originals_preserved {
        bail!("migration resume evidence failed: first={first:?}, resumed={resumed:?}");
    }

    let archive_path = root.join("lar/legacy-v1.lar");
    let verify_started = Instant::now();
    let mut archive = ArchiveReader::open(&archive_path)?;
    let resident_index_bytes = archive.resident_index_bytes();
    if archive.len() != manifest.body_count {
        bail!(
            "archive record count {} != expected {}",
            archive.len(),
            manifest.body_count
        );
    }
    let max_body_bytes = maximum_body_bytes(&spec) + 1024;
    let verified = archive.verify(max_body_bytes)?;
    let verify_duration = verify_started.elapsed();
    if verified.checked != manifest.body_count || resident_index_bytes != 0 {
        bail!("archive verification or lazy-index invariant failed");
    }

    let mut metrics = Vec::new();
    if let Some(generation_duration) = generation_duration {
        metrics.push(metric(
            "generation",
            "ms",
            vec![duration_ms(generation_duration)],
            spec.budgets.generation_ms,
        ));
    }
    metrics.push(metric(
        "migration_resume_and_validation",
        "ms",
        vec![duration_ms(migration_duration)],
        spec.budgets.migration_ms,
    ));
    metrics.push(metric(
        "archive_full_verify",
        "ms",
        vec![duration_ms(verify_duration)],
        spec.budgets.archive_verify_ms,
    ));

    let summary = measure(spec.samples, |sample| {
        let max_offset = manifest.trace_count.saturating_sub(200) as usize;
        let rows = store.search_traces(&TraceFilter {
            limit: 200,
            offset: if max_offset == 0 {
                0
            } else {
                sample.wrapping_mul(137) % max_offset
            },
            ..TraceFilter::default()
        })?;
        if rows.is_empty() {
            bail!("summary query returned no rows");
        }
        black_box(rows);
        Ok(())
    })?;
    metrics.push(metric(
        "sqlite_trace_summary_page",
        "us",
        summary,
        spec.budgets.sqlite_summary_p95_us,
    ));

    let session_summary = measure(spec.samples, |_| {
        let rows = store.sessions(None, 200)?;
        if rows.is_empty() {
            bail!("session summary query returned no rows");
        }
        black_box(rows);
        Ok(())
    })?;
    metrics.push(metric(
        "sqlite_session_summary_page",
        "us",
        session_summary,
        spec.budgets.sqlite_session_summary_p95_us,
    ));

    let filtered = measure(spec.samples, |sample| {
        let model = if sample % 2 == 0 {
            "gpt-5.6-sol"
        } else {
            "claude-sonnet-4-6"
        };
        let rows = store.search_traces(&TraceFilter {
            model: Some(model.into()),
            limit: 200,
            ..TraceFilter::default()
        })?;
        if rows.is_empty() {
            bail!("filtered search returned no rows");
        }
        black_box(rows);
        Ok(())
    })?;
    metrics.push(metric(
        "sqlite_filtered_model_search",
        "us",
        filtered,
        spec.budgets.sqlite_filtered_search_p95_us,
    ));

    let trace_get = measure(spec.samples, |sample| {
        let index = deterministic_index(sample, manifest.trace_count);
        let row = store.get_trace(&format!("scale-{index:05}"))?;
        if row.is_none() {
            bail!("random trace lookup failed");
        }
        black_box(row);
        Ok(())
    })?;
    metrics.push(metric(
        "sqlite_one_trace_get",
        "us",
        trace_get,
        spec.budgets.sqlite_trace_get_p95_us,
    ));

    let lar_read = measure(spec.samples, |sample| {
        let index = deterministic_index(sample.wrapping_add(31), manifest.trace_count);
        let kind = if sample % 2 == 0 {
            "request"
        } else {
            "response"
        };
        archive.copy_body_to(
            &format!("scale-{index:05}"),
            kind,
            max_body_bytes,
            &mut io::sink(),
        )?;
        Ok(())
    })?;
    metrics.push(metric(
        "lar_random_body_read_warm_cache",
        "us",
        lar_read,
        spec.budgets.lar_random_read_p95_us,
    ));

    let turn_open = measure(spec.samples, |sample| {
        let index = deterministic_index(sample.wrapping_add(73), manifest.trace_count);
        let trace_id = format!("scale-{index:05}");
        let session_id = format!("scale-session-{index:05}");
        let rows = store.session_traces(&session_id, None)?;
        if rows.len() != 1 || rows[0]["id"] != trace_id {
            bail!("one-turn session lookup did not return its trace");
        }
        for kind in [TraceBodyKind::Request, TraceBodyKind::Response] {
            let body = store
                .read_trace_body(&trace_id, kind, max_body_bytes)?
                .context("one-turn body missing")?;
            if body.source != StoredBodySource::Lar {
                bail!("one-turn body did not resolve through LAR");
            }
            black_box(body.bytes);
        }
        Ok(())
    })?;
    metrics.push(metric(
        "trace_one_turn_open_with_two_bodies",
        "us",
        turn_open,
        spec.budgets.trace_turn_open_p95_us,
    ));

    let peak_rss = peak_rss_bytes();
    let peak_rss_passed = peak_rss.map(|bytes| bytes <= spec.budgets.peak_rss_bytes);
    let metrics_passed = metrics.iter().all(|metric| metric.passed);
    let passed = metrics_passed && peak_rss_passed.unwrap_or(false);
    let report = ScaleReport {
        schema_version: 1,
        measured_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        passed,
        profile: spec.clone(),
        machine: machine_metadata(),
        corpus: manifest.clone(),
        legacy_bytes_on_disk: legacy_bytes,
        sqlite_bytes_on_disk: sqlite_file_bytes(root)?,
        generation_ms: generation_duration.map(duration_ms),
        migration_ms: duration_ms(migration_duration),
        archive_verify_ms: duration_ms(verify_duration),
        migration: MigrationEvidence {
            first_batch: first,
            resumed,
            partial_run_switched_zero_pointers: zero_switch,
            resumed_to_complete_validation: resumed_ok,
            originals_preserved,
        },
        archive: ArchiveEvidence {
            path: archive_path.to_string_lossy().into_owned(),
            archive_bytes: fs::metadata(&archive_path)?.len(),
            record_count: archive.len(),
            expected_record_count: manifest.body_count,
            resident_index_bytes,
            fully_verified: verified.checked == manifest.body_count,
        },
        peak_rss_bytes: peak_rss,
        peak_rss_budget_bytes: spec.budgets.peak_rss_bytes,
        peak_rss_passed,
        metrics,
        limitations: vec![
            "random LAR reads are warm-cache; privileged OS cache eviction is not attempted".into(),
            "payloads are deterministic synthetic JSON and do not claim private-corpus compression representativeness".into(),
            "web UI virtualization, HAR conversion, live capture, and tool-body archives are outside this verifier".into(),
        ],
    };
    write_json(output, &report)?;
    if enforce && !report.passed {
        bail!(
            "one or more LAR scale budgets failed; see {}",
            output.display()
        );
    }
    Ok(report)
}

pub fn run_scale(
    root: &Path,
    profile: ScaleProfile,
    output: &Path,
    enforce: bool,
) -> Result<ScaleReport> {
    let (_, generation_duration) = generate_corpus(root, profile)?;
    verify_scale(root, profile, Some(generation_duration), output, enforce)
}

pub fn generate_fable_sol_fixture(
    vector_path: &Path,
    failure_path: &Path,
    output: &Path,
) -> Result<FableFixtureReport> {
    if output.exists() {
        bail!("refusing to overwrite fixture archive {}", output.display());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let vector: Value = serde_json::from_reader(File::open(vector_path)?)?;
    let failure: Value = serde_json::from_reader(File::open(failure_path)?)?;
    let failure_name = failure["name"]
        .as_str()
        .context("failure fixture has no name")?;
    if vector["failure_fixture"] != failure_name
        || vector["request"]["model"] != "claude-fable-5"
        || vector["expected_decision"]["decision"] != "reroute"
        || vector["expected_decision"]["target"]["model"] != "gpt-5.6-sol"
        || vector["expected_decision"]["target"]["providers"]["only"][0] != "openai"
    {
        bail!("Fable→Sol middleware vector does not describe the expected route");
    }
    let failure_body: Value = serde_json::from_str(
        failure["body"]
            .as_str()
            .context("failure fixture body is not a JSON string")?,
    )?;
    if failure["provider"] != "anthropic"
        || failure["status"] != 529
        || failure_body["error"]["type"] != failure["error_kind"]
    {
        bail!("Fable failure fixture metadata and body disagree");
    }

    let temporary = output.with_file_name(format!(
        ".{}.source.{}.lar",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fable-sol"),
        std::process::id()
    ));
    let result = (|| {
        let request = json!({
            "authorization": "Bearer synthetic-fable-sol-fixture-secret",
            "request": vector["request"],
            "failure_fixture": vector["failure_fixture"],
        });
        let mut writer = ArchiveWriter::open(&temporary)?;
        writer.append_body(
            "fable-to-sol",
            "request",
            &serde_json::to_vec(&request)?,
            Vec::new(),
        )?;
        writer.append_body(
            "fable-to-sol",
            "response",
            &serde_json::to_vec(&failure_body)?,
            Vec::new(),
        )?;
        writer.append_body(
            "fable-to-sol",
            "expected-decision",
            &serde_json::to_vec(&vector["expected_decision"])?,
            Vec::new(),
        )?;
        writer.finish()?;
        let keys = [
            BodyKey::new("fable-to-sol", "request"),
            BodyKey::new("fable-to-sol", "response"),
            BodyKey::new("fable-to-sol", "expected-decision"),
        ];
        let exported: FixtureExportReport =
            export_sanitized_fixture(&temporary, output, &keys, 1024 * 1024)?;
        let mut archive = ArchiveReader::open(output)?;
        let verified = archive.verify(1024 * 1024)?;
        let metadata = archive.list(0, 16)?;
        let request: Value =
            serde_json::from_slice(&archive.read_body("fable-to-sol", "request", 1024 * 1024)?)?;
        let response: Value =
            serde_json::from_slice(&archive.read_body("fable-to-sol", "response", 1024 * 1024)?)?;
        let decision: Value = serde_json::from_slice(&archive.read_body(
            "fable-to-sol",
            "expected-decision",
            1024 * 1024,
        )?)?;
        let all_bytes = [
            serde_json::to_vec(&request)?,
            serde_json::to_vec(&response)?,
            serde_json::to_vec(&decision)?,
        ]
        .concat();
        let secret_absent = !String::from_utf8_lossy(&all_bytes)
            .contains("synthetic-fable-sol-fixture-secret")
            && !String::from_utf8_lossy(&all_bytes).contains("Bearer ");
        let report = FableFixtureReport {
            schema_version: 1,
            archive: output.to_string_lossy().into_owned(),
            bodies: exported.bodies,
            structurally_redacted_bodies: exported.sanitized_bodies,
            all_records_marked_sanitized: metadata.iter().all(|body| body.sanitized),
            archive_verified: verified.checked == 3,
            synthetic_secret_absent: secret_absent,
            fable_failure_verified: response == failure_body,
            sol_reroute_verified: decision == vector["expected_decision"],
        };
        if report.bodies != 3
            || report.structurally_redacted_bodies != 1
            || !report.all_records_marked_sanitized
            || !report.archive_verified
            || !report.synthetic_secret_absent
            || !report.fable_failure_verified
            || !report.sol_reroute_verified
            || request["authorization"] != "[REDACTED]"
        {
            bail!("generated Fable→Sol LAR fixture failed verification");
        }
        Ok(report)
    })();
    let _ = fs::remove_file(&temporary);
    result
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn ensure_empty_root(root: &Path) -> Result<()> {
    if root.exists() && fs::read_dir(root)?.next().is_some() {
        bail!(
            "scale root {} is not empty; choose a disposable empty directory",
            root.display()
        );
    }
    Ok(())
}

fn validate_manifest(manifest: &CorpusManifest, spec: &ProfileSpec) -> Result<()> {
    if manifest.schema_version != 1
        || manifest.profile != spec.profile
        || manifest.trace_count != spec.trace_count
        || manifest.body_count != spec.trace_count * 2
        || manifest.logical_body_bytes != spec.logical_body_bytes
    {
        bail!("corpus manifest does not match the selected profile");
    }
    Ok(())
}

fn deterministic_body(trace_id: &str, index: u64, target_len: u64) -> Result<Vec<u8>> {
    let prefix = format!(
        "{{\"schema\":1,\"trace_id\":\"{trace_id}\",\"search_bucket\":{},\"padding\":\"",
        index % 32
    );
    let suffix = b"\"}\n";
    let target_len = usize::try_from(target_len).context("body length does not fit usize")?;
    if target_len < prefix.len() + suffix.len() {
        bail!("configured synthetic body is too small for its JSON envelope");
    }
    let mut body = Vec::with_capacity(target_len);
    body.extend_from_slice(prefix.as_bytes());
    let padding_len = target_len - prefix.len() - suffix.len();
    for position in 0..padding_len {
        body.push(b'a' + ((position as u64 + index * 7) % 23) as u8);
    }
    body.extend_from_slice(suffix);
    debug_assert_eq!(body.len(), target_len);
    Ok(body)
}

fn write_deterministic_gzip(path: &Path, body: &[u8]) -> Result<()> {
    let file = File::create(path)?;
    let mut encoder = GzBuilder::new()
        .mtime(0)
        .write(BufWriter::new(file), Compression::fast());
    encoder.write_all(body)?;
    encoder.finish()?.flush()?;
    Ok(())
}

fn maximum_body_bytes(spec: &ProfileSpec) -> u64 {
    let per_file_total = spec.logical_body_bytes / 2;
    per_file_total.div_ceil(spec.trace_count)
}

fn deterministic_index(sample: usize, trace_count: u64) -> u64 {
    ((sample as u64)
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407))
        % trace_count
}

fn measure(mut samples: usize, mut operation: impl FnMut(usize) -> Result<()>) -> Result<Vec<u64>> {
    samples = samples.max(1);
    operation(usize::MAX)?;
    let mut durations = Vec::with_capacity(samples);
    for sample in 0..samples {
        let started = Instant::now();
        operation(sample)?;
        durations.push(started.elapsed().as_micros().min(u64::MAX as u128) as u64);
    }
    Ok(durations)
}

fn metric(name: &str, unit: &str, mut values: Vec<u64>, budget: u64) -> MetricReport {
    values.sort_unstable();
    let p50 = percentile(&values, 50);
    let p95 = percentile(&values, 95);
    MetricReport {
        name: name.into(),
        unit: unit.into(),
        samples: values.len(),
        min: values.first().copied().unwrap_or(0),
        p50,
        p95,
        max: values.last().copied().unwrap_or(0),
        budget,
        passed: p95 <= budget,
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (sorted.len() * percentile).div_ceil(100).max(1) - 1;
    sorted[rank.min(sorted.len() - 1)]
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn directory_usage(root: &Path) -> Result<(u64, u64)> {
    if !root.exists() {
        return Ok((0, 0));
    }
    let mut files = 0u64;
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                files += 1;
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok((files, total))
}

fn sqlite_file_bytes(root: &Path) -> Result<u64> {
    let mut total = 0u64;
    for suffix in ["", "-wal", "-shm"] {
        let path = root.join(format!("alexandria.sqlite3{suffix}"));
        if let Ok(metadata) = fs::metadata(path) {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn machine_metadata() -> MachineMetadata {
    MachineMetadata {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        cpu_model: cpu_model(),
        logical_cpus: std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(1),
        total_memory_bytes: total_memory_bytes(),
        rustc: command_output("rustc", &["--version"]),
        git_commit: command_output(
            "git",
            &[
                "-C",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
                "rev-parse",
                "HEAD",
            ],
        ),
        package_version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn command_output(command: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(command).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn cpu_model() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        return fs::read_to_string("/proc/cpuinfo").ok().and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("model name")
                    .and_then(|value| value.split_once(':'))
                    .map(|(_, value)| value.trim().to_string())
            })
        });
    }
    #[cfg(target_os = "macos")]
    {
        return command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
            .or_else(|| command_output("sysctl", &["-n", "hw.model"]));
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var("PROCESSOR_IDENTIFIER").ok();
    }
    #[allow(unreachable_code)]
    None
}

fn total_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        return fs::read_to_string("/proc/meminfo").ok().and_then(|text| {
            text.lines().find_map(|line| {
                line.strip_prefix("MemTotal:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
                    .map(|kib| kib * 1024)
            })
        });
    }
    #[cfg(target_os = "macos")]
    {
        return command_output("sysctl", &["-n", "hw.memsize"])
            .and_then(|value| value.parse().ok());
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the provided rusage buffer for the current
    // process and does not retain its pointer.
    let status = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: a successful getrusage call initialized the value.
    let maximum = unsafe { usage.assume_init() }.ru_maxrss as u64;
    #[cfg(target_os = "macos")]
    return Some(maximum);
    #[cfg(not(target_os = "macos"))]
    return Some(maximum.saturating_mul(1024));
}

#[cfg(windows)]
fn peak_rss_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters = std::mem::MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: Windows fills exactly `size` bytes in the caller-owned buffer.
    let status =
        unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), size) };
    (status != 0).then(|| unsafe { counters.assume_init() }.PeakWorkingSetSize as u64)
}

#[cfg(not(any(unix, windows)))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_public_corpus_is_exact_streamed_and_resumable() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("corpus");
        let output = directory.path().join("result.json");
        let report = run_scale(&root, ScaleProfile::Ci, &output, false).unwrap();
        assert_eq!(report.corpus.trace_count, 64);
        assert_eq!(report.corpus.logical_body_bytes, 8_000_000);
        assert_eq!(report.archive.record_count, 128);
        assert_eq!(report.archive.resident_index_bytes, 0);
        assert!(report.migration.partial_run_switched_zero_pointers);
        assert!(report.migration.resumed_to_complete_validation);
        assert!(output.is_file());
    }

    #[test]
    fn fable_sol_fixture_is_generated_sanitized_and_replayable() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("fable-sol.lar");
        let report = generate_fable_sol_fixture(
            &workspace.join("crates/alex-proxy/tests/fixtures/middleware/fable-to-sol-vector.json"),
            &workspace.join(
                "crates/alex-proxy/tests/fixtures/middleware/anthropic-fable-unavailable-529.json",
            ),
            &output,
        )
        .unwrap();
        assert!(report.archive_verified);
        assert!(report.synthetic_secret_absent);
        assert!(report.fable_failure_verified);
        assert!(report.sol_reroute_verified);
    }
}
