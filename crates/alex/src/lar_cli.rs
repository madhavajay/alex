use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use alex_lar::{
    upgrade_archive as rewrite_archive, verify_upgraded_archive, ArchiveReader, ArchiveWriter,
    ChunkerConfig, Exchange, ExchangeData, ExchangeMetadataData, FileHeader, HeaderAtom,
    HeaderBlock, HeaderFidelity, Limits, ManifestId, RawBodyScanner, RawSearchLimits,
    RawSearchStats, RecoveryStatus, Stage, StageData, StageId, StageKind, StreamReplaySource,
    StreamReplayTiming, TokenUsage, REQUIRED_FEATURE_CONVERSATION_DAG,
};
use alex_store::{
    LarArtifactLocation, LarBackupArtifactRef, LarBodyStoreConfig, LarBodyStoreMode,
    LarCatalogGrepMatch, LarJsonlImportOptions, LarLegacyImportOptions, LarMigrationJob,
    LarRepackConfig, LarStandaloneImportOptions, Store, TraceBackupRows,
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
    /// Emit machine-readable JSON
    #[arg(long)]
    pub(crate) json: bool,
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
            LarCommand::Export(args) => export_records(data_dir, args),
        }
    }
}

pub(crate) fn run(data_dir: &Path, command: LarCommand) -> Result<()> {
    if let LarCommand::Replay(args) = &command {
        return replay_stream(args);
    }
    let json = command.json();
    LocalLarBackend.execute(data_dir, &command)?.print(json)
}

impl LarCommand {
    fn json(&self) -> bool {
        match self {
            Self::Import(args) => args.json,
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
            Self::Export(args) => args.json,
        }
    }
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
    let listed_jobs = jobs.iter().take(args.limit).collect::<Vec<_>>();
    let incomplete_jobs = jobs.iter().filter(|job| job.state != "complete").count();
    let json = serde_json::json!({
        "kind": "live_catalog",
        "schema_version": schema_version,
        "migration_jobs": listed_jobs
            .iter()
            .map(|job| MigrationJobOutput::from(*job))
            .collect::<Vec<_>>(),
        "migration_job_count": jobs.len(),
        "incomplete_migration_jobs": incomplete_jobs,
        "limited": listed_jobs.len() < jobs.len(),
    });
    let human = format!(
        "live LAR catalog schema v{schema_version}: {} migration job(s), {incomplete_jobs} incomplete{}",
        jobs.len(),
        if listed_jobs.len() < jobs.len() {
            format!("; showing the newest {}", listed_jobs.len())
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
    manifest_ids: Vec<String>,
    limited: bool,
}

fn open_archive_summary(
    path: &Path,
    verify_bodies: bool,
    manifest_limit: usize,
) -> Result<ArchiveSummary> {
    preflight_archive(path)?;
    let file =
        fs::File::open(path).with_context(|| format!("opening LAR archive {}", path.display()))?;
    let mut reader = ArchiveReader::open(file, Limits::default())
        .map_err(|error| anyhow::anyhow!(error))
        .with_context(|| format!("scanning LAR archive {}", path.display()))?;
    let manifest_ids = reader.manifest_ids().copied().collect::<Vec<_>>();
    let mut verified_manifest_count = 0;
    if verify_bodies {
        for manifest_id in &manifest_ids {
            reader
                .write_body(manifest_id, std::io::sink())
                .map_err(|error| anyhow::anyhow!(error))
                .with_context(|| format!("verifying manifest {manifest_id}"))?;
            verified_manifest_count += 1;
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
        manifest_ids: listed_ids,
        limited: manifest_ids.len() > manifest_limit,
    })
}

fn verify_archive(path: &Path, _keep_going: bool) -> Result<LarCommandOutput> {
    let summary = open_archive_summary(path, true, usize::MAX)?;
    if summary.recovery != "clean" {
        bail!(
            "LAR archive {} has recovery state {} with {} tail bytes after offset {}; run `alex lar repair {} --output <new-file>`",
            path.display(),
            summary.recovery,
            summary.truncated_tail_bytes,
            summary.last_valid_offset.unwrap_or_default(),
            path.display(),
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

    let mut archives = args.archives.clone();
    archives.sort();
    archives.dedup();
    for archive in archives {
        preflight_archive(&archive)?;
        let stats = grep_sealed_archive(&archive, literal, args.limit, &mut matches)?;
        sources.push(stats);
    }
    matches.sort();
    let human = if matches.is_empty() {
        format!(
            "no exact raw byte matches for {:?}; scanned {} source(s)",
            args.literal,
            sources.len()
        )
    } else {
        let mut lines = matches.iter().map(grep_match_human).collect::<Vec<_>>();
        lines.push(format!(
            "{} exact reference match(es) across {} source(s)",
            matches.len(),
            sources.len()
        ));
        lines.join("\n")
    };
    Ok(LarCommandOutput {
        human,
        json: serde_json::json!({
            "literal": args.literal,
            "literal_hex": hex_bytes(literal),
            "match_count": matches.len(),
            "matches": matches,
            "sources": sources,
        }),
        raw_body: None,
    })
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
}

fn export_records(data_dir: &Path, args: &ExportArgs) -> Result<LarCommandOutput> {
    let store = Store::open(data_dir.to_path_buf()).context("opening the Alex storage catalog")?;
    let traces = load_export_traces(&store, args, !matches!(args.format, LarExportFormat::Lar))?;
    if traces.is_empty() {
        bail!("no traces matched the requested export selection");
    }
    let losses = export_loss_report();
    let (byte_count, verified) = match args.format {
        LarExportFormat::Lar => {
            let bytes = write_standalone_lar_export(&store, &traces, &args.output, args.force)?;
            (bytes, true)
        }
        format => {
            let bytes = match format {
                LarExportFormat::Har => build_har(&traces, &losses)?,
                LarExportFormat::Warc => build_warc(&traces, &losses)?,
                LarExportFormat::Jsonl => build_jsonl(&traces, &losses)?,
                LarExportFormat::OpenTelemetry => build_otel_jsonl(&traces, &losses)?,
                LarExportFormat::OpenInference => build_openinference_jsonl(&traces, &losses)?,
                LarExportFormat::Lar => unreachable!("LAR handled by streaming branch"),
            };
            write_export_file(&args.output, &bytes, args.force)?;
            (bytes.len() as u64, true)
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
        traces: traces.len(),
        bytes: byte_count,
        verified,
        loss_report: losses,
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

fn encoded_artifact(bytes: Option<&[u8]>) -> Value {
    match bytes {
        None => Value::Null,
        Some(bytes) => serde_json::json!({
            "encoding": "base64",
            "length": bytes.len(),
            "blake3": hex_bytes(&alex_lar::ChunkHash::blake3(bytes).digest),
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
    }
}

fn build_jsonl(traces: &[ExportTrace], losses: &[&str]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    serde_json::to_writer(
        &mut output,
        &serde_json::json!({
            "type": "alex.lar.export.manifest",
            "version": 1,
            "format": "jsonl",
            "loss_report": losses,
        }),
    )?;
    output.push(b'\n');
    for trace in traces {
        let metadata = sanitized_trace_metadata(&trace.row);
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({
                "type": "alex.trace",
                "metadata": metadata,
                "headers": {
                    "request": trace.request_headers,
                    "response": trace.response_headers,
                    "fidelity": "legacy_order_and_casing_unknown",
                },
                "artifacts": {
                    "client_request": encoded_artifact(trace.request.as_deref()),
                    "upstream_request": encoded_artifact(trace.upstream_request.as_deref()),
                    "client_response": encoded_artifact(trace.response.as_deref()),
                },
            }),
        )?;
        output.push(b'\n');
    }
    Ok(output)
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

fn build_har(traces: &[ExportTrace], losses: &[&str]) -> Result<Vec<u8>> {
    let entries = traces
        .iter()
        .map(|trace| {
            let request_time = trace.row["ts_request_ms"].as_i64().unwrap_or_default();
            let response_time = trace.row["ts_response_ms"].as_i64().unwrap_or(request_time);
            let duration = response_time.saturating_sub(request_time).max(0);
            let request = trace.request.as_deref().unwrap_or_default();
            let response = trace.response.as_deref().unwrap_or_default();
            let request_type = header_value(&trace.request_headers, "content-type")
                .unwrap_or("application/octet-stream");
            let response_type = header_value(&trace.response_headers, "content-type")
                .unwrap_or("application/octet-stream");
            serde_json::json!({
                "startedDateTime": rfc3339_millis(request_time),
                "time": duration,
                "request": {
                    "method": trace.row["method"].as_str().unwrap_or("POST"),
                    "url": trace.row["path"].as_str().unwrap_or("/"),
                    "httpVersion": "HTTP/1.1",
                    "cookies": [],
                    "headers": trace.request_headers,
                    "queryString": [],
                    "postData": {
                        "mimeType": request_type,
                        "text": base64::engine::general_purpose::STANDARD.encode(request),
                        "_encoding": "base64",
                    },
                    "headersSize": -1,
                    "bodySize": request.len(),
                },
                "response": {
                    "status": trace.row["status"].as_i64().unwrap_or_default(),
                    "statusText": "",
                    "httpVersion": "HTTP/1.1",
                    "cookies": [],
                    "headers": trace.response_headers,
                    "content": {
                        "size": response.len(),
                        "mimeType": response_type,
                        "text": base64::engine::general_purpose::STANDARD.encode(response),
                        "encoding": "base64",
                    },
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": response.len(),
                },
                "cache": {},
                "timings": {"send": 0, "wait": duration, "receive": 0},
                "_alex": {
                    "trace": sanitized_trace_metadata(&trace.row),
                    "upstreamRequest": encoded_artifact(trace.upstream_request.as_deref()),
                    "headerFidelity": "legacy_order_and_casing_unknown",
                    "lossReport": losses,
                }
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_vec_pretty(&serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "Alex", "version": env!("CARGO_PKG_VERSION")},
            "entries": entries,
        }
    }))?)
}

fn header_value<'a>(headers: &'a [ExportHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn rfc3339_millis(timestamp_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_owned())
}

fn build_warc(traces: &[ExportTrace], losses: &[&str]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for trace in traces {
        let id = trace.row["id"].as_str().unwrap_or("unknown");
        let request_id = warc_record_id(id, "request");
        let request_payload = http_request_payload(trace);
        append_warc_record(
            &mut output,
            "request",
            &request_id,
            None,
            &rfc3339_millis(trace.row["ts_request_ms"].as_i64().unwrap_or_default()),
            "application/http; msgtype=request",
            &request_payload,
            losses,
        )?;
        let response_payload = http_response_payload(trace);
        append_warc_record(
            &mut output,
            "response",
            &warc_record_id(id, "response"),
            Some(&request_id),
            &rfc3339_millis(
                trace.row["ts_response_ms"]
                    .as_i64()
                    .unwrap_or_else(|| trace.row["ts_request_ms"].as_i64().unwrap_or_default()),
            ),
            "application/http; msgtype=response",
            &response_payload,
            losses,
        )?;
    }
    Ok(output)
}

fn warc_record_id(trace_id: &str, kind: &str) -> String {
    let digest = Sha256::digest(format!("alex:{trace_id}:{kind}").as_bytes());
    format!("<urn:alex:sha256:{}>", hex_bytes(&digest))
}

fn http_request_payload(trace: &ExportTrace) -> Vec<u8> {
    let mut output = format!(
        "{} {} HTTP/1.1\r\n",
        trace.row["method"].as_str().unwrap_or("POST"),
        trace.row["path"].as_str().unwrap_or("/")
    )
    .into_bytes();
    append_http_headers(&mut output, &trace.request_headers);
    output.extend_from_slice(b"\r\n");
    output.extend_from_slice(trace.request.as_deref().unwrap_or_default());
    output
}

fn http_response_payload(trace: &ExportTrace) -> Vec<u8> {
    let mut output = format!(
        "HTTP/1.1 {}\r\n",
        trace.row["status"].as_i64().unwrap_or_default()
    )
    .into_bytes();
    append_http_headers(&mut output, &trace.response_headers);
    output.extend_from_slice(b"\r\n");
    output.extend_from_slice(trace.response.as_deref().unwrap_or_default());
    output
}

fn append_http_headers(output: &mut Vec<u8>, headers: &[ExportHeader]) {
    for header in headers {
        output.extend_from_slice(header.name.as_bytes());
        output.extend_from_slice(b": ");
        output.extend_from_slice(header.value.as_bytes());
        output.extend_from_slice(b"\r\n");
    }
}

#[allow(clippy::too_many_arguments)]
fn append_warc_record(
    output: &mut Vec<u8>,
    record_type: &str,
    record_id: &str,
    concurrent_to: Option<&str>,
    date: &str,
    content_type: &str,
    payload: &[u8],
    losses: &[&str],
) -> Result<()> {
    let payload_digest = hex_bytes(&Sha256::digest(payload));
    write!(output, "WARC/1.1\r\n")?;
    write!(output, "WARC-Type: {record_type}\r\n")?;
    write!(output, "WARC-Record-ID: {record_id}\r\n")?;
    write!(output, "WARC-Date: {date}\r\n")?;
    if let Some(id) = concurrent_to {
        write!(output, "WARC-Concurrent-To: {id}\r\n")?;
    }
    write!(output, "WARC-Payload-Digest: sha256:{payload_digest}\r\n")?;
    write!(output, "Content-Type: {content_type}\r\n")?;
    write!(output, "Content-Length: {}\r\n", payload.len())?;
    write!(
        output,
        "Alex-Fidelity-Loss: {}\r\n\r\n",
        losses.join(" | ").replace(['\r', '\n'], " ")
    )?;
    output.extend_from_slice(payload);
    output.extend_from_slice(b"\r\n\r\n");
    Ok(())
}

fn build_otel_jsonl(traces: &[ExportTrace], losses: &[&str]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for trace in traces {
        let alex_trace_id = trace.row["id"].as_str().unwrap_or("unknown");
        let trace_id = semantic_trace_id(alex_trace_id);
        let span_id = semantic_span_id(alex_trace_id, b"otel");
        let attributes = serde_json::json!({
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
            "alex.capture.request_body": encoded_artifact(trace.request.as_deref()),
            "alex.capture.upstream_request_body": encoded_artifact(trace.upstream_request.as_deref()),
            "alex.capture.response_body": encoded_artifact(trace.response.as_deref()),
            "alex.capture.request_headers": trace.request_headers,
            "alex.capture.response_headers": trace.response_headers,
            "alex.capture.fidelity": "legacy_order_and_casing_unknown",
        });
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({
                "resource": {"service.name": "alex"},
                "scope": {"name": "alex.lar.export", "version": env!("CARGO_PKG_VERSION")},
                "span": {
                    "name": "gen_ai.request",
                    "trace_id": trace_id,
                    "span_id": span_id,
                    "start_time_unix_nano": trace.row["ts_request_ms"].as_i64().unwrap_or_default().saturating_mul(1_000_000),
                    "end_time_unix_nano": trace.row["ts_response_ms"].as_i64().unwrap_or_default().saturating_mul(1_000_000),
                    "attributes": attributes,
                    "status": if trace.row["error"].is_null() { "OK" } else { "ERROR" },
                },
                "alex_loss_report": losses,
            }),
        )?;
        output.push(b'\n');
    }
    Ok(output)
}

/// OpenInference is an OpenTelemetry semantic-convention layer, not an alias
/// for the evolving OTel GenAI conventions. Keep it as a distinct export so
/// consumers can select the attribute vocabulary they actually implement.
fn build_openinference_jsonl(traces: &[ExportTrace], losses: &[&str]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for trace in traces {
        let alex_trace_id = trace.row["id"].as_str().unwrap_or("unknown");
        let trace_id = semantic_trace_id(alex_trace_id);
        let span_id = semantic_span_id(alex_trace_id, b"openinference");
        let prompt_tokens = trace.row["input_tokens"].as_i64();
        let completion_tokens = trace.row["output_tokens"].as_i64();
        let total_tokens = prompt_tokens
            .zip(completion_tokens)
            .map(|(input, output)| input.saturating_add(output));
        let (input_mime, input_value) = openinference_value(trace.request.as_deref())?;
        let (output_mime, output_value) = openinference_value(trace.response.as_deref())?;
        let attributes = serde_json::json!({
            "openinference.span.kind": "LLM",
            "llm.system": openinference_system(&trace.row["upstream_provider"]),
            "llm.provider": openinference_provider(&trace.row["upstream_provider"]),
            "llm.model_name": trace.row["routed_model"],
            "llm.token_count.prompt": prompt_tokens,
            "llm.token_count.prompt_details.cache_read": trace.row["cached_input_tokens"],
            "llm.token_count.prompt_details.cache_write": trace.row["cache_creation_tokens"],
            "llm.token_count.completion": completion_tokens,
            "llm.token_count.completion_details.reasoning": trace.row["reasoning_tokens"],
            "llm.token_count.total": total_tokens,
            "llm.cost.total": trace.row["cost_usd"],
            "input.mime_type": input_mime,
            "input.value": input_value,
            "output.mime_type": output_mime,
            "output.value": output_value,
            "metadata": serde_json::to_string(&sanitized_trace_metadata(&trace.row))?,
            "alex.trace.id": alex_trace_id,
            "alex.capture.request_body": encoded_artifact(trace.request.as_deref()),
            "alex.capture.upstream_request_body": encoded_artifact(trace.upstream_request.as_deref()),
            "alex.capture.response_body": encoded_artifact(trace.response.as_deref()),
            "alex.capture.request_headers": trace.request_headers,
            "alex.capture.response_headers": trace.response_headers,
            "alex.capture.fidelity": "legacy_order_and_casing_unknown",
        });
        serde_json::to_writer(
            &mut output,
            &serde_json::json!({
                "resource": {"service.name": "alex"},
                "scope": {
                    "name": "alex.lar.openinference.export",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "span": {
                    "name": "LLM",
                    "trace_id": trace_id,
                    "span_id": span_id,
                    "start_time_unix_nano": trace.row["ts_request_ms"].as_i64().unwrap_or_default().saturating_mul(1_000_000),
                    "end_time_unix_nano": trace.row["ts_response_ms"].as_i64().unwrap_or_default().saturating_mul(1_000_000),
                    "attributes": attributes,
                    "status": if trace.row["error"].is_null() { "OK" } else { "ERROR" },
                },
                "alex_loss_report": losses,
            }),
        )?;
        output.push(b'\n');
    }
    Ok(output)
}

fn openinference_value(body: Option<&[u8]>) -> Result<(Value, Value)> {
    let Some(body) = body else {
        return Ok((Value::Null, Value::Null));
    };
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        return Ok((
            Value::String("application/json".into()),
            Value::String(serde_json::to_string(&value)?),
        ));
    }
    if let Ok(text) = std::str::from_utf8(body) {
        return Ok((
            Value::String("text/plain; charset=utf-8".into()),
            Value::String(text.into()),
        ));
    }
    Ok((
        Value::String("application/octet-stream".into()),
        Value::String(base64::engine::general_purpose::STANDARD.encode(body)),
    ))
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

fn write_export_file(output: &Path, bytes: &[u8], force: bool) -> Result<()> {
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
    let temporary = parent.join(format!(".{name}.{}.lar-export.tmp", std::process::id()));
    if temporary.exists() {
        bail!(
            "temporary export output already exists: {}",
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
        publish_export_temp(&temporary, output, force)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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
            let stage_id = writer.append_stage(Stage::new(stage)).unwrap();
            let mut exchange = ExchangeData::new(
                format!("trace-{index}"),
                index as u64,
                1_000 + index as u64,
                vec![stage_id],
            );
            exchange.session_id = Some(format!("session-{index}").into_bytes());
            writer.append_exchange(Exchange::new(exchange)).unwrap();
        }
        writer.seal().unwrap();
        let file = writer.into_inner().unwrap();
        file.sync_all().unwrap();
    }

    fn write_replay_archive(path: &Path) -> (String, String) {
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
                        byte_length: 11,
                        delta_from_first_byte_ns: 0,
                        parser: StreamParser::Sse,
                        frame_kind: StreamFrameKind::SseEvent,
                    },
                    ParsedFrame {
                        byte_offset: 11,
                        byte_length: 11,
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
        )
    }

    fn grep_args(literal: &str, archives: Vec<PathBuf>, limit: usize) -> GrepArgs {
        GrepArgs {
            literal: literal.into(),
            archives,
            limit,
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
    }

    #[test]
    fn top_level_help_lists_the_complete_lar_surface() {
        let help = TestCli::command().render_long_help().to_string();
        for command in [
            "import",
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
        let (stage_id, expected) = write_replay_archive(&archive);
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
            assert_eq!(fs::read_to_string(output).unwrap(), expected);
        }
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
            method: Some("POST".into()),
            path: Some("/v1/chat/completions".into()),
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
            client_response_headers: Some(LarHeaderCapture::observed([(
                "content-type",
                "application/json",
            )])),
            upstream_attempts: vec![
                LarUpstreamAttemptCapture {
                    attempt_number: 1,
                    wall_time_ns: trace.ts_request_ms as u64 * 1_000_000 + 10,
                    request_headers: Some(LarHeaderCapture::observed([("x-attempt", "one")])),
                    response_headers: Some(LarHeaderCapture::observed([("retry-after", "1")])),
                    status_code: Some(429),
                    error_class: Some("rate_limit".into()),
                    error_message: Some("retry".into()),
                },
                LarUpstreamAttemptCapture {
                    attempt_number: 2,
                    wall_time_ns: trace.ts_request_ms as u64 * 1_000_000 + 20,
                    request_headers: Some(LarHeaderCapture::observed([("x-attempt", "two")])),
                    response_headers: Some(LarHeaderCapture::observed([(
                        "content-type",
                        "text/event-stream",
                    )])),
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
        assert_eq!(row["method"], "POST");
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
