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

/// Perform one refresh: reload the static feed, then save the zip. The in-memory
/// store is swapped only after `static.zip` is written successfully, so the served
/// zip and the `feed_version` stamped on RT feeds never disagree. Any failure
/// (download, parse, or zip write) leaves the last-good feed untouched.
pub async fn refresh_once(store: &StaticFeedStore, config: &crate::config::Config) {
    match load_static_feed(&config.static_url).await {
        Ok(feed) => {
            let dest = config.output_dir.join("static.zip");
            match save_static_zip(&config.static_url, &dest).await {
                Ok(()) => {
                    let version = feed.feed_version.clone();
                    store.set(feed).await;
                    tracing::info!(feed_version = %version, "refreshed static feed");
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to save static.zip; keeping last-good feed")
                }
            }
        }
        Err(e) => tracing::error!(error = %e, "static refresh failed; keeping last-good"),
    }
}

/// Periodically refresh the static feed. Keeps the last-good feed on failure.
pub async fn run_static_refresh(store: StaticFeedStore, config: crate::config::Config) {
    let mut ticker = tokio::time::interval(config.static_refresh_interval);
    ticker.tick().await; // the first tick fires immediately; skip it (already loaded at startup)
    loop {
        ticker.tick().await;
        refresh_once(&store, &config).await;
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

    #[tokio::test]
    async fn refresh_keeps_last_good_on_failure() {
        use crate::config::Config;
        // Store seeded with a known feed; the refresh target is a dead address
        // (localhost port 1, connection refused) so load_static_feed fails fast.
        let orig = StaticFeed { gtfs: Arc::new(Gtfs::default()), feed_version: "orig".to_string() };
        let store = StaticFeedStore::new(orig);
        let config = Config {
            static_url: "http://127.0.0.1:1/nope.zip".to_string(),
            output_dir: std::env::temp_dir(),
            poll_interval: std::time::Duration::from_secs(1),
            static_refresh_interval: std::time::Duration::from_secs(1),
            filter_capital_corridor: false,
            bind_addr: "127.0.0.1:0".parse().unwrap(),
        };
        refresh_once(&store, &config).await;
        // The failed refresh must not have replaced the last-good feed.
        assert_eq!(store.get().await.feed_version, "orig");
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
