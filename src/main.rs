//! `cuemix-web` entry point: config, logging, Tokio orchestration, clean shutdown.
//!
//! Tasks: engine (state owner) · MOTU client · OSC bridge · Web server ·
//! mDNS announcement. Ctrl-C / SIGTERM triggers a graceful shutdown.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use cuemix_web::config::Config;
use cuemix_web::engine::{self, EngineConfig};
use cuemix_web::measure;
use cuemix_web::motu::client::{self, ClientConfig};
use cuemix_web::osc::settings::start_bridge;
use cuemix_web::state::Topology;
use cuemix_web::web::server::{self, WebConfig};
use cuemix_web::zeroconf;
use tracing_subscriber::EnvFilter;

/// Grace period for tasks to finish after shutdown is requested.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

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
                println!(
                    "Usage: cuemix-web [--config <path>]\n\
                     Env: CUEMIX_CONFIG, CUEMIX_HTTP_BIND, CUEMIX_OSC_LISTEN, CUEMIX_MOTU_IP, RUST_LOG"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown argument {arg:?} (try --help)"),
        }
    }
    Ok(config)
}

async fn termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler unavailable");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
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

    let topology = Topology::default();
    // OSC first: the saved settings (osc.json) decide what the engine reports.
    let osc = start_bridge(&config.storage.osc_file, &config.network).await?;
    let (engine, link, engine_task) = engine::start(EngineConfig {
        topology,
        presets_dir: config.storage.presets_dir.clone(),
        labels_file: config.storage.labels_file.clone(),
        power_file: config.storage.power_file.clone(),
        speaker_plug_file: config.storage.speaker_plug_file.clone(),
        speaker_plug: config.speaker_plug.clone(),
        osc_file: config.storage.osc_file.clone(),
        osc: osc.settings.clone(),
        osc_error: osc.error.clone(),
        osc_unresolved: osc.unresolved.clone(),
        osc_retry_every: Duration::from_secs(60),
    })
    .await;
    let (trigger, shutdown) = engine::shutdown_channel();
    let osc_listen = osc.settings.listen.clone();

    // Bind sockets first so configuration errors fail fast.
    let listener = tokio::net::TcpListener::bind(config.network.http_bind)
        .await
        .with_context(|| format!("cannot bind HTTP on {}", config.network.http_bind))?;

    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(client::run(
        ClientConfig::from_config(&config, topology),
        link,
        shutdown.clone(),
    ));
    {
        let (engine, shutdown) = (engine.clone(), shutdown.clone());
        tasks.spawn(async move {
            if let Err(e) = osc.bridge.run(engine, shutdown).await {
                tracing::error!(error = %e, "OSC bridge stopped");
            }
        });
    }
    {
        let (engine, shutdown) = (engine.clone(), shutdown.clone());
        let web_cfg = WebConfig::from_config(&config);
        tasks.spawn(async move {
            if let Err(e) = server::serve(listener, engine, web_cfg, shutdown).await {
                tracing::error!(error = %e, "web server stopped");
            }
        });
    }
    if let Some(settings) =
        measure::Settings::from_config(&config.measure, config.network.http_bind.port())
    {
        tracing::info!("room test enabled (MPD configured)");
        let settings = settings.with_restore_file(
            config
                .storage
                .labels_file
                .with_file_name("measure-restore.json"),
        );
        tasks.spawn(measure::run(settings, engine.clone(), shutdown.clone()));
    }
    let mdns = zeroconf::announce(
        &config.network.mdns_hostname,
        config.network.http_bind.port(),
    );

    tracing::info!(
        http = %config.network.http_bind,
        osc = %osc_listen,
        "cuemix-web {} running",
        env!("CARGO_PKG_VERSION")
    );

    termination().await;
    tracing::info!("shutdown requested");
    trigger.trigger();
    if let Some(a) = mdns {
        a.stop();
    }
    let drained = tokio::time::timeout(SHUTDOWN_GRACE, async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tracing::warn!("some tasks did not stop in time, aborting them");
        tasks.abort_all();
    }
    drop(engine);
    engine_task.abort();
    tracing::info!("bye");
    Ok(())
}
