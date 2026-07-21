use alex_lar_scale::{
    generate_corpus, generate_fable_sol_fixture, run_scale, verify_scale, write_json, ScaleProfile,
};
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "alex-lar-scale",
    about = "Generate and verify public synthetic LAR scale corpora"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate only the deterministic legacy SQLite/gzip corpus.
    Generate {
        #[arg(long, value_enum, default_value_t = ScaleProfile::Full)]
        profile: ScaleProfile,
        #[arg(long)]
        root: PathBuf,
    },
    /// Resume-migrate and benchmark an already generated corpus.
    Verify {
        #[arg(long, value_enum, default_value_t = ScaleProfile::Full)]
        profile: ScaleProfile,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Write results without returning failure when a budget is exceeded.
        #[arg(long)]
        no_enforce: bool,
    },
    /// Generate, resume-migrate, fully verify, benchmark, and enforce budgets.
    Run {
        #[arg(long, value_enum, default_value_t = ScaleProfile::Full)]
        profile: ScaleProfile,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Write results without returning failure when a budget is exceeded.
        #[arg(long)]
        no_enforce: bool,
    },
    /// Build and verify a sanitized replayable Fable→Sol LAR fixture.
    FixtureFableSol {
        #[arg(long)]
        vector: PathBuf,
        #[arg(long)]
        failure: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        report: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Generate { profile, root } => {
            let (manifest, elapsed) = generate_corpus(&root, profile)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "manifest": manifest,
                    "generation_ms": elapsed.as_millis(),
                    "root": root,
                }))?
            );
        }
        Command::Verify {
            profile,
            root,
            output,
            no_enforce,
        } => {
            let report = verify_scale(&root, profile, None, &output, !no_enforce)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::Run {
            profile,
            root,
            output,
            no_enforce,
        } => {
            let report = run_scale(&root, profile, &output, !no_enforce)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        Command::FixtureFableSol {
            vector,
            failure,
            output,
            report,
        } => {
            let result = generate_fable_sol_fixture(&vector, &failure, &output)?;
            if let Some(path) = report {
                write_json(&path, &result)?;
            }
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}
