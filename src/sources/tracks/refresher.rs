//! Background task that reads the track boards on its own schedule.
//!
//! Board requests never run inside feed generation: this task fetches each configured RailData
//! station and the Hartford board every `refresh_interval`, bounds every request by
//! `request_timeout`, and publishes successful results to the [`AssignmentStore`]. Generation only
//! reads the store, so a slow or failing board cannot delay the feed; a failure simply leaves that
//! station's earlier rows to expire.

use super::hartford::{parse_hartford_board, BOARD_MARKER};
use super::raildata::{assignments_for_station, BoardError, RailDataClient};
use super::store::{Assignment, AssignmentStore};
use crate::config::TrackConfig;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// Runs forever, refreshing boards every `config.refresh_interval`.
///
/// `token_path` is where the RailData token cache is persisted (normally
/// `{output_dir}/tracks/raildata-token.json`, outside every served route).
pub async fn run_board_refresher(
    store: Arc<AssignmentStore>,
    config: TrackConfig,
    token_path: PathBuf,
) {
    let http = board_client(&config);
    let credentials = config
        .raildata_credentials()
        .map(|(username, password)| (username.to_string(), password.to_string()));
    let mut raildata =
        RailDataClient::new(http.clone(), &config.raildata_base, credentials, token_path);
    if !raildata.is_configured() && !config.njt_stations.is_empty() {
        tracing::warn!(
            source = "njt",
            "RailData credentials are not configured; NJ Transit stations are skipped"
        );
    }
    let mut reported = HashSet::new();
    let mut ticker = tokio::time::interval(config.refresh_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        refresh_once(&store, &config, &mut raildata, &http, &mut reported).await;
    }
}

/// HTTP client used for every board request: per-request timeout, no redirects.
pub fn board_client(config: &TrackConfig) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(config.request_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("amtrak-gtfs-rt-tracks")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// One refresh pass over every configured source.
///
/// Repeated conditions that would otherwise log every minute (unconfigured credentials, a spent
/// budget, a suspension) are reported once per kind until a request succeeds again.
pub async fn refresh_once(
    store: &AssignmentStore,
    config: &TrackConfig,
    raildata: &mut RailDataClient,
    http: &reqwest::Client,
    reported: &mut HashSet<BoardError>,
) {
    if raildata.is_configured() {
        for station in &config.njt_stations {
            match raildata.schedule(station, SystemTime::now()).await {
                Ok(body) => match assignments_for_station(&body, station, &config.station_map) {
                    Ok(rows) => {
                        reported.clear();
                        store.replace_station(&format!("njt:{station}"), rows, SystemTime::now());
                    }
                    Err(_) => {
                        tracing::warn!(source = "njt", station = %station, "unusable schedule")
                    }
                },
                Err(error @ (BoardError::Http | BoardError::Unusable)) => {
                    tracing::warn!(source = "njt", station = %station, ?error, "schedule fetch failed");
                }
                Err(error) => {
                    if reported.insert(error) {
                        tracing::warn!(source = "njt", ?error, "RailData requests paused");
                    }
                    break;
                }
            }
        }
    }
    if let Some(url) = config.hartford_url.as_deref() {
        match fetch_hartford(http, url, &config.hartford_stop_id).await {
            Some(rows) => store.replace_station(
                &format!("hartford:{}", config.hartford_stop_id),
                rows,
                SystemTime::now(),
            ),
            None => tracing::warn!(source = "hartford", "board fetch failed"),
        }
    }
}

async fn fetch_hartford(
    http: &reqwest::Client,
    url: &str,
    stop_id: &str,
) -> Option<Vec<Assignment>> {
    let response = http.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let html = response.text().await.ok()?;
    if !html.contains(BOARD_MARKER) {
        return None;
    }
    Some(
        parse_hartford_board(&html)
            .into_iter()
            .map(|(train_number, track)| Assignment {
                train_number,
                stop_id: stop_id.to_string(),
                track,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use std::time::{Duration, Instant};

    async fn serve(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    }

    fn config(hartford: String) -> TrackConfig {
        let mut config = TrackConfig::from_map(|_| None).unwrap();
        config.hartford_url = Some(hartford);
        config.request_timeout = Duration::from_millis(300);
        config
    }

    #[tokio::test]
    async fn hartford_rows_reach_the_store_and_failures_keep_them() {
        let base = serve(Router::new().route(
            "/board",
            get(|| async { include_str!("../../../fixtures/tracks/hartford_board.html") }),
        ))
        .await;
        let store = AssignmentStore::new();
        let config = config(format!("{base}/board"));
        let http = board_client(&config);
        let mut raildata =
            RailDataClient::new(http.clone(), &config.raildata_base, None, "unused".into());
        let mut reported = HashSet::new();
        refresh_once(&store, &config, &mut raildata, &http, &mut reported).await;
        let fresh = store.fresh_board(SystemTime::now(), Duration::from_secs(300));
        assert_eq!(fresh.track("141", "NHV"), Some("3"));

        // A later failed fetch leaves the earlier rows, which then age out on their own.
        let broken = TrackConfig {
            hartford_url: Some(format!("{base}/missing")),
            ..config.clone()
        };
        refresh_once(&store, &broken, &mut raildata, &http, &mut reported).await;
        let fresh = store.fresh_board(SystemTime::now(), Duration::from_secs(300));
        assert_eq!(fresh.track("141", "NHV"), Some("3"));
        let expired = store.fresh_board(
            SystemTime::now() + Duration::from_secs(301),
            Duration::from_secs(300),
        );
        assert!(expired.is_empty());
    }

    #[tokio::test]
    async fn hanging_boards_are_bounded_by_the_request_timeout() {
        let base = serve(Router::new().route(
            "/board",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                "never"
            }),
        ))
        .await;
        let store = AssignmentStore::new();
        let config = config(format!("{base}/board"));
        let http = board_client(&config);
        let mut raildata =
            RailDataClient::new(http.clone(), &config.raildata_base, None, "unused".into());
        let started = Instant::now();
        refresh_once(&store, &config, &mut raildata, &http, &mut HashSet::new()).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(store
            .fresh_board(SystemTime::now(), Duration::from_secs(300))
            .is_empty());
    }
}
