//! Ignored-by-default live verification against Amtrak's real feeds.
//!
//! These tests hit the network and depend on live service, so they are `#[ignore]`d and excluded
//! from CI. Run explicitly:
//!
//! ```sh
//! cargo test --features status -- --ignored live
//! ```
//!
//! Manual verification recorded 2026-08-17: `amtrak-status --source amtrak train 2159` reported
//! Acela 2159 (Boston → Washington) near Stamford, CT with its 2261-point route geometry, the live
//! "~25 minutes late, departed Boston" delay alert, and remaining stops in Eastern local time
//! (NYP 13:26 EDT). `amtrak-status --source amtrak station NYP` listed the same 2159 departure at
//! 13:26 EDT among 34 time-ordered upcoming trains with friendly route names and per-train alerts,
//! matching Amtrak's own status pages.

use super::index::FeedIndex;
use super::source::{AmtrakDirectSource, FeedSource};
use super::station::{station_query, StationResult};
use super::train::{train_query, TrainResult};

fn amtrak_source() -> AmtrakDirectSource {
    AmtrakDirectSource {
        static_url: "https://content.amtrak.com/content/gtfs/GTFS.zip".to_string(),
        client: reqwest::Client::new(),
    }
}

#[tokio::test]
#[ignore = "live: hits Amtrak endpoints; run with --ignored"]
async fn live_nyp_board_resolves_and_orders() {
    let data = amtrak_source().load().await.expect("live Amtrak load");
    let index = FeedIndex::build(&data);
    match station_query(&index, "NYP", 0) {
        StationResult::Board {
            station_name, rows, ..
        } => {
            assert!(
                station_name.to_uppercase().contains("PENN"),
                "NYP should be New York Penn, got {station_name}"
            );
            // Rows are returned soonest-first.
            assert!(rows.windows(2).all(|w| w[0].time_unix <= w[1].time_unix));
        }
        StationResult::Unresolved { .. } => panic!("NYP must resolve in the live static GTFS"),
    }
}

#[tokio::test]
#[ignore = "live: hits Amtrak endpoints; run with --ignored"]
async fn live_train_query_runs() {
    let data = amtrak_source().load().await.expect("live Amtrak load");
    let index = FeedIndex::build(&data);
    // A specific train may or may not be active when the test runs; either outcome is valid, we
    // only assert the query completes and returns a well-formed result.
    match train_query(&index, "2159", 0) {
        TrainResult::Trains { trains, .. } => {
            assert!(trains.iter().all(|t| t.train_number == "2159"));
        }
        TrainResult::NotRunning { train_number } => assert_eq!(train_number, "2159"),
    }
}
