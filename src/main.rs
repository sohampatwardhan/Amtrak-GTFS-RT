mod config;
mod orchestrator;
mod serve;
mod sources;
mod static_gtfs;
mod writer;

use crate::config::Config;
use crate::sources::amtrak::AmtrakSource;
use crate::sources::RtSource;
use crate::static_gtfs::{load_static_feed, save_static_zip, StaticFeedStore};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env().map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
        e.into()
    })?;
    std::fs::create_dir_all(&config.output_dir)?;

    let feed = load_static_feed(&config.static_url).await?;
    tracing::info!(feed_version = %feed.feed_version, "loaded static feed");
    save_static_zip(&config.static_url, &config.output_dir.join("static.zip")).await?;
    let store = StaticFeedStore::new(feed);

    let sources: Arc<Vec<Box<dyn RtSource>>> = Arc::new(vec![Box::new(AmtrakSource::new())]);

    let poller = tokio::spawn(orchestrator::run_poller(
        sources.clone(),
        store.clone(),
        config.clone(),
    ));
    let refresher = tokio::spawn(static_gtfs::run_static_refresh(store.clone(), config.clone()));
    let server = tokio::spawn(serve::run_server(config.clone()));

    // These tasks are meant to run forever. If any of them resolves, something went
    // wrong; exit non-zero so a process supervisor (systemd Restart=on-failure,
    // container orchestrator, etc.) actually restarts the service.
    tokio::select! {
        _ = poller => tracing::error!("poller task exited unexpectedly"),
        _ = refresher => tracing::error!("refresh task exited unexpectedly"),
        r = server => tracing::error!(result = ?r, "server task exited unexpectedly"),
    }
    std::process::exit(1);
}
