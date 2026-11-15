//! `cuemix-web` entry point: config, logging, Tokio orchestration, clean shutdown.
//!
//! Phase 1 only wires configuration and the offline demo state; the MOTU
//! client, OSC bridge and Web server are started here in Phase 2.

use std::path::PathBuf;

use anyhow::{bail, Result};
use cuemix_web::config::Config;
use cuemix_web::state::MotuState;
use tracing_subscriber::EnvFilter;

fn parse_args() -> Result<Option<PathBuf>> {
    let mut args = std::env::args_os().skip(1);
    let mut config = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--config" | "-c") => match args.next() {
                Some(p) => config = Some(PathBuf::from(p)),
                None => bail!("--config requires a path"),
            },
            Some("--version" | "-V") => {
                println!("cuemix-web {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Some("--help" | "-h") => {
                println!("Usage: cuemix-web [--config <path>]\nEnv: CUEMIX_CONFIG, CUEMIX_HTTP_BIND, CUEMIX_OSC_LISTEN, CUEMIX_MOTU_IP, RUST_LOG");
                std::process::exit(0);
            }
            _ => bail!("unknown argument {arg:?} (try --help)"),
        }
    }
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let explicit = parse_args()?;
    let (config, source) = Config::load(explicit.as_deref())?;
    match &source {
        Some(p) => tracing::info!(path = %p.display(), "configuration loaded"),
        None => tracing::warn!("no config file found, using defaults"),
    }

    let state = MotuState::default();
    tracing::info!(
        http = %config.network.http_bind,
        osc = %config.network.osc_listen,
        mixes = state.mixes.len(),
        "cuemix-web {} started (Phase 1: demo state only)",
        env!("CARGO_PKG_VERSION")
    );

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown requested, exiting");
    Ok(())
}
