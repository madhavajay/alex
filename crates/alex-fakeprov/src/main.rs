use std::io::Write;
use std::net::IpAddr;
use std::path::PathBuf;

use alex_fakeprov::{Config, FakeProv};
use anyhow::Result;
use clap::Parser;
use serde_json::json;

#[derive(Parser)]
#[command(name = "alex-fakeprov")]
struct Args {
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
    #[arg(long, default_value = "ok")]
    scenario: String,
    #[arg(long)]
    fixtures: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let server = FakeProv::spawn(Config {
        bind: args.bind,
        port: args.port,
        scenario: args.scenario,
        fixtures_dir: args.fixtures,
        ..Config::default()
    })
    .await?;
    println!(
        "{}",
        json!({
            "port": server.port(),
            "base_url": server.base_url(),
            "control_key": server.control_key(),
        })
    );
    std::io::stdout().flush()?;
    tokio::signal::ctrl_c().await?;
    Ok(())
}
