use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

use alex_lar::{
    upgrade_archive as rewrite_archive, verify_upgraded_archive, ArchiveReader, ArchiveWriter,
    ChunkerConfig, Exchange, ExchangeData, ExchangeMetadataData, FileHeader, HeaderAtom,
    HeaderBlock, HeaderFidelity, Limits, ManifestId, RawBodyScanner, RawSearchLimits,
    RawSearchStats, RecoveryStatus, Stage, StageData, StageId, StageKind, StreamReplaySource,
    StreamReplayTiming, TokenUsage, REQUIRED_FEATURE_CONVERSATION_DAG,
};
use alex_store::{
    grep_lar_archive_records, LarArchiveReattachOptions, LarArtifactLocation, LarBackupArtifactRef,
    LarBodyStoreConfig, LarBodyStoreMode, LarCatalogGrepMatch, LarExportTraceCursor,
    LarInterchangeBody, LarInterchangeStage, LarInterchangeTrace, LarJsonlImportOptions,
    LarLegacyImportOptions, LarMigrationJob, LarRecordGrepCoverage, LarRecordGrepMatch,
    LarRepackConfig, LarStandaloneImportOptions, LarTransactionExportReport, Store,
    TraceBackupRows, LAR_TRANSACTION_FORMAT, LAR_TRANSACTION_VERSION,
};
use anyhow::{bail, Context, Result};
use base64::Engine as _;
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// Commands for the LLM Archive (LAR) body store.
#[derive(Debug, Subcommand)]
pub(crate) enum LarCommand {
    /// Import a sealed standalone LAR archive or Alex JSONL export
    Import(ImportArgs),
    /// Mark one sealed cataloged archive offline without moving its bytes
    Detach(DetachArgs),
    /// Identity-validate and reattach one offline or relocated archive
    Reattach(ReattachArgs),
    /// Convert legacy gzip body files into the LAR store
    ImportLegacy(ImportLegacyArgs),
    /// Inspect or control the background legacy migration
    Migration {
        #[command(subcommand)]
        command: LarMigrationCommand,
    },
    /// Remove legacy files after a completed, verified migration
    Cleanup(CleanupArgs),
    /// Plan, apply, or resume reference-safe garbage collection
    Gc {
        #[command(subcommand)]
        command: LarGcCommand,
    },
    /// Plan, apply, or resume immutable body-pack compaction
    Repack {
        #[command(subcommand)]
        command: LarRepackCommand,
    },
    /// Verify archive framing, indexes, checksums, and references
    Verify(VerifyArgs),
    /// Recover readable records into a new archive; never edits the input
    Repair(RepairArgs),
    /// Rewrite a clean sealed archive into the latest v1 format
    Upgrade(UpgradeArgs),
    /// List files, sessions, traces, stages, and artifacts
    Ls(ListArgs),
    /// Search exact raw artifact bytes without scanning duplicate chunks twice
    Grep(GrepArgs),
    /// Reconstruct one raw artifact into a file or stdout
    Extract(ExtractArgs),
    /// Replay a captured raw stream with instant, original, or scaled timing
    Replay(ReplayArgs),
    /// Export one complete raw transaction as a bounded JSON sequence
    Transaction(TransactionArgs),
    /// Replay a captured stream from a transaction JSON sequence
    TransactionReplay(TransactionReplayArgs),
    /// Export selected records to a standalone archive or interchange format
    Export(ExportArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ImportArgs {
    /// Sealed LAR archive or Alex JSONL export
    pub(crate) source: PathBuf,
    /// Input representation; auto inspects file magic/content
    #[arg(long, value_enum, default_value_t = LarImportFormat::Auto)]
    pub(crate) format: LarImportFormat,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DetachArgs {
    /// Exact 32-hex-digit catalog file UUID; active writers cannot be detached
    #[arg(long, value_name = "FILE_UUID", value_parser = parse_lar_file_uuid)]
    pub(crate) file_uuid: String,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ReattachArgs {
    /// Exact 32-hex-digit catalog file UUID expected inside the archive
    #[arg(long, value_name = "FILE_UUID", value_parser = parse_lar_file_uuid)]
    pub(crate) file_uuid: String,
    /// Clean sealed archive candidate; its immutable identity must match
    #[arg(long, value_name = "PATH")]
    pub(crate) archive: PathBuf,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum LarImportFormat {
    Auto,
    Lar,
    Jsonl,
}

#[derive(Debug, Args)]
pub(crate) struct ImportLegacyArgs {
    /// Inventory the work without writing LAR records or changing trace pointers
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Read back and hash-check every imported artifact before pointer switches
    #[arg(long)]
    pub(crate) verify: bool,
    /// Process at most this many unique legacy body artifacts
    #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
    pub(crate) limit: Option<usize>,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LarMigrationCommand {
    /// Show persisted progress, throughput, failures, and the migration lease
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Pause background conversion after its current atomic item
    Pause {
        #[arg(long)]
        json: bool,
    },
    /// Resume an incomplete or paused background conversion
    Resume {
        #[arg(long)]
        json: bool,
    },
    /// Read back and hash-check every migrated artifact
    Verify {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LarGcCommand {
    /// Calculate reachability and reclaimable bytes without changing state
    Plan {
        #[arg(long)]
        json: bool,
    },
    /// Persist a candidate snapshot, recheck it, and logically sweep it
    Apply {
        #[arg(long)]
        json: bool,
    },
    /// Resume a previously persisted marking/sweeping run
    Resume {
        run_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum LarRepackCommand {
    /// List sealed packs whose unreachable bytes exceed the thresholds
    Plan(RepackSelectionArgs),
    /// Copy, verify, atomically switch, and recoverably retire one eligible pack
    Apply(RepackSelectionArgs),
    /// Continue a durable copy/switch/retire run
    Resume {
        run_id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct RepackSelectionArgs {
    /// Minimum compressed garbage bytes required for an eligible pack
    #[arg(long, default_value_t = 64 * 1024 * 1024)]
    pub(crate) min_garbage_bytes: u64,
    /// Minimum fraction of compressed chunk bytes that must be garbage (0..=1)
    #[arg(long, default_value_t = 0.25, value_parser = parse_ratio)]
    pub(crate) min_garbage_ratio: f64,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("mode")
        .required(true)
        .multiple(false)
        .args(["dry_run", "apply"])
))]
pub(crate) struct CleanupArgs {
    /// Report eligible files and byte totals without removing anything
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Quarantine or remove files proven safe by a completed verification pass
    #[arg(long)]
    pub(crate) apply: bool,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct VerifyArgs {
    /// Standalone archive to verify; omit to verify the configured live store
    pub(crate) archive: Option<PathBuf>,
    /// Continue checking after recoverable errors
    #[arg(long)]
    pub(crate) keep_going: bool,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RepairArgs {
    /// Damaged or unsealed archive to scan
    pub(crate) input: PathBuf,
    /// New archive to receive recovered records
    #[arg(long)]
    pub(crate) output: PathBuf,
    /// Replace an existing output file
    #[arg(long)]
    pub(crate) force: bool,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct UpgradeArgs {
    /// Clean, sealed archive to rewrite; it is never modified
    pub(crate) input: PathBuf,
    /// New archive path; it must not already exist
    #[arg(long)]
    pub(crate) output: PathBuf,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Standalone archive to list; omit to list the configured live store
    pub(crate) archive: Option<PathBuf>,
    /// Restrict results to a session
    #[arg(long, conflicts_with = "trace_id")]
    pub(crate) session: Option<String>,
    /// Restrict results to a trace
    #[arg(long, conflicts_with = "session")]
    pub(crate) trace_id: Option<String>,
    /// Maximum number of records to print
    #[arg(long, default_value_t = 100, value_parser = parse_nonzero_usize)]
    pub(crate) limit: usize,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct GrepArgs {
    /// Exact byte string to find (not a regular expression)
    pub(crate) literal: String,
    /// Sealed archives to search in addition to the configured live store
    #[arg(value_name = "ARCHIVE")]
    pub(crate) archives: Vec<PathBuf>,
    /// Stop after this many referencing trace/stage matches
    #[arg(long, default_value_t = 100, value_parser = parse_nonzero_usize)]
    pub(crate) limit: usize,
    /// Search body bytes only, or bodies plus safe canonical record fields
    #[arg(long, value_enum, default_value_t = LarGrepScope::Bodies)]
    pub(crate) scope: LarGrepScope,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum LarGrepScope {
    Bodies,
    WholeRecord,
}

#[derive(Debug, Args)]
pub(crate) struct ExtractArgs {
    /// Trace containing the artifact
    #[arg(long)]
    pub(crate) trace_id: String,
    /// Artifact kind, such as request, upstream-request, response, or raw-stream
    #[arg(long)]
    pub(crate) artifact: String,
    /// Write to this path instead of stdout
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Replace an existing output file
    #[arg(long, requires = "output")]
    pub(crate) force: bool,
    /// Emit a JSON result; body bytes still go to --output
    #[arg(long, requires = "output")]
    pub(crate) json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum LarReplaySpeed {
    /// Emit every captured range immediately
    Instant,
    /// Preserve original timing
    #[value(name = "1x")]
    Realtime,
    /// Replay at one quarter of original speed
    #[value(name = "0.25x")]
    Quarter,
    /// Replay at half of original speed
    #[value(name = "0.5x")]
    Half,
    /// Replay at twice original speed
    #[value(name = "2x")]
    Double,
    /// Replay at four times original speed
    #[value(name = "4x")]
    Quadruple,
}

impl LarReplaySpeed {
    fn timing(self) -> StreamReplayTiming {
        match self {
            Self::Instant => StreamReplayTiming::Instant,
            Self::Realtime => StreamReplayTiming::Original,
            Self::Quarter => StreamReplayTiming::Scaled {
                speed_numerator: 1,
                speed_denominator: 4,
            },
            Self::Half => StreamReplayTiming::Scaled {
                speed_numerator: 1,
                speed_denominator: 2,
            },
            Self::Double => StreamReplayTiming::Scaled {
                speed_numerator: 2,
                speed_denominator: 1,
            },
            Self::Quadruple => StreamReplayTiming::Scaled {
                speed_numerator: 4,
                speed_denominator: 1,
            },
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct ReplayArgs {
    /// Standalone or sealed LAR archive containing the stream
    pub(crate) archive: PathBuf,
    /// Trace whose captured response stream should be replayed
    #[arg(long)]
    pub(crate) trace_id: String,
    /// Select one stage when a trace contains multiple captured streams
    #[arg(long)]
    pub(crate) stage_id: Option<String>,
    /// Emit parsed SSE/NDJSON events instead of exact observed HTTP reads
    #[arg(long)]
    pub(crate) parsed: bool,
    /// Playback speed; instant is the safe default for long captures
    #[arg(long, value_enum, default_value_t = LarReplaySpeed::Instant)]
    pub(crate) speed: LarReplaySpeed,
    /// Write replay bytes to this path instead of stdout
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Replace an existing output file
    #[arg(long, requires = "output")]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TransactionArgs {
    /// Trace whose complete ordered transaction should be exported
    #[arg(long)]
    pub(crate) trace_id: String,
    /// Read from this sealed standalone archive instead of the live catalog
    #[arg(long)]
    pub(crate) archive: Option<PathBuf>,
    /// Required destination; transaction bytes never print to a terminal
    #[arg(long)]
    pub(crate) output: PathBuf,
    /// Replace an existing destination
    #[arg(long)]
    pub(crate) force: bool,
    /// Emit a machine-readable export report
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct TransactionReplayArgs {
    /// Transaction JSON-sequence file
    pub(crate) input: PathBuf,
    /// Select one stage when the transaction contains multiple captured streams
    #[arg(long)]
    pub(crate) stage_id: Option<String>,
    /// Replay parsed SSE/NDJSON frame ranges instead of observed reads
    #[arg(long)]
    pub(crate) parsed: bool,
    /// Playback speed; instant is the safe default for long captures
    #[arg(long, value_enum, default_value_t = LarReplaySpeed::Instant)]
    pub(crate) speed: LarReplaySpeed,
    /// Write replay bytes here instead of stdout
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Replace an existing output file
    #[arg(long, requires = "output")]
    pub(crate) force: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum LarExportFormat {
    Lar,
    Har,
    Warc,
    Jsonl,
    #[value(name = "otel")]
    OpenTelemetry,
    #[value(name = "openinference")]
    OpenInference,
}

#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// Destination file
    pub(crate) output: PathBuf,
    /// Output representation
    #[arg(long, value_enum, default_value_t = LarExportFormat::Lar)]
    pub(crate) format: LarExportFormat,
    /// Export one trace
    #[arg(long, conflicts_with = "session")]
    pub(crate) trace_id: Option<String>,
    /// Export one session
    #[arg(long)]
    pub(crate) session: Option<String>,
    /// Replace an existing destination
    #[arg(long)]
    pub(crate) force: bool,
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
}

/// Narrow seam between clap and storage. The LAR storage crate can replace this
/// backend without making command-line argument types part of its public API.
pub(crate) trait LarCommandBackend {
    fn execute(&self, data_dir: &Path, command: &LarCommand) -> Result<LarCommandOutput>;
}

#[derive(Debug)]
pub(crate) struct LarCommandOutput {
    human: String,
    json: Value,
    raw_body: Option<Vec<u8>>,
}

impl LarCommandOutput {
    fn print(self, json: bool) -> Result<()> {
        if let Some(body) = self.raw_body {
            if json {
                bail!("raw body output cannot be combined with JSON output");
            }
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&body)?;
            stdout.flush()?;
            return Ok(());
        }
        if json {
            println!("{}", serde_json::to_string_pretty(&self.json)?);
        } else {
            println!("{}", self.human);
        }
        Ok(())
    }
}

/// Local implementation backed by Alex's durable catalog. Archive-codec
/// operations remain explicit errors until their readers and writers land.
struct LocalLarBackend;

impl LarCommandBackend for LocalLarBackend {
    fn execute(&self, data_dir: &Path, command: &LarCommand) -> Result<LarCommandOutput> {
        match command {
            LarCommand::Import(args) => import_archive(data_dir, args),
            LarCommand::Detach(args) => detach_archive(data_dir, args),
            LarCommand::Reattach(args) => reattach_archive(data_dir, args),
            LarCommand::ImportLegacy(args) if args.dry_run => {
                legacy_import_inventory(data_dir, args.limit, args.verify)
            }
            LarCommand::ImportLegacy(args) => run_legacy_import(data_dir, args),
            LarCommand::Migration { command } => migration_command(data_dir, command),
            LarCommand::Cleanup(args) => cleanup_legacy(data_dir, args),
            LarCommand::Gc { command } => gc_command(data_dir, command),
            LarCommand::Repack { command } => repack_command(data_dir, command),
            LarCommand::Verify(args) => {
                if let Some(archive) = args.archive.as_deref() {
                    verify_archive(archive, args.keep_going)
                } else {
                    let store = Store::open(data_dir.to_path_buf())
                        .context("opening the Alex storage catalog")?;
                    migration_verification(&store)
                }
            }
            LarCommand::Repair(args) => repair_archive(args),
            LarCommand::Upgrade(args) => upgrade_archive_command(args),
            LarCommand::Ls(args) => {
                if let Some(archive) = args.archive.as_deref() {
                    list_archive(archive, args)
                } else {
                    live_catalog_summary(data_dir, args)
                }
            }
            LarCommand::Grep(args) => grep_records(data_dir, args),
            LarCommand::Extract(args) => extract_artifact(data_dir, args),
            LarCommand::Replay(_) => bail!("internal error: replay bypassed streaming runner"),
            LarCommand::Transaction(args) => export_transaction(data_dir, args),
            LarCommand::TransactionReplay(_) => {
                bail!("internal error: transaction replay bypassed command backend")
            }
            LarCommand::Export(args) => export_records(data_dir, args),
        }
    }
}

pub(crate) fn run(data_dir: &Path, command: LarCommand) -> Result<()> {
    if let LarCommand::Replay(args) = &command {
        return replay_stream(args);
    }
    if let LarCommand::TransactionReplay(args) = &command {
        return replay_transaction(args);
    }
    let json = command.json();
    LocalLarBackend.execute(data_dir, &command)?.print(json)
}

impl LarCommand {
    fn json(&self) -> bool {
        match self {
            Self::Import(args) => args.json,
            Self::Detach(args) => args.json,
            Self::Reattach(args) => args.json,
            Self::ImportLegacy(args) => args.json,
            Self::Migration { command } => match command {
                LarMigrationCommand::Status { json }
                | LarMigrationCommand::Pause { json }
                | LarMigrationCommand::Resume { json }
                | LarMigrationCommand::Verify { json } => *json,
            },
            Self::Cleanup(args) => args.json,
            Self::Gc { command } => match command {
                LarGcCommand::Plan { json }
                | LarGcCommand::Apply { json }
                | LarGcCommand::Resume { json, .. } => *json,
            },
            Self::Repack { command } => match command {
                LarRepackCommand::Plan(args) | LarRepackCommand::Apply(args) => args.json,
                LarRepackCommand::Resume { json, .. } => *json,
            },
            Self::Verify(args) => args.json,
            Self::Repair(args) => args.json,
            Self::Upgrade(args) => args.json,
            Self::Ls(args) => args.json,
            Self::Grep(args) => args.json,
            Self::Extract(args) => args.json,
            Self::Replay(_) => false,
            Self::Transaction(args) => args.json,
            Self::TransactionReplay(_) => false,
            Self::Export(args) => args.json,
        }
    }
}

fn export_transaction(data_dir: &Path, args: &TransactionArgs) -> Result<LarCommandOutput> {
    if args.output.exists() && !args.force {
        bail!(
            "transaction output already exists: {} (use --force to replace it)",
            args.output.display()
        );
    }
    if let Some(source) = &args.archive {
        preflight_archive(source)?;
        if source == &args.output {
            bail!("transaction output must differ from its source archive");
        }
    }
    let parent = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = args
        .output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("transaction");
    let temporary = parent.join(format!(
        ".{name}.{}.lar-transaction.tmp",
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<LarTransactionExportReport> {
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary)?;
        let report = if let Some(archive) = &args.archive {
            let file = fs::File::open(archive)
                .with_context(|| format!("opening {}", archive.display()))?;
            let mut reader = ArchiveReader::open(BufReader::new(file), Limits::default())
                .map_err(anyhow::Error::new)
                .with_context(|| format!("reading {}", archive.display()))?;
            if !reader.is_sealed() {
                bail!("transaction source archive must be sealed");
            }
            let mut buffered = BufWriter::new(&mut output);
            let report =
                alex_store::write_archive_transaction(&mut reader, &args.trace_id, &mut buffered)?;
            buffered.flush()?;
            report
        } else {
            let store =
                Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
            let direct = {
                let mut buffered = BufWriter::new(&mut output);
                let report = store.write_lar_transaction(&args.trace_id, &mut buffered)?;
                buffered.flush()?;
                report
            };
            if let Some(report) = direct {
                report
            } else {
                let mut buffered = BufWriter::new(&mut output);
                let report = store
                    .write_legacy_transaction(&args.trace_id, &mut buffered)?
                    .with_context(|| format!("trace {} was not found", args.trace_id))?;
                buffered.flush()?;
                report
            }
        };
        output.sync_all()?;
        drop(output);
        validate_transaction_sequence(&temporary, Some(&args.trace_id))?;
        publish_export_temp(&temporary, &args.output, args.force)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(report)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    let report = result?;
    Ok(LarCommandOutput {
        human: format!(
            "exported {} exchange(s)/{} ordered stage(s) for trace {} to {} as {} v{} ({} artifact byte(s), fidelity {}, verified)",
            report.exchanges,
            report.stages,
            report.trace_id,
            args.output.display(),
            report.format,
            report.version,
            report.artifact_bytes,
            report.fidelity,
        ),
        json: serde_json::json!({
            "output": args.output,
            "report": report,
            "verified": true,
        }),
        raw_body: None,
    })
}

const MAX_TRANSACTION_JSON_RECORD_BYTES: usize = 32 * 1024 * 1024;

fn visit_transaction_records(
    path: &Path,
    mut visitor: impl FnMut(Value) -> Result<()>,
) -> Result<()> {
    let file =
        fs::File::open(path).with_context(|| format!("opening transaction {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut record = Vec::new();
    let mut inside_record = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let consumed = available.len();
        for &byte in available {
            if byte == 0x1e {
                if inside_record {
                    visitor(parse_transaction_json_record(&record)?)?;
                    record.clear();
                }
                inside_record = true;
                continue;
            }
            if !inside_record {
                if !byte.is_ascii_whitespace() {
                    bail!("transaction bytes precede the first RFC 7464 record separator");
                }
                continue;
            }
            if record.len() >= MAX_TRANSACTION_JSON_RECORD_BYTES {
                bail!(
                    "transaction JSON record exceeds {} byte limit",
                    MAX_TRANSACTION_JSON_RECORD_BYTES
                );
            }
            record.push(byte);
        }
        reader.consume(consumed);
    }
    if inside_record {
        visitor(parse_transaction_json_record(&record)?)?;
    }
    Ok(())
}

fn parse_transaction_json_record(record: &[u8]) -> Result<Value> {
    let start = record
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .context("transaction contains an empty RFC 7464 record")?;
    let end = record
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .expect("a non-whitespace record byte was found");
    serde_json::from_slice(&record[start..=end]).context("parsing transaction JSON record")
}

fn validate_transaction_sequence(path: &Path, expected_trace_id: Option<&str>) -> Result<()> {
    let mut active_artifact: Option<(String, u64, blake3::Hasher, u64, String)> = None;
    let mut artifact_ids = BTreeSet::<String>::new();
    let mut record_count = 0u64;
    let mut complete = false;
    visit_transaction_records(path, |record| {
        if record_count == 0 {
            if record["type"] != "format"
                || record["format"] != LAR_TRANSACTION_FORMAT
                || record["version"] != LAR_TRANSACTION_VERSION
            {
                bail!("unsupported transaction format or version");
            }
            if expected_trace_id.is_some_and(|expected| record["trace_id"] != expected) {
                bail!("transaction trace ID does not match the requested trace");
            }
        }
        if complete {
            bail!("transaction contains records after its complete marker");
        }
        match record["type"].as_str() {
            Some("format") if record_count != 0 => {
                bail!("transaction contains a repeated format record");
            }
            Some("artifact_start") => {
                if active_artifact.is_some() {
                    bail!("transaction artifacts overlap");
                }
                let content_id = record["content_id"]
                    .as_str()
                    .context("artifact has no content ID")?
                    .to_owned();
                if !artifact_ids.insert(content_id.clone()) {
                    bail!("transaction emits artifact {content_id} more than once");
                }
                active_artifact = Some((
                    content_id,
                    0,
                    blake3::Hasher::new(),
                    record["total_length"]
                        .as_u64()
                        .context("artifact has no total length")?,
                    record["whole_body_hash"]
                        .as_str()
                        .context("artifact has no whole-body hash")?
                        .into(),
                ));
            }
            Some("artifact_bytes") => {
                let (id, length, hasher, _, _) = active_artifact
                    .as_mut()
                    .context("artifact bytes appear outside an artifact")?;
                if record["content_id"].as_str() != Some(id) {
                    bail!("artifact byte content ID changed mid-artifact");
                }
                if record["logical_offset"].as_u64() != Some(*length) {
                    bail!("artifact bytes are not contiguous");
                }
                let bytes = base64::engine::general_purpose::STANDARD.decode(
                    record["data_base64"]
                        .as_str()
                        .context("artifact byte record has no base64 data")?,
                )?;
                if bytes.len() > alex_store::LAR_TRANSACTION_ARTIFACT_PIECE_BYTES {
                    bail!("transaction artifact byte record exceeds the bounded piece size");
                }
                *length = length.saturating_add(bytes.len() as u64);
                hasher.update(&bytes);
            }
            Some("artifact_end") => {
                let (id, length, hasher, expected_length, expected_hash) =
                    active_artifact
                        .take()
                        .context("artifact end appears without a start")?;
                if record["content_id"].as_str() != Some(&id)
                    || record["total_length"].as_u64() != Some(length)
                    || length != expected_length
                    || hasher.finalize().to_hex().as_str() != expected_hash
                    || record["verified"] != true
                {
                    bail!("artifact end record does not match reconstructed bytes");
                }
            }
            Some("end") => {
                if record["complete"] != true {
                    bail!("transaction end record is not complete");
                }
                complete = true;
            }
            _ => {}
        }
        record_count = record_count.saturating_add(1);
        Ok(())
    })?;
    if record_count == 0 {
        bail!("transaction is empty");
    }
    if !complete {
        bail!("transaction has no complete end record");
    }
    if active_artifact.is_some() {
        bail!("transaction ended inside an artifact");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct TransactionReplayEvent {
    byte_offset: u64,
    byte_length: u64,
    delta_ns: u64,
}

#[derive(Clone, Debug)]
struct TransactionReplayStream {
    stage_id: String,
    body_content_id: String,
    events: Vec<TransactionReplayEvent>,
}

fn replay_transaction(args: &TransactionReplayArgs) -> Result<()> {
    validate_transaction_sequence(&args.input, None)?;
    let Some(path) = &args.output else {
        return replay_transaction_to(args, &mut std::io::stdout().lock());
    };
    if path == &args.input {
        bail!("transaction replay output must differ from its input");
    }
    if path.exists() && !args.force {
        bail!(
            "transaction replay output already exists: {} (use --force to replace it)",
            path.display()
        );
    }
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("transaction-replay");
    let temporary = parent.join(format!(
        ".{name}.{}.lar-transaction-replay.tmp",
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary)?;
        {
            let mut output = BufWriter::new(&mut file);
            replay_transaction_to(args, &mut output)?;
            output.flush()?;
        }
        file.sync_all()?;
        drop(file);
        publish_export_temp(&temporary, path, args.force)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn replay_transaction_to(args: &TransactionReplayArgs, output: &mut dyn Write) -> Result<()> {
    let mut candidates = Vec::<TransactionReplayStream>::new();
    let mut selected: Option<TransactionReplayStream> = None;
    let mut selected_artifact = false;
    let mut replayed = false;
    let mut event_index = 0usize;
    let mut event_written = 0u64;
    let mut previous_delta = 0u64;
    let mut saw_format = false;

    visit_transaction_records(&args.input, |record| {
        match record["type"].as_str() {
            Some("format") => {
                if saw_format
                    || record["format"] != LAR_TRANSACTION_FORMAT
                    || record["version"] != LAR_TRANSACTION_VERSION
                {
                    bail!("unsupported or repeated transaction format record");
                }
                saw_format = true;
            }
            Some("stream_index") => {
                if selected.is_some() {
                    bail!("stream index appeared after transaction artifacts began");
                }
                let event_field = if args.parsed {
                    "parsed_frames"
                } else {
                    "observed_reads"
                };
                let events = record[event_field]
                    .as_array()
                    .context("stream index event list is missing")?
                    .iter()
                    .map(|event| {
                        Ok(TransactionReplayEvent {
                            byte_offset: event["byte_offset"]
                                .as_u64()
                                .context("stream event has no byte offset")?,
                            byte_length: event["byte_length"]
                                .as_u64()
                                .context("stream event has no byte length")?,
                            delta_ns: event["delta_from_first_byte_ns"]
                                .as_u64()
                                .context("stream event has no timing delta")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                validate_transaction_replay_events(&events, args.parsed)?;
                candidates.push(TransactionReplayStream {
                    stage_id: record["stage_id"]
                        .as_str()
                        .context("stream index has no stage ID")?
                        .into(),
                    body_content_id: record["raw_body_content_id"]
                        .as_str()
                        .context("stream index has no raw body content ID")?
                        .into(),
                    events,
                });
            }
            Some("artifact_start") if selected.is_none() => {
                selected = Some(select_transaction_stream(&candidates, args)?);
                let stream = selected.as_ref().expect("selected above");
                if stream.events.is_empty() {
                    bail!(
                        "stream stage {} has no {} timing ranges",
                        stream.stage_id,
                        if args.parsed { "parsed" } else { "observed" }
                    );
                }
                selected_artifact =
                    record["content_id"].as_str() == Some(stream.body_content_id.as_str());
                if selected_artifact {
                    validate_transaction_replay_length(
                        stream,
                        record["total_length"]
                            .as_u64()
                            .context("stream artifact has no total length")?,
                        args.parsed,
                    )?;
                }
            }
            Some("artifact_start") => {
                let stream = selected.as_ref().expect("selection is initialized");
                selected_artifact =
                    record["content_id"].as_str() == Some(stream.body_content_id.as_str());
                if selected_artifact {
                    validate_transaction_replay_length(
                        stream,
                        record["total_length"]
                            .as_u64()
                            .context("stream artifact has no total length")?,
                        args.parsed,
                    )?;
                }
            }
            Some("artifact_bytes") if selected_artifact => {
                let stream = selected.as_ref().expect("selected artifact has a stream");
                let chunk_offset = record["logical_offset"]
                    .as_u64()
                    .context("artifact bytes have no logical offset")?;
                let bytes = base64::engine::general_purpose::STANDARD.decode(
                    record["data_base64"]
                        .as_str()
                        .context("artifact bytes have no base64 data")?,
                )?;
                replay_transaction_piece(
                    output,
                    &stream.events,
                    &mut event_index,
                    &mut event_written,
                    &mut previous_delta,
                    args.speed,
                    chunk_offset,
                    &bytes,
                )?;
            }
            Some("artifact_end") if selected_artifact => {
                let stream = selected.as_ref().expect("selected artifact has a stream");
                if event_index != stream.events.len() || event_written != 0 {
                    bail!("selected stream ranges were not fully reconstructed");
                }
                selected_artifact = false;
                replayed = true;
            }
            _ => {}
        }
        Ok(())
    })?;
    if !saw_format {
        bail!("transaction has no format record");
    }
    if selected.is_none() {
        let _ = select_transaction_stream(&candidates, args)?;
        bail!("transaction has no artifact bytes for the selected stream");
    }
    if !replayed {
        bail!("selected stream body was not present in the transaction");
    }
    output.flush()?;
    Ok(())
}

fn validate_transaction_replay_events(
    events: &[TransactionReplayEvent],
    parsed: bool,
) -> Result<()> {
    let mut previous_end = 0_u64;
    let mut previous_delta = 0_u64;
    for event in events {
        if event.byte_length == 0 {
            bail!("transaction stream event has zero length");
        }
        if (parsed && event.byte_offset < previous_end)
            || (!parsed && event.byte_offset != previous_end)
        {
            bail!(
                "transaction stream events are {}",
                if parsed {
                    "overlapping or out of order"
                } else {
                    "overlapping or non-contiguous"
                }
            );
        }
        if event.delta_ns < previous_delta {
            bail!("transaction stream event timing is not monotonic");
        }
        previous_end = event
            .byte_offset
            .checked_add(event.byte_length)
            .context("transaction stream event offset overflow")?;
        previous_delta = event.delta_ns;
    }
    Ok(())
}

fn validate_transaction_replay_length(
    stream: &TransactionReplayStream,
    artifact_length: u64,
    parsed: bool,
) -> Result<()> {
    let covered = stream
        .events
        .last()
        .map(|event| event.byte_offset.saturating_add(event.byte_length))
        .unwrap_or_default();
    if (parsed && covered > artifact_length) || (!parsed && covered != artifact_length) {
        bail!(
            "stream stage {} timing ranges {} the {artifact_length}-byte artifact (range end {covered})",
            stream.stage_id,
            if parsed { "exceed" } else { "do not cover" },
        );
    }
    Ok(())
}

fn select_transaction_stream(
    candidates: &[TransactionReplayStream],
    args: &TransactionReplayArgs,
) -> Result<TransactionReplayStream> {
    let matching = candidates
        .iter()
        .filter(|stream| {
            args.stage_id
                .as_deref()
                .is_none_or(|selected| stream.stage_id == selected)
        })
        .cloned()
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [] if args.stage_id.is_some() => bail!(
            "stage {} is not a captured stream stage in this transaction",
            args.stage_id.as_deref().unwrap_or_default()
        ),
        [] => bail!("transaction has no captured stream"),
        [only] => Ok(only.clone()),
        many => bail!(
            "transaction has multiple captured streams; use --stage-id with one of: {}",
            many.iter()
                .map(|stream| stream.stage_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn replay_transaction_piece(
    output: &mut dyn Write,
    events: &[TransactionReplayEvent],
    event_index: &mut usize,
    event_written: &mut u64,
    previous_delta: &mut u64,
    speed: LarReplaySpeed,
    chunk_offset: u64,
    bytes: &[u8],
) -> Result<()> {
    let chunk_end = chunk_offset
        .checked_add(bytes.len() as u64)
        .context("transaction artifact chunk offset overflow")?;
    while *event_index < events.len() {
        let event = &events[*event_index];
        let event_end = event
            .byte_offset
            .checked_add(event.byte_length)
            .context("transaction stream event offset overflow")?;
        if event_end <= chunk_offset {
            if *event_written != event.byte_length {
                bail!("transaction stream event has a gap in artifact bytes");
            }
            *event_index += 1;
            *event_written = 0;
            continue;
        }
        if event.byte_offset >= chunk_end {
            break;
        }
        let overlap_start = event.byte_offset.max(chunk_offset);
        let overlap_end = event_end.min(chunk_end);
        if overlap_start < overlap_end {
            if *event_written == 0 {
                let delay = event.delta_ns.saturating_sub(*previous_delta);
                if let Some(duration) = transaction_replay_delay(speed, delay) {
                    std::thread::sleep(duration);
                }
                *previous_delta = event.delta_ns;
            }
            let start = usize::try_from(overlap_start - chunk_offset)
                .context("transaction replay slice start exceeds address space")?;
            let end = usize::try_from(overlap_end - chunk_offset)
                .context("transaction replay slice end exceeds address space")?;
            output.write_all(&bytes[start..end])?;
            *event_written = event_written.saturating_add(overlap_end - overlap_start);
        }
        if overlap_end == event_end {
            if *event_written != event.byte_length {
                bail!("transaction stream event reconstruction length mismatch");
            }
            *event_index += 1;
            *event_written = 0;
        } else {
            break;
        }
    }
    Ok(())
}

fn transaction_replay_delay(speed: LarReplaySpeed, delta_ns: u64) -> Option<std::time::Duration> {
    let scaled = match speed {
        LarReplaySpeed::Instant => return None,
        LarReplaySpeed::Realtime => delta_ns,
        LarReplaySpeed::Quarter => delta_ns.saturating_mul(4),
        LarReplaySpeed::Half => delta_ns.saturating_mul(2),
        LarReplaySpeed::Double => delta_ns / 2,
        LarReplaySpeed::Quadruple => delta_ns / 4,
    };
    Some(std::time::Duration::from_nanos(scaled))
}

fn replay_stream(args: &ReplayArgs) -> Result<()> {
    let file = fs::File::open(&args.archive)
        .with_context(|| format!("opening {}", args.archive.display()))?;
    let mut reader = ArchiveReader::open(std::io::BufReader::new(file), Limits::default())
        .with_context(|| format!("reading {}", args.archive.display()))?;
    let exchange = reader
        .exchange_by_trace(args.trace_id.as_bytes())
        .cloned()
        .with_context(|| {
            format!(
                "trace {} is not present in {}",
                args.trace_id,
                args.archive.display()
            )
        })?;
    let candidates = exchange
        .data
        .stages
        .iter()
        .filter_map(|id| reader.stage(id))
        .filter(|stage| stage.data.stream_index_ref.is_some())
        .filter(|stage| {
            args.stage_id
                .as_ref()
                .is_none_or(|selected| stage.id.to_string() == *selected)
        })
        .map(|stage| (stage.id, stage.data.stream_index_ref.expect("filtered")))
        .collect::<Vec<_>>();
    let (stage_id, stream_id) = match candidates.as_slice() {
        [] if args.stage_id.is_some() => bail!(
            "stage {} is not a captured stream stage in trace {}",
            args.stage_id.as_deref().unwrap_or_default(),
            args.trace_id
        ),
        [] => bail!("trace {} has no captured stream", args.trace_id),
        [only] => *only,
        many => {
            let choices = many
                .iter()
                .map(|(id, _)| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "trace {} has multiple captured streams; use --stage-id with one of: {}",
                args.trace_id,
                choices
            )
        }
    };
    let source = if args.parsed {
        StreamReplaySource::ParsedFrames
    } else {
        StreamReplaySource::ObservedReads
    };
    let replay = reader
        .read_stream_replay(&stream_id, source, args.speed.timing())
        .with_context(|| {
            format!(
                "preparing stream {} from stage {} in trace {}",
                stream_id, stage_id, args.trace_id
            )
        })?;
    if args.parsed && replay.events().is_empty() {
        bail!(
            "stream {} has no parsed SSE/NDJSON frames; replay raw observed reads instead",
            stream_id
        );
    }

    if let Some(path) = &args.output {
        let mut options = fs::OpenOptions::new();
        options.write(true);
        if args.force {
            options.create(true).truncate(true);
        } else {
            options.create_new(true);
        }
        let output = options
            .open(path)
            .with_context(|| format!("creating {}", path.display()))?;
        replay
            .play_to_realtime(std::io::BufWriter::new(output))
            .with_context(|| format!("replaying stream to {}", path.display()))?;
    } else {
        let stdout = std::io::stdout();
        replay
            .play_to_realtime(stdout.lock())
            .context("replaying stream to stdout")?;
    }
    Ok(())
}

fn import_archive(data_dir: &Path, args: &ImportArgs) -> Result<LarCommandOutput> {
    let format = match args.format {
        LarImportFormat::Auto => detect_import_format(&args.source)?,
        explicit => explicit,
    };
    match format {
        LarImportFormat::Lar => {
            let store =
                Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
            let report = store
                .import_sealed_lar_archive(&args.source, &LarStandaloneImportOptions::default())
                .with_context(|| {
                    format!("importing standalone archive {}", args.source.display())
                })?;
            let mut json = serde_json::to_value(&report)?;
            json.as_object_mut()
                .expect("standalone report serializes as an object")
                .insert("format".into(), Value::String("lar".into()));
            Ok(LarCommandOutput {
                human: format!(
                    "attached {}: {} exchange(s), {} manifest(s), {} chunk(s), {} reused manifest(s), {} reused chunk(s){}",
                    report.catalog_path,
                    report.exchanges,
                    report.manifests,
                    report.chunks,
                    report.manifests_reused,
                    report.chunks_reused,
                    if report.relocated {
                        "; validated archive relocation"
                    } else if report.already_attached {
                        "; already attached (catalog verified)"
                    } else {
                        ""
                    },
                ),
                json,
                raw_body: None,
            })
        }
        LarImportFormat::Jsonl => {
            let store = Store::open_with_lar_body_store(
                data_dir.to_path_buf(),
                LarBodyStoreConfig {
                    mode: LarBodyStoreMode::LarWithFallback,
                    ..LarBodyStoreConfig::default()
                },
            )
            .context("opening the Alex LAR body store")?;
            let input = fs::File::open(&args.source)
                .with_context(|| format!("opening JSONL import {}", args.source.display()))?;
            let report = store
                .import_lar_jsonl(input, &LarJsonlImportOptions::default())
                .with_context(|| format!("importing Alex JSONL {}", args.source.display()))?;
            Ok(LarCommandOutput {
                human: format!(
                    "imported Alex JSONL {}: {} trace(s) imported, {} idempotently skipped, {} body artifact(s), {} decoded byte(s); source fidelity loss: {}",
                    args.source.display(),
                    report.traces_imported,
                    report.traces_skipped,
                    report.bodies_written,
                    report.decoded_body_bytes,
                    report.source_loss_report.join("; "),
                ),
                json: serde_json::json!({
                    "format": "jsonl",
                    "source": args.source,
                    "report": report,
                }),
                raw_body: None,
            })
        }
        LarImportFormat::Auto => unreachable!("auto import format was resolved"),
    }
}

fn detach_archive(data_dir: &Path, args: &DetachArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let report = store
        .detach_lar_archive(&args.file_uuid)
        .with_context(|| format!("detaching LAR archive {}", args.file_uuid))?;
    let human = if report.already_offline {
        format!(
            "LAR archive {} was already archived_offline (role {}, catalog path {}, bytes currently present: {}); detach does not move or delete files",
            report.file.file_uuid,
            report.file.role,
            report.file.catalog_path,
            report.file.exists,
        )
    } else {
        format!(
            "LAR archive {} marked archived_offline (role {}, catalog path {}, bytes currently present: {}); detach changed catalog state only and did not move or delete files",
            report.file.file_uuid,
            report.file.role,
            report.file.catalog_path,
            report.file.exists,
        )
    };
    Ok(LarCommandOutput {
        human,
        json: serde_json::to_value(report)?,
        raw_body: None,
    })
}

fn reattach_archive(data_dir: &Path, args: &ReattachArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let report = store
        .reattach_lar_archive(
            &args.file_uuid,
            &args.archive,
            &LarArchiveReattachOptions::default(),
        )
        .with_context(|| {
            format!(
                "reattaching LAR archive {} from {}",
                args.file_uuid,
                args.archive.display()
            )
        })?;
    let disposition = if report.relocated {
        "relocated"
    } else if report.already_attached {
        "already attached"
    } else {
        "reattached"
    };
    Ok(LarCommandOutput {
        human: format!(
            "LAR archive {} {disposition} at {} after sealed-file identity validation ({} bytes, blake3:{})",
            report.file_uuid,
            report.catalog_path,
            report.source_size,
            report.source_blake3,
        ),
        json: serde_json::to_value(report)?,
        raw_body: None,
    })
}

fn detect_import_format(path: &Path) -> Result<LarImportFormat> {
    let mut input = fs::File::open(path)
        .with_context(|| format!("opening import source {}", path.display()))?;
    let mut prefix = [0u8; 4096];
    let read = input
        .read(&mut prefix)
        .with_context(|| format!("reading import source {}", path.display()))?;
    let prefix = &prefix[..read];
    if prefix.starts_with(b"LAR1") {
        return Ok(LarImportFormat::Lar);
    }
    let first = prefix
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    if first == Some(b'{') {
        return Ok(LarImportFormat::Jsonl);
    }
    bail!(
        "could not auto-detect import format for {}; use --format lar or --format jsonl",
        path.display()
    )
}

fn gc_command(data_dir: &Path, command: &LarGcCommand) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let report = match command {
        LarGcCommand::Plan { .. } => store
            .plan_lar_gc()
            .context("planning LAR garbage collection")?,
        LarGcCommand::Apply { .. } => store
            .run_lar_gc(unix_time_ms()?)
            .context("applying LAR garbage collection")?,
        LarGcCommand::Resume { run_id, .. } => store
            .resume_lar_gc(run_id, unix_time_ms()?)
            .with_context(|| format!("resuming LAR garbage collection run {run_id}"))?,
    };
    let action = if report.dry_run { "plan" } else { "run" };
    Ok(LarCommandOutput {
        human: format!(
            "LAR GC {action} {}: {} reachable manifest(s), {} reachable chunk(s), {} unreachable manifest(s), {} unreachable chunk(s), {} compressed garbage byte(s); {} manifest(s)/{} chunk(s) logically swept, {} physical byte(s) reclaimed",
            report.state,
            report.reachable_manifests,
            report.reachable_chunks,
            report.unreachable_manifests,
            report.unreachable_chunks,
            report.garbage_compressed_bytes,
            report.swept_manifests,
            report.swept_chunks,
            report.physical_bytes_reclaimed,
        ),
        json: serde_json::to_value(report)?,
        raw_body: None,
    })
}

fn repack_config(args: &RepackSelectionArgs) -> LarRepackConfig {
    LarRepackConfig {
        min_garbage_bytes: args.min_garbage_bytes,
        min_garbage_ratio: args.min_garbage_ratio,
    }
}

fn repack_command(data_dir: &Path, command: &LarRepackCommand) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    match command {
        LarRepackCommand::Plan(args) => {
            let config = repack_config(args);
            let candidates = store
                .plan_lar_repacks(&config)
                .context("planning LAR body-pack compaction")?;
            let garbage_bytes = candidates
                .iter()
                .map(|candidate| candidate.garbage_compressed_bytes)
                .sum::<u64>();
            Ok(LarCommandOutput {
                human: format!(
                    "LAR repack plan: {} eligible sealed pack(s), {} compressed garbage byte(s); no files or catalog locations changed",
                    candidates.len(), garbage_bytes
                ),
                json: serde_json::json!({
                    "kind": "lar_repack_plan",
                    "dry_run": true,
                    "min_garbage_bytes": config.min_garbage_bytes,
                    "min_garbage_ratio": config.min_garbage_ratio,
                    "candidate_count": candidates.len(),
                    "garbage_compressed_bytes": garbage_bytes,
                    "candidates": candidates,
                }),
                raw_body: None,
            })
        }
        LarRepackCommand::Apply(args) => {
            let config = repack_config(args);
            let report = store
                .run_lar_repack(&config, unix_time_ms()?)
                .context("applying LAR body-pack compaction")?;
            match report {
                Some(report) => Ok(LarCommandOutput {
                    human: format!(
                        "LAR repack {} completed: {} reachable chunk(s), {} garbage chunk(s), {} logical byte(s) reclaimed; source remains recoverable at {} ({} physical byte(s) reclaimed)",
                        report.run_id,
                        report.reachable_chunks,
                        report.garbage_chunks,
                        report.logical_bytes_reclaimed,
                        report.quarantine_path.display(),
                        report.physical_bytes_reclaimed,
                    ),
                    json: serde_json::json!({
                        "kind": "lar_repack_run",
                        "candidate_found": true,
                        "report": report,
                    }),
                    raw_body: None,
                }),
                None => Ok(LarCommandOutput {
                    human: "LAR repack: no sealed body pack meets the configured garbage thresholds; no changes made".into(),
                    json: serde_json::json!({
                        "kind": "lar_repack_run",
                        "candidate_found": false,
                        "min_garbage_bytes": config.min_garbage_bytes,
                        "min_garbage_ratio": config.min_garbage_ratio,
                    }),
                    raw_body: None,
                }),
            }
        }
        LarRepackCommand::Resume { run_id, .. } => {
            let report = store
                .resume_lar_repack(run_id, unix_time_ms()?)
                .with_context(|| format!("resuming LAR repack run {run_id}"))?;
            Ok(LarCommandOutput {
                human: format!(
                    "LAR repack {} is {}: {} reachable chunk(s), {} garbage chunk(s), {} logical byte(s) reclaimed, {} physical byte(s) reclaimed",
                    report.run_id,
                    report.state,
                    report.reachable_chunks,
                    report.garbage_chunks,
                    report.logical_bytes_reclaimed,
                    report.physical_bytes_reclaimed,
                ),
                json: serde_json::json!({
                    "kind": "lar_repack_run",
                    "candidate_found": true,
                    "report": report,
                }),
                raw_body: None,
            })
        }
    }
}

fn migration_command(data_dir: &Path, command: &LarMigrationCommand) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    match command {
        LarMigrationCommand::Status { .. } => migration_status(&store, None),
        LarMigrationCommand::Pause { .. } => set_migration_paused(&store, true),
        LarMigrationCommand::Resume { .. } => set_migration_paused(&store, false),
        LarMigrationCommand::Verify { .. } => migration_verification(&store),
    }
}

fn migration_verification(store: &Store) -> Result<LarCommandOutput> {
    let report = store
        .verify_lar_migration()
        .context("verifying the live LAR migration")?;
    let human = if report.valid {
        format!(
            "LAR migration verification passed: {} file(s), {} manifest(s), {} artifact pointer(s), {} migrated item(s), {} reconstructed bytes; {} checksum {}",
            report.files_checked,
            report.manifests_checked,
            report.artifacts_checked,
            report.migrated_items_checked,
            report.bytes_reconstructed,
            report.checksum_algorithm,
            report.report_checksum,
        )
    } else {
        let details = report
            .issues
            .iter()
            .take(10)
            .map(|issue| {
                format!(
                    "- {} {} [{}]: {}",
                    issue.scope, issue.id, issue.kind, issue.detail
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "LAR migration verification failed with {} issue(s) after checking {} file(s), {} manifest(s), and {} artifact pointer(s); {} checksum {}\n{}",
            report.issues.len(),
            report.files_checked,
            report.manifests_checked,
            report.artifacts_checked,
            report.checksum_algorithm,
            report.report_checksum,
            details,
        )
    };
    let mut json = serde_json::to_value(&report)?;
    json["kind"] = serde_json::json!("migration_verification");
    Ok(LarCommandOutput {
        human,
        json,
        raw_body: None,
    })
}

#[derive(Debug, Clone, Serialize)]
struct LegacyCleanupCandidate {
    source: PathBuf,
    relative_path: PathBuf,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct LegacyCleanupReport {
    mode: &'static str,
    eligible: bool,
    migration_jobs: usize,
    verification_valid: bool,
    candidate_files: usize,
    candidate_bytes: u64,
    already_absent_files: usize,
    ineligible_reasons: Vec<String>,
    quarantine_dir: Option<PathBuf>,
    moved_files: usize,
    moved_bytes: u64,
    recoverable: bool,
    alex_version: &'static str,
    timestamp_ms: i64,
}

fn cleanup_legacy(data_dir: &Path, args: &CleanupArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let jobs = store
        .list_lar_migration_jobs()
        .context("reading LAR migration jobs")?;
    let verification = store
        .verify_lar_migration()
        .context("running the required full LAR verification pass")?;
    let rows = store
        .export_trace_backup_rows()
        .context("inventorying legacy trace and tool body paths")?;
    let timestamp_ms = unix_time_ms()?;
    let mut reasons = Vec::new();
    if jobs.is_empty() {
        reasons.push("no LAR migration job has completed".to_owned());
    }
    for job in &jobs {
        if job.state != "complete" {
            reasons.push(format!(
                "migration job {} is {:?}, not complete",
                job.job_id, job.state
            ));
        }
        if job.pending_count != 0 || job.failed_count != 0 {
            reasons.push(format!(
                "migration job {} still has {} pending and {} failed item(s)",
                job.job_id, job.pending_count, job.failed_count
            ));
        }
    }
    if !verification.valid {
        reasons.push(format!(
            "full LAR verification reported {} issue(s)",
            verification.issues.len()
        ));
    }

    let body_root = data_dir.join("bodies");
    let canonical_body_root = fs::canonicalize(&body_root).ok();
    let mut referenced_paths = Vec::new();
    for row in &rows.traces {
        let Some(owner_id) = row.get("id").and_then(Value::as_str) else {
            reasons.push("a trace row has no string id".to_owned());
            continue;
        };
        for (column, artifact_kind) in [
            ("req_body_path", "client_request"),
            ("upstream_req_body_path", "upstream_request"),
            ("resp_body_path", "client_response"),
        ] {
            if let Some(path) = row.get(column).and_then(Value::as_str) {
                referenced_paths.push(("trace", owner_id, artifact_kind, path));
            }
        }
    }
    for row in &rows.tool_calls {
        let Some(owner_id) = row.get("id").and_then(Value::as_str) else {
            reasons.push("a tool-call row has no string id".to_owned());
            continue;
        };
        for (column, artifact_kind) in [
            ("args_body_path", "tool_arguments"),
            ("result_body_path", "tool_result"),
        ] {
            if let Some(path) = row.get(column).and_then(Value::as_str) {
                referenced_paths.push(("tool_call", owner_id, artifact_kind, path));
            }
        }
    }

    let mut unique = BTreeSet::new();
    let mut absent = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut already_absent_files = 0usize;
    for (owner_kind, owner_id, artifact_kind, stored_path) in referenced_paths {
        if !matches!(
            store.lar_artifact_location(owner_kind, owner_id, artifact_kind, None)?,
            Some(LarArtifactLocation::Lar { .. })
        ) {
            reasons.push(format!(
                "{owner_kind} {owner_id} artifact {artifact_kind} has no validated LAR pointer"
            ));
            continue;
        }
        let path = resolve_legacy_path(data_dir, stored_path);
        let canonical = match fs::canonicalize(&path) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if absent.insert(path) {
                    already_absent_files += 1;
                }
                continue;
            }
            Err(error) => {
                reasons.push(format!("cannot resolve {}: {error}", path.display()));
                continue;
            }
        };
        if !unique.insert(canonical.clone()) {
            continue;
        }
        let Some(root) = canonical_body_root.as_deref() else {
            reasons.push(format!(
                "legacy body root {} is unavailable",
                body_root.display()
            ));
            continue;
        };
        let Ok(relative_path) = canonical.strip_prefix(root) else {
            reasons.push(format!(
                "refusing to clean body outside {}: {}",
                root.display(),
                canonical.display()
            ));
            continue;
        };
        let metadata = fs::metadata(&canonical)
            .with_context(|| format!("reading legacy body metadata for {}", canonical.display()))?;
        if !metadata.is_file() {
            reasons.push(format!(
                "legacy body is not a regular file: {}",
                canonical.display()
            ));
            continue;
        }
        let relative_path = relative_path.to_path_buf();
        candidates.push(LegacyCleanupCandidate {
            source: canonical,
            relative_path,
            bytes: metadata.len(),
        });
    }
    candidates.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    reasons.sort();
    reasons.dedup();
    let candidate_bytes = candidates.iter().map(|value| value.bytes).sum::<u64>();
    let eligible = reasons.is_empty();
    if args.apply && !eligible {
        bail!(
            "legacy cleanup is not eligible: {}; no files were moved",
            reasons.join("; ")
        );
    }

    let mut report = LegacyCleanupReport {
        mode: if args.apply { "apply" } else { "dry-run" },
        eligible,
        migration_jobs: jobs.len(),
        verification_valid: verification.valid,
        candidate_files: candidates.len(),
        candidate_bytes,
        already_absent_files,
        ineligible_reasons: reasons,
        quarantine_dir: None,
        moved_files: 0,
        moved_bytes: 0,
        recoverable: true,
        alex_version: env!("CARGO_PKG_VERSION"),
        timestamp_ms,
    };

    if args.apply && !candidates.is_empty() {
        let quarantine = data_dir
            .join("lar")
            .join("quarantine")
            .join(format!("legacy-{timestamp_ms}-{}", std::process::id()));
        fs::create_dir_all(&quarantine).with_context(|| {
            format!(
                "creating legacy cleanup quarantine {}",
                quarantine.display()
            )
        })?;
        report.quarantine_dir = Some(quarantine.clone());
        write_cleanup_audit(&quarantine, "cleanup-plan.json", &report)?;
        for candidate in &candidates {
            let destination = quarantine.join(&candidate.relative_path);
            if destination.exists() {
                bail!(
                    "quarantine destination already exists: {}; cleanup stopped after moving {} file(s)",
                    destination.display(),
                    report.moved_files
                );
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&candidate.source, &destination).with_context(|| {
                format!(
                    "moving verified legacy body {} to recoverable quarantine {}",
                    candidate.source.display(),
                    destination.display()
                )
            })?;
            report.moved_files += 1;
            report.moved_bytes = report.moved_bytes.saturating_add(candidate.bytes);
        }
        write_cleanup_audit(&quarantine, "cleanup-result.json", &report)?;
    }

    let human = if args.apply {
        format!(
            "legacy cleanup quarantined {} verified file(s) ({} bytes){}; files remain recoverable and no LAR data was removed",
            report.moved_files,
            report.moved_bytes,
            report
                .quarantine_dir
                .as_ref()
                .map(|path| format!(" at {}", path.display()))
                .unwrap_or_default(),
        )
    } else {
        format!(
            "legacy cleanup dry-run: {} file(s), {} bytes eligible; {} already absent; verification {}; no files were moved{}",
            report.candidate_files,
            report.candidate_bytes,
            report.already_absent_files,
            if report.verification_valid { "passed" } else { "failed" },
            if report.eligible {
                String::new()
            } else {
                format!("; blocked: {}", report.ineligible_reasons.join("; "))
            }
        )
    };
    Ok(LarCommandOutput {
        human,
        json: serde_json::to_value(report)?,
        raw_body: None,
    })
}

fn write_cleanup_audit(quarantine: &Path, name: &str, report: &LegacyCleanupReport) -> Result<()> {
    let path = quarantine.join(name);
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("creating cleanup audit {}", path.display()))?;
    file.write_all(&serde_json::to_vec_pretty(report)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct MigrationJobOutput<'a> {
    job_id: &'a str,
    state: &'a str,
    discovered: u64,
    pending: u64,
    migrated: u64,
    skipped: u64,
    failed: u64,
    bytes_read: u64,
    unique_bytes: u64,
    deduplicated_bytes: u64,
    last_committed_cursor: Option<&'a str>,
    last_error: Option<&'a str>,
    lease_owner: Option<&'a str>,
    lease_expires_at_ms: Option<i64>,
    paused: bool,
    running: bool,
}

impl<'a> From<&'a LarMigrationJob> for MigrationJobOutput<'a> {
    fn from(job: &'a LarMigrationJob) -> Self {
        Self {
            job_id: &job.job_id,
            state: &job.state,
            discovered: job.discovered_count,
            pending: job.pending_count,
            migrated: job.migrated_count,
            skipped: job.skipped_count,
            failed: job.failed_count,
            bytes_read: job.bytes_read,
            unique_bytes: job.unique_bytes_written,
            deduplicated_bytes: job.bytes_deduplicated,
            last_committed_cursor: job.last_committed_cursor.as_deref(),
            last_error: job.last_error.as_deref(),
            lease_owner: job.lease_owner.as_deref(),
            lease_expires_at_ms: job.lease_expires_at_ms,
            paused: job.state == "paused",
            running: job.state == "running",
        }
    }
}

fn migration_status(store: &Store, action: Option<&str>) -> Result<LarCommandOutput> {
    let jobs = store
        .list_lar_migration_jobs()
        .context("reading LAR migration jobs")?;
    let incomplete_jobs = jobs.iter().filter(|job| job.state != "complete").count();
    let json = serde_json::json!({
        "jobs": jobs.iter().map(MigrationJobOutput::from).collect::<Vec<_>>(),
        "total_jobs": jobs.len(),
        "incomplete_jobs": incomplete_jobs,
    });

    let mut lines = Vec::with_capacity(jobs.len() + 1);
    match (action, jobs.is_empty()) {
        (Some(action), _) => lines.push(format!(
            "LAR migration {action}; {} job(s), {incomplete_jobs} incomplete",
            jobs.len()
        )),
        (None, true) => lines.push("LAR migration: no migration jobs have started".to_owned()),
        (None, false) => lines.push(format!(
            "LAR migration: {} job(s), {incomplete_jobs} incomplete",
            jobs.len()
        )),
    }
    for job in &jobs {
        let mut line = format!(
            "- {} [{}]: {} discovered, {} migrated, {} pending, {} skipped, {} failed; {} bytes read, {} unique bytes, {} deduplicated bytes",
            job.job_id,
            job.state,
            job.discovered_count,
            job.migrated_count,
            job.pending_count,
            job.skipped_count,
            job.failed_count,
            job.bytes_read,
            job.unique_bytes_written,
            job.bytes_deduplicated,
        );
        if let Some(owner) = &job.lease_owner {
            line.push_str(&format!(
                "; lease {owner} until {}",
                job.lease_expires_at_ms
                    .map_or_else(|| "unknown".to_owned(), |expiry| expiry.to_string())
            ));
        }
        if let Some(error) = &job.last_error {
            line.push_str(&format!("; last error: {error}"));
        }
        lines.push(line);
    }

    Ok(LarCommandOutput {
        human: lines.join("\n"),
        json,
        raw_body: None,
    })
}

fn select_incomplete_migration_job(jobs: &[LarMigrationJob]) -> Result<&LarMigrationJob> {
    let incomplete = jobs
        .iter()
        .filter(|job| job.state != "complete")
        .collect::<Vec<_>>();
    match incomplete.as_slice() {
        [] => bail!(
            "no incomplete LAR migration job exists; start an import before changing migration state"
        ),
        [job] => Ok(*job),
        jobs => bail!(
            "{} incomplete LAR migration jobs exist ({}); resolve the older jobs before changing global migration state",
            jobs.len(),
            jobs.iter()
                .map(|job| job.job_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn set_migration_paused(store: &Store, paused: bool) -> Result<LarCommandOutput> {
    let jobs = store
        .list_lar_migration_jobs()
        .context("reading LAR migration jobs")?;
    let job = select_incomplete_migration_job(&jobs)?;
    let job_id = job.job_id.clone();
    let state = job.state.clone();
    let changed = store
        .set_lar_migration_paused(&job_id, paused, unix_time_ms()?)
        .with_context(|| {
            format!(
                "{} LAR migration job {job_id}",
                if paused { "pausing" } else { "resuming" }
            )
        })?;
    if !changed {
        bail!(
            "LAR migration job {job_id} cannot be {} from state {state:?}; {}",
            if paused { "paused" } else { "resumed" },
            if paused {
                "only pending, running, or failed jobs can be paused"
            } else {
                "only paused jobs can be resumed"
            }
        );
    }
    migration_status(store, Some(if paused { "paused" } else { "resumed" }))
}

fn unix_time_ms() -> Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    i64::try_from(elapsed.as_millis()).context("current Unix timestamp exceeds i64")
}

fn live_catalog_summary(data_dir: &Path, args: &ListArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    if let Some(trace_id) = &args.trace_id {
        let trace = store
            .get_trace(trace_id)?
            .with_context(|| format!("trace {trace_id} was not found"))?;
        return Ok(LarCommandOutput {
            human: format!("live trace {trace_id}"),
            json: serde_json::json!({"kind": "live_trace", "trace": trace}),
            raw_body: None,
        });
    }
    if let Some(session_id) = &args.session {
        let traces = store.session_traces(session_id, None)?;
        let total = traces.len();
        let traces = traces.into_iter().take(args.limit).collect::<Vec<_>>();
        return Ok(LarCommandOutput {
            human: format!(
                "live session {session_id}: {total} trace(s){}",
                if traces.len() < total {
                    format!("; showing {}", traces.len())
                } else {
                    String::new()
                }
            ),
            json: serde_json::json!({
                "kind": "live_session",
                "session_id": session_id,
                "trace_count": total,
                "limited": traces.len() < total,
                "traces": traces,
            }),
            raw_body: None,
        });
    }
    let schema_version = store
        .lar_catalog_schema_version()
        .context("reading the LAR catalog schema version")?;
    let jobs = store
        .list_lar_migration_jobs()
        .context("reading LAR migration jobs")?;
    let archive_files = store
        .lar_archive_file_statuses()
        .context("reading LAR archive file statuses")?;
    let listed_jobs = jobs.iter().take(args.limit).collect::<Vec<_>>();
    let listed_archive_files = archive_files.iter().take(args.limit).collect::<Vec<_>>();
    let incomplete_jobs = jobs.iter().filter(|job| job.state != "complete").count();
    let unavailable_archive_files = archive_files
        .iter()
        .filter(|file| file.availability.code() != "online")
        .count();
    let json = serde_json::json!({
        "kind": "live_catalog",
        "schema_version": schema_version,
        "archive_files": listed_archive_files,
        "archive_file_count": archive_files.len(),
        "unavailable_archive_files": unavailable_archive_files,
        "migration_jobs": listed_jobs
            .iter()
            .map(|job| MigrationJobOutput::from(*job))
            .collect::<Vec<_>>(),
        "migration_job_count": jobs.len(),
        "incomplete_migration_jobs": incomplete_jobs,
        "limited": listed_jobs.len() < jobs.len()
            || listed_archive_files.len() < archive_files.len(),
    });
    let human = format!(
        "live LAR catalog schema v{schema_version}: {} archive file(s), {unavailable_archive_files} unavailable; {} migration job(s), {incomplete_jobs} incomplete{}",
        archive_files.len(),
        jobs.len(),
        if listed_jobs.len() < jobs.len() || listed_archive_files.len() < archive_files.len() {
            format!("; each list limited to {}", args.limit)
        } else {
            String::new()
        }
    );
    Ok(LarCommandOutput {
        human,
        json,
        raw_body: None,
    })
}

fn parse_nonzero_usize(value: &str) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| format!("expected a positive integer, got {value:?}"))?;
    if value == 0 {
        return Err("value must be greater than zero".into());
    }
    Ok(value)
}

fn parse_ratio(value: &str) -> std::result::Result<f64, String> {
    let ratio = value
        .parse::<f64>()
        .map_err(|_| format!("expected a number between 0 and 1, got {value:?}"))?;
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        return Err(format!("ratio must be between 0 and 1, got {value:?}"));
    }
    Ok(ratio)
}

fn parse_lar_file_uuid(value: &str) -> std::result::Result<String, String> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "expected a 32-hex-digit LAR file UUID, got {value:?}"
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn preflight_archive(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("cannot open LAR archive {}", path.display()))?;
    if !metadata.is_file() {
        bail!("LAR archive is not a regular file: {}", path.display());
    }
    let mut file =
        fs::File::open(path).with_context(|| format!("opening LAR archive {}", path.display()))?;
    let mut magic = [0_u8; 4];
    use std::io::Read;
    file.read_exact(&mut magic)
        .with_context(|| format!("reading LAR magic from {}", path.display()))?;
    if &magic != b"LAR1" {
        bail!("not a LAR1 archive: {}", path.display());
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct ArchiveSummary {
    path: String,
    container_major: u16,
    container_minor: u16,
    file_uuid: String,
    file_role: String,
    record_count: usize,
    chunk_count: usize,
    manifest_count: usize,
    header_block_count: usize,
    recovery: &'static str,
    last_valid_offset: Option<u64>,
    truncated_tail_bytes: u64,
    verified_manifest_count: usize,
    verification_failures: Vec<ArchiveVerificationFailure>,
    manifest_ids: Vec<String>,
    limited: bool,
}

#[derive(Debug, Serialize)]
struct ArchiveVerificationFailure {
    kind: &'static str,
    manifest_id: Option<String>,
    detail: String,
}

fn open_archive_summary(
    path: &Path,
    verify_bodies: bool,
    manifest_limit: usize,
) -> Result<ArchiveSummary> {
    open_archive_summary_with_policy(path, verify_bodies, manifest_limit, false)
}

fn open_archive_summary_with_policy(
    path: &Path,
    verify_bodies: bool,
    manifest_limit: usize,
    keep_going: bool,
) -> Result<ArchiveSummary> {
    preflight_archive(path)?;
    let file =
        fs::File::open(path).with_context(|| format!("opening LAR archive {}", path.display()))?;
    let mut reader = ArchiveReader::open(file, Limits::default())
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("scanning LAR archive {}", path.display()))?;
    let mut manifest_ids = reader.manifest_ids().copied().collect::<Vec<_>>();
    manifest_ids.sort_by_key(ToString::to_string);
    let mut verified_manifest_count = 0;
    let mut verification_failures = Vec::new();
    if verify_bodies {
        for manifest_id in &manifest_ids {
            match reader.write_body(manifest_id, std::io::sink()) {
                Ok(_) => verified_manifest_count += 1,
                Err(error) if keep_going => {
                    verification_failures.push(ArchiveVerificationFailure {
                        kind: "manifest",
                        manifest_id: Some(manifest_id.to_string()),
                        detail: error.to_string(),
                    });
                }
                Err(error) => {
                    return Err(anyhow::anyhow!(error))
                        .with_context(|| format!("verifying manifest {manifest_id}"));
                }
            }
        }
    }
    let (recovery, last_valid_offset, truncated_tail_bytes) = match reader.recovery_status() {
        RecoveryStatus::Clean => ("clean", None, 0),
        RecoveryStatus::TruncatedTail {
            last_valid_offset,
            tail_bytes,
        } => ("truncated_tail", Some(last_valid_offset), tail_bytes),
        RecoveryStatus::CorruptIndexFallback {
            last_valid_offset,
            tail_bytes,
        } => (
            "corrupt_index_fallback",
            Some(last_valid_offset),
            tail_bytes,
        ),
    };
    let header = reader.header();
    let mut listed_ids = manifest_ids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    listed_ids.sort();
    listed_ids.truncate(manifest_limit);
    Ok(ArchiveSummary {
        path: path.display().to_string(),
        container_major: header.container_major,
        container_minor: header.container_minor,
        file_uuid: hex_bytes(&header.file_uuid),
        file_role: format!("{:?}", header.file_role),
        record_count: reader.record_count(),
        chunk_count: reader.chunk_count(),
        manifest_count: manifest_ids.len(),
        header_block_count: reader.header_block_count(),
        recovery,
        last_valid_offset,
        truncated_tail_bytes,
        verified_manifest_count,
        verification_failures,
        manifest_ids: listed_ids,
        limited: manifest_ids.len() > manifest_limit,
    })
}

fn verify_archive(path: &Path, keep_going: bool) -> Result<LarCommandOutput> {
    let mut summary = open_archive_summary_with_policy(path, true, usize::MAX, keep_going)?;
    if summary.recovery != "clean" {
        let detail = format!(
            "recovery state {} with {} tail bytes after offset {}; run `alex lar repair {} --output <new-file>`",
            summary.recovery,
            summary.truncated_tail_bytes,
            summary.last_valid_offset.unwrap_or_default(),
            path.display(),
        );
        if !keep_going {
            bail!("LAR archive {} has {detail}", path.display());
        }
        summary
            .verification_failures
            .push(ArchiveVerificationFailure {
                kind: "recovery",
                manifest_id: None,
                detail,
            });
    }
    if !summary.verification_failures.is_empty() {
        let details = summary
            .verification_failures
            .iter()
            .map(|failure| match &failure.manifest_id {
                Some(manifest_id) => {
                    format!("- {} {manifest_id}: {}", failure.kind, failure.detail)
                }
                None => format!("- {}: {}", failure.kind, failure.detail),
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "LAR archive {} failed verification with {} issue(s) after reconstructing {}/{} manifests:\n{}",
            path.display(),
            summary.verification_failures.len(),
            summary.verified_manifest_count,
            summary.manifest_count,
            details,
        );
    }
    let human = format!(
        "verified {}: {} records, {} unique chunks, {} manifests reconstructed, {} header blocks",
        path.display(),
        summary.record_count,
        summary.chunk_count,
        summary.verified_manifest_count,
        summary.header_block_count,
    );
    Ok(LarCommandOutput {
        human,
        json: serde_json::to_value(summary)?,
        raw_body: None,
    })
}

fn list_archive(path: &Path, args: &ListArgs) -> Result<LarCommandOutput> {
    let summary = open_archive_summary(path, false, args.limit)?;
    let file = fs::File::open(path)?;
    let reader =
        ArchiveReader::open(file, Limits::default()).map_err(|error| anyhow::anyhow!(error))?;
    let mut exchange_ids = if let Some(trace_id) = &args.trace_id {
        vec![
            reader
                .exchange_by_trace(trace_id.as_bytes())
                .with_context(|| format!("trace {trace_id} is not present in {}", path.display()))?
                .id,
        ]
    } else if let Some(session_id) = &args.session {
        reader
            .exchanges_for_session(session_id.as_bytes())
            .with_context(|| format!("session {session_id} is not present in {}", path.display()))?
            .to_vec()
    } else {
        reader.exchange_ids().copied().collect::<Vec<_>>()
    };
    exchange_ids.sort_by(|left, right| {
        let left = reader.exchange(left).expect("listed exchange must resolve");
        let right = reader
            .exchange(right)
            .expect("listed exchange must resolve");
        (left.data.capture_sequence, &left.data.trace_id)
            .cmp(&(right.data.capture_sequence, &right.data.trace_id))
    });
    let exchange_total = exchange_ids.len();
    exchange_ids.truncate(args.limit);
    let exchanges = exchange_ids
        .iter()
        .map(|id| archive_exchange_json(&reader, id))
        .collect::<Vec<_>>();
    let human = format!(
        "{} LAR {}.{} {}: {} records, {} chunks, {} manifests, {} header blocks, {} exchange(s); recovery {}{}",
        path.display(),
        summary.container_major,
        summary.container_minor,
        summary.file_role,
        summary.record_count,
        summary.chunk_count,
        summary.manifest_count,
        summary.header_block_count,
        exchange_total,
        summary.recovery,
        if summary.limited || exchanges.len() < exchange_total {
            format!(
                "; showing {} manifest IDs and {} exchanges",
                summary.manifest_ids.len(),
                exchanges.len()
            )
        } else {
            String::new()
        },
    );
    let mut json = serde_json::to_value(summary)?;
    json["exchange_total"] = serde_json::json!(exchange_total);
    json["exchanges"] = serde_json::json!(exchanges);
    Ok(LarCommandOutput {
        human,
        json,
        raw_body: None,
    })
}

fn archive_exchange_json<R: Read + std::io::Seek>(
    reader: &ArchiveReader<R>,
    id: &alex_lar::ExchangeId,
) -> Value {
    let exchange = reader.exchange(id).expect("listed exchange must resolve");
    let stages = exchange
        .data
        .stages
        .iter()
        .filter_map(|id| reader.stage(id))
        .map(|stage| {
            serde_json::json!({
                "id": stage.id.to_string(),
                "kind": format!("{:?}", stage.data.kind),
                "attempt": stage.data.attempt_number,
                "wall_time_ns": stage.data.wall_time_ns,
                "first_byte_delta_ns": stage.data.first_byte_delta_ns,
                "last_byte_delta_ns": stage.data.last_byte_delta_ns,
                "request_headers": stage.data.request_headers_ref.map(|id| id.to_string()),
                "request_body": stage.data.request_body_manifest_ref.map(|id| id.to_string()),
                "response_headers": stage.data.response_headers_ref.map(|id| id.to_string()),
                "response_body": stage.data.response_body_manifest_ref.map(|id| id.to_string()),
                "trailers": stage.data.trailers_ref.map(|id| id.to_string()),
                "stream_index": stage.data.stream_index_ref.map(|id| id.to_string()),
                "provider": stage.data.provider.as_deref().map(String::from_utf8_lossy),
                "requested_model": stage.data.requested_model.as_deref().map(String::from_utf8_lossy),
                "routed_model": stage.data.routed_model.as_deref().map(String::from_utf8_lossy),
                "status": stage.data.status_code,
                "error_class": stage.data.error_class.as_deref().map(String::from_utf8_lossy),
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": id.to_string(),
        "trace_id": String::from_utf8_lossy(&exchange.data.trace_id),
        "session_id": exchange.data.session_id.as_deref().map(String::from_utf8_lossy),
        "run_id": exchange.data.run_id.as_deref().map(String::from_utf8_lossy),
        "parent_trace_id": exchange.data.parent_trace_id.as_deref().map(String::from_utf8_lossy),
        "capture_sequence": exchange.data.capture_sequence,
        "wall_time_ns": exchange.data.wall_time_ns,
        "stages": stages,
    })
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct GrepMatch {
    source: String,
    archive: Option<String>,
    manifest_id: String,
    match_offset: u64,
    owner_kind: Option<String>,
    owner_id: Option<String>,
    artifact_kind: Option<String>,
    stage_id: Option<String>,
    trace_id: Option<String>,
    session_id: Option<String>,
    timestamp_ms: Option<i64>,
    timestamp_ns: Option<u64>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ArchiveGrepAnchor {
    artifact_kind: String,
    stage_id: String,
    trace_id: Option<String>,
    session_id: Option<String>,
    timestamp_ns: u64,
}

#[derive(Debug, Serialize)]
struct GrepSourceStats {
    source: String,
    archive: Option<String>,
    manifests_scanned: u64,
    manifest_ranges_scanned: u64,
    logical_bytes_scanned: u64,
    unique_chunks_read: u64,
    decompressed_chunk_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct GrepRecordMatch {
    source: String,
    archive: Option<String>,
    #[serde(flatten)]
    matched: LarRecordGrepMatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct GrepRecordCoverage {
    source: String,
    archive: Option<String>,
    #[serde(flatten)]
    coverage: LarRecordGrepCoverage,
}

impl GrepSourceStats {
    fn new(source: String, archive: Option<String>, stats: RawSearchStats) -> Self {
        Self {
            source,
            archive,
            manifests_scanned: stats.manifests_scanned,
            manifest_ranges_scanned: stats.manifest_ranges_scanned,
            logical_bytes_scanned: stats.logical_bytes_scanned,
            unique_chunks_read: stats.unique_chunks_read,
            decompressed_chunk_bytes: stats.decompressed_chunk_bytes,
        }
    }
}

fn grep_records(data_dir: &Path, args: &GrepArgs) -> Result<LarCommandOutput> {
    if args.literal.is_empty() {
        bail!("LAR grep literal must not be empty");
    }
    let limits = RawSearchLimits::default();
    let literal = args.literal.as_bytes();
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let live = store
        .grep_lar_catalog_raw(literal, args.limit, limits)
        .context("searching the configured live LAR store")?;
    let mut matches = live
        .matches
        .into_iter()
        .map(live_grep_match)
        .collect::<Vec<_>>();
    let mut sources = vec![GrepSourceStats::new(
        "live-catalog".into(),
        None,
        live.stats,
    )];
    let mut record_matches = Vec::<GrepRecordMatch>::new();
    let mut record_coverage = Vec::<GrepRecordCoverage>::new();
    if args.scope == LarGrepScope::WholeRecord {
        let report = store
            .grep_lar_catalog_records(literal, args.limit)
            .context("searching safe canonical records in the configured live LAR store")?;
        append_cli_record_matches(
            &mut record_matches,
            report.matches,
            "live-catalog",
            None,
            args.limit.saturating_sub(matches.len()),
        )?;
        record_coverage.extend(
            report
                .coverage
                .into_iter()
                .map(|coverage| GrepRecordCoverage {
                    source: "live-catalog".into(),
                    archive: None,
                    coverage,
                }),
        );
    }

    let mut archives = args.archives.clone();
    archives.sort();
    archives.dedup();
    for archive in archives {
        preflight_archive(&archive)?;
        let stats = grep_sealed_archive(&archive, literal, args.limit, &mut matches)?;
        sources.push(stats);
        if matches.len() + record_matches.len() > args.limit {
            bail!(
                "LAR grep result limit exceeded (more than {} matches); refine the literal or raise --limit",
                args.limit
            );
        }
        if args.scope == LarGrepScope::WholeRecord {
            let report = grep_sealed_archive_records(&archive, literal, args.limit)?;
            let archive_name = archive.display().to_string();
            let remaining = args
                .limit
                .saturating_sub(matches.len() + record_matches.len());
            append_cli_record_matches(
                &mut record_matches,
                report.matches,
                &format!("archive:{archive_name}"),
                Some(&archive_name),
                remaining,
            )?;
            record_coverage.extend(report.coverage.into_iter().map(|coverage| {
                GrepRecordCoverage {
                    source: format!("archive:{archive_name}"),
                    archive: Some(archive_name.clone()),
                    coverage,
                }
            }));
        }
    }
    matches.sort();
    record_matches.sort_by(|left, right| {
        (&left.source, &left.archive, &left.matched).cmp(&(
            &right.source,
            &right.archive,
            &right.matched,
        ))
    });
    let total_matches = matches.len() + record_matches.len();
    let human = if total_matches == 0 {
        format!(
            "no exact {:?} matches for {:?}; scanned {} source(s)",
            grep_scope_name(args.scope),
            args.literal,
            sources.len()
        )
    } else {
        let mut lines = matches.iter().map(grep_match_human).collect::<Vec<_>>();
        lines.extend(record_matches.iter().map(grep_record_match_human));
        lines.push(format!(
            "{} exact {} match(es) across {} source(s)",
            total_matches,
            grep_scope_name(args.scope),
            sources.len()
        ));
        lines.join("\n")
    };
    let json = if args.scope == LarGrepScope::Bodies {
        serde_json::json!({
            "literal": args.literal,
            "literal_hex": hex_bytes(literal),
            "match_count": matches.len(),
            "matches": matches,
            "sources": sources,
        })
    } else {
        serde_json::json!({
            "scope": "whole-record",
            "literal": args.literal,
            "literal_hex": hex_bytes(literal),
            "match_count": total_matches,
            "body_match_count": matches.len(),
            "record_match_count": record_matches.len(),
            "body_matches": matches,
            "record_matches": record_matches,
            "sources": sources,
            "record_coverage": record_coverage,
        })
    };
    Ok(LarCommandOutput {
        human,
        json,
        raw_body: None,
    })
}

fn grep_scope_name(scope: LarGrepScope) -> &'static str {
    match scope {
        LarGrepScope::Bodies => "raw body byte",
        LarGrepScope::WholeRecord => "whole-record",
    }
}

fn append_cli_record_matches(
    output: &mut Vec<GrepRecordMatch>,
    values: Vec<LarRecordGrepMatch>,
    source: &str,
    archive: Option<&str>,
    remaining: usize,
) -> Result<()> {
    if values.len() > remaining {
        bail!(
            "LAR grep result limit exceeded after combining body and canonical-record matches; refine the literal or raise --limit"
        );
    }
    output.extend(values.into_iter().map(|matched| GrepRecordMatch {
        source: source.into(),
        archive: archive.map(str::to_owned),
        matched,
    }));
    Ok(())
}

fn live_grep_match(value: LarCatalogGrepMatch) -> GrepMatch {
    GrepMatch {
        source: "live-catalog".into(),
        archive: None,
        manifest_id: value.manifest_id,
        match_offset: value.match_offset,
        owner_kind: value.owner_kind,
        owner_id: value.owner_id,
        artifact_kind: value.artifact_kind,
        stage_id: value.stage_id,
        trace_id: value.trace_id,
        session_id: value.session_id,
        timestamp_ms: value.timestamp_ms,
        timestamp_ns: value.timestamp_ns,
    }
}

fn grep_sealed_archive(
    path: &Path,
    literal: &[u8],
    result_limit: usize,
    matches: &mut Vec<GrepMatch>,
) -> Result<GrepSourceStats> {
    let file =
        fs::File::open(path).with_context(|| format!("opening LAR archive {}", path.display()))?;
    let mut reader = ArchiveReader::open(file, Limits::default())
        .map_err(anyhow::Error::new)
        .with_context(|| format!("opening LAR archive {}", path.display()))?;
    if !reader.is_sealed() {
        bail!(
            "supplied LAR archive is not sealed: {}; active packs must be searched through the live catalog",
            path.display()
        );
    }
    let anchors = archive_grep_anchors(&reader);
    let mut manifest_ids = reader.manifest_ids().copied().collect::<Vec<_>>();
    manifest_ids.sort_by_key(ToString::to_string);
    let mut scanner =
        RawBodyScanner::new(literal, RawSearchLimits::default()).map_err(anyhow::Error::new)?;
    let source = format!("archive:{}", path.display());
    let archive = path.display().to_string();
    for manifest_id in manifest_ids {
        let manifest = reader
            .manifest(&manifest_id)
            .cloned()
            .with_context(|| format!("archive manifest {manifest_id} disappeared"))?;
        let found = scanner
            .search_manifest(&manifest, |hash| reader.read_chunk(hash))
            .map_err(anyhow::Error::new)
            .with_context(|| {
                format!(
                    "searching manifest {manifest_id} in sealed archive {}",
                    path.display()
                )
            })?;
        let Some(match_offset) = found else { continue };
        let references = anchors.get(&manifest_id);
        if let Some(references) = references.filter(|references| !references.is_empty()) {
            for anchor in references {
                push_archive_grep_match(
                    matches,
                    GrepMatch {
                        source: source.clone(),
                        archive: Some(archive.clone()),
                        manifest_id: manifest_id.to_string(),
                        match_offset,
                        owner_kind: None,
                        owner_id: None,
                        artifact_kind: Some(anchor.artifact_kind.clone()),
                        stage_id: Some(anchor.stage_id.clone()),
                        trace_id: anchor.trace_id.clone(),
                        session_id: anchor.session_id.clone(),
                        timestamp_ms: None,
                        timestamp_ns: Some(anchor.timestamp_ns),
                    },
                    result_limit,
                )?;
            }
        } else {
            push_archive_grep_match(
                matches,
                GrepMatch {
                    source: source.clone(),
                    archive: Some(archive.clone()),
                    manifest_id: manifest_id.to_string(),
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
        }
    }
    Ok(GrepSourceStats::new(source, Some(archive), scanner.stats()))
}

fn grep_sealed_archive_records(
    path: &Path,
    literal: &[u8],
    result_limit: usize,
) -> Result<alex_store::LarRecordGrepReport> {
    let file =
        fs::File::open(path).with_context(|| format!("opening LAR archive {}", path.display()))?;
    let reader = ArchiveReader::open(file, Limits::default())
        .map_err(anyhow::Error::new)
        .with_context(|| format!("opening LAR archive {}", path.display()))?;
    if !reader.is_sealed() {
        bail!(
            "supplied LAR archive is not sealed: {}; active packs must be searched through the live catalog",
            path.display()
        );
    }
    grep_lar_archive_records(&reader, literal, result_limit)
        .with_context(|| format!("searching canonical records in {}", path.display()))
}

fn push_archive_grep_match(
    matches: &mut Vec<GrepMatch>,
    value: GrepMatch,
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

fn archive_grep_anchors<R: Read + std::io::Seek>(
    reader: &ArchiveReader<R>,
) -> HashMap<ManifestId, BTreeSet<ArchiveGrepAnchor>> {
    let mut stage_owners = HashMap::<StageId, Vec<(String, Option<String>)>>::new();
    let mut exchange_ids = reader.exchange_ids().copied().collect::<Vec<_>>();
    exchange_ids.sort_by_key(|id| id.0);
    for exchange_id in exchange_ids {
        let Some(exchange) = reader.exchange(&exchange_id) else {
            continue;
        };
        let trace_id = String::from_utf8_lossy(&exchange.data.trace_id).into_owned();
        let session_id = exchange
            .data
            .session_id
            .as_deref()
            .map(String::from_utf8_lossy)
            .map(|value| value.into_owned());
        for stage_id in &exchange.data.stages {
            stage_owners
                .entry(*stage_id)
                .or_default()
                .push((trace_id.clone(), session_id.clone()));
        }
    }
    for owners in stage_owners.values_mut() {
        owners.sort();
        owners.dedup();
    }

    let mut anchors = HashMap::<ManifestId, BTreeSet<ArchiveGrepAnchor>>::new();
    let mut stage_ids = reader.stage_ids().copied().collect::<Vec<_>>();
    stage_ids.sort_by_key(|id| id.0);
    for stage_id in stage_ids {
        let Some(stage) = reader.stage(&stage_id) else {
            continue;
        };
        let owners = stage_owners
            .get(&stage_id)
            .cloned()
            .unwrap_or_else(|| vec![(String::new(), None)]);
        for (manifest_id, artifact_kind) in [
            (stage.data.request_body_manifest_ref, "request_body"),
            (stage.data.response_body_manifest_ref, "response_body"),
        ] {
            let Some(manifest_id) = manifest_id else {
                continue;
            };
            for (trace_id, session_id) in &owners {
                anchors
                    .entry(manifest_id)
                    .or_default()
                    .insert(ArchiveGrepAnchor {
                        artifact_kind: format!("{:?}:{artifact_kind}", stage.data.kind),
                        stage_id: stage_id.to_string(),
                        trace_id: (!trace_id.is_empty()).then(|| trace_id.clone()),
                        session_id: session_id.clone(),
                        timestamp_ns: stage.data.wall_time_ns,
                    });
            }
        }
    }
    anchors
}

fn grep_match_human(value: &GrepMatch) -> String {
    let mut fields = vec![
        value.source.clone(),
        format!("manifest={}", value.manifest_id),
        format!("offset={}", value.match_offset),
    ];
    for (name, field) in [
        ("artifact", value.artifact_kind.as_deref()),
        ("stage", value.stage_id.as_deref()),
        ("trace", value.trace_id.as_deref()),
        ("session", value.session_id.as_deref()),
    ] {
        if let Some(field) = field {
            fields.push(format!("{name}={field}"));
        }
    }
    if let Some(timestamp) = value.timestamp_ms {
        fields.push(format!("timestamp_ms={timestamp}"));
    }
    if let Some(timestamp) = value.timestamp_ns {
        fields.push(format!("timestamp_ns={timestamp}"));
    }
    fields.join(" ")
}

fn grep_record_match_human(value: &GrepRecordMatch) -> String {
    let mut fields = vec![
        value.source.clone(),
        format!("category={}", value.matched.category),
        format!("field={}", value.matched.field),
        format!("offset={}", value.matched.match_offset),
        format!("trace={}", value.matched.trace_id),
    ];
    if let Some(stage_id) = &value.matched.stage_id {
        fields.push(format!("stage={stage_id}"));
    }
    if let Some(ordinal) = value.matched.header_ordinal {
        fields.push(format!("header_ordinal={ordinal}"));
    }
    if let Some(session_id) = &value.matched.session_id {
        fields.push(format!("session={session_id}"));
    }
    fields.push(format!("timestamp_ns={}", value.matched.timestamp_ns));
    fields.join(" ")
}

fn repair_archive(args: &RepairArgs) -> Result<LarCommandOutput> {
    let output_parent = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent)?;
    let canonical_input = fs::canonicalize(&args.input)
        .with_context(|| format!("resolving repair input {}", args.input.display()))?;
    let resolved_output = if args.output.exists() {
        fs::canonicalize(&args.output)
            .with_context(|| format!("resolving repair output {}", args.output.display()))?
    } else {
        fs::canonicalize(output_parent)
            .with_context(|| {
                format!(
                    "resolving repair output directory {}",
                    output_parent.display()
                )
            })?
            .join(
                args.output
                    .file_name()
                    .context("repair output must name a file")?,
            )
    };
    if canonical_input == resolved_output {
        bail!(
            "repair output must differ from the input; {} was not modified",
            args.input.display()
        );
    }
    let summary = open_archive_summary(&args.input, true, usize::MAX)?;
    let source_length = fs::metadata(&args.input)?.len();
    let recovered_length = summary.last_valid_offset.unwrap_or(source_length);
    if summary.recovery == "clean" {
        bail!(
            "LAR archive {} is already clean; repair did not create a copy",
            args.input.display()
        );
    }
    if args.output.exists() && !args.force {
        bail!(
            "repair output already exists: {} (use --force to replace it)",
            args.output.display()
        );
    }
    let output_name = args
        .output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repaired.lar");
    let temporary = output_parent.join(format!(
        ".{output_name}.{}.lar-repair.tmp",
        std::process::id()
    ));
    if temporary.exists() {
        bail!(
            "temporary repair output already exists: {}",
            temporary.display()
        );
    }
    let result = (|| -> Result<()> {
        let mut source = fs::File::open(&args.input)?;
        let mut bounded = (&mut source).take(recovered_length);
        let mut destination = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let copied = std::io::copy(&mut bounded, &mut destination)?;
        if copied != recovered_length {
            bail!("repair copied {copied} bytes, expected {recovered_length}");
        }
        destination.sync_all()?;
        let repaired = open_archive_summary(&temporary, true, usize::MAX)?;
        if repaired.recovery != "clean" {
            bail!("repaired archive still has an incomplete tail");
        }
        #[cfg(windows)]
        if args.output.exists() {
            fs::remove_file(&args.output)
                .with_context(|| format!("replacing repair output {}", args.output.display()))?;
        }
        fs::rename(&temporary, &args.output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    let repaired = open_archive_summary(&args.output, true, usize::MAX)?;
    let human = format!(
        "repaired {} into {}: retained {} of {} bytes and verified {} manifests; input was not modified",
        args.input.display(),
        args.output.display(),
        recovered_length,
        source_length,
        repaired.verified_manifest_count,
    );
    Ok(LarCommandOutput {
        human,
        json: serde_json::json!({
            "input": args.input,
            "output": args.output,
            "source_bytes": source_length,
            "recovered_bytes": recovered_length,
            "discarded_tail_bytes": source_length.saturating_sub(recovered_length),
            "verified_manifests": repaired.verified_manifest_count,
            "input_modified": false,
        }),
        raw_body: None,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpgradeBoundary {
    VerifiedBeforePublish,
}

fn upgrade_archive_command(args: &UpgradeArgs) -> Result<LarCommandOutput> {
    upgrade_archive_with_hook(args, |_| Ok(()))
}

fn upgrade_archive_with_hook<F>(args: &UpgradeArgs, before_publish: F) -> Result<LarCommandOutput>
where
    F: FnOnce(UpgradeBoundary) -> Result<()>,
{
    preflight_archive(&args.input)?;
    let output_parent = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent).with_context(|| {
        format!(
            "creating upgrade output directory {}",
            output_parent.display()
        )
    })?;
    let canonical_input = fs::canonicalize(&args.input)
        .with_context(|| format!("resolving upgrade input {}", args.input.display()))?;
    let canonical_parent = fs::canonicalize(output_parent).with_context(|| {
        format!(
            "resolving upgrade output directory {}",
            output_parent.display()
        )
    })?;
    let output_name = args
        .output
        .file_name()
        .context("upgrade output must name a file")?;
    let resolved_output = if args.output.exists() {
        fs::canonicalize(&args.output)
            .with_context(|| format!("resolving upgrade output {}", args.output.display()))?
    } else {
        canonical_parent.join(output_name)
    };
    if canonical_input == resolved_output {
        bail!(
            "upgrade output must differ from the input; {} was not modified",
            args.input.display()
        );
    }
    if args.output.exists() {
        bail!(
            "upgrade output already exists and will not be overwritten: {}",
            args.output.display()
        );
    }

    let mut source = fs::File::open(&canonical_input)
        .with_context(|| format!("opening upgrade input {}", args.input.display()))?;
    let source_bytes = source.metadata()?.len();
    let source_sha256 = sha256_file(&mut source)?;
    let source_uuid = {
        let reader = ArchiveReader::open(&mut source, Limits::default())
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| format!("reading upgrade input {}", args.input.display()))?;
        reader.header().file_uuid
    };
    let mut output_uuid = rand::random::<[u8; 16]>();
    while output_uuid == source_uuid {
        output_uuid = rand::random::<[u8; 16]>();
    }
    let created_at_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_nanos()
        .try_into()
        .context("current timestamp exceeds the LAR timestamp range")?;

    let output_name_lossy = output_name.to_string_lossy();
    let mut temporary = None;
    let mut temporary_file = None;
    for _ in 0..16 {
        let candidate = canonical_parent.join(format!(
            ".{output_name_lossy}.{}-{:016x}.lar-upgrade.tmp",
            std::process::id(),
            rand::random::<u64>()
        ));
        match fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some(candidate);
                temporary_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("creating temporary upgrade archive"),
        }
    }
    let temporary = temporary.context("could not allocate a unique temporary upgrade path")?;
    let temporary_file = temporary_file.expect("temporary path and file are created together");
    let mut published = false;
    let result = (|| -> Result<_> {
        source.rewind()?;
        let (mut temporary_file, report) = rewrite_archive(
            &mut source,
            temporary_file,
            output_uuid,
            created_at_ns,
            b"alex lar upgrade".to_vec(),
            Limits::default(),
        )
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("upgrading {}", args.input.display()))?;
        temporary_file.sync_all()?;
        source.rewind()?;
        temporary_file.rewind()?;
        verify_upgraded_archive(&mut source, &mut temporary_file, Limits::default())
            .map_err(|error| anyhow::anyhow!(error))
            .context("verifying the complete upgraded archive")?;
        let source_sha256_after = sha256_file(&mut source)?;
        if source_sha256_after != source_sha256 {
            bail!("upgrade input changed while it was being rewritten; output was not published");
        }
        let output_sha256 = sha256_file(&mut temporary_file)?;
        drop(temporary_file);
        before_publish(UpgradeBoundary::VerifiedBeforePublish)?;

        fs::hard_link(&temporary, &resolved_output).with_context(|| {
            format!(
                "publishing upgrade output without overwriting {}",
                args.output.display()
            )
        })?;
        published = true;
        sync_parent_directory(&canonical_parent)?;
        fs::remove_file(&temporary)?;
        sync_parent_directory(&canonical_parent)?;
        let output_bytes = fs::metadata(&resolved_output)?.len();
        Ok((report, output_bytes, output_sha256))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        if published {
            let _ = fs::remove_file(&resolved_output);
            let _ = sync_parent_directory(&canonical_parent);
        }
    }
    let (report, output_bytes, output_sha256) = result?;
    let source_uuid = hex_bytes(&report.source_uuid);
    let output_uuid = hex_bytes(&report.output_uuid);
    let human = format!(
        "upgraded {} into {}: copied {} canonical records, replaced {} derived index records, verified {} manifests and {} chunks; physical UUID {} -> {}; input was not modified",
        args.input.display(),
        args.output.display(),
        report.canonical_records_copied,
        report.derived_records_replaced,
        report.manifests_verified,
        report.chunks_verified,
        source_uuid,
        output_uuid,
    );
    Ok(LarCommandOutput {
        human,
        json: serde_json::json!({
            "input": args.input,
            "output": args.output,
            "source_bytes": source_bytes,
            "output_bytes": output_bytes,
            "source_sha256": source_sha256,
            "output_sha256": output_sha256,
            "source_uuid": source_uuid,
            "output_uuid": output_uuid,
            "source_container_major": report.source_container_major,
            "source_container_minor": report.source_container_minor,
            "output_container_major": report.output_container_major,
            "output_container_minor": report.output_container_minor,
            "file_role": format!("{:?}", report.file_role),
            "source_created_at_ns": report.source_created_at_ns,
            "output_created_at_ns": report.output_created_at_ns,
            "canonical_records_copied": report.canonical_records_copied,
            "derived_records_replaced": report.derived_records_replaced,
            "verified_manifests": report.manifests_verified,
            "verified_chunks": report.chunks_verified,
            "input_modified": false,
            "catalog_modified": false,
            "published_atomically": true,
        }),
        raw_body: None,
    })
}

fn sha256_file(file: &mut fs::File) -> Result<String> {
    file.rewind()?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    file.rewind()?;
    let digest = digest.finalize();
    Ok(hex_bytes(&digest))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    // Windows does not support opening directories as ordinary File handles.
    // The archive itself has already been synced before the atomic hard link.
    Ok(())
}

fn normalize_artifact_kind(value: &str) -> Result<&'static str> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "request" | "client-request" => Ok("client_request"),
        "upstream-request" => Ok("upstream_request"),
        "response" | "client-response" | "raw-stream" => Ok("client_response"),
        _ => bail!(
            "unsupported trace artifact {value:?}; expected request, upstream-request, response, or raw-stream"
        ),
    }
}

fn extract_artifact(data_dir: &Path, args: &ExtractArgs) -> Result<LarCommandOutput> {
    let artifact_kind = normalize_artifact_kind(&args.artifact)?;
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let location = store
        .lar_artifact_location("trace", &args.trace_id, artifact_kind, None)
        .with_context(|| format!("locating {artifact_kind} for trace {}", args.trace_id))?;
    let source = match location {
        Some(LarArtifactLocation::Lar { .. }) => "lar",
        Some(LarArtifactLocation::Legacy { .. }) => "legacy",
        Some(LarArtifactLocation::Unavailable { error, .. }) => bail!(
            "trace {} artifact {} is unavailable ({}): {}",
            args.trace_id,
            args.artifact,
            error.kind,
            error.detail
        ),
        None => bail!(
            "trace {} has no captured {} artifact",
            args.trace_id,
            args.artifact
        ),
    };
    let bytes = store
        .read_lar_or_legacy_artifact("trace", &args.trace_id, artifact_kind, None)?
        .context("artifact disappeared after it was resolved")?;
    let hash = hex_bytes(&alex_lar::ChunkHash::blake3(&bytes).digest);

    if let Some(output) = &args.output {
        write_extracted_file(output, &bytes, args.force)?;
        return Ok(LarCommandOutput {
            human: format!(
                "extracted {} bytes from trace {} {} ({source}) to {} (BLAKE3 {hash})",
                bytes.len(),
                args.trace_id,
                args.artifact,
                output.display()
            ),
            json: serde_json::json!({
                "trace_id": args.trace_id,
                "artifact": args.artifact,
                "source": source,
                "output": output,
                "bytes": bytes.len(),
                "hash_algorithm": "blake3",
                "hash": hash,
            }),
            raw_body: None,
        });
    }

    Ok(LarCommandOutput {
        human: String::new(),
        json: Value::Null,
        raw_body: Some(bytes),
    })
}

fn write_extracted_file(output: &Path, bytes: &[u8], force: bool) -> Result<()> {
    if output.exists() && !force {
        bail!(
            "extract output already exists: {} (use --force to replace it)",
            output.display()
        );
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let temporary = parent.join(format!(".{name}.{}.lar-extract.tmp", std::process::id()));
    if temporary.exists() {
        bail!(
            "temporary extract output already exists: {}",
            temporary.display()
        );
    }
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(windows)]
        if output.exists() {
            fs::remove_file(output)
                .with_context(|| format!("replacing extract output {}", output.display()))?;
        }
        fs::rename(&temporary, output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone, Debug, Serialize)]
struct ExportHeader {
    name: String,
    value: String,
}

#[derive(Debug)]
struct ExportTrace {
    row: Value,
    request_headers: Vec<ExportHeader>,
    response_headers: Vec<ExportHeader>,
    request: Option<Vec<u8>>,
    upstream_request: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
}

#[derive(Debug)]
struct InterchangeExportTrace {
    row: Value,
    request_headers: Vec<ExportHeader>,
    response_headers: Vec<ExportHeader>,
    canonical: Option<LarInterchangeTrace>,
}

#[derive(Clone, Copy, Debug, Default)]
struct InterchangeSelectionSummary {
    traces: usize,
    canonical: usize,
    legacy: usize,
}

struct BackupExtraBody {
    owner_id: String,
    artifact_kind: &'static str,
    bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct ExportReport {
    output: PathBuf,
    format: &'static str,
    traces: usize,
    bytes: u64,
    verified: bool,
    loss_report: Vec<&'static str>,
    canonical_traces: usize,
    legacy_traces: usize,
}

fn export_records(data_dir: &Path, args: &ExportArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let upper = store
        .lar_export_trace_upper_bound(args.trace_id.as_deref(), args.session.as_deref())?
        .with_context(|| match (&args.trace_id, &args.session) {
            (Some(trace_id), _) => format!("trace {trace_id} was not found"),
            (_, Some(session)) => format!("session {session} has no traces"),
            _ => "no traces matched the requested export selection".into(),
        })?;
    let summary = summarize_export_selection(&store, args, &upper)?;
    let losses = interchange_loss_report(args.format, summary);
    let (byte_count, verified) = match args.format {
        LarExportFormat::Lar => {
            let traces = load_export_traces(&store, args, false)?;
            let bytes = write_standalone_lar_export(&store, &traces, &args.output, args.force)?;
            (bytes, true)
        }
        format => {
            let bytes =
                write_streaming_interchange_export(&store, args, format, summary, &upper, &losses)?;
            (bytes, true)
        }
    };
    if !verified {
        bail!(
            "export verification failed after writing {}; the output was retained for diagnosis",
            args.output.display()
        );
    }
    let report = ExportReport {
        output: args.output.clone(),
        format: export_format_name(args.format),
        traces: summary.traces,
        bytes: byte_count,
        verified,
        loss_report: losses,
        canonical_traces: summary.canonical,
        legacy_traces: summary.legacy,
    };
    Ok(LarCommandOutput {
        human: format!(
            "exported {} trace(s) to {} as {} ({} bytes, verified); fidelity loss: {}",
            report.traces,
            report.output.display(),
            report.format,
            report.bytes,
            report.loss_report.join("; ")
        ),
        json: serde_json::to_value(report)?,
        raw_body: None,
    })
}

fn export_format_name(format: LarExportFormat) -> &'static str {
    match format {
        LarExportFormat::Lar => "lar",
        LarExportFormat::Har => "har",
        LarExportFormat::Warc => "warc",
        LarExportFormat::Jsonl => "jsonl",
        LarExportFormat::OpenTelemetry => "otel-jsonl",
        LarExportFormat::OpenInference => "openinference-jsonl",
    }
}

fn export_loss_report() -> Vec<&'static str> {
    vec![
        "legacy header order, duplicates, and original casing may be unavailable",
        "upstream headers and trailers were not captured by the legacy schema",
        "raw HTTP framing and connection metadata were not captured",
        "stream read timing is unavailable for legacy bodies",
        "tool-call rows are not included in this trace-only exporter",
    ]
}

fn interchange_loss_report(
    format: LarExportFormat,
    summary: InterchangeSelectionSummary,
) -> Vec<&'static str> {
    let mut losses = Vec::new();
    if summary.canonical > 0 {
        match format {
            LarExportFormat::Lar | LarExportFormat::Jsonl => {}
            LarExportFormat::Har => losses.extend([
                "HAR standard fields synthesize HTTP framing and expose non-client stages through _alex.canonicalGraph",
                "HAR viewers may ignore trailers, retries, tool stages, and stream timing retained in Alex extensions",
            ]),
            LarExportFormat::Warc => losses.extend([
                "WARC HTTP projections synthesize framing; canonical stage metadata and bodies are retained as linked records",
            ]),
            LarExportFormat::OpenTelemetry => losses.extend([
                "OpenTelemetry semantic spans do not standardize exact transport replay; Alex canonical graph extensions retain captured transport details",
            ]),
            LarExportFormat::OpenInference => losses.extend([
                "OpenInference semantic spans do not standardize exact transport replay; Alex canonical graph extensions retain captured transport details",
            ]),
        }
    }
    if summary.legacy > 0 {
        losses.extend(export_loss_report());
    }
    losses
}

const INTERCHANGE_TRACE_PAGE_SIZE: usize = 32;

fn for_each_selected_export_row<F>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    mut visit: F,
) -> Result<usize>
where
    F: FnMut(Value) -> Result<()>,
{
    let mut cursor: Option<LarExportTraceCursor> = None;
    let mut count = 0usize;
    loop {
        let rows = store
            .lar_export_trace_rows_page(
                args.trace_id.as_deref(),
                args.session.as_deref(),
                cursor.as_ref(),
                upper,
                INTERCHANGE_TRACE_PAGE_SIZE,
            )
            .context("reading a bounded trace export page")?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let trace_id = row["id"]
                .as_str()
                .context("trace export row has no string id")?
                .to_string();
            let ts_request_ms = row["ts_request_ms"]
                .as_i64()
                .context("trace export row has no request timestamp")?;
            visit(row)?;
            count = count.saturating_add(1);
            cursor = Some(LarExportTraceCursor {
                ts_request_ms,
                trace_id,
                max_rowid: upper.max_rowid,
            });
        }
    }
    if count == 0 {
        if let Some(trace_id) = &args.trace_id {
            bail!("trace {trace_id} was not found");
        }
        if let Some(session) = &args.session {
            bail!("session {session} has no traces");
        }
        bail!("no traces matched the requested export selection");
    }
    Ok(count)
}

fn summarize_export_selection(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
) -> Result<InterchangeSelectionSummary> {
    let mut summary = InterchangeSelectionSummary::default();
    for_each_selected_export_row(store, args, upper, |row| {
        let trace_id = row["id"]
            .as_str()
            .context("trace export row has no string id")?;
        if store.lar_trace_has_canonical_exchange(trace_id)? {
            summary.canonical += 1;
        } else {
            summary.legacy += 1;
        }
        Ok(())
    })?;
    summary.traces = summary.canonical.saturating_add(summary.legacy);
    Ok(summary)
}

fn for_each_interchange_trace<F>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    mut visit: F,
) -> Result<usize>
where
    F: FnMut(InterchangeExportTrace) -> Result<()>,
{
    for_each_selected_export_row(store, args, upper, |row| {
        let trace_id = row["id"]
            .as_str()
            .context("trace export row has no string id")?;
        let canonical = store
            .lar_interchange_trace(trace_id)
            .with_context(|| format!("loading canonical exchange graph for trace {trace_id}"))?;
        visit(InterchangeExportTrace {
            request_headers: parse_legacy_headers(row.get("req_headers_json")),
            response_headers: parse_legacy_headers(row.get("resp_headers_json")),
            row,
            canonical,
        })
    })
}

fn write_streaming_interchange_export(
    store: &Store,
    args: &ExportArgs,
    format: LarExportFormat,
    summary: InterchangeSelectionSummary,
    upper: &LarExportTraceCursor,
    losses: &[&str],
) -> Result<u64> {
    if args.output.exists() && !args.force {
        bail!(
            "export output already exists: {} (use --force to replace it)",
            args.output.display()
        );
    }
    let parent = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = args
        .output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    let temporary = parent.join(format!(".{name}.{}.interchange.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<u64> {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let mut output = BufWriter::with_capacity(64 * 1024, file);
        let emitted = match format {
            LarExportFormat::Har => stream_har(store, args, upper, summary, losses, &mut output)?,
            LarExportFormat::Warc => stream_warc(store, args, upper, losses, &mut output)?,
            LarExportFormat::Jsonl => {
                stream_jsonl(store, args, upper, summary, losses, &mut output)?
            }
            LarExportFormat::OpenTelemetry => {
                stream_otel_jsonl(store, args, upper, losses, &mut output)?
            }
            LarExportFormat::OpenInference => {
                stream_openinference_jsonl(store, args, upper, losses, &mut output)?
            }
            LarExportFormat::Lar => unreachable!("LAR uses its native archive writer"),
        };
        if emitted != summary.traces {
            bail!(
                "trace selection changed during export: preflight selected {} trace(s), but {} remained at the frozen high-water mark",
                summary.traces,
                emitted
            );
        }
        output.flush()?;
        let file = output
            .into_inner()
            .map_err(|error| anyhow::anyhow!(error.into_error()))?;
        file.sync_all()?;
        let bytes = file.metadata()?.len();
        drop(file);
        publish_export_temp(&temporary, &args.output, args.force)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn transport_bytes(value: Option<&[u8]>) -> Value {
    match value {
        None => Value::Null,
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => serde_json::json!({"encoding": "utf8", "data": text}),
            Err(_) => serde_json::json!({
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        },
    }
}

fn optional_content_id<T: std::fmt::Display>(value: Option<T>) -> Value {
    value
        .map(|id| Value::String(id.to_string()))
        .unwrap_or(Value::Null)
}

fn canonical_stage_json(trace: &LarInterchangeTrace, stage: &LarInterchangeStage) -> Value {
    let data = &stage.data;
    serde_json::json!({
        "stage_id": stage.stage_id,
        "record_id": stage.record_id,
        "ordinal": stage.ordinal,
        "capture_sequence": stage.capture_sequence,
        "kind": stage.kind,
        "attempt_number": data.attempt_number,
        "wall_time_ns": data.wall_time_ns,
        "monotonic_delta_ns": data.monotonic_delta_ns,
        "first_byte_delta_ns": data.first_byte_delta_ns,
        "last_byte_delta_ns": data.last_byte_delta_ns,
        "request_headers_ref": optional_content_id(data.request_headers_ref),
        "request_body_manifest_ref": optional_content_id(data.request_body_manifest_ref),
        "response_headers_ref": optional_content_id(data.response_headers_ref),
        "response_body_manifest_ref": optional_content_id(data.response_body_manifest_ref),
        "trailers_ref": optional_content_id(data.trailers_ref),
        "stream_index_ref": optional_content_id(data.stream_index_ref),
        "provider": transport_bytes(data.provider.as_deref()),
        "requested_model": transport_bytes(data.requested_model.as_deref()),
        "routed_model": transport_bytes(data.routed_model.as_deref()),
        "account_id": transport_bytes(data.account_id.as_deref()),
        "routing_reason": transport_bytes(data.routing_reason.as_deref()),
        "status_code": data.status_code,
        "usage": data.usage.as_ref().map(|usage| serde_json::json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "cached_tokens": usage.cached_tokens,
            "reasoning_tokens": usage.reasoning_tokens,
        })),
        "cost_nanos": data.cost_nanos,
        "cost_currency": transport_bytes(data.cost_currency.as_deref()),
        "error_class": transport_bytes(data.error_class.as_deref()),
        "error_message": transport_bytes(data.error_message.as_deref()),
        "tool_link": stage.tool_id.as_ref().map(|tool_id| serde_json::json!({
            "tool_id": tool_id,
            "phase": stage.tool_phase,
            "supplement_trace_id": stage.supplement_trace_id,
            "supplement_exchange_content_id": stage.supplement_exchange_id,
        })),
        "origin": {
            "trace_id": stage.supplement_trace_id.as_ref().map(|value| transport_bytes(Some(value.as_bytes()))).unwrap_or_else(|| transport_bytes(Some(&trace.trace_id))),
            "exchange_content_id": stage.supplement_exchange_id.as_deref().unwrap_or(&trace.exchange_id),
            "stage_occurrence_id": stage.stage_id,
            "stage_record_content_id": stage.record_id,
        },
    })
}

fn canonical_metadata_json(metadata: Option<&ExchangeMetadataData>) -> Value {
    let Some(metadata) = metadata else {
        return Value::Null;
    };
    serde_json::json!({
        "ts_request_ms": metadata.ts_request_ms,
        "ts_response_ms": metadata.ts_response_ms,
        "harness": transport_bytes(metadata.harness.as_deref()),
        "client_format": transport_bytes(metadata.client_format.as_deref()),
        "upstream_format": transport_bytes(metadata.upstream_format.as_deref()),
        "method": transport_bytes(metadata.method.as_deref()),
        "path": transport_bytes(metadata.path.as_deref()),
        "streamed": metadata.streamed,
        "status": metadata.status,
        "cost_usd_bits": metadata.cost_usd_bits,
        "billing_bucket": transport_bytes(metadata.billing_bucket.as_deref()),
        "error_kind": transport_bytes(metadata.error_kind.as_deref()),
        "error_code": transport_bytes(metadata.error_code.as_deref()),
        "substituted": metadata.substituted,
        "original_model": transport_bytes(metadata.original_model.as_deref()),
        "served_model": transport_bytes(metadata.served_model.as_deref()),
        "substitution_reason": transport_bytes(metadata.substitution_reason.as_deref()),
        "injected": metadata.injected,
        "fixture_name": transport_bytes(metadata.fixture_name.as_deref()),
        "attempts_json": transport_bytes(metadata.attempts_json.as_deref()),
        "original_account_id": transport_bytes(metadata.original_account_id.as_deref()),
        "served_account_id": transport_bytes(metadata.served_account_id.as_deref()),
        "subscription_identity": transport_bytes(metadata.subscription_identity.as_deref()),
        "via_dario": metadata.via_dario,
        "dario_generation": transport_bytes(metadata.dario_generation.as_deref()),
        "tags_json": transport_bytes(metadata.tags_json.as_deref()),
        "client_ip": transport_bytes(metadata.client_ip.as_deref()),
        "key_fingerprint": transport_bytes(metadata.key_fingerprint.as_deref()),
        "reasoning_effort": transport_bytes(metadata.reasoning_effort.as_deref()),
        "thinking_budget": metadata.thinking_budget,
        "input_tokens": metadata.input_tokens,
        "cached_input_tokens": metadata.cached_input_tokens,
        "cache_creation_tokens": metadata.cache_creation_tokens,
        "output_tokens": metadata.output_tokens,
        "reasoning_tokens": metadata.reasoning_tokens,
        "unknown_attributes": metadata.unknown_attributes.iter().map(|attribute| serde_json::json!({
            "key": transport_bytes(Some(&attribute.key)),
            "value": transport_bytes(Some(&attribute.value)),
        })).collect::<Vec<_>>(),
    })
}

fn stream_parser_name(value: alex_lar::StreamParser) -> String {
    match value {
        alex_lar::StreamParser::Opaque => "opaque".into(),
        alex_lar::StreamParser::Sse => "sse".into(),
        alex_lar::StreamParser::Ndjson => "ndjson".into(),
        alex_lar::StreamParser::Unknown(code) => format!("unknown_{code}"),
    }
}

fn stream_frame_kind_name(value: alex_lar::StreamFrameKind) -> String {
    match value {
        alex_lar::StreamFrameKind::Opaque => "opaque".into(),
        alex_lar::StreamFrameKind::SseEvent => "sse_event".into(),
        alex_lar::StreamFrameKind::NdjsonRecord => "ndjson_record".into(),
        alex_lar::StreamFrameKind::Unknown(code) => format!("unknown_{code}"),
    }
}

fn canonical_graph_metadata(trace: &LarInterchangeTrace) -> Value {
    serde_json::json!({
        "schema": "alex.lar.canonical-timeline-projection.v2",
        "exchange": {
            "base_exchange_content_id": trace.exchange_id,
            "trace_id": transport_bytes(Some(&trace.trace_id)),
            "session_id": transport_bytes(trace.session_id.as_deref()),
            "run_id": transport_bytes(trace.run_id.as_deref()),
            "parent_trace_id": transport_bytes(trace.parent_trace_id.as_deref()),
            "capture_sequence": trace.capture_sequence,
            "wall_time_ns": trace.wall_time_ns,
            "monotonic_delta_ns": trace.monotonic_delta_ns,
            "clock_id": transport_bytes(trace.clock_id.as_deref()),
        },
        "exchange_metadata": canonical_metadata_json(trace.metadata.as_ref()),
        "stages": trace.stages.iter().map(|stage| canonical_stage_json(trace, stage)).collect::<Vec<_>>(),
        "header_blocks": trace.header_blocks.iter().map(|block| serde_json::json!({
            "block_id": block.block_id,
            "fidelity": block.fidelity,
            "atoms": block.atoms.iter().enumerate().map(|(ordinal, atom)| serde_json::json!({
                "ordinal": ordinal,
                "original_name": transport_bytes(Some(&atom.original_name)),
                "value": transport_bytes(Some(&atom.value)),
                "flags": atom.flags,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "streams": trace.streams.iter().map(|stream| serde_json::json!({
            "stream_index_id": stream.stream_index_id,
            "raw_body_manifest_id": stream.raw_body_manifest_id,
            "reads": stream.reads.iter().map(|read| serde_json::json!({
                "byte_offset": read.byte_offset,
                "byte_length": read.byte_length,
                "delta_from_first_byte_ns": read.delta_from_first_byte_ns,
            })).collect::<Vec<_>>(),
            "frames": stream.frames.iter().map(|frame| serde_json::json!({
                "byte_offset": frame.byte_offset,
                "byte_length": frame.byte_length,
                "delta_from_first_byte_ns": frame.delta_from_first_byte_ns,
                "parser": stream_parser_name(frame.parser),
                "frame_kind": stream_frame_kind_name(frame.frame_kind),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn body_descriptor(body: &LarInterchangeBody) -> Value {
    serde_json::json!({
        "manifest_id": body.manifest_id,
        "encoding": "base64",
        "length": body.total_length,
        "blake3": body.whole_body_blake3,
        "media_type": transport_bytes(body.media_type.as_deref()),
        "content_encoding": transport_bytes(body.content_encoding.as_deref()),
    })
}

fn write_canonical_graph<W: Write>(
    store: &Store,
    trace: &LarInterchangeTrace,
    include_body_data: bool,
    output: &mut W,
) -> Result<()> {
    output.write_all(b"{\"capture\":")?;
    serde_json::to_writer(&mut *output, &canonical_graph_metadata(trace))?;
    output.write_all(b",\"bodies\":[")?;
    for (index, body) in trace.bodies.iter().enumerate() {
        if index > 0 {
            output.write_all(b",")?;
        }
        if include_body_data {
            write_canonical_body(store, body, output)?;
        } else {
            serde_json::to_writer(&mut *output, &body_descriptor(body))?;
        }
    }
    output.write_all(b"]}")?;
    Ok(())
}

fn write_canonical_body<W: Write>(
    store: &Store,
    body: &LarInterchangeBody,
    output: &mut W,
) -> Result<()> {
    write!(
        output,
        "{{\"manifest_id\":{},\"encoding\":\"base64\",\"length\":{},\"blake3\":{},\"media_type\":{},\"content_encoding\":{},\"data\":\"",
        serde_json::to_string(&body.manifest_id)?,
        body.total_length,
        serde_json::to_string(&body.whole_body_blake3)?,
        serde_json::to_string(&transport_bytes(body.media_type.as_deref()))?,
        serde_json::to_string(&transport_bytes(body.content_encoding.as_deref()))?,
    )?;
    let written = {
        let mut encoder = base64::write::EncoderWriter::new(
            &mut *output,
            &base64::engine::general_purpose::STANDARD,
        );
        let written = store
            .write_lar_manifest_body(&body.manifest_id, &mut encoder)
            .with_context(|| format!("streaming canonical manifest {}", body.manifest_id))?;
        encoder.finish()?;
        written
    };
    if written != body.total_length {
        bail!(
            "canonical manifest {} streamed {written} bytes, expected {}",
            body.manifest_id,
            body.total_length
        );
    }
    output.write_all(b"\"}")?;
    Ok(())
}

struct HashCountWriter<W> {
    inner: W,
    hash: blake3::Hasher,
    bytes: u64,
}

struct Sha256CountWriter<W> {
    inner: W,
    hash: Sha256,
    bytes: u64,
}

impl<W> Sha256CountWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hash: Sha256::new(),
            bytes: 0,
        }
    }

    fn finish(self) -> (W, u64, String) {
        (self.inner, self.bytes, hex_bytes(&self.hash.finalize()))
    }
}

impl<W: Write> Write for Sha256CountWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hash.update(&bytes[..written]);
        self.bytes = self.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<W> HashCountWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hash: blake3::Hasher::new(),
            bytes: 0,
        }
    }

    fn finish(self) -> (W, u64, String) {
        (
            self.inner,
            self.bytes,
            self.hash.finalize().to_hex().to_string(),
        )
    }
}

impl<W: Write> Write for HashCountWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hash.update(&bytes[..written]);
        self.bytes = self.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_legacy_artifact<W: Write>(
    store: &Store,
    trace_id: &str,
    artifact_kind: &str,
    output: &mut W,
) -> Result<()> {
    if store
        .lar_artifact_location("trace", trace_id, artifact_kind, None)?
        .is_none()
    {
        output.write_all(b"null")?;
        return Ok(());
    }
    output.write_all(b"{\"encoding\":\"base64\",\"data\":\"")?;
    let (length, digest) = {
        let encoder = base64::write::EncoderWriter::new(
            &mut *output,
            &base64::engine::general_purpose::STANDARD,
        );
        let mut tracked = HashCountWriter::new(encoder);
        if !store.write_lar_or_legacy_artifact(
            "trace",
            trace_id,
            artifact_kind,
            None,
            &mut tracked,
        )? {
            bail!("artifact {artifact_kind} for trace {trace_id} disappeared during export");
        }
        let (mut encoder, length, digest) = tracked.finish();
        encoder.finish()?;
        (length, digest)
    };
    write!(
        output,
        "\",\"length\":{length},\"blake3\":{}}}",
        serde_json::to_string(&digest)?
    )?;
    Ok(())
}

fn write_legacy_trace_payload<W: Write>(
    store: &Store,
    trace: &InterchangeExportTrace,
    output: &mut W,
) -> Result<()> {
    let trace_id = trace.row["id"]
        .as_str()
        .context("trace export row has no string id")?;
    output.write_all(b"{\"type\":\"alex.trace\",\"metadata\":")?;
    serde_json::to_writer(&mut *output, &sanitized_trace_metadata(&trace.row))?;
    output.write_all(b",\"headers\":")?;
    serde_json::to_writer(
        &mut *output,
        &serde_json::json!({
            "request": trace.request_headers,
            "response": trace.response_headers,
            "fidelity": "legacy_order_and_casing_unknown",
        }),
    )?;
    output.write_all(b",\"artifacts\":{\"client_request\":")?;
    write_legacy_artifact(store, trace_id, "client_request", output)?;
    output.write_all(b",\"upstream_request\":")?;
    write_legacy_artifact(store, trace_id, "upstream_request", output)?;
    output.write_all(b",\"client_response\":")?;
    write_legacy_artifact(store, trace_id, "client_response", output)?;
    output.write_all(b"}}")?;
    Ok(())
}

fn stream_jsonl<W: Write>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    summary: InterchangeSelectionSummary,
    losses: &[&str],
    output: &mut W,
) -> Result<usize> {
    let version = if summary.canonical == 0 { 1 } else { 2 };
    if version == 1 {
        serde_json::to_writer(
            &mut *output,
            &serde_json::json!({
                "type": "alex.lar.export.manifest",
                "version": 1,
                "format": "jsonl",
                "loss_report": losses,
            }),
        )?;
    } else {
        serde_json::to_writer(
            &mut *output,
            &serde_json::json!({
                "type": "alex.lar.export.manifest",
                "version": 2,
                "format": "jsonl",
                "record_schema": "alex.trace.canonical.v2",
                "canonical_traces": summary.canonical,
                "legacy_traces": summary.legacy,
                "body_part_bytes": JSONL_BODY_PART_BYTES,
                "loss_report": losses,
            }),
        )?;
    }
    output.write_all(b"\n")?;
    let emitted = for_each_interchange_trace(store, args, upper, |trace| {
        if let Some(canonical) = &trace.canonical {
            output.write_all(b"{\"type\":\"alex.trace.canonical\",\"version\":2,\"metadata\":")?;
            serde_json::to_writer(&mut *output, &sanitized_trace_metadata(&trace.row))?;
            output.write_all(b",\"fidelity\":\"canonical\",\"graph\":")?;
            write_canonical_graph(store, canonical, false, output)?;
            output.write_all(b"}\n")?;
            let trace_id = trace.row["id"].as_str().unwrap_or("unknown");
            for body in &canonical.bodies {
                stream_jsonl_body_parts(store, trace_id, body, output)?;
            }
        } else if version == 1 {
            write_legacy_trace_payload(store, &trace, output)?;
            output.write_all(b"\n")?;
        } else {
            output.write_all(b"{\"type\":\"alex.trace.legacy\",\"version\":1,\"loss_report\":")?;
            serde_json::to_writer(&mut *output, &export_loss_report())?;
            output.write_all(b",\"record\":")?;
            write_legacy_trace_payload(store, &trace, output)?;
            output.write_all(b"}\n")?;
        }
        Ok(())
    })?;
    Ok(emitted)
}

const JSONL_BODY_PART_BYTES: usize = 48 * 1024;

struct JsonlBodyPartWriter<'a, W> {
    output: &'a mut W,
    trace_id: &'a str,
    manifest_id: &'a str,
    buffer: Vec<u8>,
    offset: u64,
    hash: blake3::Hasher,
}

impl<W: Write> JsonlBodyPartWriter<'_, W> {
    fn emit(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        serde_json::to_writer(
            &mut *self.output,
            &serde_json::json!({
                "type": "alex.body.part",
                "version": 2,
                "trace_id": self.trace_id,
                "manifest_id": self.manifest_id,
                "byte_offset": self.offset,
                "byte_length": self.buffer.len(),
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(&self.buffer),
            }),
        )?;
        self.output.write_all(b"\n")?;
        self.hash.update(&self.buffer);
        self.offset = self.offset.saturating_add(self.buffer.len() as u64);
        self.buffer.clear();
        Ok(())
    }
}

impl<W: Write> Write for JsonlBodyPartWriter<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<usize> {
        let original = bytes.len();
        while !bytes.is_empty() {
            let available = JSONL_BODY_PART_BYTES.saturating_sub(self.buffer.len());
            let take = available.min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() == JSONL_BODY_PART_BYTES {
                self.emit().map_err(std::io::Error::other)?;
            }
        }
        Ok(original)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.emit().map_err(std::io::Error::other)?;
        self.output.flush()
    }
}

fn stream_jsonl_body_parts<W: Write>(
    store: &Store,
    trace_id: &str,
    body: &LarInterchangeBody,
    output: &mut W,
) -> Result<()> {
    let mut writer = JsonlBodyPartWriter {
        output,
        trace_id,
        manifest_id: &body.manifest_id,
        buffer: Vec::with_capacity(JSONL_BODY_PART_BYTES),
        offset: 0,
        hash: blake3::Hasher::new(),
    };
    let written = store.write_lar_manifest_body(&body.manifest_id, &mut writer)?;
    writer.emit()?;
    let actual_hash = writer.hash.finalize().to_hex().to_string();
    if written != body.total_length
        || writer.offset != body.total_length
        || actual_hash != body.whole_body_blake3
    {
        bail!("JSONL body {} changed while streaming", body.manifest_id);
    }
    serde_json::to_writer(
        &mut *writer.output,
        &serde_json::json!({
            "type": "alex.body.end",
            "version": 2,
            "trace_id": trace_id,
            "manifest_id": body.manifest_id,
            "length": body.total_length,
            "blake3": body.whole_body_blake3,
        }),
    )?;
    writer.output.write_all(b"\n")?;
    Ok(())
}

fn standard_headers(
    trace: &LarInterchangeTrace,
    reference: Option<alex_lar::HeaderBlockId>,
) -> Vec<ExportHeader> {
    let Some(reference) = reference else {
        return Vec::new();
    };
    trace
        .header_blocks
        .iter()
        .find(|block| block.block_id == reference.to_string())
        .map(|block| {
            block
                .atoms
                .iter()
                .map(|atom| ExportHeader {
                    name: String::from_utf8_lossy(&atom.original_name).into_owned(),
                    value: String::from_utf8_lossy(&atom.value).into_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn metadata_text(value: Option<&[u8]>, fallback: &str) -> String {
    value
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .unwrap_or_else(|| fallback.into())
}

fn canonical_har_projection(trace: &LarInterchangeTrace, row: &Value, losses: &[&str]) -> Value {
    let request_stage = trace
        .stages
        .iter()
        .find(|stage| stage.kind == "client_request");
    let response_stage = trace
        .stages
        .iter()
        .rev()
        .find(|stage| matches!(stage.kind.as_str(), "client_response" | "injected_response"));
    let metadata = trace.metadata.as_ref();
    let request_time = metadata
        .and_then(|value| value.ts_request_ms)
        .unwrap_or_else(|| row["ts_request_ms"].as_i64().unwrap_or_default());
    let response_time = metadata
        .and_then(|value| value.ts_response_ms)
        .unwrap_or(request_time);
    let duration = response_time.saturating_sub(request_time).max(0);
    let request_headers = standard_headers(
        trace,
        request_stage.and_then(|stage| stage.data.request_headers_ref),
    );
    let response_headers = standard_headers(
        trace,
        response_stage.and_then(|stage| stage.data.response_headers_ref),
    );
    serde_json::json!({
        "startedDateTime": rfc3339_millis(request_time),
        "time": duration,
        "request": {
            "method": metadata_text(metadata.and_then(|value| value.method.as_deref()), "POST"),
            "url": metadata_text(metadata.and_then(|value| value.path.as_deref()), "/"),
            "httpVersion": "HTTP/1.1",
            "cookies": [],
            "headers": request_headers,
            "queryString": [],
            "headersSize": -1,
            "bodySize": request_stage.and_then(|stage| stage.data.request_body_manifest_ref)
                .and_then(|id| trace.bodies.iter().find(|body| body.manifest_id == id.to_string()))
                .map(|body| body.total_length).unwrap_or_default(),
            "postData": request_stage.and_then(|stage| stage.data.request_body_manifest_ref)
                .map(|id| serde_json::json!({"mimeType":"application/octet-stream", "_alexManifestRef": id.to_string()})),
        },
        "response": {
            "status": response_stage.and_then(|stage| stage.data.status_code).unwrap_or_default(),
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "cookies": [],
            "headers": response_headers,
            "content": {
                "size": response_stage.and_then(|stage| stage.data.response_body_manifest_ref)
                    .and_then(|id| trace.bodies.iter().find(|body| body.manifest_id == id.to_string()))
                    .map(|body| body.total_length).unwrap_or_default(),
                "mimeType": "application/octet-stream",
                "_alexManifestRef": response_stage.and_then(|stage| stage.data.response_body_manifest_ref).map(|id| id.to_string()),
            },
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": response_stage.and_then(|stage| stage.data.response_body_manifest_ref)
                .and_then(|id| trace.bodies.iter().find(|body| body.manifest_id == id.to_string()))
                .map(|body| body.total_length).unwrap_or_default(),
        },
        "cache": {},
        "timings": {"send": 0, "wait": duration, "receive": 0},
        "_alex": {"fidelity": "canonical", "lossReport": losses},
    })
}

fn write_json_object_except<W: Write>(
    output: &mut W,
    object: &serde_json::Map<String, Value>,
    skipped: &[&str],
    first: &mut bool,
) -> Result<()> {
    for (key, value) in object {
        if skipped.contains(&key.as_str()) {
            continue;
        }
        if !*first {
            output.write_all(b",")?;
        }
        *first = false;
        serde_json::to_writer(&mut *output, key)?;
        output.write_all(b":")?;
        serde_json::to_writer(&mut *output, value)?;
    }
    Ok(())
}

fn write_har_canonical_content<W: Write>(
    store: &Store,
    body: Option<&LarInterchangeBody>,
    output: &mut W,
) -> Result<()> {
    let Some(body) = body else {
        output.write_all(b"null")?;
        return Ok(());
    };
    write!(
        output,
        "{{\"mimeType\":{},\"text\":\"",
        serde_json::to_string(
            &body
                .media_type
                .as_deref()
                .map(|value| String::from_utf8_lossy(value).into_owned())
                .unwrap_or_else(|| "application/octet-stream".into())
        )?
    )?;
    let written = {
        let mut encoder = base64::write::EncoderWriter::new(
            &mut *output,
            &base64::engine::general_purpose::STANDARD,
        );
        let written = store.write_lar_manifest_body(&body.manifest_id, &mut encoder)?;
        encoder.finish()?;
        written
    };
    if written != body.total_length {
        bail!("HAR manifest {} changed during export", body.manifest_id);
    }
    write!(
        output,
        "\",\"encoding\":\"base64\",\"size\":{},\"_alexManifestRef\":{}}}",
        body.total_length,
        serde_json::to_string(&body.manifest_id)?
    )?;
    Ok(())
}

fn write_har_legacy_content<W: Write>(
    store: &Store,
    trace_id: &str,
    artifact_kind: &str,
    output: &mut W,
) -> Result<()> {
    output.write_all(
        b"{\"mimeType\":\"application/octet-stream\",\"encoding\":\"base64\",\"text\":\"",
    )?;
    if store
        .lar_artifact_location("trace", trace_id, artifact_kind, None)?
        .is_some()
    {
        let mut encoder = base64::write::EncoderWriter::new(
            &mut *output,
            &base64::engine::general_purpose::STANDARD,
        );
        store.write_lar_or_legacy_artifact("trace", trace_id, artifact_kind, None, &mut encoder)?;
        encoder.finish()?;
    }
    output.write_all(b"\"}")?;
    Ok(())
}

fn stream_har<W: Write>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    summary: InterchangeSelectionSummary,
    losses: &[&str],
    output: &mut W,
) -> Result<usize> {
    write!(
        output,
        "{{\"log\":{{\"version\":\"1.2\",\"creator\":{{\"name\":\"Alex\",\"version\":{}}},\"_alexSummary\":{},\"entries\":[",
        serde_json::to_string(env!("CARGO_PKG_VERSION"))?,
        serde_json::to_string(&serde_json::json!({
            "canonical_traces": summary.canonical,
            "legacy_traces": summary.legacy,
            "loss_report": losses,
        }))?
    )?;
    let mut first = true;
    let emitted = for_each_interchange_trace(store, args, upper, |trace| {
        if !first {
            output.write_all(b",")?;
        }
        first = false;
        if let Some(canonical) = &trace.canonical {
            let projection = canonical_har_projection(canonical, &trace.row, losses);
            let object = projection
                .as_object()
                .cloned()
                .context("canonical HAR projection is not an object")?;
            output.write_all(b"{")?;
            let mut wrote = true;
            write_json_object_except(
                output,
                &object,
                &["request", "response", "_alex"],
                &mut wrote,
            )?;
            let request_stage = canonical
                .stages
                .iter()
                .find(|stage| stage.kind == "client_request");
            let response_stage = canonical.stages.iter().rev().find(|stage| {
                matches!(stage.kind.as_str(), "client_response" | "injected_response")
            });
            let request_body_id = request_stage
                .and_then(|stage| stage.data.request_body_manifest_ref)
                .map(|id| id.to_string());
            let response_body_id = response_stage
                .and_then(|stage| stage.data.response_body_manifest_ref)
                .map(|id| id.to_string());
            let request_body = request_body_id
                .as_deref()
                .and_then(|id| canonical.bodies.iter().find(|body| body.manifest_id == id));
            let response_body = response_body_id
                .as_deref()
                .and_then(|id| canonical.bodies.iter().find(|body| body.manifest_id == id));
            output.write_all(b",\"request\":{")?;
            let request = object["request"]
                .as_object()
                .context("HAR request is not an object")?;
            let mut request_first = true;
            write_json_object_except(output, request, &["postData"], &mut request_first)?;
            if !request_first {
                output.write_all(b",")?;
            }
            output.write_all(b"\"postData\":")?;
            write_har_canonical_content(store, request_body, output)?;
            output.write_all(b"},\"response\":{")?;
            let response = object["response"]
                .as_object()
                .context("HAR response is not an object")?;
            let mut response_first = true;
            write_json_object_except(output, response, &["content"], &mut response_first)?;
            if !response_first {
                output.write_all(b",")?;
            }
            output.write_all(b"\"content\":")?;
            write_har_canonical_content(store, response_body, output)?;
            output.write_all(b"},\"_alex\":{\"fidelity\":\"canonical\",\"lossReport\":")?;
            serde_json::to_writer(&mut *output, losses)?;
            output.write_all(b",\"canonicalGraph\":")?;
            write_canonical_graph(store, canonical, false, output)?;
            output.write_all(b",\"stageBodyData\":[")?;
            let mut first_body = true;
            for body in &canonical.bodies {
                if request_body_id.as_deref() == Some(body.manifest_id.as_str())
                    || response_body_id.as_deref() == Some(body.manifest_id.as_str())
                {
                    continue;
                }
                if !first_body {
                    output.write_all(b",")?;
                }
                first_body = false;
                write_canonical_body(store, body, output)?;
            }
            output.write_all(b"]")?;
            output.write_all(b"}}")?;
        } else {
            // Legacy HAR remains an explicit projection; its bodies are still
            // streamed as one trace rather than accumulated across selection.
            let request_time = trace.row["ts_request_ms"].as_i64().unwrap_or_default();
            let response_time = trace.row["ts_response_ms"].as_i64().unwrap_or(request_time);
            let legacy = serde_json::json!({
                "startedDateTime": rfc3339_millis(request_time),
                "time": response_time.saturating_sub(request_time).max(0),
                "request": {"method": trace.row["method"], "url": trace.row["path"], "httpVersion":"HTTP/1.1", "cookies":[], "headers":trace.request_headers, "queryString":[], "headersSize":-1, "bodySize":-1},
                "response": {"status":trace.row["status"], "statusText":"", "httpVersion":"HTTP/1.1", "cookies":[], "headers":trace.response_headers, "content":{"size":-1,"mimeType":"application/octet-stream"}, "redirectURL":"", "headersSize":-1, "bodySize":-1},
                "cache":{}, "timings":{"send":0,"wait":response_time.saturating_sub(request_time).max(0),"receive":0},
            });
            let legacy_object = legacy
                .as_object()
                .context("legacy HAR entry is not an object")?;
            output.write_all(b"{")?;
            let mut legacy_first = true;
            write_json_object_except(
                output,
                legacy_object,
                &["request", "response", "_alex"],
                &mut legacy_first,
            )?;
            let trace_id = trace.row["id"].as_str().unwrap_or("unknown");
            output.write_all(b",\"request\":{")?;
            let request = legacy_object["request"]
                .as_object()
                .context("legacy HAR request is not an object")?;
            let mut request_first = true;
            write_json_object_except(output, request, &[], &mut request_first)?;
            output.write_all(b",\"postData\":")?;
            write_har_legacy_content(store, trace_id, "client_request", output)?;
            output.write_all(b"},\"response\":{")?;
            let response = legacy_object["response"]
                .as_object()
                .context("legacy HAR response is not an object")?;
            let mut response_first = true;
            write_json_object_except(output, response, &["content"], &mut response_first)?;
            output.write_all(b",\"content\":")?;
            write_har_legacy_content(store, trace_id, "client_response", output)?;
            output.write_all(b"},\"_alex\":{\"trace\":")?;
            serde_json::to_writer(&mut *output, &sanitized_trace_metadata(&trace.row))?;
            output.write_all(b",\"fidelity\":\"legacy\",\"lossReport\":")?;
            serde_json::to_writer(&mut *output, &export_loss_report())?;
            output.write_all(b",\"upstreamRequest\":")?;
            write_legacy_artifact(store, trace_id, "upstream_request", output)?;
            output.write_all(b"}}")?;
        }
        Ok(())
    })?;
    output.write_all(b"]}}")?;
    Ok(emitted)
}

fn semantic_base_record(trace: &InterchangeExportTrace, openinference: bool) -> Value {
    let alex_trace_id = trace.row["id"].as_str().unwrap_or("unknown");
    let trace_id = semantic_trace_id(alex_trace_id);
    let span_id = semantic_span_id(
        alex_trace_id,
        if openinference {
            b"openinference"
        } else {
            b"otel"
        },
    );
    let prompt_tokens = trace.row["input_tokens"].as_i64();
    let completion_tokens = trace.row["output_tokens"].as_i64();
    let attributes = if openinference {
        serde_json::json!({
            "openinference.span.kind": "LLM",
            "llm.system": openinference_system(&trace.row["upstream_provider"]),
            "llm.provider": openinference_provider(&trace.row["upstream_provider"]),
            "llm.model_name": trace.row["routed_model"],
            "llm.token_count.prompt": prompt_tokens,
            "llm.token_count.prompt_details.cache_read": trace.row["cached_input_tokens"],
            "llm.token_count.prompt_details.cache_write": trace.row["cache_creation_tokens"],
            "llm.token_count.completion": completion_tokens,
            "llm.token_count.completion_details.reasoning": trace.row["reasoning_tokens"],
            "llm.token_count.total": prompt_tokens.zip(completion_tokens).map(|(left, right)| left.saturating_add(right)),
            "llm.cost.total": trace.row["cost_usd"],
            "input.mime_type": "application/json",
            "output.mime_type": "application/json",
            "alex.trace.id": alex_trace_id,
        })
    } else {
        serde_json::json!({
            "gen_ai.operation.name": "chat",
            "gen_ai.provider.name": otel_provider_name(&trace.row["upstream_provider"]),
            "gen_ai.request.model": trace.row["requested_model"],
            "gen_ai.response.model": trace.row["routed_model"],
            "gen_ai.usage.input_tokens": trace.row["input_tokens"],
            "gen_ai.usage.cache_read.input_tokens": trace.row["cached_input_tokens"],
            "gen_ai.usage.cache_creation.input_tokens": trace.row["cache_creation_tokens"],
            "gen_ai.usage.output_tokens": trace.row["output_tokens"],
            "gen_ai.usage.reasoning.output_tokens": trace.row["reasoning_tokens"],
            "http.request.method": trace.row["method"],
            "http.response.status_code": trace.row["status"],
            "alex.trace.id": alex_trace_id,
        })
    };
    serde_json::json!({
        "resource": {"service.name":"alex"},
        "scope": {
            "name": if openinference {"alex.lar.openinference.export"} else {"alex.lar.export"},
            "version": env!("CARGO_PKG_VERSION"),
        },
        "span": {
            "name": if openinference {"LLM"} else {"gen_ai.request"},
            "trace_id": trace_id,
            "span_id": span_id,
            "start_time_unix_nano": trace.row["ts_request_ms"].as_i64().unwrap_or_default().saturating_mul(1_000_000),
            "end_time_unix_nano": trace.row["ts_response_ms"].as_i64().unwrap_or_default().saturating_mul(1_000_000),
            "attributes": attributes,
            "status": if trace.row["error"].is_null() {"OK"} else {"ERROR"},
        },
    })
}

fn stream_semantic_jsonl<W: Write>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    losses: &[&str],
    openinference: bool,
    output: &mut W,
) -> Result<usize> {
    for_each_interchange_trace(store, args, upper, |trace| {
        let base = semantic_base_record(&trace, openinference);
        let object = base
            .as_object()
            .context("semantic export record is not an object")?;
        output.write_all(b"{")?;
        for (index, (key, value)) in object.iter().enumerate() {
            if index > 0 {
                output.write_all(b",")?;
            }
            serde_json::to_writer(&mut *output, key)?;
            output.write_all(b":")?;
            serde_json::to_writer(&mut *output, value)?;
        }
        output.write_all(b",\"alex_loss_report\":")?;
        serde_json::to_writer(&mut *output, losses)?;
        if let Some(canonical) = &trace.canonical {
            output
                .write_all(b",\"alex_capture_fidelity\":\"canonical\",\"alex_canonical_graph\":")?;
            write_canonical_graph(store, canonical, true, output)?;
        } else {
            let trace_id = trace.row["id"].as_str().unwrap_or("unknown");
            output.write_all(b",\"alex_capture_fidelity\":\"legacy\",\"alex_legacy_artifacts\":{\"client_request\":")?;
            write_legacy_artifact(store, trace_id, "client_request", output)?;
            output.write_all(b",\"upstream_request\":")?;
            write_legacy_artifact(store, trace_id, "upstream_request", output)?;
            output.write_all(b",\"client_response\":")?;
            write_legacy_artifact(store, trace_id, "client_response", output)?;
            output.write_all(b"}")?;
        }
        output.write_all(b"}\n")?;
        Ok(())
    })
}

fn stream_otel_jsonl<W: Write>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    losses: &[&str],
    output: &mut W,
) -> Result<usize> {
    stream_semantic_jsonl(store, args, upper, losses, false, output)
}

fn stream_openinference_jsonl<W: Write>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    losses: &[&str],
    output: &mut W,
) -> Result<usize> {
    stream_semantic_jsonl(store, args, upper, losses, true, output)
}

#[allow(clippy::too_many_arguments)]
fn append_warc_stream_record<W: Write, R: Read>(
    output: &mut W,
    record_type: &str,
    record_id: &str,
    concurrent_to: Option<&str>,
    date: &str,
    content_type: &str,
    content_length: u64,
    block_digest_algorithm: &str,
    block_digest: &str,
    input: &mut R,
    losses: &[&str],
) -> Result<()> {
    write!(output, "WARC/1.1\r\n")?;
    write!(output, "WARC-Type: {record_type}\r\n")?;
    write!(output, "WARC-Record-ID: {record_id}\r\n")?;
    write!(output, "WARC-Date: {date}\r\n")?;
    if let Some(id) = concurrent_to {
        write!(output, "WARC-Concurrent-To: {id}\r\n")?;
    }
    write!(
        output,
        "WARC-Block-Digest: {block_digest_algorithm}:{block_digest}\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\n"
    )?;
    write!(
        output,
        "Alex-Fidelity-Loss: {}\r\n\r\n",
        losses.join(" | ").replace(['\r', '\n'], " ")
    )?;
    let copied = std::io::copy(input, output)?;
    if copied != content_length {
        bail!("WARC payload changed while streaming: copied {copied}, expected {content_length}");
    }
    output.write_all(b"\r\n\r\n")?;
    Ok(())
}

fn stream_canonical_warc_body<W: Write>(
    store: &Store,
    trace_id: &str,
    body: &LarInterchangeBody,
    date: &str,
    metadata_record_id: &str,
    losses: &[&str],
    spool_parent: &Path,
    output: &mut W,
) -> Result<()> {
    let record_id = warc_record_id(trace_id, &format!("manifest:{}", body.manifest_id));
    let spool = spool_parent.join(format!(
        ".warc-manifest-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&spool)?;
        let mut tracked = Sha256CountWriter::new(&mut file);
        let written = store.write_lar_manifest_body(&body.manifest_id, &mut tracked)?;
        let (_, length, sha256) = tracked.finish();
        if written != body.total_length || length != body.total_length {
            bail!(
                "canonical WARC body {} changed length during export",
                body.manifest_id
            );
        }
        file.seek(std::io::SeekFrom::Start(0))?;
        append_warc_stream_record(
            output,
            "resource",
            &record_id,
            Some(metadata_record_id),
            date,
            "application/octet-stream",
            length,
            "sha256",
            &sha256,
            &mut file,
            losses,
        )
    })();
    let _ = fs::remove_file(&spool);
    result
}

fn stream_legacy_warc_artifact<W: Write>(
    store: &Store,
    trace_id: &str,
    artifact_kind: &str,
    date: &str,
    metadata_record_id: &str,
    losses: &[&str],
    spool_parent: &Path,
    output: &mut W,
) -> Result<()> {
    if store
        .lar_artifact_location("trace", trace_id, artifact_kind, None)?
        .is_none()
    {
        return Ok(());
    }
    let spool = spool_parent.join(format!(
        ".warc-body-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&spool)?;
        let mut tracked = Sha256CountWriter::new(&mut file);
        if !store.write_lar_or_legacy_artifact(
            "trace",
            trace_id,
            artifact_kind,
            None,
            &mut tracked,
        )? {
            bail!("artifact {artifact_kind} for trace {trace_id} disappeared during export");
        }
        let (_, length, digest) = tracked.finish();
        file.seek(std::io::SeekFrom::Start(0))?;
        append_warc_stream_record(
            output,
            "resource",
            &warc_record_id(trace_id, artifact_kind),
            Some(metadata_record_id),
            date,
            "application/octet-stream",
            length,
            "sha256",
            &digest,
            &mut file,
            losses,
        )
    })();
    let _ = fs::remove_file(&spool);
    result
}

fn append_projected_headers(output: &mut Vec<u8>, headers: &[ExportHeader]) {
    for header in headers {
        output.extend_from_slice(header.name.replace(['\r', '\n'], " ").as_bytes());
        output.extend_from_slice(b": ");
        output.extend_from_slice(header.value.replace(['\r', '\n'], " ").as_bytes());
        output.extend_from_slice(b"\r\n");
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_canonical_http_projection<W: Write>(
    store: &Store,
    _trace_id: &str,
    canonical: &LarInterchangeTrace,
    stage: &LarInterchangeStage,
    record_type: &str,
    record_id: &str,
    concurrent_to: &str,
    date: &str,
    losses: &[&str],
    spool_parent: &Path,
    output: &mut W,
) -> Result<()> {
    let (headers_ref, body_ref) = if record_type == "request" {
        (
            stage.data.request_headers_ref,
            stage.data.request_body_manifest_ref,
        )
    } else {
        (
            stage.data.response_headers_ref,
            stage.data.response_body_manifest_ref,
        )
    };
    let headers = standard_headers(canonical, headers_ref);
    let metadata = canonical.metadata.as_ref();
    let mut prefix = if record_type == "request" {
        let method = metadata_text(metadata.and_then(|value| value.method.as_deref()), "POST")
            .replace(['\r', '\n'], " ");
        let path = metadata_text(metadata.and_then(|value| value.path.as_deref()), "/")
            .replace(['\r', '\n'], " ");
        format!("{} {} HTTP/1.1\r\n", method, path).into_bytes()
    } else {
        format!(
            "HTTP/1.1 {} \r\n",
            stage
                .data
                .status_code
                .or_else(|| metadata
                    .and_then(|value| value.status)
                    .and_then(|value| u16::try_from(value).ok()))
                .unwrap_or_default()
        )
        .into_bytes()
    };
    append_projected_headers(&mut prefix, &headers);
    prefix.extend_from_slice(b"\r\n");
    let body = body_ref
        .map(|id| id.to_string())
        .and_then(|id| canonical.bodies.iter().find(|body| body.manifest_id == id));
    let spool = spool_parent.join(format!(
        ".warc-http-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&spool)?;
        let mut tracked = Sha256CountWriter::new(&mut file);
        tracked.write_all(&prefix)?;
        if let Some(body) = body {
            store.write_lar_manifest_body(&body.manifest_id, &mut tracked)?;
        }
        let (_, length, digest) = tracked.finish();
        file.seek(std::io::SeekFrom::Start(0))?;
        append_warc_stream_record(
            output,
            record_type,
            record_id,
            Some(concurrent_to),
            date,
            &format!("application/http; msgtype={record_type}"),
            length,
            "sha256",
            &digest,
            &mut file,
            losses,
        )
    })();
    let _ = fs::remove_file(&spool);
    result
}

fn stream_canonical_http_timeline<W: Write>(
    store: &Store,
    trace_id: &str,
    canonical: &LarInterchangeTrace,
    date: &str,
    metadata_record_id: &str,
    losses: &[&str],
    spool_parent: &Path,
    output: &mut W,
) -> Result<()> {
    let mut attempt_requests = HashMap::<u32, String>::new();
    let mut client_request = None::<String>;
    for stage in &canonical.stages {
        let is_request = matches!(
            stage.kind.as_str(),
            "client_request" | "upstream_request" | "dario_request"
        );
        let is_response = matches!(
            stage.kind.as_str(),
            "upstream_response"
                | "upstream_failure"
                | "client_response"
                | "dario_response"
                | "injected_response"
        );
        if !is_request && !is_response {
            continue;
        }
        let record_type = if is_request { "request" } else { "response" };
        let record_id =
            warc_record_id(trace_id, &format!("stage:{}:{record_type}", stage.stage_id));
        let concurrent_to = if is_response {
            stage
                .data
                .attempt_number
                .and_then(|attempt| attempt_requests.get(&attempt))
                .or(client_request.as_ref())
                .map(String::as_str)
                .unwrap_or(metadata_record_id)
        } else {
            metadata_record_id
        };
        stream_canonical_http_projection(
            store,
            trace_id,
            canonical,
            stage,
            record_type,
            &record_id,
            concurrent_to,
            date,
            losses,
            spool_parent,
            output,
        )?;
        if is_request {
            if let Some(attempt) = stage.data.attempt_number {
                attempt_requests.insert(attempt, record_id.clone());
            }
            if stage.kind == "client_request" {
                client_request = Some(record_id);
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stream_legacy_http_projection<W: Write>(
    store: &Store,
    trace: &InterchangeExportTrace,
    record_type: &str,
    artifact_kind: &str,
    record_id: &str,
    concurrent_to: &str,
    date: &str,
    losses: &[&str],
    spool_parent: &Path,
    output: &mut W,
) -> Result<()> {
    let trace_id = trace.row["id"].as_str().unwrap_or("unknown");
    let mut prefix = if record_type == "request" {
        let method = trace.row["method"]
            .as_str()
            .unwrap_or("POST")
            .replace(['\r', '\n'], " ");
        let path = trace.row["path"]
            .as_str()
            .unwrap_or("/")
            .replace(['\r', '\n'], " ");
        format!("{} {} HTTP/1.1\r\n", method, path).into_bytes()
    } else {
        format!(
            "HTTP/1.1 {} \r\n",
            trace.row["status"].as_i64().unwrap_or_default()
        )
        .into_bytes()
    };
    append_projected_headers(
        &mut prefix,
        if record_type == "request" {
            &trace.request_headers
        } else {
            &trace.response_headers
        },
    );
    prefix.extend_from_slice(b"\r\n");
    let spool = spool_parent.join(format!(
        ".warc-legacy-http-{}-{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&spool)?;
        let mut tracked = Sha256CountWriter::new(&mut file);
        tracked.write_all(&prefix)?;
        store.write_lar_or_legacy_artifact("trace", trace_id, artifact_kind, None, &mut tracked)?;
        let (_, length, digest) = tracked.finish();
        file.seek(std::io::SeekFrom::Start(0))?;
        append_warc_stream_record(
            output,
            record_type,
            record_id,
            Some(concurrent_to),
            date,
            &format!("application/http; msgtype={record_type}"),
            length,
            "sha256",
            &digest,
            &mut file,
            losses,
        )
    })();
    let _ = fs::remove_file(&spool);
    result
}

fn stream_warc<W: Write>(
    store: &Store,
    args: &ExportArgs,
    upper: &LarExportTraceCursor,
    losses: &[&str],
    output: &mut W,
) -> Result<usize> {
    let spool_parent = args
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for_each_interchange_trace(store, args, upper, |trace| {
        let trace_id = trace.row["id"].as_str().unwrap_or("unknown");
        let date = rfc3339_millis(trace.row["ts_request_ms"].as_i64().unwrap_or_default());
        let metadata_record_id = warc_record_id(trace_id, "canonical-metadata");
        if let Some(canonical) = &trace.canonical {
            let mut metadata = Vec::new();
            write_canonical_graph(store, canonical, false, &mut metadata)?;
            append_warc_record(
                output,
                "metadata",
                &metadata_record_id,
                None,
                &date,
                "application/vnd.alex.lar-canonical+json",
                &metadata,
                losses,
            )?;
            stream_canonical_http_timeline(
                store,
                trace_id,
                canonical,
                &date,
                &metadata_record_id,
                losses,
                spool_parent,
                output,
            )?;
            for body in &canonical.bodies {
                stream_canonical_warc_body(
                    store,
                    trace_id,
                    body,
                    &date,
                    &metadata_record_id,
                    losses,
                    spool_parent,
                    output,
                )?;
            }
        } else {
            let metadata = serde_json::to_vec(&serde_json::json!({
                "schema":"alex.legacy-trace.v1",
                "trace":sanitized_trace_metadata(&trace.row),
                "headers":{"request":trace.request_headers,"response":trace.response_headers},
                "loss_report":export_loss_report(),
            }))?;
            append_warc_record(
                output,
                "metadata",
                &metadata_record_id,
                None,
                &date,
                "application/vnd.alex.legacy-trace+json",
                &metadata,
                losses,
            )?;
            let request_record_id = warc_record_id(trace_id, "request");
            stream_legacy_http_projection(
                store,
                &trace,
                "request",
                "client_request",
                &request_record_id,
                &metadata_record_id,
                &date,
                losses,
                spool_parent,
                output,
            )?;
            stream_legacy_http_projection(
                store,
                &trace,
                "response",
                "client_response",
                &warc_record_id(trace_id, "response"),
                &request_record_id,
                &date,
                losses,
                spool_parent,
                output,
            )?;
            stream_legacy_warc_artifact(
                store,
                trace_id,
                "upstream_request",
                &date,
                &metadata_record_id,
                losses,
                spool_parent,
                output,
            )?;
        }
        Ok(())
    })
}

fn load_export_traces(
    store: &Store,
    args: &ExportArgs,
    load_bodies: bool,
) -> Result<Vec<ExportTrace>> {
    let rows = store
        .export_trace_backup_rows()
        .context("reading trace metadata for export")?;
    let mut selected = Vec::new();
    for row in rows.traces {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .context("trace export row has no string id")?;
        if args.trace_id.as_deref().is_some_and(|wanted| wanted != id) {
            continue;
        }
        if args
            .session
            .as_deref()
            .is_some_and(|wanted| row.get("session_id").and_then(Value::as_str) != Some(wanted))
        {
            continue;
        }
        let (request, upstream_request, response) = if load_bodies {
            (
                store
                    .read_lar_or_legacy_artifact("trace", id, "client_request", None)
                    .with_context(|| format!("reading request body for trace {id}"))?,
                store
                    .read_lar_or_legacy_artifact("trace", id, "upstream_request", None)
                    .with_context(|| format!("reading upstream request body for trace {id}"))?,
                store
                    .read_lar_or_legacy_artifact("trace", id, "client_response", None)
                    .with_context(|| format!("reading response body for trace {id}"))?,
            )
        } else {
            (None, None, None)
        };
        let request_headers = parse_legacy_headers(row.get("req_headers_json"));
        let response_headers = parse_legacy_headers(row.get("resp_headers_json"));
        selected.push(ExportTrace {
            row,
            request_headers,
            response_headers,
            request,
            upstream_request,
            response,
        });
    }
    if let Some(trace_id) = &args.trace_id {
        if selected.is_empty() {
            bail!("trace {trace_id} was not found");
        }
    }
    if let Some(session) = &args.session {
        if selected.is_empty() {
            bail!("session {session} has no traces");
        }
    }
    Ok(selected)
}

fn parse_legacy_headers(value: Option<&Value>) -> Vec<ExportHeader> {
    let parsed = match value {
        Some(Value::String(raw)) => serde_json::from_str(raw).ok(),
        Some(value @ (Value::Array(_) | Value::Object(_))) => Some(value.clone()),
        _ => None,
    };
    let Some(parsed) = parsed else {
        return Vec::new();
    };
    let mut headers = Vec::new();
    match parsed {
        Value::Object(values) => {
            for (name, value) in values {
                match value {
                    Value::Array(items) => {
                        for item in items {
                            if let Some(value) = header_value_string(&item) {
                                headers.push(ExportHeader {
                                    name: name.clone(),
                                    value,
                                });
                            }
                        }
                    }
                    value => {
                        if let Some(value) = header_value_string(&value) {
                            headers.push(ExportHeader { name, value });
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
                            (pair[0].as_str(), header_value_string(&pair[1]))
                        {
                            headers.push(ExportHeader {
                                name: name.to_owned(),
                                value,
                            });
                        }
                    }
                    Value::Object(pair) => {
                        if let (Some(name), Some(value)) = (
                            pair.get("name").and_then(Value::as_str),
                            pair.get("value").and_then(header_value_string),
                        ) {
                            headers.push(ExportHeader {
                                name: name.to_owned(),
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
    headers
}

fn header_value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn sanitized_trace_metadata(row: &Value) -> Value {
    let mut metadata = row.clone();
    if let Some(object) = metadata.as_object_mut() {
        for field in ["req_body_path", "upstream_req_body_path", "resp_body_path"] {
            object.remove(field);
        }
    }
    metadata
}

fn rfc3339_millis(timestamp_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_owned())
}

fn warc_record_id(trace_id: &str, kind: &str) -> String {
    let digest = Sha256::digest(format!("alex:{trace_id}:{kind}").as_bytes());
    format!("<urn:alex:sha256:{}>", hex_bytes(&digest))
}

#[allow(clippy::too_many_arguments)]
fn append_warc_record<W: Write>(
    output: &mut W,
    record_type: &str,
    record_id: &str,
    concurrent_to: Option<&str>,
    date: &str,
    content_type: &str,
    payload: &[u8],
    losses: &[&str],
) -> Result<()> {
    let block_digest = hex_bytes(&Sha256::digest(payload));
    write!(output, "WARC/1.1\r\n")?;
    write!(output, "WARC-Type: {record_type}\r\n")?;
    write!(output, "WARC-Record-ID: {record_id}\r\n")?;
    write!(output, "WARC-Date: {date}\r\n")?;
    if let Some(id) = concurrent_to {
        write!(output, "WARC-Concurrent-To: {id}\r\n")?;
    }
    write!(output, "WARC-Block-Digest: sha256:{block_digest}\r\n")?;
    write!(output, "Content-Type: {content_type}\r\n")?;
    write!(output, "Content-Length: {}\r\n", payload.len())?;
    write!(
        output,
        "Alex-Fidelity-Loss: {}\r\n\r\n",
        losses.join(" | ").replace(['\r', '\n'], " ")
    )?;
    output.write_all(payload)?;
    output.write_all(b"\r\n\r\n")?;
    Ok(())
}

fn semantic_trace_id(value: &str) -> String {
    let compact = value.replace('-', "");
    if compact.len() == 32 && compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return compact.to_ascii_lowercase();
    }
    let digest = Sha256::digest(value.as_bytes());
    hex_bytes(&digest[..16])
}

fn semantic_span_id(trace_id: &str, domain: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(&[0]);
    hash.update(trace_id.as_bytes());
    hex_bytes(&hash.finalize()[..8])
}

fn provider_string(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.is_empty())
}

fn otel_provider_name(value: &Value) -> Value {
    provider_string(value).map_or(Value::Null, |value| {
        Value::String(
            match value {
                "xai" | "grok" => "x_ai",
                "gemini" | "google" => "gcp.gemini",
                other => other,
            }
            .to_string(),
        )
    })
}

fn openinference_system(value: &Value) -> Value {
    provider_string(value).map_or(Value::Null, |value| {
        Value::String(
            match value {
                "gemini" | "google" => "vertexai",
                "grok" => "xai",
                other => other,
            }
            .to_string(),
        )
    })
}

fn openinference_provider(value: &Value) -> Value {
    provider_string(value).map_or(Value::Null, |value| {
        Value::String(
            match value {
                "gemini" | "google" => "google",
                "grok" => "xai",
                "kimi" => "moonshot",
                other => other,
            }
            .to_string(),
        )
    })
}

/// Build the self-contained body closure used by trace-backup v2. Trace
/// exchanges retain their ordinary standalone records; tool bodies are stored
/// once as manifests and returned as explicit owner edges for restore.
pub(crate) fn build_trace_backup_lar(
    store: &Store,
    rows: &TraceBackupRows,
) -> Result<(Vec<u8>, Vec<LarBackupArtifactRef>)> {
    let mut traces = Vec::with_capacity(rows.traces.len());
    for row in &rows.traces {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .context("trace backup row has no string id")?;
        traces.push(ExportTrace {
            request_headers: parse_legacy_headers(row.get("req_headers_json")),
            response_headers: parse_legacy_headers(row.get("resp_headers_json")),
            request: store
                .read_lar_or_legacy_artifact("trace", id, "client_request", None)
                .with_context(|| format!("reading request body for backup trace {id}"))?,
            upstream_request: store
                .read_lar_or_legacy_artifact("trace", id, "upstream_request", None)
                .with_context(|| format!("reading upstream request for backup trace {id}"))?,
            response: store
                .read_lar_or_legacy_artifact("trace", id, "client_response", None)
                .with_context(|| format!("reading response body for backup trace {id}"))?,
            row: row.clone(),
        });
    }

    let mut extras = Vec::new();
    for row in &rows.tool_calls {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .context("tool-call backup row has no string id")?;
        for artifact_kind in ["tool_arguments", "tool_result"] {
            if let Some(bytes) = store
                .read_lar_or_legacy_artifact("tool_call", id, artifact_kind, None)
                .with_context(|| format!("reading {artifact_kind} for backup tool call {id}"))?
            {
                extras.push(BackupExtraBody {
                    owner_id: id.to_string(),
                    artifact_kind,
                    bytes,
                });
            }
        }
    }
    build_standalone_lar_with_extras(Some(store), &traces, &extras)
}

fn build_standalone_lar_with_extras(
    store: Option<&Store>,
    traces: &[ExportTrace],
    extras: &[BackupExtraBody],
) -> Result<(Vec<u8>, Vec<LarBackupArtifactRef>)> {
    let cursor = std::io::Cursor::new(Vec::new());
    let (cursor, artifacts) = write_standalone_lar_to(store, traces, extras, cursor)?;
    Ok((cursor.into_inner(), artifacts))
}

fn write_standalone_lar_to<W: Read + Write + Seek>(
    store: Option<&Store>,
    traces: &[ExportTrace],
    extras: &[BackupExtraBody],
    output: W,
) -> Result<(W, Vec<LarBackupArtifactRef>)> {
    let created_at_ns = traces
        .first()
        .and_then(|trace| trace.row["ts_request_ms"].as_u64())
        .unwrap_or_default()
        .saturating_mul(1_000_000);
    let file_uuid = *uuid::Uuid::new_v4().as_bytes();
    let mut header = FileHeader::standalone(file_uuid, created_at_ns, b"alex-lar-export".to_vec());
    if let Some(store) = store {
        for trace in traces {
            let trace_id = trace.row["id"]
                .as_str()
                .context("trace export row has no id")?;
            if store.lar_conversation_has_turn(trace_id)? {
                header.required_feature_bits |= REQUIRED_FEATURE_CONVERSATION_DAG;
                break;
            }
        }
    }
    let mut writer =
        ArchiveWriter::create(output, header, ChunkerConfig::default(), Limits::default())
            .map_err(|error| anyhow::anyhow!(error))?;

    for (capture_sequence, trace) in traces.iter().enumerate() {
        let trace_id = trace.row["id"]
            .as_str()
            .context("trace export row has no id")?;
        if let Some(store) = store {
            if store.append_exact_trace_to_standalone(&mut writer, trace_id)? {
                continue;
            }
        }
        let request_fallback = if trace.request.is_none() {
            store
                .map(|store| {
                    store.read_lar_or_legacy_artifact("trace", trace_id, "client_request", None)
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        let upstream_fallback = if trace.upstream_request.is_none() {
            store
                .map(|store| {
                    store.read_lar_or_legacy_artifact("trace", trace_id, "upstream_request", None)
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        let response_fallback = if trace.response.is_none() {
            store
                .map(|store| {
                    store.read_lar_or_legacy_artifact("trace", trace_id, "client_response", None)
                })
                .transpose()?
                .flatten()
        } else {
            None
        };
        let request_manifest = trace
            .request
            .as_deref()
            .or(request_fallback.as_deref())
            .map(|bytes| writer.append_body(bytes))
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;
        let upstream_manifest = trace
            .upstream_request
            .as_deref()
            .or(upstream_fallback.as_deref())
            .map(|bytes| writer.append_body(bytes))
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;
        let response_manifest = trace
            .response
            .as_deref()
            .or(response_fallback.as_deref())
            .map(|bytes| writer.append_body(bytes))
            .transpose()
            .map_err(|error| anyhow::anyhow!(error))?;
        let request_headers = append_export_headers(&mut writer, &trace.request_headers)?;
        let response_headers = append_export_headers(&mut writer, &trace.response_headers)?;
        let request_time = trace.row["ts_request_ms"]
            .as_u64()
            .unwrap_or_default()
            .saturating_mul(1_000_000);
        let response_time = trace.row["ts_response_ms"]
            .as_u64()
            .unwrap_or_else(|| request_time / 1_000_000)
            .saturating_mul(1_000_000);
        let mut stage_ids = Vec::new();

        let mut client_request = StageData::new(StageKind::ClientRequest, request_time);
        client_request.request_headers_ref = request_headers;
        client_request.request_body_manifest_ref = request_manifest;
        client_request.requested_model = json_bytes(&trace.row["requested_model"]);
        stage_ids.push(
            writer
                .append_stage(Stage::new(client_request))
                .map_err(|error| anyhow::anyhow!(error))?,
        );

        let mut routing = StageData::new(StageKind::RouterDecision, request_time);
        routing.provider = json_bytes(&trace.row["upstream_provider"]);
        routing.requested_model = json_bytes(&trace.row["requested_model"]);
        routing.routed_model = json_bytes(&trace.row["routed_model"]);
        routing.account_id = json_bytes(&trace.row["account_id"]);
        routing.routing_reason = json_bytes(&trace.row["substitution_reason"]);
        stage_ids.push(
            writer
                .append_stage(Stage::new(routing))
                .map_err(|error| anyhow::anyhow!(error))?,
        );

        let mut upstream_request = StageData::new(StageKind::UpstreamRequest, request_time);
        upstream_request.attempt_number = Some(1);
        upstream_request.request_body_manifest_ref = upstream_manifest;
        upstream_request.provider = json_bytes(&trace.row["upstream_provider"]);
        upstream_request.requested_model = json_bytes(&trace.row["requested_model"]);
        upstream_request.routed_model = json_bytes(&trace.row["routed_model"]);
        stage_ids.push(
            writer
                .append_stage(Stage::new(upstream_request))
                .map_err(|error| anyhow::anyhow!(error))?,
        );

        let mut upstream_response = StageData::new(StageKind::UpstreamResponse, response_time);
        upstream_response.attempt_number = Some(1);
        upstream_response.response_body_manifest_ref = response_manifest;
        upstream_response.provider = json_bytes(&trace.row["upstream_provider"]);
        upstream_response.status_code = trace.row["status"]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok());
        stage_ids.push(
            writer
                .append_stage(Stage::new(upstream_response))
                .map_err(|error| anyhow::anyhow!(error))?,
        );

        let mut client_response = StageData::new(StageKind::ClientResponse, response_time);
        client_response.response_headers_ref = response_headers;
        client_response.response_body_manifest_ref = response_manifest;
        client_response.status_code = trace.row["status"]
            .as_u64()
            .and_then(|value| u16::try_from(value).ok());
        client_response.usage = Some(TokenUsage {
            input_tokens: json_u64(&trace.row["input_tokens"]),
            output_tokens: json_u64(&trace.row["output_tokens"]),
            cached_tokens: json_u64(&trace.row["cached_input_tokens"]),
            reasoning_tokens: json_u64(&trace.row["reasoning_tokens"]),
        });
        client_response.error_class = json_bytes(&trace.row["error_class"]);
        client_response.error_message = json_bytes(&trace.row["error"]);
        if let Some(cost) = trace.row["cost_usd"].as_f64() {
            client_response.cost_nanos = Some((cost.max(0.0) * 1_000_000_000.0) as u64);
            client_response.cost_currency = Some(b"USD".to_vec());
        }
        stage_ids.push(
            writer
                .append_stage(Stage::new(client_response))
                .map_err(|error| anyhow::anyhow!(error))?,
        );

        let mut exchange = ExchangeData::new(
            trace_id.as_bytes(),
            capture_sequence as u64,
            request_time,
            stage_ids,
        );
        exchange.session_id = json_bytes(&trace.row["session_id"]);
        exchange.run_id = json_bytes(&trace.row["run_id"]);
        writer
            .append_exchange_with_metadata(
                Exchange::new(exchange),
                export_exchange_metadata(&trace.row),
            )
            .map_err(|error| anyhow::anyhow!(error))?;
    }
    let mut artifact_refs = Vec::with_capacity(extras.len());
    for extra in extras {
        writer
            .append_body(&extra.bytes)
            .map_err(|error| anyhow::anyhow!(error))?;
        artifact_refs.push(LarBackupArtifactRef {
            owner_kind: "tool_call".into(),
            owner_id: extra.owner_id.clone(),
            artifact_kind: extra.artifact_kind.into(),
            stage_id: String::new(),
            blake3: blake3::hash(&extra.bytes).to_hex().to_string(),
            total_length: extra.bytes.len() as u64,
        });
    }
    writer.seal().map_err(|error| anyhow::anyhow!(error))?;
    let output = writer
        .into_inner()
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok((output, artifact_refs))
}

fn export_exchange_metadata(row: &Value) -> ExchangeMetadataData {
    let string_bytes = |name: &str| row[name].as_str().map(str::as_bytes).map(Vec::from);
    ExchangeMetadataData {
        ts_request_ms: row["ts_request_ms"].as_i64(),
        ts_response_ms: row["ts_response_ms"].as_i64(),
        harness: string_bytes("harness"),
        client_format: string_bytes("client_format"),
        upstream_format: string_bytes("upstream_format"),
        method: string_bytes("method"),
        path: string_bytes("path"),
        streamed: json_bool(&row["streamed"]),
        status: row["status"].as_i64(),
        cost_usd_bits: row["cost_usd"].as_f64().map(f64::to_bits),
        billing_bucket: string_bytes("billing_bucket"),
        error_kind: string_bytes("error_kind"),
        error_code: string_bytes("error_code"),
        substituted: json_bool(&row["substituted"]).unwrap_or(false),
        original_model: string_bytes("original_model"),
        served_model: string_bytes("served_model"),
        substitution_reason: string_bytes("substitution_reason"),
        injected: json_bool(&row["injected"]).unwrap_or(false),
        fixture_name: string_bytes("fixture_name"),
        attempts_json: json_encoded_bytes(&row["attempts"]),
        original_account_id: string_bytes("original_account_id"),
        served_account_id: string_bytes("served_account_id"),
        subscription_identity: string_bytes("subscription_identity"),
        via_dario: json_bool(&row["via_dario"]).unwrap_or(false),
        dario_generation: string_bytes("dario_generation"),
        tags_json: string_bytes("tags_json"),
        client_ip: string_bytes("client_ip"),
        key_fingerprint: string_bytes("key_fingerprint"),
        reasoning_effort: string_bytes("reasoning_effort"),
        thinking_budget: row["thinking_budget"].as_i64(),
        input_tokens: row["input_tokens"].as_i64(),
        cached_input_tokens: row["cached_input_tokens"].as_i64(),
        cache_creation_tokens: row["cache_creation_tokens"].as_i64(),
        output_tokens: row["output_tokens"].as_i64(),
        reasoning_tokens: row["reasoning_tokens"].as_i64(),
        unknown_attributes: Vec::new(),
    }
}

fn json_bool(value: &Value) -> Option<bool> {
    value
        .as_bool()
        .or_else(|| value.as_i64().map(|value| value != 0))
}

fn json_encoded_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.as_bytes().to_vec()),
        value => serde_json::to_vec(value).ok(),
    }
}

fn append_export_headers<W: Read + Write + std::io::Seek>(
    writer: &mut ArchiveWriter<W>,
    headers: &[ExportHeader],
) -> Result<Option<alex_lar::HeaderBlockId>> {
    if headers.is_empty() {
        return Ok(None);
    }
    let block = HeaderBlock::new(
        HeaderFidelity::LegacyOrderAndCasingUnknown,
        headers
            .iter()
            .map(|header| HeaderAtom {
                original_name: header.name.as_bytes().to_vec(),
                value: header.value.as_bytes().to_vec(),
                flags: 0,
            })
            .collect(),
    );
    writer
        .append_header_block(block)
        .map(Some)
        .map_err(|error| anyhow::anyhow!(error))
}

fn json_bytes(value: &Value) -> Option<Vec<u8>> {
    value.as_str().map(|value| value.as_bytes().to_vec())
}

fn json_u64(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .unwrap_or_default()
}

fn write_standalone_lar_export(
    store: &Store,
    traces: &[ExportTrace],
    output: &Path,
    force: bool,
) -> Result<u64> {
    if output.exists() && !force {
        bail!(
            "export output already exists: {} (use --force to replace it)",
            output.display()
        );
    }
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("export");
    let temporary = parent.join(format!(".{name}.{}.lar-export.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<u64> {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary)?;
        let (file, _) = write_standalone_lar_to(Some(store), traces, &[], file)?;
        file.sync_all()?;
        drop(file);
        verify_standalone_lar_export(&temporary)?;
        let bytes = fs::metadata(&temporary)?.len();
        publish_export_temp(&temporary, output, force)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn verify_standalone_lar_export(path: &Path) -> Result<()> {
    let file = fs::File::open(path)?;
    let mut reader =
        ArchiveReader::open(file, Limits::default()).map_err(|error| anyhow::anyhow!(error))?;
    if reader.header().file_role != alex_lar::FileRole::Standalone
        || reader.recovery_status() != RecoveryStatus::Clean
        || !reader.is_sealed()
    {
        bail!("standalone LAR export did not seal cleanly");
    }
    let manifest_ids = reader.manifest_ids().copied().collect::<Vec<_>>();
    for id in manifest_ids {
        reader
            .write_body(&id, std::io::sink())
            .map_err(|error| anyhow::anyhow!(error))?;
    }
    Ok(())
}

fn publish_export_temp(temporary: &Path, output: &Path, force: bool) -> Result<()> {
    if !force {
        // A sibling hard-link is an atomic no-clobber publish. If another
        // process creates OUTPUT after the initial check, the link fails and
        // that process's file remains untouched.
        fs::hard_link(temporary, output).with_context(|| {
            format!(
                "publishing export without replacing {} (use --force to replace it)",
                output.display()
            )
        })?;
        fs::remove_file(temporary)?;
        return Ok(());
    }

    #[cfg(windows)]
    if output.exists() {
        fs::remove_file(output)
            .with_context(|| format!("replacing export output {}", output.display()))?;
    }
    fs::rename(temporary, output)
        .with_context(|| format!("publishing export output {}", output.display()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[derive(Debug, Serialize)]
struct LegacyInventory {
    mode: &'static str,
    unique_body_references: usize,
    selected_body_references: usize,
    selected_compressed_bytes: u64,
    missing_body_references: usize,
    corrupt_body_references: usize,
    source_bodies_verified: usize,
    decompressed_bytes_verified: u64,
    inline_header_records: usize,
    inline_header_bytes: u64,
    limited: bool,
    writes_performed: u8,
}

fn legacy_import_inventory(
    data_dir: &Path,
    limit: Option<usize>,
    verify_sources: bool,
) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf())?;
    let rows = store
        .export_trace_backup_rows()
        .context("reading legacy trace and tool artifact references")?;

    let mut paths = BTreeSet::new();
    let mut inline_header_records = 0_usize;
    let mut inline_header_bytes = 0_u64;
    for row in &rows.traces {
        for field in ["req_body_path", "upstream_req_body_path", "resp_body_path"] {
            if let Some(path) = row.get(field).and_then(Value::as_str) {
                paths.insert(path.to_owned());
            }
        }
        for field in ["req_headers_json", "resp_headers_json"] {
            if let Some(headers) = row.get(field).and_then(Value::as_str) {
                inline_header_records += 1;
                inline_header_bytes += headers.len() as u64;
            }
        }
    }
    for row in &rows.tool_calls {
        for field in ["args_body_path", "result_body_path"] {
            if let Some(path) = row.get(field).and_then(Value::as_str) {
                paths.insert(path.to_owned());
            }
        }
    }

    let unique_body_references = paths.len();
    let selected_limit = limit.unwrap_or(usize::MAX);
    let mut selected_body_references = 0_usize;
    let mut selected_compressed_bytes = 0_u64;
    let mut missing_body_references = 0_usize;
    let mut corrupt_body_references = 0_usize;
    let mut source_bodies_verified = 0_usize;
    let mut decompressed_bytes_verified = 0_u64;
    for stored_path in paths.iter().take(selected_limit) {
        selected_body_references += 1;
        let path = resolve_legacy_path(data_dir, stored_path);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                selected_compressed_bytes += metadata.len();
                if verify_sources {
                    let verification = (|| -> Result<u64> {
                        let file = fs::File::open(&path)?;
                        let mut decoder = flate2::read::GzDecoder::new(file);
                        let mut buffer = [0_u8; 64 * 1024];
                        let mut length = 0_u64;
                        loop {
                            let read = decoder.read(&mut buffer)?;
                            if read == 0 {
                                break;
                            }
                            length = length
                                .checked_add(read as u64)
                                .context("decompressed legacy body length overflow")?;
                        }
                        Ok(length)
                    })();
                    match verification {
                        Ok(length) => {
                            source_bodies_verified += 1;
                            decompressed_bytes_verified += length;
                        }
                        Err(_) => corrupt_body_references += 1,
                    }
                }
            }
            _ => missing_body_references += 1,
        }
    }

    let inventory = LegacyInventory {
        mode: "dry-run",
        unique_body_references,
        selected_body_references,
        selected_compressed_bytes,
        missing_body_references,
        corrupt_body_references,
        source_bodies_verified,
        decompressed_bytes_verified,
        inline_header_records,
        inline_header_bytes,
        limited: selected_body_references < unique_body_references,
        writes_performed: 0,
    };
    let human = format!(
        "legacy import dry-run: {} of {} unique body references, {} compressed bytes, {} missing, {} corrupt; {} source bodies verified ({} decompressed bytes); {} inline header records ({} bytes); no LAR records or trace pointers were written{}",
        inventory.selected_body_references,
        inventory.unique_body_references,
        inventory.selected_compressed_bytes,
        inventory.missing_body_references,
        inventory.corrupt_body_references,
        inventory.source_bodies_verified,
        inventory.decompressed_bytes_verified,
        inventory.inline_header_records,
        inventory.inline_header_bytes,
        if inventory.limited { " (limited)" } else { "" },
    );
    Ok(LarCommandOutput {
        human,
        json: serde_json::to_value(inventory)?,
        raw_body: None,
    })
}

fn run_legacy_import(data_dir: &Path, args: &ImportLegacyArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let options = LarLegacyImportOptions {
        limit: args.limit,
        ..LarLegacyImportOptions::default()
    };
    let report = store
        .run_lar_legacy_import(&options)
        .context("importing legacy gzip bodies into LAR")?;
    let human = if !report.claimed && report.job_state != "complete" {
        format!(
            "LAR legacy import job {} is {}; another worker may hold its lease. No duplicate worker was started.",
            report.job_id, report.job_state
        )
    } else {
        format!(
            "LAR legacy import {}: {} inventoried, {} attempted, {} migrated, {} skipped, {} failed; {} bytes read, {} unique bytes written, {} bytes deduplicated{}; every published pointer passed LAR readback length+BLAKE3 validation",
            report.job_state,
            report.inventoried,
            report.attempted,
            report.migrated,
            report.skipped,
            report.failed,
            report.bytes_read,
            report.unique_bytes_written,
            report.bytes_deduplicated,
            if report.limit_reached { " (limit reached)" } else { "" },
        )
    };
    Ok(LarCommandOutput {
        human,
        json: serde_json::to_value(report)?,
        raw_body: None,
    })
}

fn resolve_legacy_path(data_dir: &Path, stored_path: &str) -> PathBuf {
    let path = Path::new(stored_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        data_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: LarCommand,
    }

    fn parse(args: &[&str]) -> LarCommand {
        TestCli::try_parse_from(std::iter::once("lar-test").chain(args.iter().copied()))
            .unwrap()
            .command
    }

    fn tmpdir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "alex-lar-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[derive(Debug)]
    struct ParsedWarcRecord {
        headers: HashMap<String, String>,
        payload: Vec<u8>,
    }

    fn parse_warc_records(bytes: &[u8]) -> Vec<ParsedWarcRecord> {
        let mut records = Vec::new();
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            assert!(bytes[cursor..].starts_with(b"WARC/1.1\r\n"));
            let relative_header_end = bytes[cursor..]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("WARC record has a header terminator");
            let header_end = cursor + relative_header_end;
            let header_text = std::str::from_utf8(&bytes[cursor..header_end]).unwrap();
            let headers = header_text
                .split("\r\n")
                .skip(1)
                .map(|line| line.split_once(": ").expect("valid WARC header"))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect::<HashMap<_, _>>();
            let content_length = headers["Content-Length"].parse::<usize>().unwrap();
            let payload_start = header_end + 4;
            let payload_end = payload_start + content_length;
            assert!(payload_end + 4 <= bytes.len());
            assert_eq!(&bytes[payload_end..payload_end + 4], b"\r\n\r\n");
            records.push(ParsedWarcRecord {
                headers,
                payload: bytes[payload_start..payload_end].to_vec(),
            });
            cursor = payload_end + 4;
        }
        records
    }

    fn create_migration_job(store: &Store, job_id: &str, source_key: &str, now_ms: i64) {
        store
            .ensure_lar_migration_job(
                &alex_store::LarMigrationJobSpec {
                    job_id: job_id.to_owned(),
                    format_version: 1,
                    source_version: "legacy-v1".to_owned(),
                    source_key: source_key.to_owned(),
                },
                now_ms,
            )
            .unwrap();
    }

    fn write_test_archive(path: &Path, interrupted_tail: bool) {
        use alex_lar::{ArchiveWriter, ChunkerConfig, FileHeader, Limits};

        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut writer = ArchiveWriter::create(
            file,
            FileHeader::standalone([7; 16], 123, b"lar-cli-test".to_vec()),
            ChunkerConfig::default(),
            Limits::default(),
        )
        .unwrap();
        writer.append_body(b"first complete body").unwrap();
        writer.flush().unwrap();
        let complete_length = fs::metadata(path).unwrap().len();
        if interrupted_tail {
            writer.append_body(&vec![42; 200_000]).unwrap();
        }
        let file = writer.into_inner().unwrap();
        file.sync_all().unwrap();
        drop(file);
        if interrupted_tail {
            fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_len(complete_length + 11)
                .unwrap();
        }
    }

    fn write_identity_archive(path: &Path, file_uuid: [u8; 16], body: &[u8]) -> String {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut writer = ArchiveWriter::create(
            file,
            FileHeader::standalone(file_uuid, 123, b"lar-cli-archive-lifecycle".to_vec()),
            ChunkerConfig::default(),
            Limits::default(),
        )
        .unwrap();
        let manifest = writer.append_body(body).unwrap();
        writer.seal().unwrap();
        let file = writer.into_inner().unwrap();
        file.sync_all().unwrap();
        manifest.to_string()
    }

    fn write_archive_with_two_corrupt_manifests(path: &Path) -> Vec<String> {
        use std::io::{Seek, SeekFrom};

        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut writer = ArchiveWriter::create(
            file,
            FileHeader::standalone([6; 16], 123, b"lar-verify-keep-going".to_vec()),
            ChunkerConfig::default(),
            Limits::default(),
        )
        .unwrap();
        let mut manifest_ids = [
            writer
                .append_body(b"first independently corrupt body")
                .unwrap(),
            writer
                .append_body(b"second independently corrupt body")
                .unwrap(),
        ]
        .map(|manifest| manifest.to_string())
        .to_vec();
        manifest_ids.sort();
        writer.seal().unwrap();
        let file = writer.into_inner().unwrap();
        file.sync_all().unwrap();
        drop(file);

        let reader = ArchiveReader::open(fs::File::open(path).unwrap(), Limits::default()).unwrap();
        let mut chunk_offsets = reader
            .chunk_records()
            .map(|descriptor| descriptor.frame_offset)
            .collect::<Vec<_>>();
        chunk_offsets.sort_unstable();
        assert_eq!(chunk_offsets.len(), 2);
        drop(reader);

        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        for frame_offset in chunk_offsets {
            file.seek(SeekFrom::Start(frame_offset)).unwrap();
            let (status, frame) = alex_lar::FrameReader::new(&mut file, &Limits::default())
                .read_next()
                .unwrap();
            assert_eq!(status, alex_lar::FrameRead::Frame);
            let mut frame = frame.unwrap();
            assert_eq!(frame.record_type, alex_lar::RecordType::Chunk);
            // Keep the frame structurally valid (including its CRC) while
            // changing the compressed body. Fast footer loading can still
            // index the archive; the corruption surfaces only when a
            // referencing manifest reconstructs and verifies its chunk.
            *frame.payload.last_mut().unwrap() ^= 1;
            file.seek(SeekFrom::Start(frame_offset)).unwrap();
            frame.write(&mut file).unwrap();
        }
        file.sync_all().unwrap();
        manifest_ids
    }

    fn write_search_archive(path: &Path) {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut writer = ArchiveWriter::create(
            file,
            FileHeader::standalone([8; 16], 123, b"lar-grep-test".to_vec()),
            ChunkerConfig {
                min_size: 4,
                target_size: 4,
                max_size: 4,
            },
            Limits::default(),
        )
        .unwrap();
        for (index, body) in [b"abcdNEEDtail".as_slice(), b"abcdNONEtail".as_slice()]
            .into_iter()
            .enumerate()
        {
            let manifest = writer.append_body(body).unwrap();
            let mut stage = StageData::new(StageKind::ClientRequest, 1_000 + index as u64);
            stage.request_body_manifest_ref = Some(manifest);
            if index == 0 {
                stage.provider = Some(b"sealed-provider-needle".to_vec());
                stage.routing_reason = Some(b"sealed-control-needle".to_vec());
                stage.request_headers_ref = Some(
                    writer
                        .append_header_block(HeaderBlock::new(
                            HeaderFidelity::Exact,
                            vec![
                                HeaderAtom {
                                    original_name: b"x-search-safe".to_vec(),
                                    value: b"sealed-header-needle".to_vec(),
                                    flags: 0,
                                },
                                // Deliberately unflagged to model a foreign
                                // archive: name-based safety must still win.
                                HeaderAtom {
                                    original_name: b"Authorization".to_vec(),
                                    value: b"sealed-secret-must-not-match".to_vec(),
                                    flags: 0,
                                },
                            ],
                        ))
                        .unwrap(),
                );
                stage.trailers_ref = Some(
                    writer
                        .append_header_block(HeaderBlock::new(
                            HeaderFidelity::Exact,
                            vec![HeaderAtom {
                                original_name: b"x-safe-trailer".to_vec(),
                                value: b"sealed-trailer-needle".to_vec(),
                                flags: 0,
                            }],
                        ))
                        .unwrap(),
                );
            }
            let stage_id = writer.append_stage(Stage::new(stage)).unwrap();
            let mut exchange = ExchangeData::new(
                format!("trace-{index}"),
                index as u64,
                1_000 + index as u64,
                vec![stage_id],
            );
            exchange.session_id = Some(format!("session-{index}").into_bytes());
            let mut metadata = ExchangeMetadataData::default();
            if index == 0 {
                metadata.harness = Some(b"sealed-harness-needle".to_vec());
            }
            writer
                .append_exchange_with_metadata(Exchange::new(exchange), metadata)
                .unwrap();
        }
        writer.seal().unwrap();
        let file = writer.into_inner().unwrap();
        file.sync_all().unwrap();
    }

    fn write_replay_archive(path: &Path) -> (String, String, String) {
        use alex_lar::{ParsedFrame, StreamFrameKind, StreamIndex, StreamParser, StreamRead};

        let file = fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let mut writer = ArchiveWriter::create(
            file,
            FileHeader::standalone([9; 16], 123, b"lar-replay-test".to_vec()),
            ChunkerConfig::default(),
            Limits::default(),
        )
        .unwrap();
        let body = b"data: one\n\ndata: two\n\n";
        let manifest = writer.append_body(body).unwrap();
        let stream = writer
            .append_stream_index(StreamIndex::new(
                manifest,
                vec![
                    StreamRead {
                        byte_offset: 0,
                        byte_length: 11,
                        delta_from_first_byte_ns: 0,
                    },
                    StreamRead {
                        byte_offset: 11,
                        byte_length: 11,
                        delta_from_first_byte_ns: 1_000,
                    },
                ],
                vec![
                    ParsedFrame {
                        byte_offset: 0,
                        byte_length: 9,
                        delta_from_first_byte_ns: 0,
                        parser: StreamParser::Sse,
                        frame_kind: StreamFrameKind::SseEvent,
                    },
                    ParsedFrame {
                        byte_offset: 11,
                        byte_length: 9,
                        delta_from_first_byte_ns: 1_000,
                        parser: StreamParser::Sse,
                        frame_kind: StreamFrameKind::SseEvent,
                    },
                ],
            ))
            .unwrap();
        let mut stage = StageData::new(StageKind::ClientResponse, 123);
        stage.response_body_manifest_ref = Some(manifest);
        stage.stream_index_ref = Some(stream);
        let stage = writer.append_stage(Stage::new(stage)).unwrap();
        writer
            .append_exchange(Exchange::new(ExchangeData::new(
                b"replay-trace".to_vec(),
                1,
                123,
                vec![stage],
            )))
            .unwrap();
        writer.seal().unwrap();
        let file = writer.into_inner().unwrap();
        file.sync_all().unwrap();
        (
            stage.to_string(),
            String::from_utf8_lossy(body).into_owned(),
            "data: onedata: two".into(),
        )
    }

    fn grep_args(literal: &str, archives: Vec<PathBuf>, limit: usize) -> GrepArgs {
        GrepArgs {
            literal: literal.into(),
            archives,
            limit,
            scope: LarGrepScope::Bodies,
            json: true,
        }
    }

    #[test]
    fn parses_import_and_migration_commands() {
        assert!(matches!(
            parse(&["import", "archive.lar", "--json"]),
            LarCommand::Import(ImportArgs {
                format: LarImportFormat::Auto,
                json: true,
                ..
            })
        ));
        assert!(matches!(
            parse(&["import", "archive.jsonl", "--format", "jsonl"]),
            LarCommand::Import(ImportArgs {
                format: LarImportFormat::Jsonl,
                ..
            })
        ));
        match parse(&["import-legacy", "--dry-run", "--limit", "42", "--json"]) {
            LarCommand::ImportLegacy(args) => {
                assert!(args.dry_run);
                assert_eq!(args.limit, Some(42));
                assert!(args.json);
            }
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(matches!(
            parse(&["migration", "verify", "--json"]),
            LarCommand::Migration {
                command: LarMigrationCommand::Verify { json: true }
            }
        ));
        assert!(matches!(
            parse(&["gc", "resume", "gc-123", "--json"]),
            LarCommand::Gc {
                command: LarGcCommand::Resume { json: true, .. }
            }
        ));
        match parse(&[
            "repack",
            "plan",
            "--min-garbage-bytes",
            "4096",
            "--min-garbage-ratio",
            "0.5",
            "--json",
        ]) {
            LarCommand::Repack {
                command: LarRepackCommand::Plan(args),
            } => {
                assert_eq!(args.min_garbage_bytes, 4096);
                assert_eq!(args.min_garbage_ratio, 0.5);
                assert!(args.json);
            }
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(matches!(
            parse(&["repack", "resume", "repack-123", "--json"]),
            LarCommand::Repack {
                command: LarRepackCommand::Resume { json: true, .. }
            }
        ));
        assert!(matches!(
            parse(&["upgrade", "old.lar", "--output", "new.lar", "--json"]),
            LarCommand::Upgrade(UpgradeArgs { json: true, .. })
        ));
        assert!(matches!(
            parse(&[
                "detach",
                "--file-uuid",
                "07070707070707070707070707070707",
                "--json"
            ]),
            LarCommand::Detach(DetachArgs { json: true, .. })
        ));
        assert!(matches!(
            parse(&[
                "reattach",
                "--file-uuid",
                "07070707070707070707070707070707",
                "--archive",
                "cold/archive.lar",
                "--json"
            ]),
            LarCommand::Reattach(ReattachArgs { json: true, .. })
        ));
        assert!(TestCli::try_parse_from([
            "lar-test",
            "import-legacy",
            "--dry-run",
            "--limit",
            "0"
        ])
        .is_err());
        assert!(TestCli::try_parse_from([
            "lar-test",
            "repack",
            "apply",
            "--min-garbage-ratio",
            "1.01",
        ])
        .is_err());
        assert!(
            TestCli::try_parse_from(["lar-test", "detach", "--file-uuid", "not-a-file-uuid",])
                .is_err()
        );
        assert!(TestCli::try_parse_from([
            "lar-test",
            "reattach",
            "--file-uuid",
            "07070707070707070707070707070707",
        ])
        .is_err());
    }

    #[test]
    fn top_level_help_lists_the_complete_lar_surface() {
        let help = TestCli::command().render_long_help().to_string();
        for command in [
            "import",
            "detach",
            "reattach",
            "import-legacy",
            "migration",
            "cleanup",
            "gc",
            "repack",
            "verify",
            "repair",
            "upgrade",
            "ls",
            "grep",
            "extract",
            "replay",
            "transaction",
            "transaction-replay",
            "export",
        ] {
            assert!(help.contains(command), "help omitted {command}: {help}");
        }
    }

    #[test]
    fn cleanup_requires_exactly_one_mode() {
        assert!(TestCli::try_parse_from(["lar-test", "cleanup"]).is_err());
        assert!(TestCli::try_parse_from(["lar-test", "cleanup", "--dry-run", "--apply"]).is_err());
        assert!(TestCli::try_parse_from(["lar-test", "cleanup", "--dry-run"]).is_ok());
    }

    #[test]
    fn detach_move_and_identity_validated_reattach_are_operator_safe() {
        let dir = tmpdir("archive-lifecycle");
        let source = dir.join("archives/source.lar");
        let expected = b"archive lifecycle body";
        let manifest_id = write_identity_archive(&source, [0x31; 16], expected);
        let imported = LocalLarBackend
            .execute(
                &dir,
                &parse(&["import", source.to_str().unwrap(), "--json"]),
            )
            .unwrap();
        let file_uuid = imported.json["file_uuid"].as_str().unwrap().to_owned();
        assert_eq!(file_uuid, "31".repeat(16));

        let detached = LocalLarBackend
            .execute(
                &dir,
                &parse(&["detach", "--file-uuid", &file_uuid, "--json"]),
            )
            .unwrap();
        assert_eq!(detached.json["already_offline"], false);
        assert_eq!(detached.json["file"]["availability"], "archived_offline");
        assert_eq!(detached.json["file"]["identity_validated"], true);
        assert_eq!(detached.json["file"]["exists"], true);
        assert!(detached.human.contains("did not move or delete files"));
        let listing = LocalLarBackend
            .execute(&dir, &parse(&["ls", "--json"]))
            .unwrap();
        assert_eq!(listing.json["archive_file_count"], 1);
        assert_eq!(listing.json["unavailable_archive_files"], 1);
        assert_eq!(listing.json["archive_files"][0]["file_uuid"], file_uuid);

        let moved = dir.join("cold/moved.lar");
        fs::create_dir_all(moved.parent().unwrap()).unwrap();
        fs::rename(&source, &moved).unwrap();
        let wrong = dir.join("cold/wrong.lar");
        write_identity_archive(&wrong, [0x32; 16], expected);

        let rejected = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "reattach",
                    "--file-uuid",
                    &file_uuid,
                    "--archive",
                    wrong.to_str().unwrap(),
                    "--json",
                ]),
            )
            .unwrap_err();
        assert!(rejected.to_string().contains("reattaching LAR archive"));
        let still_offline = Store::open(dir.clone())
            .unwrap()
            .lar_archive_file_status(&file_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(still_offline.availability.code(), "archived_offline");
        assert_eq!(still_offline.catalog_path, "archives/source.lar");
        assert!(!still_offline.exists);

        let reattached = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "reattach",
                    "--file-uuid",
                    &file_uuid,
                    "--archive",
                    moved.to_str().unwrap(),
                    "--json",
                ]),
            )
            .unwrap();
        assert_eq!(reattached.json["file_uuid"], file_uuid);
        assert_eq!(reattached.json["catalog_path"], "cold/moved.lar");
        assert_eq!(reattached.json["relocated"], true);
        assert_eq!(reattached.json["file"]["availability"], "online");
        assert_eq!(reattached.json["file"]["identity_validated"], true);
        assert_eq!(reattached.json["source_blake3"].as_str().unwrap().len(), 64);
        assert!(reattached.human.contains("sealed-file identity validation"));
        assert_eq!(
            Store::open(dir.clone())
                .unwrap()
                .read_lar_manifest_body(&manifest_id)
                .unwrap(),
            expected
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_read_and_export_commands() {
        assert!(matches!(
            parse(&["grep", "needle", "one.lar", "two.lar", "--limit", "7"]),
            LarCommand::Grep(GrepArgs { limit: 7, .. })
        ));
        assert!(matches!(
            parse(&[
                "replay",
                "trace.lar",
                "--trace-id",
                "trace-1",
                "--parsed",
                "--speed",
                "4x",
                "--output",
                "events.sse"
            ]),
            LarCommand::Replay(ReplayArgs {
                parsed: true,
                speed: LarReplaySpeed::Quadruple,
                ..
            })
        ));
        assert!(matches!(
            parse(&[
                "transaction",
                "--trace-id",
                "trace-1",
                "--output",
                "trace.jsonseq"
            ]),
            LarCommand::Transaction(TransactionArgs { json: false, .. })
        ));
        assert!(matches!(
            parse(&[
                "transaction-replay",
                "trace.jsonseq",
                "--parsed",
                "--stage-id",
                "stage-1",
                "--output",
                "events.sse"
            ]),
            LarCommand::TransactionReplay(TransactionReplayArgs { parsed: true, .. })
        ));
        assert!(matches!(
            parse(&[
                "export",
                "out.warc",
                "--format",
                "warc",
                "--session",
                "session-1"
            ]),
            LarCommand::Export(ExportArgs {
                format: LarExportFormat::Warc,
                ..
            })
        ));
        assert!(TestCli::try_parse_from([
            "lar-test",
            "export",
            "out.lar",
            "--trace-id",
            "trace-1",
            "--session",
            "session-1"
        ])
        .is_err());
    }

    #[test]
    fn replay_command_emits_exact_raw_reads_and_parsed_frames() {
        let dir = tmpdir("replay");
        let archive = dir.join("stream.lar");
        let (stage_id, raw_expected, parsed_expected) = write_replay_archive(&archive);
        for parsed in [false, true] {
            let output = dir.join(if parsed { "parsed.sse" } else { "raw.sse" });
            replay_stream(&ReplayArgs {
                archive: archive.clone(),
                trace_id: "replay-trace".into(),
                stage_id: Some(stage_id.clone()),
                parsed,
                speed: LarReplaySpeed::Instant,
                output: Some(output.clone()),
                force: false,
            })
            .unwrap();
            assert_eq!(
                fs::read_to_string(output).unwrap(),
                if parsed {
                    parsed_expected.as_str()
                } else {
                    raw_expected.as_str()
                }
            );
        }
    }

    #[test]
    fn transaction_export_and_replay_are_verified_bounded_and_atomic() {
        let dir = tmpdir("transaction-replay");
        let archive = dir.join("stream.lar");
        let (stage_id, raw_expected, parsed_expected) = write_replay_archive(&archive);
        let transaction = dir.join("trace.transaction.jsonseq");
        let report = export_transaction(
            &dir,
            &TransactionArgs {
                trace_id: "replay-trace".into(),
                archive: Some(archive),
                output: transaction.clone(),
                force: false,
                json: true,
            },
        )
        .unwrap();
        assert_eq!(report.json["verified"], true);
        validate_transaction_sequence(&transaction, Some("replay-trace")).unwrap();

        let pretty_transaction = dir.join("pretty.transaction.jsonseq");
        let mut pretty_bytes = Vec::new();
        for line in fs::read(&transaction)
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let record: Value = serde_json::from_slice(&line[1..]).unwrap();
            pretty_bytes.push(0x1e);
            if record["type"] == "transaction_timeline" {
                serde_json::to_writer_pretty(&mut pretty_bytes, &record).unwrap();
            } else {
                serde_json::to_writer(&mut pretty_bytes, &record).unwrap();
            }
            pretty_bytes.extend_from_slice(b"\n \t\n");
        }
        fs::write(&pretty_transaction, pretty_bytes).unwrap();
        validate_transaction_sequence(&pretty_transaction, Some("replay-trace")).unwrap();

        let oversized = dir.join("oversized.transaction.jsonseq");
        let mut oversized_file = fs::File::create(&oversized).unwrap();
        oversized_file.write_all(&[0x1e]).unwrap();
        oversized_file
            .write_all(&vec![b' '; MAX_TRANSACTION_JSON_RECORD_BYTES])
            .unwrap();
        oversized_file.write_all(b"x").unwrap();
        drop(oversized_file);
        let oversized_error = validate_transaction_sequence(&oversized, None).unwrap_err();
        assert!(oversized_error.to_string().contains("exceeds"));

        for parsed in [false, true] {
            let output = dir.join(if parsed {
                "transaction-parsed.sse"
            } else {
                "transaction-raw.sse"
            });
            replay_transaction(&TransactionReplayArgs {
                input: transaction.clone(),
                stage_id: Some(stage_id.clone()),
                parsed,
                speed: LarReplaySpeed::Instant,
                output: Some(output.clone()),
                force: false,
            })
            .unwrap();
            assert_eq!(
                fs::read_to_string(output).unwrap(),
                if parsed {
                    parsed_expected.as_str()
                } else {
                    raw_expected.as_str()
                }
            );
        }

        let existing_export = dir.join("existing.transaction.jsonseq");
        fs::write(&existing_export, b"sentinel export").unwrap();
        let export_error = export_transaction(
            &dir,
            &TransactionArgs {
                trace_id: "replay-trace".into(),
                archive: Some(dir.join("stream.lar")),
                output: existing_export.clone(),
                force: false,
                json: false,
            },
        )
        .unwrap_err();
        assert!(export_error.to_string().contains("--force"));
        assert_eq!(fs::read(&existing_export).unwrap(), b"sentinel export");

        let truncated = dir.join("truncated.transaction.jsonseq");
        let mut truncated_bytes = fs::read(&transaction).unwrap();
        let final_record = truncated_bytes
            .iter()
            .rposition(|byte| *byte == 0x1e)
            .unwrap();
        truncated_bytes.truncate(final_record);
        fs::write(&truncated, truncated_bytes).unwrap();
        assert!(validate_transaction_sequence(&truncated, None).is_err());

        let corrupt = dir.join("corrupt.transaction.jsonseq");
        let mut corrupt_bytes = fs::read(&transaction).unwrap();
        let marker = b"\"data_base64\":\"";
        let marker_at = corrupt_bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        let corrupt_at = marker_at + marker.len();
        corrupt_bytes[corrupt_at] = if corrupt_bytes[corrupt_at] == b'A' {
            b'B'
        } else {
            b'A'
        };
        fs::write(&corrupt, corrupt_bytes).unwrap();
        assert!(validate_transaction_sequence(&corrupt, None).is_err());

        let protected = dir.join("protected.sse");
        fs::write(&protected, b"existing replay").unwrap();
        let corrupt_error = replay_transaction(&TransactionReplayArgs {
            input: corrupt,
            stage_id: Some(stage_id.clone()),
            parsed: false,
            speed: LarReplaySpeed::Instant,
            output: Some(protected.clone()),
            force: true,
        })
        .unwrap_err();
        assert!(corrupt_error.to_string().contains("artifact end"));
        assert_eq!(fs::read(&protected).unwrap(), b"existing replay");

        let overlapping = dir.join("overlapping.transaction.jsonseq");
        let mut rewritten = Vec::new();
        for line in fs::read(&transaction)
            .unwrap()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            let mut record: Value = serde_json::from_slice(&line[1..]).unwrap();
            if record["type"] == "stream_index" {
                record["observed_reads"][1]["byte_offset"] = 10.into();
            }
            rewritten.push(0x1e);
            serde_json::to_writer(&mut rewritten, &record).unwrap();
            rewritten.push(b'\n');
        }
        fs::write(&overlapping, rewritten).unwrap();
        let overlap_error = replay_transaction(&TransactionReplayArgs {
            input: overlapping,
            stage_id: Some(stage_id),
            parsed: false,
            speed: LarReplaySpeed::Instant,
            output: Some(dir.join("overlap.sse")),
            force: false,
        })
        .unwrap_err();
        assert!(overlap_error
            .to_string()
            .contains("overlapping or non-contiguous"));
    }

    #[test]
    fn transaction_cli_labels_and_streams_legacy_synthesis() {
        let dir = tmpdir("transaction-legacy");
        let store = Store::open(dir.clone()).unwrap();
        let mut body = vec![0x5a; alex_store::LAR_TRANSACTION_ARTIFACT_PIECE_BYTES * 3 + 9];
        body[0] = 0;
        body[1] = 0xff;
        let path = store
            .write_body("legacy-transaction-cli", "request.json", &body)
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "legacy-transaction-cli".into(),
                method: Some("POST".into()),
                path: Some("/v1/legacy".into()),
                req_body_path: Some(path),
                ..Default::default()
            })
            .unwrap();
        drop(store);

        let output = dir.join("legacy.transaction.jsonseq");
        let report = export_transaction(
            &dir,
            &TransactionArgs {
                trace_id: "legacy-transaction-cli".into(),
                archive: None,
                output: output.clone(),
                force: false,
                json: true,
            },
        )
        .unwrap();
        assert_eq!(report.json["report"]["fidelity"], "synthesized_legacy");
        validate_transaction_sequence(&output, Some("legacy-transaction-cli")).unwrap();
        let bytes = fs::read(output).unwrap();
        let first_line = bytes.split(|byte| *byte == b'\n').next().unwrap();
        let format: Value = serde_json::from_slice(&first_line[1..]).unwrap();
        assert_eq!(format["fidelity"], "synthesized_legacy");
        assert!(format["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().unwrap().contains("synthesized")));
    }

    #[test]
    fn semantic_exports_use_valid_ids_and_provider_vocabularies() {
        let trace_id = semantic_trace_id("019f6872-a3ee-7431-b4bb-2bafbabb7235");
        assert_eq!(trace_id, "019f6872a3ee7431b4bb2bafbabb7235");
        assert_eq!(semantic_span_id("trace", b"otel").len(), 16);
        assert_eq!(otel_provider_name(&serde_json::json!("xai")), "x_ai");
        assert_eq!(
            otel_provider_name(&serde_json::json!("gemini")),
            "gcp.gemini"
        );
        assert_eq!(
            openinference_system(&serde_json::json!("gemini")),
            "vertexai"
        );
        assert_eq!(
            openinference_provider(&serde_json::json!("kimi")),
            "moonshot"
        );
    }

    #[test]
    fn grep_sealed_archive_reuses_chunks_and_finds_cross_range_literal() {
        let dir = tmpdir("grep-sealed");
        let archive = dir.join("sealed.lar");
        write_search_archive(&archive);

        let shared = grep_records(
            &dir.join("live"),
            &grep_args("abcd", vec![archive.clone()], 10),
        )
        .unwrap();
        assert_eq!(shared.json["match_count"], 2);
        let archive_stats = shared.json["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["archive"].as_str() == Some(archive.to_str().unwrap()))
            .unwrap();
        // Both manifests start with the same content-addressed chunk. A raw
        // archive scan must decompress that physical chunk only once.
        assert_eq!(archive_stats["unique_chunks_read"], 1);
        assert_eq!(shared.json["matches"][0]["stage_id"].is_string(), true);
        assert_eq!(shared.json["matches"][0]["trace_id"].is_string(), true);
        assert_eq!(shared.json["matches"][0]["session_id"].is_string(), true);
        assert_eq!(shared.json["matches"][0]["timestamp_ns"].is_number(), true);

        let boundary =
            grep_records(&dir.join("live-2"), &grep_args("dNE", vec![archive], 10)).unwrap();
        assert_eq!(boundary.json["match_count"], 1);
        assert_eq!(boundary.json["matches"][0]["match_offset"], 3);
        assert_eq!(boundary.json["matches"][0]["trace_id"], "trace-0");
    }

    #[test]
    fn whole_record_grep_searches_sealed_safe_headers_trailers_and_metadata() {
        let dir = tmpdir("grep-sealed-whole-record");
        let archive = dir.join("sealed.lar");
        write_search_archive(&archive);
        let search = |literal: &str| {
            let mut args = grep_args(literal, vec![archive.clone()], 20);
            args.scope = LarGrepScope::WholeRecord;
            grep_records(&dir.join("empty-live"), &args).unwrap().json
        };

        let header = search("sealed-header-needle");
        assert_eq!(header["record_match_count"], 1);
        assert_eq!(header["record_matches"][0]["category"], "ordered_headers");
        assert_eq!(
            header["record_matches"][0]["field"],
            "request_headers.value"
        );
        assert_eq!(header["record_matches"][0]["header_ordinal"], 0);

        let trailer = search("sealed-trailer-needle");
        assert_eq!(trailer["record_matches"][0]["category"], "ordered_trailers");
        let provider = search("sealed-provider-needle");
        assert_eq!(provider["record_matches"][0]["field"], "provider");
        let harness = search("sealed-harness-needle");
        assert_eq!(harness["record_matches"][0]["field"], "harness");
        let body = search("NEED");
        assert_eq!(body["body_match_count"], 1);

        let secret = search("sealed-secret-must-not-match");
        assert_eq!(secret["match_count"], 0);
        let header_coverage = secret["record_coverage"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| {
                item["archive"].as_str() == Some(archive.to_str().unwrap())
                    && item["category"] == "ordered_headers"
            })
            .unwrap();
        assert_eq!(header_coverage["values_skipped"], 1);
    }

    #[test]
    fn whole_record_grep_searches_active_catalog_and_preserves_body_default() {
        use alex_store::{
            LarBodyArtifact, LarExchangeBodyRefs, LarExchangeCapture, LarHeaderCapture,
        };

        let data_dir = tmpdir("grep-active-whole-record");
        let store = Store::open_with_lar_body_store(
            data_dir.clone(),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                ..Default::default()
            },
        )
        .unwrap();
        let trace_id = "active-whole-record-trace";
        let manifest_id = store
            .write_body_artifact(
                &LarBodyArtifact::trace(trace_id, "client_request"),
                "request.json",
                b"active-body-needle",
            )
            .unwrap()
            .manifest_id
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: trace_id.into(),
                ts_request_ms: 42,
                session_id: Some("active-whole-session".into()),
                ..Default::default()
            })
            .unwrap();
        let capture = LarExchangeCapture {
            trace_id: trace_id.into(),
            session_id: Some("active-whole-session".into()),
            run_id: None,
            wall_time_ns: 42_000_000,
            client_request_headers: Some(LarHeaderCapture::observed([
                ("x-safe-active", "active-header-needle"),
                ("authorization", "active-secret-must-not-match"),
            ])),
            client_request_trailers: Some(LarHeaderCapture::observed([(
                "x-active-trailer",
                "active-trailer-needle",
            )])),
            client_response_headers: None,
            client_response_trailers: None,
            upstream_attempts: Vec::new(),
            upstream_stream_reads: None,
            provider: Some("active-provider-needle".into()),
            requested_model: Some("active-model-needle".into()),
            routed_model: None,
            account_id: Some("active-account-excluded".into()),
            routing_reason: Some("active-control-needle".into()),
            status_code: Some(200),
            error_class: None,
            error_message: None,
        };
        store
            .write_lar_exchange_capture(
                &capture,
                &LarExchangeBodyRefs {
                    client_request_manifest_id: Some(manifest_id),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        drop(store);

        let whole = |literal: &str| {
            let mut args = grep_args(literal, vec![], 20);
            args.scope = LarGrepScope::WholeRecord;
            grep_records(&data_dir, &args).unwrap().json
        };
        for literal in [
            "active-header-needle",
            "active-trailer-needle",
            "active-provider-needle",
        ] {
            assert!(whole(literal)["record_match_count"].as_u64().unwrap() >= 1);
        }
        assert!(
            whole("active-body-needle")["body_match_count"]
                .as_u64()
                .unwrap()
                >= 1
        );
        assert_eq!(whole("active-secret-must-not-match")["match_count"], 0);
        assert_eq!(whole("active-account-excluded")["match_count"], 0);

        let body_default =
            grep_records(&data_dir, &grep_args("active-header-needle", vec![], 20)).unwrap();
        assert_eq!(body_default.json["match_count"], 0);
        assert!(body_default.json.get("record_matches").is_none());
    }

    #[test]
    fn grep_live_catalog_reconstructs_cross_pack_manifest_and_anchors_trace() {
        let data_dir = tmpdir("grep-live-cross-pack");
        let mut config = alex_store::LarBodyStoreConfig::default();
        config.mode = alex_store::LarBodyStoreMode::LarWithFallback;
        config.max_pack_bytes = 1;
        config.chunker.min_size = 4;
        config.chunker.target_size = 4;
        config.chunker.max_size = 4;
        let store = Store::open_with_lar_body_store(data_dir.clone(), config).unwrap();
        store.write_body("seed", "request.json", b"abcd").unwrap();
        let legacy_path = store
            .write_body(
                "live-trace",
                "request.json",
                br#"abcd{"messages":[{"role":"user","content":"catalog needle"}]}"#,
            )
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "live-trace".into(),
                ts_request_ms: 42,
                session_id: Some("live-session".into()),
                client_format: Some("anthropic".into()),
                req_body_path: Some(legacy_path.clone()),
                ..Default::default()
            })
            .unwrap();
        fs::remove_file(legacy_path).unwrap();
        drop(store);

        let output = grep_records(&data_dir, &grep_args("catalog needle", vec![], 10)).unwrap();
        assert_eq!(output.json["match_count"], 1);
        let matched = &output.json["matches"][0];
        assert_eq!(matched["source"], "live-catalog");
        assert_eq!(matched["owner_kind"], "trace");
        assert_eq!(matched["owner_id"], "live-trace");
        assert_eq!(matched["trace_id"], "live-trace");
        assert_eq!(matched["session_id"], "live-session");
        assert_eq!(matched["timestamp_ms"], 42);
        assert!(
            output.json["sources"][0]["unique_chunks_read"]
                .as_u64()
                .unwrap()
                > 1
        );
    }

    #[test]
    fn grep_result_limit_is_an_explicit_error() {
        let data_dir = tmpdir("grep-result-limit");
        let mut config = alex_store::LarBodyStoreConfig::default();
        config.mode = alex_store::LarBodyStoreMode::LarWithFallback;
        let store = Store::open_with_lar_body_store(data_dir.clone(), config).unwrap();
        for index in 0..2 {
            let id = format!("limit-trace-{index}");
            let path = store
                .write_body(&id, "request.json", b"same limit needle")
                .unwrap();
            store
                .insert_trace(&alex_core::TraceRecord {
                    id,
                    ts_request_ms: index,
                    session_id: Some("limit-session".into()),
                    req_body_path: Some(path),
                    ..Default::default()
                })
                .unwrap();
        }
        drop(store);
        let error = grep_records(&data_dir, &grep_args("limit needle", vec![], 1)).unwrap_err();
        assert!(
            format!("{error:#}").contains("result limit exceeded"),
            "{error:#}"
        );
    }

    #[test]
    fn preflight_rejects_non_lar_files() {
        let dir = tmpdir("bad-magic");
        let path = dir.join("not.lar");
        fs::write(&path, b"NOPE").unwrap();
        let error = preflight_archive(&path).unwrap_err();
        assert!(error.to_string().contains("not a LAR1 archive"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archive_listing_and_verification_reconstruct_every_manifest() {
        let dir = tmpdir("archive-verify");
        let archive = dir.join("good.lar");
        write_test_archive(&archive, false);

        let listed = LocalLarBackend
            .execute(&dir, &parse(&["ls", archive.to_str().unwrap(), "--json"]))
            .unwrap();
        assert_eq!(listed.json["recovery"], "clean");
        assert_eq!(listed.json["manifest_count"], 1);
        assert_eq!(listed.json["verified_manifest_count"], 0);

        let verified = LocalLarBackend
            .execute(
                &dir,
                &parse(&["verify", archive.to_str().unwrap(), "--json"]),
            )
            .unwrap();
        assert_eq!(verified.json["verified_manifest_count"], 1);
        assert_eq!(
            verified.json["verification_failures"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archive_verify_keep_going_reports_every_manifest_failure() {
        let dir = tmpdir("archive-verify-keep-going");
        let archive = dir.join("corrupt.lar");
        let manifest_ids = write_archive_with_two_corrupt_manifests(&archive);

        let fail_fast = format!("{:#}", verify_archive(&archive, false).unwrap_err());
        assert_eq!(
            manifest_ids
                .iter()
                .filter(|manifest_id| fail_fast.contains(manifest_id.as_str()))
                .count(),
            1,
            "fail-fast verification should report only the first failed manifest: {fail_fast}"
        );

        let complete = format!("{:#}", verify_archive(&archive, true).unwrap_err());
        assert!(complete.contains("2 issue(s)"), "{complete}");
        assert!(
            complete.contains("reconstructing 0/2 manifests"),
            "{complete}"
        );
        for manifest_id in manifest_ids {
            assert!(
                complete.contains(&manifest_id),
                "keep-going report omitted {manifest_id}: {complete}"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn repair_copies_only_valid_prefix_and_never_modifies_input() {
        let dir = tmpdir("archive-repair");
        let archive = dir.join("truncated.lar");
        let repaired = dir.join("repaired.lar");
        write_test_archive(&archive, true);
        let input_before = fs::read(&archive).unwrap();

        let same_path_error = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "repair",
                    archive.to_str().unwrap(),
                    "--output",
                    archive.to_str().unwrap(),
                    "--force",
                ]),
            )
            .unwrap_err();
        assert!(same_path_error
            .to_string()
            .contains("output must differ from the input"));
        assert_eq!(fs::read(&archive).unwrap(), input_before);

        let verification =
            LocalLarBackend.execute(&dir, &parse(&["verify", archive.to_str().unwrap()]));
        assert!(verification
            .unwrap_err()
            .to_string()
            .contains("recovery state truncated_tail"));

        let output = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "repair",
                    archive.to_str().unwrap(),
                    "--output",
                    repaired.to_str().unwrap(),
                    "--json",
                ]),
            )
            .unwrap();
        assert_eq!(output.json["input_modified"], false);
        assert_eq!(fs::read(&archive).unwrap(), input_before);
        assert!(fs::metadata(&repaired).unwrap().len() < input_before.len() as u64);
        let repaired_verify = verify_archive(&repaired, false).unwrap();
        assert_eq!(repaired_verify.json["recovery"], "clean");
        fs::remove_dir_all(dir).unwrap();
    }

    fn upgrade_args(input: &Path, output: &Path) -> UpgradeArgs {
        UpgradeArgs {
            input: input.to_path_buf(),
            output: output.to_path_buf(),
            json: true,
        }
    }

    fn assert_no_upgrade_temps(directory: &Path) {
        let leftovers = fs::read_dir(directory)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("lar-upgrade.tmp")
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert!(
            leftovers.is_empty(),
            "upgrade temp files remain: {leftovers:?}"
        );
    }

    #[test]
    fn upgrade_publishes_verified_copy_with_stable_logical_ids() {
        let dir = tmpdir("archive-upgrade");
        let input = dir.join("source.lar");
        let output = dir.join("latest.lar");
        write_search_archive(&input);
        let input_before = fs::read(&input).unwrap();
        let source_ids = {
            let reader =
                ArchiveReader::open(fs::File::open(&input).unwrap(), Limits::default()).unwrap();
            let mut ids = reader.manifest_ids().copied().collect::<Vec<_>>();
            ids.sort_by_key(|id| id.0);
            ids
        };

        let report = upgrade_archive_command(&upgrade_args(&input, &output)).unwrap();
        assert_eq!(report.json["input_modified"], false);
        assert_eq!(report.json["catalog_modified"], false);
        assert_eq!(report.json["published_atomically"], true);
        assert!(report.json["source_sha256"].is_string());
        assert!(report.json["output_sha256"].is_string());
        assert_ne!(report.json["source_uuid"], report.json["output_uuid"]);
        assert_eq!(fs::read(&input).unwrap(), input_before);

        let output_ids = {
            let reader =
                ArchiveReader::open(fs::File::open(&output).unwrap(), Limits::default()).unwrap();
            assert!(reader.is_sealed());
            assert_eq!(reader.recovery_status(), RecoveryStatus::Clean);
            let mut ids = reader.manifest_ids().copied().collect::<Vec<_>>();
            ids.sort_by_key(|id| id.0);
            ids
        };
        assert_eq!(source_ids, output_ids);
        let mut source_file = fs::File::open(&input).unwrap();
        let mut output_file = fs::File::open(&output).unwrap();
        verify_upgraded_archive(&mut source_file, &mut output_file, Limits::default()).unwrap();
        assert_no_upgrade_temps(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn upgrade_refuses_aliases_existing_outputs_and_publish_races() {
        let dir = tmpdir("archive-upgrade-path-safety");
        let input = dir.join("source.lar");
        write_search_archive(&input);
        let input_before = fs::read(&input).unwrap();

        let same_error = upgrade_archive_command(&upgrade_args(&input, &input)).unwrap_err();
        assert!(format!("{same_error:#}").contains("must differ from the input"));
        assert_eq!(fs::read(&input).unwrap(), input_before);

        let hardlink = dir.join("source-hardlink.lar");
        fs::hard_link(&input, &hardlink).unwrap();
        let hardlink_error = upgrade_archive_command(&upgrade_args(&input, &hardlink)).unwrap_err();
        assert!(
            format!("{hardlink_error:#}").contains("will not be overwritten"),
            "{hardlink_error:#}"
        );

        #[cfg(unix)]
        {
            let symlink = dir.join("source-symlink.lar");
            std::os::unix::fs::symlink(&input, &symlink).unwrap();
            let symlink_error =
                upgrade_archive_command(&upgrade_args(&input, &symlink)).unwrap_err();
            assert!(format!("{symlink_error:#}").contains("must differ from the input"));
        }

        let existing = dir.join("existing.lar");
        fs::write(&existing, b"sentinel").unwrap();
        let existing_error = upgrade_archive_command(&upgrade_args(&input, &existing)).unwrap_err();
        assert!(format!("{existing_error:#}").contains("will not be overwritten"));
        assert_eq!(fs::read(&existing).unwrap(), b"sentinel");

        let raced = dir.join("raced.lar");
        let raced_args = upgrade_args(&input, &raced);
        let race_error = upgrade_archive_with_hook(&raced_args, |_| {
            fs::write(&raced, b"concurrent winner")?;
            Ok(())
        })
        .unwrap_err();
        assert!(format!("{race_error:#}").contains("without overwriting"));
        assert_eq!(fs::read(&raced).unwrap(), b"concurrent winner");
        assert_eq!(fs::read(&input).unwrap(), input_before);
        assert_no_upgrade_temps(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn upgrade_failure_and_interruption_leave_no_partial_output() {
        let dir = tmpdir("archive-upgrade-interruption");
        let input = dir.join("source.lar");
        write_search_archive(&input);
        let input_before = fs::read(&input).unwrap();

        let interrupted_output = dir.join("interrupted.lar");
        let interruption =
            upgrade_archive_with_hook(&upgrade_args(&input, &interrupted_output), |_| {
                bail!("simulated interruption before publish")
            })
            .unwrap_err();
        assert!(format!("{interruption:#}").contains("simulated interruption"));
        assert!(!interrupted_output.exists());
        assert_eq!(fs::read(&input).unwrap(), input_before);
        assert_no_upgrade_temps(&dir);

        let truncated = dir.join("truncated.lar");
        fs::write(&truncated, &input_before[..input_before.len() - 9]).unwrap();
        let corrupt_output = dir.join("corrupt-output.lar");
        assert!(upgrade_archive_command(&upgrade_args(&truncated, &corrupt_output)).is_err());
        assert!(!corrupt_output.exists());

        let corrupt = dir.join("corrupt.lar");
        let mut corrupt_bytes = input_before.clone();
        let corrupt_at = corrupt_bytes.len() / 2;
        corrupt_bytes[corrupt_at] ^= 0x80;
        fs::write(&corrupt, corrupt_bytes).unwrap();
        let checksum_output = dir.join("checksum-output.lar");
        assert!(upgrade_archive_command(&upgrade_args(&corrupt, &checksum_output)).is_err());
        assert!(!checksum_output.exists());
        assert_no_upgrade_temps(&dir);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dry_run_inventory_is_real_and_non_mutating() {
        let dir = tmpdir("inventory");
        let store = Store::open(dir.clone()).unwrap();
        let body = store
            .write_body("trace-1", "request.json", b"hello")
            .unwrap();
        let mut trace = alex_core::TraceRecord {
            id: "trace-1".into(),
            method: Some("POST".into()),
            path: Some("/v1/messages".into()),
            ..Default::default()
        };
        trace.req_body_path = Some(body);
        trace.req_headers_json = Some("[[\"content-type\",\"application/json\"]]".into());
        store.insert_trace(&trace).unwrap();
        drop(store);

        let output = legacy_import_inventory(&dir, None, true).unwrap();
        assert_eq!(output.json["mode"], "dry-run");
        assert_eq!(output.json["unique_body_references"], 1);
        assert_eq!(output.json["selected_body_references"], 1);
        assert_eq!(output.json["missing_body_references"], 0);
        assert_eq!(output.json["source_bodies_verified"], 1);
        assert_eq!(output.json["corrupt_body_references"], 0);
        assert_eq!(output.json["inline_header_records"], 1);
        assert_eq!(output.json["writes_performed"], 0);
        assert!(!dir.join("lar").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_status_reports_every_job() {
        let dir = tmpdir("migration-status");
        let store = Store::open(dir.clone()).unwrap();
        create_migration_job(&store, "job-1", "source-1", 10);
        create_migration_job(&store, "job-2", "source-2", 20);
        drop(store);

        let command = parse(&["migration", "status", "--json"]);
        let output = LocalLarBackend.execute(&dir, &command).unwrap();
        assert_eq!(output.json["total_jobs"], 2);
        assert_eq!(output.json["incomplete_jobs"], 2);
        assert_eq!(output.json["jobs"].as_array().unwrap().len(), 2);
        assert!(output.human.contains("2 job(s), 2 incomplete"));
        assert!(output.human.contains("job-1 [pending]"));
        assert!(output.human.contains("job-2 [pending]"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_pause_and_resume_the_only_incomplete_job() {
        let dir = tmpdir("migration-pause-resume");
        let store = Store::open(dir.clone()).unwrap();
        create_migration_job(&store, "job-1", "source-1", 10);
        drop(store);

        let paused = LocalLarBackend
            .execute(&dir, &parse(&["migration", "pause", "--json"]))
            .unwrap();
        assert_eq!(paused.json["jobs"][0]["job_id"], "job-1");
        assert_eq!(paused.json["jobs"][0]["state"], "paused");
        assert!(paused.human.starts_with("LAR migration paused"));

        let resumed = LocalLarBackend
            .execute(&dir, &parse(&["migration", "resume", "--json"]))
            .unwrap();
        assert_eq!(resumed.json["jobs"][0]["state"], "pending");
        assert!(resumed.human.starts_with("LAR migration resumed"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_controls_reject_missing_or_ambiguous_jobs() {
        let empty_dir = tmpdir("migration-empty");
        let no_job_error = LocalLarBackend
            .execute(&empty_dir, &parse(&["migration", "pause"]))
            .unwrap_err()
            .to_string();
        assert!(no_job_error.contains("no incomplete LAR migration job"));
        fs::remove_dir_all(empty_dir).unwrap();

        let ambiguous_dir = tmpdir("migration-ambiguous");
        let store = Store::open(ambiguous_dir.clone()).unwrap();
        create_migration_job(&store, "job-1", "source-1", 10);
        create_migration_job(&store, "job-2", "source-2", 20);
        drop(store);
        let ambiguous_error = LocalLarBackend
            .execute(&ambiguous_dir, &parse(&["migration", "resume"]))
            .unwrap_err()
            .to_string();
        assert!(ambiguous_error.contains("2 incomplete LAR migration jobs"));
        assert!(ambiguous_error.contains("job-1"));
        assert!(ambiguous_error.contains("job-2"));
        fs::remove_dir_all(ambiguous_dir).unwrap();
    }

    #[test]
    fn migration_verify_uses_the_shared_read_only_verifier() {
        let dir = tmpdir("migration-verify");
        let output = LocalLarBackend
            .execute(&dir, &parse(&["migration", "verify"]))
            .unwrap();
        assert_eq!(output.json["kind"], "migration_verification");
        assert_eq!(output.json["valid"], true);
        assert_eq!(output.json["files_checked"], 0);
        assert_eq!(
            output.json["report_schema"],
            "alex-lar-migration-verification-v1"
        );
        assert_eq!(output.json["checksum_algorithm"], "blake3");
        assert_eq!(output.json["report_checksum"].as_str().unwrap().len(), 64);
        assert!(output.human.contains("verification passed"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn live_ls_reports_the_available_catalog_summary() {
        let dir = tmpdir("live-ls");
        let store = Store::open(dir.clone()).unwrap();
        create_migration_job(&store, "job-1", "source-1", 10);
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "trace-live-ls".into(),
                session_id: Some("session-1".into()),
                ts_request_ms: 11,
                ..Default::default()
            })
            .unwrap();
        drop(store);

        let output = LocalLarBackend
            .execute(&dir, &parse(&["ls", "--json"]))
            .unwrap();
        assert_eq!(output.json["kind"], "live_catalog");
        assert_eq!(output.json["migration_job_count"], 1);
        assert_eq!(output.json["migration_jobs"][0]["job_id"], "job-1");
        assert!(output.human.contains("live LAR catalog schema v"));

        let trace = LocalLarBackend
            .execute(&dir, &parse(&["ls", "--session", "session-1"]))
            .unwrap();
        assert_eq!(trace.json["kind"], "live_session");
        assert_eq!(trace.json["trace_count"], 1);
        let trace = LocalLarBackend
            .execute(&dir, &parse(&["ls", "--trace-id", "trace-live-ls"]))
            .unwrap();
        assert_eq!(trace.json["kind"], "live_trace");
        assert_eq!(trace.json["trace"]["id"], "trace-live-ls");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn gc_cli_plans_and_applies_a_restartable_logical_sweep() {
        let dir = tmpdir("gc-cli");
        let store = Store::open_with_lar_body_store(
            dir.clone(),
            alex_store::LarBodyStoreConfig {
                mode: alex_store::LarBodyStoreMode::LarWithFallback,
                ..Default::default()
            },
        )
        .unwrap();
        store
            .write_body_artifact(
                &alex_store::LarBodyArtifact::trace("trace-gc-cli", "client_request"),
                "request.json",
                b"reachable bytes",
            )
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "trace-gc-cli".into(),
                ts_request_ms: 1,
                ..Default::default()
            })
            .unwrap();
        drop(store);

        let plan = LocalLarBackend
            .execute(&dir, &parse(&["gc", "plan", "--json"]))
            .unwrap();
        assert_eq!(plan.json["dry_run"], true);
        assert_eq!(plan.json["reachable_manifests"], 1);

        Store::open(dir.clone())
            .unwrap()
            .delete_trace("trace-gc-cli")
            .unwrap();
        let applied = LocalLarBackend
            .execute(&dir, &parse(&["gc", "apply", "--json"]))
            .unwrap();
        assert_eq!(applied.json["state"], "complete");
        assert_eq!(applied.json["swept_manifests"], 1);
        assert_eq!(applied.json["physical_bytes_reclaimed"], 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn import_legacy_uses_shared_validating_importer_and_keeps_legacy_file() {
        let dir = tmpdir("real-import");
        let store = Store::open(dir.clone()).unwrap();
        let expected = br#"{"messages":[{"role":"user","content":"hello LAR"}]}"#;
        let legacy_path = store
            .write_body("trace-import", "request.json", expected)
            .unwrap();
        let mut trace = alex_core::TraceRecord {
            id: "trace-import".into(),
            session_id: Some("session-import".into()),
            method: Some("POST".into()),
            path: Some("/v1/messages".into()),
            ..Default::default()
        };
        trace.req_body_path = Some(legacy_path.clone());
        store.insert_trace(&trace).unwrap();
        drop(store);

        let output = LocalLarBackend
            .execute(&dir, &parse(&["import-legacy", "--verify", "--json"]))
            .unwrap();
        assert_eq!(output.json["migrated"], 1);
        assert_eq!(output.json["failed"], 0);
        assert!(Path::new(&legacy_path).exists());

        let reopened = Store::open(dir.clone()).unwrap();
        assert_eq!(
            reopened
                .read_lar_or_legacy_artifact("trace", "trace-import", "client_request", None,)
                .unwrap()
                .unwrap(),
            expected
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn extract_reads_legacy_and_validated_lar_without_mixing_metadata_into_stdout() {
        let dir = tmpdir("extract-mixed");
        let store = Store::open(dir.clone()).unwrap();
        let expected = b"exact body bytes\nincluding a trailing newline\n";
        let legacy_path = store
            .write_body("trace-extract", "response.body", expected)
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "trace-extract".into(),
                resp_body_path: Some(legacy_path.clone()),
                ..Default::default()
            })
            .unwrap();

        let stdout = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "extract",
                    "--trace-id",
                    "trace-extract",
                    "--artifact",
                    "response",
                ]),
            )
            .unwrap();
        assert_eq!(stdout.raw_body.as_deref(), Some(expected.as_slice()));
        assert!(stdout.human.is_empty());

        store
            .run_lar_legacy_import(&LarLegacyImportOptions::default())
            .unwrap();
        fs::remove_file(&legacy_path).unwrap();
        let output = dir.join("exported.response");
        let extracted = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "extract",
                    "--trace-id",
                    "trace-extract",
                    "--artifact",
                    "raw-stream",
                    "--output",
                    output.to_str().unwrap(),
                    "--json",
                ]),
            )
            .unwrap();
        assert_eq!(fs::read(&output).unwrap(), expected);
        assert_eq!(extracted.json["source"], "lar");
        assert_eq!(extracted.json["bytes"], expected.len());
        assert_eq!(extracted.json["hash_algorithm"], "blake3");

        let exists = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "extract",
                    "--trace-id",
                    "trace-extract",
                    "--artifact",
                    "response",
                    "--output",
                    output.to_str().unwrap(),
                ]),
            )
            .unwrap_err();
        assert!(exists.to_string().contains("--force"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_writes_verified_interchange_formats_and_jsonl_round_trips() {
        let dir = tmpdir("export-formats");
        let store = Store::open(dir.clone()).unwrap();
        let request = br#"{"model":"alex/test","messages":[{"role":"user","content":"hi"}]}"#;
        let response = br#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        let request_path = store
            .write_body("trace-export", "request.json", request)
            .unwrap();
        let response_path = store
            .write_body("trace-export", "response.body", response)
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "trace-export".into(),
                ts_request_ms: 1_700_000_000_000,
                ts_response_ms: Some(1_700_000_000_125),
                session_id: Some("session-export".into()),
                method: Some("POST".into()),
                path: Some("/v1/chat/completions".into()),
                requested_model: Some("alex/test".into()),
                routed_model: Some("provider/test".into()),
                upstream_provider: Some("test".into()),
                status: Some(200),
                req_body_path: Some(request_path),
                resp_body_path: Some(response_path),
                req_headers_json: Some(
                    r#"[["Content-Type","application/json"],["X-Duplicate","one"],["X-Duplicate","two"]]"#
                        .into(),
                ),
                resp_headers_json: Some(r#"{"content-type":"application/json"}"#.into()),
                ..Default::default()
            })
            .unwrap();
        drop(store);

        for (format, extension) in [
            ("lar", "lar"),
            ("har", "har"),
            ("warc", "warc"),
            ("jsonl", "jsonl"),
            ("otel", "otel.jsonl"),
            ("openinference", "openinference.jsonl"),
        ] {
            let output = dir.join(format!("trace.{extension}"));
            let result = LocalLarBackend
                .execute(
                    &dir,
                    &parse(&[
                        "export",
                        output.to_str().unwrap(),
                        "--format",
                        format,
                        "--trace-id",
                        "trace-export",
                        "--json",
                    ]),
                )
                .unwrap();
            assert_eq!(result.json["traces"], 1);
            assert_eq!(result.json["verified"], true);
            assert!(!result.json["loss_report"].as_array().unwrap().is_empty());
            assert!(output.is_file());

            match format {
                "lar" => {
                    let mut reader =
                        ArchiveReader::open(fs::File::open(&output).unwrap(), Limits::default())
                            .unwrap();
                    assert!(reader.is_sealed());
                    let exchange = reader.exchange_by_trace(b"trace-export").unwrap();
                    assert_eq!(
                        exchange.data.session_id.as_deref(),
                        Some(b"session-export".as_slice())
                    );
                    let bodies = reader
                        .manifest_ids()
                        .copied()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .map(|id| reader.read_body(&id).unwrap())
                        .collect::<Vec<_>>();
                    assert!(bodies.iter().any(|body| body.as_slice() == request));
                    assert!(bodies.iter().any(|body| body.as_slice() == response));
                    let listed = LocalLarBackend
                        .execute(
                            &dir,
                            &parse(&[
                                "ls",
                                output.to_str().unwrap(),
                                "--trace-id",
                                "trace-export",
                                "--json",
                            ]),
                        )
                        .unwrap();
                    assert_eq!(listed.json["exchange_total"], 1);
                    assert_eq!(listed.json["exchanges"][0]["trace_id"], "trace-export");
                    assert_eq!(
                        listed.json["exchanges"][0]["stages"]
                            .as_array()
                            .unwrap()
                            .len(),
                        5
                    );

                    let imported_dir = dir.join("standalone-import-store");
                    fs::create_dir_all(&imported_dir).unwrap();
                    let imported = LocalLarBackend
                        .execute(
                            &imported_dir,
                            &parse(&["import", output.to_str().unwrap(), "--json"]),
                        )
                        .unwrap();
                    assert_eq!(imported.json["exchanges"], 1);
                    let imported_store = Store::open(imported_dir).unwrap();
                    assert_eq!(
                        imported_store
                            .read_lar_or_legacy_artifact(
                                "trace",
                                "trace-export",
                                "client_request",
                                None,
                            )
                            .unwrap()
                            .unwrap(),
                        request
                    );
                }
                "har" => {
                    let value: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
                    assert_eq!(value["log"]["entries"][0]["response"]["status"], 200);
                    assert!(value["log"]["entries"][0]["_alex"]["trace"]
                        .get("req_body_path")
                        .is_none());
                    assert!(value["log"]["entries"][0]["_alex"]["trace"]
                        .get("resp_body_path")
                        .is_none());
                    assert_eq!(
                        value["log"]["entries"][0]["request"]["headers"]
                            .as_array()
                            .unwrap()
                            .len(),
                        3
                    );
                }
                "warc" => {
                    let bytes = fs::read(&output).unwrap();
                    assert!(bytes.starts_with(b"WARC/1.1\r\n"));
                    assert!(bytes.windows(request.len()).any(|window| window == request));
                }
                "jsonl" => {
                    let lines = fs::read_to_string(&output).unwrap();
                    let values = lines
                        .lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap())
                        .collect::<Vec<_>>();
                    assert_eq!(values[0]["type"], "alex.lar.export.manifest");
                    assert_eq!(values[1]["type"], "alex.trace");
                    assert!(values[1]["metadata"].get("req_body_path").is_none());
                    assert!(values[1]["metadata"].get("resp_body_path").is_none());
                    let decoded = base64::engine::general_purpose::STANDARD
                        .decode(
                            values[1]["artifacts"]["client_request"]["data"]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                    assert_eq!(decoded, request);

                    let imported_dir = dir.join("jsonl-import-store");
                    fs::create_dir_all(&imported_dir).unwrap();
                    let imported = LocalLarBackend
                        .execute(
                            &imported_dir,
                            &parse(&["import", output.to_str().unwrap(), "--json"]),
                        )
                        .unwrap();
                    assert_eq!(imported.json["format"], "jsonl");
                    assert_eq!(imported.json["report"]["traces_imported"], 1);
                    let repeated = LocalLarBackend
                        .execute(
                            &imported_dir,
                            &parse(&[
                                "import",
                                output.to_str().unwrap(),
                                "--format",
                                "jsonl",
                                "--json",
                            ]),
                        )
                        .unwrap();
                    assert_eq!(repeated.json["report"]["traces_skipped"], 1);
                    let imported_store = Store::open(imported_dir.clone()).unwrap();
                    let imported_rows = imported_store.export_trace_backup_rows().unwrap();
                    assert_eq!(imported_rows.traces.len(), 1);
                    assert_eq!(imported_rows.traces[0]["session_id"], "session-export");
                    assert_eq!(
                        imported_store
                            .read_lar_or_legacy_artifact(
                                "trace",
                                "trace-export",
                                "client_request",
                                None,
                            )
                            .unwrap()
                            .unwrap(),
                        request
                    );
                    assert_eq!(
                        imported_store
                            .read_lar_or_legacy_artifact(
                                "trace",
                                "trace-export",
                                "client_response",
                                None,
                            )
                            .unwrap()
                            .unwrap(),
                        response
                    );
                    drop(imported_store);
                    let reexported = imported_dir.join("round-trip.jsonl");
                    LocalLarBackend
                        .execute(
                            &imported_dir,
                            &parse(&[
                                "export",
                                reexported.to_str().unwrap(),
                                "--format",
                                "jsonl",
                                "--trace-id",
                                "trace-export",
                            ]),
                        )
                        .unwrap();
                    let round_trip_values = fs::read_to_string(reexported)
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap())
                        .collect::<Vec<_>>();
                    assert_eq!(round_trip_values, values);
                }
                "otel" => {
                    let line = fs::read_to_string(&output).unwrap();
                    let value: Value = serde_json::from_str(line.trim()).unwrap();
                    assert_eq!(value["span"]["trace_id"].as_str().unwrap().len(), 32);
                    assert_eq!(value["span"]["span_id"].as_str().unwrap().len(), 16);
                    assert_eq!(value["span"]["attributes"]["alex.trace.id"], "trace-export");
                    assert_eq!(value["span"]["attributes"]["gen_ai.provider.name"], "test");
                    assert_eq!(value["span"]["attributes"]["gen_ai.operation.name"], "chat");
                }
                "openinference" => {
                    let line = fs::read_to_string(&output).unwrap();
                    let value: Value = serde_json::from_str(line.trim()).unwrap();
                    assert_eq!(value["span"]["trace_id"].as_str().unwrap().len(), 32);
                    assert_eq!(value["span"]["span_id"].as_str().unwrap().len(), 16);
                    assert_eq!(value["span"]["attributes"]["alex.trace.id"], "trace-export");
                    assert_eq!(
                        value["span"]["attributes"]["openinference.span.kind"],
                        "LLM"
                    );
                    assert_eq!(value["span"]["attributes"]["llm.system"], "test");
                    assert_eq!(
                        value["span"]["attributes"]["input.mime_type"],
                        "application/json"
                    );
                }
                _ => unreachable!(),
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn standalone_lar_export_preserves_exact_stage_closure_and_metadata() {
        use alex_store::{
            LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarConversationEntryCapture,
            LarConversationEntryKind, LarConversationGenerationEvent, LarConversationRawRange,
            LarConversationRole, LarConversationSemantics, LarConversationTurnCapture,
            LarExchangeBodyRefs, LarExchangeCapture, LarHeaderCapture, LarStreamReadCapture,
            LarUpstreamAttemptCapture,
        };

        let dir = tmpdir("exact-standalone-export");
        let store = Store::open_with_lar_body_store(
            dir.clone(),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                ..Default::default()
            },
        )
        .unwrap();
        let trace_id = "trace-exact-closure";
        let client_request = br#"{"client":true}"#;
        let upstream_request = br#"{"upstream":true}"#;
        let upstream_response = b"data: one\n\n";
        let client_response = br#"{"translated":true}"#;
        let append = |kind: &str, legacy: &str, bytes: &[u8]| {
            store
                .write_body_artifact(&LarBodyArtifact::trace(trace_id, kind), legacy, bytes)
                .unwrap()
                .manifest_id
                .unwrap()
        };
        let bodies = LarExchangeBodyRefs {
            client_request_manifest_id: Some(append(
                "client_request",
                "request.json",
                client_request,
            )),
            upstream_request_manifest_id: Some(append(
                "upstream_request",
                "upstream-request.json",
                upstream_request,
            )),
            upstream_response_manifest_id: Some(append(
                "upstream_response",
                "upstream-response.body",
                upstream_response,
            )),
            client_response_manifest_id: Some(append(
                "client_response",
                "response.body",
                client_response,
            )),
        };
        let mut trace = alex_core::TraceRecord {
            id: trace_id.into(),
            ts_request_ms: 1_700_100_000_000,
            ts_response_ms: Some(1_700_100_000_250),
            session_id: Some("session-exact-closure".into()),
            harness: Some("pi".into()),
            client_format: Some("openai-chat".into()),
            upstream_provider: Some("xai".into()),
            upstream_format: Some("openai-chat".into()),
            requested_model: Some("alex/grok-code-fast-1".into()),
            routed_model: Some("grok-code-fast-1".into()),
            method: Some("POST\r\nX-Forged-Method: no".into()),
            path: Some("/v1/chat/completions\r\nX-Forged-Path: no".into()),
            status: Some(200),
            streamed: Some(true),
            usage: alex_core::Usage {
                input_tokens: Some(41),
                cached_input_tokens: Some(7),
                cache_creation_tokens: Some(3),
                output_tokens: Some(11),
                reasoning_tokens: Some(5),
            },
            cost_usd: Some(0.0123456789),
            billing_bucket: Some("subscription".into()),
            attempts: Some(r#"[{"account":"first"},{"account":"second"}]"#.into()),
            substituted: true,
            original_model: Some("grok-code-fast-1".into()),
            served_model: Some("grok-code-fast-1".into()),
            substitution_reason: Some("rate_limit".into()),
            original_account_id: Some("xai-a".into()),
            served_account_id: Some("xai-b".into()),
            account_id: Some("xai-b".into()),
            subscription_identity: Some("xai-subscription".into()),
            run_id: Some("run-exact".into()),
            tags: Some(r#"{"suite":"lar"}"#.into()),
            client_ip: Some("127.0.0.1".into()),
            key_fingerprint: Some("fingerprint".into()),
            reasoning_effort: Some("high".into()),
            thinking_budget: Some(2048),
            ..Default::default()
        };
        trace.req_headers_json = Some(r#"[["x-repeat","one"],["x-repeat","two"]]"#.into());
        trace.resp_headers_json = Some(r#"[["content-type","application/json"]]"#.into());
        store.insert_trace(&trace).unwrap();
        let capture = LarExchangeCapture {
            trace_id: trace_id.into(),
            session_id: trace.session_id.clone(),
            run_id: trace.run_id.clone(),
            wall_time_ns: trace.ts_request_ms as u64 * 1_000_000,
            client_request_headers: Some(LarHeaderCapture::observed([
                ("x-repeat", "one"),
                ("x-repeat", "two"),
            ])),
            client_request_trailers: None,
            client_response_headers: Some(LarHeaderCapture::observed([(
                "content-type",
                "application/json",
            )])),
            client_response_trailers: None,
            upstream_attempts: vec![
                LarUpstreamAttemptCapture {
                    attempt_number: 1,
                    wall_time_ns: trace.ts_request_ms as u64 * 1_000_000 + 10,
                    request_headers: Some(LarHeaderCapture::observed([("x-attempt", "one")])),
                    request_trailers: None,
                    response_headers: Some(LarHeaderCapture::observed([("retry-after", "1")])),
                    response_trailers: None,
                    status_code: Some(429),
                    error_class: Some("rate_limit".into()),
                    error_message: Some("retry".into()),
                },
                LarUpstreamAttemptCapture {
                    attempt_number: 2,
                    wall_time_ns: trace.ts_request_ms as u64 * 1_000_000 + 20,
                    request_headers: Some(LarHeaderCapture::observed([("x-attempt", "two")])),
                    request_trailers: None,
                    response_headers: Some(LarHeaderCapture::observed([(
                        "content-type",
                        "text/event-stream",
                    )])),
                    response_trailers: None,
                    status_code: Some(200),
                    error_class: None,
                    error_message: None,
                },
            ],
            upstream_stream_reads: Some(vec![LarStreamReadCapture {
                byte_offset: 0,
                byte_length: upstream_response.len() as u64,
                delta_from_first_byte_ns: 0,
            }]),
            provider: trace.upstream_provider.clone(),
            requested_model: trace.requested_model.clone(),
            routed_model: trace.routed_model.clone(),
            account_id: trace.account_id.clone(),
            routing_reason: trace.substitution_reason.clone(),
            status_code: Some(200),
            error_class: None,
            error_message: None,
        };
        let metadata = export_exchange_metadata(&store.get_trace(trace_id).unwrap().unwrap());
        store
            .write_lar_exchange_capture_with_metadata(&capture, &bodies, &metadata)
            .unwrap()
            .unwrap();
        let entry_id = store
            .register_lar_conversation_entry(&LarConversationEntryCapture {
                semantics: LarConversationSemantics::Known {
                    source_format: "openai-chat".into(),
                    role: LarConversationRole::User,
                    kind: LarConversationEntryKind::Message,
                    name: None,
                    tool_call_id: None,
                },
                raw_ranges: vec![LarConversationRawRange {
                    manifest_id: bodies.client_request_manifest_id.clone().unwrap(),
                    byte_offset: 0,
                    byte_length: client_request.len() as u64,
                }],
            })
            .unwrap();
        store
            .record_lar_conversation_turn(&LarConversationTurnCapture {
                trace_id: trace_id.into(),
                session_id: trace.session_id.clone().unwrap(),
                event: LarConversationGenerationEvent::Initial,
                generation_entry_ids: vec![entry_id],
                upto_index: 0,
                response_entry_ids: Vec::new(),
            })
            .unwrap();
        drop(store);

        let output = dir.join("exact.lar");
        LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "export",
                    output.to_str().unwrap(),
                    "--format",
                    "lar",
                    "--trace-id",
                    trace_id,
                ]),
            )
            .unwrap();
        let mut reader =
            ArchiveReader::open(fs::File::open(&output).unwrap(), Limits::default()).unwrap();
        let exchange = reader.exchange_by_trace(trace_id.as_bytes()).unwrap();
        let kinds = exchange
            .data
            .stages
            .iter()
            .map(|id| reader.stage(id).unwrap().data.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                StageKind::ClientRequest,
                StageKind::RouterDecision,
                StageKind::UpstreamRequest,
                StageKind::UpstreamResponse,
                StageKind::UpstreamRequest,
                StageKind::UpstreamResponse,
                StageKind::ClientResponse,
            ]
        );
        assert_eq!(reader.stream_index_count(), 1);
        assert_eq!(reader.conversation_entry_count(), 1);
        assert_eq!(reader.generation_count(), 1);
        assert_eq!(reader.turn_view_count(), 1);
        assert_eq!(
            reader.exchange_metadata(&exchange.id).unwrap().data,
            metadata
        );
        let archived_bodies = reader
            .manifest_ids()
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| reader.read_body(&id).unwrap())
            .collect::<Vec<_>>();
        for body in [
            client_request.as_slice(),
            upstream_request.as_slice(),
            upstream_response.as_slice(),
            client_response.as_slice(),
        ] {
            assert!(archived_bodies.iter().any(|value| value == body));
        }

        let imported_dir = dir.join("imported");
        fs::create_dir_all(&imported_dir).unwrap();
        LocalLarBackend
            .execute(&imported_dir, &parse(&["import", output.to_str().unwrap()]))
            .unwrap();
        let imported = Store::open(imported_dir).unwrap();
        let row = imported.get_trace(trace_id).unwrap().unwrap();
        assert_eq!(row["harness"], "pi");
        assert_eq!(row["method"], trace.method.as_deref().unwrap());
        assert_eq!(row["path"], trace.path.as_deref().unwrap());
        assert_eq!(row["input_tokens"], 41);
        assert_eq!(row["cache_creation_tokens"], 3);
        assert_eq!(
            row["attempts"],
            serde_json::from_str::<Value>(trace.attempts.as_deref().unwrap()).unwrap()
        );
        assert_eq!(row["req_headers_json"], trace.req_headers_json.unwrap());
        let conversation = imported
            .lar_conversation_events_page("session-exact-closure", None, 10)
            .unwrap();
        assert_eq!(conversation.events.len(), 1);
        assert_eq!(conversation.events[0].trace_id, trace_id);
        assert_eq!(conversation.events[0].entries.len(), 1);
        assert_eq!(
            imported
                .read_lar_or_legacy_artifact("trace", trace_id, "upstream_response", None)
                .unwrap()
                .unwrap(),
            upstream_response
        );
        assert_eq!(
            imported
                .read_lar_or_legacy_artifact("trace", trace_id, "client_response", None)
                .unwrap()
                .unwrap(),
            client_response
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn canonical_interchange_streams_complete_retry_tool_and_transport_timeline() {
        use alex_store::{
            LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarExchangeBodyRefs,
            LarExchangeCapture, LarHeaderCapture, LarStreamReadCapture, LarUpstreamAttemptCapture,
            ToolCallRecord,
        };

        let dir = tmpdir("canonical-interchange");
        let store = Store::open_with_lar_body_store(
            dir.clone(),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                max_pack_bytes: 1_024,
                ..Default::default()
            },
        )
        .unwrap();
        let trace_id = "trace-canonical-interchange";
        let session_id = "session-canonical-interchange";
        let large_request = (0..(JSONL_BODY_PART_BYTES * 3 + 17))
            .map(|index| (index.wrapping_mul(31) % 251) as u8)
            .collect::<Vec<_>>();
        let upstream_request = br#"{"messages":[{"role":"user","content":"hello"}]}"#;
        let streamed_response = b"data: one\n\ndata: two\n\n";
        let append = |kind: &str, legacy: &str, bytes: &[u8]| {
            store
                .write_body_artifact(&LarBodyArtifact::trace(trace_id, kind), legacy, bytes)
                .unwrap()
                .manifest_id
                .unwrap()
        };
        let client_manifest = append("client_request", "request.json", &large_request);
        let upstream_manifest = append(
            "upstream_request",
            "upstream-request.json",
            upstream_request,
        );
        let response_manifest = append(
            "upstream_response",
            "upstream-response.body",
            streamed_response,
        );
        let body_refs = LarExchangeBodyRefs {
            client_request_manifest_id: Some(client_manifest),
            upstream_request_manifest_id: Some(upstream_manifest),
            upstream_response_manifest_id: Some(response_manifest.clone()),
            // Deliberately share the same logical body across two stage roles.
            client_response_manifest_id: Some(response_manifest),
        };
        let trace = alex_core::TraceRecord {
            id: trace_id.into(),
            session_id: Some(session_id.into()),
            ts_request_ms: 1_701_000_000_000,
            ts_response_ms: Some(1_701_000_000_250),
            method: Some("POST".into()),
            path: Some("/v1/chat/completions".into()),
            upstream_provider: Some("xai".into()),
            requested_model: Some("alex/grok".into()),
            routed_model: Some("grok".into()),
            status: Some(200),
            streamed: Some(true),
            ..Default::default()
        };
        store.insert_trace(&trace).unwrap();
        let capture = LarExchangeCapture {
            trace_id: trace_id.into(),
            session_id: Some(session_id.into()),
            run_id: Some("run-canonical".into()),
            wall_time_ns: trace.ts_request_ms as u64 * 1_000_000,
            client_request_headers: Some(LarHeaderCapture::observed([
                ("X-Duplicate", "one"),
                ("X-Duplicate", "two"),
            ])),
            client_request_trailers: Some(LarHeaderCapture::observed([(
                "x-client-request-trailer",
                "request-end",
            )])),
            client_response_headers: Some(LarHeaderCapture::observed([(
                "content-type",
                "text/event-stream",
            )])),
            client_response_trailers: Some(LarHeaderCapture::observed([(
                "x-client-response-trailer",
                "response-end",
            )])),
            upstream_attempts: vec![
                LarUpstreamAttemptCapture {
                    attempt_number: 1,
                    wall_time_ns: trace.ts_request_ms as u64 * 1_000_000 + 10,
                    request_headers: Some(LarHeaderCapture::observed([("x-attempt", "one")])),
                    request_trailers: Some(LarHeaderCapture::observed([(
                        "x-request-trailer",
                        "one-end",
                    )])),
                    response_headers: Some(LarHeaderCapture::observed([("retry-after", "1")])),
                    response_trailers: Some(LarHeaderCapture::observed([(
                        "x-response-trailer",
                        "one-end",
                    )])),
                    status_code: Some(429),
                    error_class: Some("rate_limit".into()),
                    error_message: Some("retry".into()),
                },
                LarUpstreamAttemptCapture {
                    attempt_number: 2,
                    wall_time_ns: trace.ts_request_ms as u64 * 1_000_000 + 20,
                    request_headers: Some(LarHeaderCapture::observed([("x-attempt", "two")])),
                    request_trailers: None,
                    response_headers: Some(LarHeaderCapture::observed([(
                        "content-type",
                        "text/event-stream",
                    )])),
                    response_trailers: Some(LarHeaderCapture::observed([(
                        "x-response-trailer",
                        "two-end",
                    )])),
                    status_code: Some(200),
                    error_class: None,
                    error_message: None,
                },
            ],
            upstream_stream_reads: Some(vec![
                LarStreamReadCapture {
                    byte_offset: 0,
                    byte_length: 11,
                    delta_from_first_byte_ns: 0,
                },
                LarStreamReadCapture {
                    byte_offset: 11,
                    byte_length: (streamed_response.len() - 11) as u64,
                    delta_from_first_byte_ns: 5_000,
                },
            ]),
            provider: Some("xai".into()),
            requested_model: Some("alex/grok".into()),
            routed_model: Some("grok".into()),
            account_id: Some("xai-1".into()),
            routing_reason: Some("retry".into()),
            status_code: Some(200),
            error_class: None,
            error_message: None,
        };
        store
            .write_lar_exchange_capture_with_metadata(
                &capture,
                &body_refs,
                &export_exchange_metadata(&store.get_trace(trace_id).unwrap().unwrap()),
            )
            .unwrap()
            .unwrap();

        let live = store.lar_interchange_trace(trace_id).unwrap().unwrap();
        assert_eq!(
            live.stages
                .iter()
                .filter(|stage| stage.kind == "upstream_request")
                .count(),
            2
        );
        assert!(live
            .stages
            .iter()
            .any(|stage| stage.data.trailers_ref.is_some()));
        assert_eq!(live.streams.len(), 1);
        assert_eq!(live.streams[0].reads.len(), 2);
        assert_eq!(
            live.bodies
                .iter()
                .filter(|body| body.manifest_id
                    == body_refs.client_response_manifest_id.clone().unwrap())
                .count(),
            1
        );

        // A subsequent write rotates the tiny active pack. The same exact
        // projection must resolve from the now-sealed archive.
        store
            .write_body_artifact(
                &LarBodyArtifact::trace("rotation-filler", "client_request"),
                "request.json",
                &vec![0x5a; 4_096],
            )
            .unwrap();
        let sealed = store.lar_interchange_trace(trace_id).unwrap().unwrap();
        assert_eq!(sealed.stages, live.stages);
        assert_eq!(sealed.header_blocks, live.header_blocks);
        assert_eq!(sealed.streams, live.streams);

        let tool_args = store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-canonical", "tool_arguments"),
                "tool-args.json",
                br#"{"path":"/tmp"}"#,
            )
            .unwrap();
        let tool_result = store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-canonical", "tool_result"),
                "tool-result.json",
                b"file-a\nfile-b\n",
            )
            .unwrap();
        store
            .upsert_live_tool_call_with_timeline(&ToolCallRecord {
                id: "tool-canonical".into(),
                harness: "pi".into(),
                session_id: session_id.into(),
                turn_id: Some("turn-1".into()),
                tool_call_id: "call-1".into(),
                trace_id: Some(trace_id.into()),
                tool_name: "ls".into(),
                ts_start_ms: trace.ts_request_ms + 100,
                ts_end_ms: Some(trace.ts_request_ms + 150),
                is_error: Some(false),
                exit_status: Some(0),
                args_body_path: Some(tool_args.legacy_path),
                result_body_path: Some(tool_result.legacy_path),
            })
            .unwrap();
        let graph = store.lar_interchange_trace(trace_id).unwrap().unwrap();
        assert_eq!(
            graph
                .stages
                .iter()
                .filter(|stage| stage.kind == "tool_call")
                .count(),
            1
        );
        assert_eq!(
            graph
                .stages
                .iter()
                .filter(|stage| stage.kind == "tool_result")
                .count(),
            1
        );
        for tool_stage in graph.stages.iter().filter(|stage| stage.tool_id.is_some()) {
            assert_eq!(tool_stage.tool_id.as_deref(), Some("tool-canonical"));
            assert!(!tool_stage.stage_id.is_empty());
            assert!(!tool_stage.record_id.is_empty());
            assert!(tool_stage.supplement_trace_id.is_some());
            assert!(tool_stage.supplement_exchange_id.is_some());
        }

        // Exercise more than one selection page without retaining all rows.
        for index in 0..(INTERCHANGE_TRACE_PAGE_SIZE + 3) {
            store
                .insert_trace(&alex_core::TraceRecord {
                    id: format!("legacy-page-{index:03}"),
                    session_id: Some(session_id.into()),
                    ts_request_ms: trace.ts_request_ms + 1_000 + index as i64,
                    ..Default::default()
                })
                .unwrap();
        }
        drop(store);

        let jsonl = dir.join("canonical.jsonl");
        let report = LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "export",
                    jsonl.to_str().unwrap(),
                    "--format",
                    "jsonl",
                    "--session",
                    session_id,
                    "--json",
                ]),
            )
            .unwrap();
        assert_eq!(report.json["canonical_traces"], 1);
        assert_eq!(
            report.json["traces"],
            (INTERCHANGE_TRACE_PAGE_SIZE + 4) as u64
        );
        let file = fs::File::open(&jsonl).unwrap();
        let mut canonical_record = None;
        let mut body_parts = HashMap::<String, usize>::new();
        let mut maximum_line = 0usize;
        use std::io::BufRead as _;
        for line in std::io::BufReader::new(file).lines() {
            let line = line.unwrap();
            maximum_line = maximum_line.max(line.len());
            let value: Value = serde_json::from_str(&line).unwrap();
            match value["type"].as_str() {
                Some("alex.trace.canonical") => canonical_record = Some(value),
                Some("alex.body.part") => {
                    assert!(value["byte_length"].as_u64().unwrap() <= JSONL_BODY_PART_BYTES as u64);
                    *body_parts
                        .entry(value["manifest_id"].as_str().unwrap().into())
                        .or_default() += 1;
                }
                _ => {}
            }
        }
        assert!(maximum_line < 512 * 1024);
        assert!(body_parts.values().any(|count| *count >= 4));
        let canonical_record = canonical_record.unwrap();
        let capture = &canonical_record["graph"]["capture"];
        assert_eq!(
            capture["schema"],
            "alex.lar.canonical-timeline-projection.v2"
        );
        assert!(capture["exchange"].get("exchange_id").is_none());
        assert_eq!(
            capture["exchange"]["base_exchange_content_id"],
            graph.exchange_id
        );
        assert_eq!(
            capture["stages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|stage| stage["kind"] == "tool_call" || stage["kind"] == "tool_result")
                .count(),
            2
        );
        assert_eq!(capture["streams"][0]["reads"].as_array().unwrap().len(), 2);

        let import_error = LocalLarBackend
            .execute(
                &dir.join("v2-import"),
                &parse(&["import", jsonl.to_str().unwrap(), "--format", "jsonl"]),
            )
            .unwrap_err();
        assert!(format!("{import_error:#}")
            .contains("without discarding retries, trailers, streams, or tool links"));

        for format in ["har", "warc", "otel", "openinference"] {
            let output = dir.join(format!("canonical.{format}"));
            let exported = LocalLarBackend
                .execute(
                    &dir,
                    &parse(&[
                        "export",
                        output.to_str().unwrap(),
                        "--format",
                        format,
                        "--trace-id",
                        trace_id,
                        "--json",
                    ]),
                )
                .unwrap();
            assert_eq!(exported.json["canonical_traces"], 1);
            assert!(!exported.json["loss_report"].as_array().unwrap().is_empty());
            let bytes = fs::read(&output).unwrap();
            match format {
                "har" => {
                    let har: Value = serde_json::from_slice(&bytes).unwrap();
                    let entry = &har["log"]["entries"][0];
                    assert!(!entry["request"]["postData"]["text"]
                        .as_str()
                        .unwrap()
                        .is_empty());
                    assert!(!entry["response"]["content"]["text"]
                        .as_str()
                        .unwrap()
                        .is_empty());
                    assert_eq!(
                        entry["_alex"]["canonicalGraph"]["capture"]["stages"]
                            .as_array()
                            .unwrap()
                            .len(),
                        graph.stages.len()
                    );
                }
                "warc" => {
                    let records = parse_warc_records(&bytes);
                    let mut record_ids = std::collections::HashSet::new();
                    for record in &records {
                        assert!(record_ids.insert(record.headers["WARC-Record-ID"].clone()));
                        assert_eq!(
                            record.headers["WARC-Block-Digest"],
                            format!("sha256:{}", hex_bytes(&Sha256::digest(&record.payload)))
                        );
                        if record.headers["Content-Type"].starts_with("application/http") {
                            for forbidden in [
                                b"\r\nX-Forged-Method".as_slice(),
                                b"\r\nX-Forged-Path:".as_slice(),
                            ] {
                                assert!(!record
                                    .payload
                                    .windows(forbidden.len())
                                    .any(|window| window == forbidden));
                            }
                        }
                    }
                    assert_eq!(
                        records
                            .iter()
                            .filter(|record| record.headers["WARC-Type"] == "resource")
                            .count(),
                        graph.bodies.len()
                    );
                    assert!(
                        records
                            .iter()
                            .filter(|record| {
                                record.headers["Content-Type"]
                                    == "application/http; msgtype=request"
                            })
                            .count()
                            >= 3
                    );
                    assert!(records.iter().any(|record| {
                        record.headers["Content-Type"] == "application/http; msgtype=response"
                            && record.payload.starts_with(b"HTTP/1.1 200 \r\n")
                    }));
                    assert!(
                        records
                            .iter()
                            .filter(|record| {
                                record.headers["Content-Type"]
                                    == "application/http; msgtype=response"
                            })
                            .count()
                            >= 3
                    );
                    for response in graph.stages.iter().filter(|stage| {
                        matches!(
                            stage.kind.as_str(),
                            "upstream_response" | "upstream_failure"
                        )
                    }) {
                        let attempt = response.data.attempt_number.unwrap();
                        let request = graph
                            .stages
                            .iter()
                            .find(|stage| {
                                stage.kind == "upstream_request"
                                    && stage.data.attempt_number == Some(attempt)
                            })
                            .unwrap();
                        let response_id = warc_record_id(
                            trace_id,
                            &format!("stage:{}:response", response.stage_id),
                        );
                        let request_id = warc_record_id(
                            trace_id,
                            &format!("stage:{}:request", request.stage_id),
                        );
                        let response_record = records
                            .iter()
                            .find(|record| record.headers["WARC-Record-ID"] == response_id)
                            .unwrap();
                        assert_eq!(response_record.headers["WARC-Concurrent-To"], request_id);
                    }
                }
                "otel" | "openinference" => {
                    let value: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(
                        value["alex_canonical_graph"]["capture"]["stages"]
                            .as_array()
                            .unwrap()
                            .len(),
                        graph.stages.len()
                    );
                }
                _ => unreachable!(),
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_trace_snapshot_excludes_concurrent_backdated_inserts() {
        let dir = tmpdir("export-snapshot");
        let store = Store::open(dir.clone()).unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "snapshot-original".into(),
                ts_request_ms: 200,
                ..Default::default()
            })
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "snapshot-second".into(),
                ts_request_ms: 300,
                ..Default::default()
            })
            .unwrap();
        let upper = store
            .lar_export_trace_upper_bound(None, None)
            .unwrap()
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "snapshot-concurrent-backdated".into(),
                ts_request_ms: 100,
                ..Default::default()
            })
            .unwrap();
        let rows = store
            .lar_export_trace_rows_page(None, None, None, &upper, 32)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "snapshot-original");
        assert_eq!(rows[1]["id"], "snapshot-second");

        let output = dir.join("diverged.jsonl");
        let args = ExportArgs {
            output: output.clone(),
            format: LarExportFormat::Jsonl,
            trace_id: None,
            session: None,
            force: false,
            json: false,
        };
        let summary = summarize_export_selection(&store, &args, &upper).unwrap();
        assert_eq!(summary.traces, 2);
        store.delete_trace("snapshot-original").unwrap();
        let error = write_streaming_interchange_export(
            &store,
            &args,
            LarExportFormat::Jsonl,
            summary,
            &upper,
            &[],
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(
            "preflight selected 2 trace(s), but 1 remained at the frozen high-water mark"
        ));
        assert!(!output.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bodies_only_prune_is_authoritative_for_canonical_exports() {
        use alex_store::{
            LarBodyArtifact, LarBodyStoreConfig, LarBodyStoreMode, LarExchangeBodyRefs,
            LarExchangeCapture, ToolCallRecord,
        };

        let dir = tmpdir("export-after-bodies-prune");
        let store = Store::open_with_lar_body_store(
            dir.clone(),
            LarBodyStoreConfig {
                mode: LarBodyStoreMode::LarWithFallback,
                ..Default::default()
            },
        )
        .unwrap();
        let trace_id = "trace-bodies-pruned";
        let session_id = "session-bodies-pruned";
        let request_manifest = store
            .write_body_artifact(
                &LarBodyArtifact::trace(trace_id, "client_request"),
                "request.json",
                br#"{"secret":"request"}"#,
            )
            .unwrap()
            .manifest_id
            .unwrap();
        let response_manifest = store
            .write_body_artifact(
                &LarBodyArtifact::trace(trace_id, "client_response"),
                "response.json",
                br#"{"secret":"response"}"#,
            )
            .unwrap()
            .manifest_id
            .unwrap();
        let trace = alex_core::TraceRecord {
            id: trace_id.into(),
            session_id: Some(session_id.into()),
            ts_request_ms: 100,
            ts_response_ms: Some(150),
            method: Some("POST".into()),
            path: Some("/v1/messages".into()),
            status: Some(200),
            ..Default::default()
        };
        store.insert_trace(&trace).unwrap();
        store
            .write_lar_exchange_capture_with_metadata(
                &LarExchangeCapture {
                    trace_id: trace_id.into(),
                    session_id: Some(session_id.into()),
                    run_id: None,
                    wall_time_ns: 100_000_000,
                    client_request_headers: None,
                    client_request_trailers: None,
                    client_response_headers: None,
                    client_response_trailers: None,
                    upstream_attempts: Vec::new(),
                    upstream_stream_reads: None,
                    provider: None,
                    requested_model: None,
                    routed_model: None,
                    account_id: None,
                    routing_reason: None,
                    status_code: Some(200),
                    error_class: None,
                    error_message: None,
                },
                &LarExchangeBodyRefs {
                    client_request_manifest_id: Some(request_manifest),
                    client_response_manifest_id: Some(response_manifest),
                    ..Default::default()
                },
                &export_exchange_metadata(&store.get_trace(trace_id).unwrap().unwrap()),
            )
            .unwrap()
            .unwrap();
        let tool_args = store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-bodies-pruned", "tool_arguments"),
                "tool-args.json",
                br#"{"secret":"tool arguments"}"#,
            )
            .unwrap();
        let tool_result = store
            .write_body_artifact(
                &LarBodyArtifact::tool_call("tool-bodies-pruned", "tool_result"),
                "tool-result.json",
                br#"{"secret":"tool result"}"#,
            )
            .unwrap();
        store
            .upsert_live_tool_call_with_timeline(&ToolCallRecord {
                id: "tool-bodies-pruned".into(),
                harness: "pi".into(),
                session_id: session_id.into(),
                turn_id: Some("turn-pruned".into()),
                tool_call_id: "call-pruned".into(),
                trace_id: Some(trace_id.into()),
                tool_name: "read".into(),
                ts_start_ms: 120,
                ts_end_ms: Some(140),
                is_error: Some(false),
                exit_status: Some(0),
                args_body_path: Some(tool_args.legacy_path),
                result_body_path: Some(tool_result.legacy_path),
            })
            .unwrap();
        let before_prune = store.lar_interchange_trace(trace_id).unwrap().unwrap();
        assert_eq!(
            before_prune.bodies.len(),
            4,
            "base and late tool bodies must all be visible before pruning"
        );
        let supplement_trace_ids = before_prune
            .stages
            .iter()
            .filter_map(|stage| stage.supplement_trace_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(supplement_trace_ids.len(), 2);
        store.prune(200, true, false).unwrap();
        let pruned = store.lar_interchange_trace(trace_id).unwrap().unwrap();
        assert!(pruned.bodies.is_empty());
        assert!(pruned.stages.iter().all(|stage| {
            stage.data.request_body_manifest_ref.is_none()
                && stage.data.response_body_manifest_ref.is_none()
        }));
        drop(store);

        let jsonl = dir.join("pruned.jsonl");
        LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "export",
                    jsonl.to_str().unwrap(),
                    "--format",
                    "jsonl",
                    "--trace-id",
                    trace_id,
                ]),
            )
            .unwrap();
        let canonical = fs::read_to_string(&jsonl)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|line| line["type"] == "alex.trace.canonical")
            .unwrap();
        assert!(canonical["graph"]["bodies"].as_array().unwrap().is_empty());
        assert!(canonical["graph"]["capture"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["request_body_manifest_ref"].is_null()
                && stage["response_body_manifest_ref"].is_null()));

        let lar = dir.join("pruned.lar");
        LocalLarBackend
            .execute(
                &dir,
                &parse(&[
                    "export",
                    lar.to_str().unwrap(),
                    "--format",
                    "lar",
                    "--trace-id",
                    trace_id,
                ]),
            )
            .unwrap();
        let reader = ArchiveReader::open(fs::File::open(&lar).unwrap(), Limits::default()).unwrap();
        let exchange = reader
            .exchange_by_trace(trace_id.as_bytes())
            .unwrap()
            .clone();
        assert!(exchange.data.stages.iter().all(|stage_id| {
            let stage = reader.stage(stage_id).unwrap();
            stage.data.request_body_manifest_ref.is_none()
                && stage.data.response_body_manifest_ref.is_none()
        }));
        for supplement_trace_id in supplement_trace_ids {
            let supplement = reader
                .exchange_by_trace(supplement_trace_id.as_bytes())
                .unwrap();
            assert!(supplement.data.stages.iter().all(|stage_id| {
                let stage = reader.stage(stage_id).unwrap();
                stage.data.request_body_manifest_ref.is_none()
                    && stage.data.response_body_manifest_ref.is_none()
            }));
        }
        assert_eq!(reader.manifest_ids().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cleanup_requires_completed_verified_migration_then_quarantines_legacy_files() {
        let dir = tmpdir("cleanup");
        let store = Store::open(dir.clone()).unwrap();
        let expected = b"legacy bytes remain recoverable";
        let legacy_path = store
            .write_body("trace-cleanup", "request.json", expected)
            .unwrap();
        store
            .insert_trace(&alex_core::TraceRecord {
                id: "trace-cleanup".into(),
                req_body_path: Some(legacy_path.clone()),
                ..Default::default()
            })
            .unwrap();
        drop(store);

        let blocked = LocalLarBackend
            .execute(&dir, &parse(&["cleanup", "--dry-run", "--json"]))
            .unwrap();
        assert_eq!(blocked.json["eligible"], false);
        assert_eq!(blocked.json["moved_files"], 0);
        assert!(Path::new(&legacy_path).exists());

        LocalLarBackend
            .execute(&dir, &parse(&["import-legacy", "--verify"]))
            .unwrap();
        let dry_run = LocalLarBackend
            .execute(&dir, &parse(&["cleanup", "--dry-run", "--json"]))
            .unwrap();
        assert_eq!(dry_run.json["eligible"], true);
        assert_eq!(dry_run.json["candidate_files"], 1);
        assert_eq!(
            dry_run.json["candidate_bytes"],
            fs::metadata(&legacy_path).unwrap().len()
        );
        assert_eq!(dry_run.json["moved_files"], 0);
        assert!(Path::new(&legacy_path).exists());

        let applied = LocalLarBackend
            .execute(&dir, &parse(&["cleanup", "--apply", "--json"]))
            .unwrap();
        assert_eq!(applied.json["moved_files"], 1);
        assert_eq!(applied.json["recoverable"], true);
        assert!(!Path::new(&legacy_path).exists());
        let quarantine = PathBuf::from(applied.json["quarantine_dir"].as_str().unwrap());
        assert!(quarantine.join("cleanup-plan.json").is_file());
        assert!(quarantine.join("cleanup-result.json").is_file());
        assert_eq!(
            Store::open(dir.clone())
                .unwrap()
                .read_lar_or_legacy_artifact("trace", "trace-cleanup", "client_request", None,)
                .unwrap()
                .as_deref(),
            Some(expected.as_slice())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn no_force_publication_cannot_clobber_a_racing_destination() {
        let dir = tmpdir("publish-no-clobber");
        let temporary = dir.join(".export.tmp");
        let output = dir.join("archive.lar");
        fs::write(&temporary, b"new archive").unwrap();
        fs::write(&output, b"racing writer").unwrap();

        let error = publish_export_temp(&temporary, &output, false).unwrap_err();
        assert!(error.to_string().contains("without replacing"));
        assert_eq!(fs::read(&output).unwrap(), b"racing writer");
        assert_eq!(fs::read(&temporary).unwrap(), b"new archive");
        fs::remove_dir_all(dir).unwrap();
    }
}
