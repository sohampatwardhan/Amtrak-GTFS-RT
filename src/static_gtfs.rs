use gtfs_structures::Gtfs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct StaticFeed {
    pub gtfs: Arc<Gtfs>,
    pub feed_version: String,
}

/// A cheaply-clonable handle to a shared, swappable value. Clones share one
/// inner lock, so a background refresh task can replace the value while readers
/// hold their own handle.
pub struct SharedStore<T> {
    inner: Arc<RwLock<T>>,
}

impl<T> Clone for SharedStore<T> {
    fn clone(&self) -> Self {
        SharedStore { inner: self.inner.clone() }
    }
}

impl<T: Clone> SharedStore<T> {
    pub fn new(v: T) -> Self {
        SharedStore { inner: Arc::new(RwLock::new(v)) }
    }
    pub async fn get(&self) -> T {
        self.inner.read().await.clone()
    }
    pub async fn set(&self, v: T) {
        *self.inner.write().await = v;
    }
}

pub type StaticFeedStore = SharedStore<StaticFeed>;

/// Download and parse Amtrak's static GTFS into an in-memory `Gtfs`.
pub async fn load_static_feed(
    url: &str,
) -> Result<StaticFeed, Box<dyn std::error::Error + Send + Sync>> {
    let gtfs = Gtfs::from_url_async(url).await?;
    let feed_version = gtfs
        .feed_info
        .first()
        .and_then(|fi| fi.version.clone())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(StaticFeed { gtfs: Arc::new(gtfs), feed_version })
}

/// Download the raw GTFS.zip bytes and write them to `dest` so the static feed
/// can be served alongside the realtime feeds.
pub async fn save_static_zip(
    url: &str,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bytes = reqwest::get(url).await?.error_for_status()?.bytes().await?;
    crate::writer::write_atomic(dest, &bytes)?;
    Ok(())
}

/// Periodically refresh the static feed. Keeps the last-good feed on failure.
pub async fn run_static_refresh(store: StaticFeedStore, config: crate::config::Config) {
    let mut ticker = tokio::time::interval(config.static_refresh_interval);
    ticker.tick().await; // the first tick fires immediately; skip it (already loaded at startup)
    loop {
        ticker.tick().await;
        match load_static_feed(&config.static_url).await {
            Ok(feed) => {
                let version = feed.feed_version.clone();
                store.set(feed).await;
                if let Err(e) =
                    save_static_zip(&config.static_url, &config.output_dir.join("static.zip")).await
                {
                    tracing::error!(error = %e, "failed to save static.zip");
                }
                tracing::info!(feed_version = %version, "refreshed static feed");
            }
            Err(e) => tracing::error!(error = %e, "static refresh failed; keeping last-good"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_store_round_trips() {
        let store: SharedStore<String> = SharedStore::new("a".to_string());
        assert_eq!(store.get().await, "a");
        store.set("b".to_string()).await;
        assert_eq!(store.get().await, "b");
    }

    #[tokio::test]
    async fn shared_store_clone_shares_state() {
        let store: SharedStore<String> = SharedStore::new("a".to_string());
        let clone = store.clone();
        clone.set("b".to_string()).await;
        // The clone shares the same inner Arc<RwLock>, so the original sees the update.
        assert_eq!(store.get().await, "b");
    }

    // Live test: downloads Amtrak's real GTFS.zip (~19 MB).
    //   cargo test static_gtfs -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_load_static_feed() {
        let feed =
            load_static_feed("https://content.amtrak.com/content/gtfs/GTFS.zip").await.unwrap();
        assert!(!feed.gtfs.trips.is_empty(), "static feed should have trips");
    }
}
