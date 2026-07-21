use alex_lar::{
    discover_legacy_bodies, export_sanitized_fixture, import_legacy, ArchiveReader, BodyKey,
    ImportOptions,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "alex-lar", about = "Inspect and migrate Alex LAR archives")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Inspect {
        archive: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    Read {
        archive: PathBuf,
        trace_id: String,
        body_kind: String,
        #[arg(long, default_value_t = 536_870_912)]
        max_bytes: u64,
    },
    Verify {
        archive: PathBuf,
        #[arg(long, default_value_t = 536_870_912)]
        max_body_bytes: u64,
    },
    ImportBodies {
        bodies: PathBuf,
        archive: PathBuf,
        #[arg(long)]
        checkpoint: Option<PathBuf>,
        #[arg(long)]
        max_entries: Option<usize>,
        #[arg(long, default_value_t = 128)]
        checkpoint_every: usize,
    },
    ExportFixture {
        source: PathBuf,
        output: PathBuf,
        /// JSONL rows with `trace_id` and `body_kind`.
        selections: PathBuf,
        #[arg(long, default_value_t = 536_870_912)]
        max_body_bytes: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            archive,
            limit,
            offset,
        } => {
            let mut reader = ArchiveReader::open(&archive)?;
            println!("archive={} bodies={}", archive.display(), reader.len());
            for metadata in reader.list(offset, limit)? {
                println!(
                    "{}\t{}\t{}",
                    metadata.trace_id, metadata.body_kind, metadata.sha256
                );
            }
        }
        Command::Read {
            archive,
            trace_id,
            body_kind,
            max_bytes,
        } => {
            ArchiveReader::open(archive)?.copy_body_to(
                &trace_id,
                &body_kind,
                max_bytes,
                &mut io::stdout().lock(),
            )?;
        }
        Command::Verify {
            archive,
            max_body_bytes,
        } => {
            let report = ArchiveReader::open(archive)?.verify(max_body_bytes)?;
            println!("verified {} bodies", report.checked);
        }
        Command::ImportBodies {
            bodies,
            archive,
            checkpoint,
            max_entries,
            checkpoint_every,
        } => {
            let refs = discover_legacy_bodies(&bodies)?;
            let checkpoint =
                checkpoint.unwrap_or_else(|| archive.with_extension("lar.import.json"));
            let report = import_legacy(
                &refs,
                &archive,
                checkpoint,
                ImportOptions {
                    max_entries_this_run: max_entries,
                    checkpoint_every,
                    ..ImportOptions::default()
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::ExportFixture {
            source,
            output,
            selections,
            max_body_bytes,
        } => {
            let file = BufReader::new(
                File::open(&selections)
                    .with_context(|| format!("opening selection file {}", selections.display()))?,
            );
            let mut keys = Vec::new();
            for (line_number, line) in file.lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                keys.push(serde_json::from_str::<BodyKey>(&line).with_context(|| {
                    format!("invalid selection JSON on line {}", line_number + 1)
                })?);
            }
            let report = export_sanitized_fixture(source, output, &keys, max_body_bytes)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}
