//! Ignored-by-default live verification of the advisory scraper against Amtrak's real pages.
//!
//! Hits the network (live static GTFS + the live Service Alerts & Notices page), so it is
//! `#[ignore]`d and excluded from CI. Run explicitly:
//!
//! ```sh
//! cargo test advisories_live -- --ignored --nocapture
//! ```
//!
//! Asserts that the scraper produces well-formed scoped alerts from the live page: station
//! advisories carry a `stop_id`, passenger advisories carry `route_id`(s), and at least one advisory
//! of some kind resolves. A failure here signals Amtrak changed the page markup (the production path
//! degrades to zero advisories via fail-open, so this is a drift alarm, not a service outage).

use super::advisories::fetch_advisory_alerts;

#[tokio::test]
#[ignore = "live: hits Amtrak GTFS + Service Alerts page; run with --ignored"]
async fn advisories_live_resolve_scoped() {
    let gtfs = gtfs_structures::Gtfs::from_url_async(
        "https://content.amtrak.com/content/gtfs/GTFS.zip",
    )
    .await
    .expect("live static GTFS");
    let client = reqwest::Client::new();
    let alerts = fetch_advisory_alerts(
        &client,
        &gtfs,
        "https://www.amtrak.com/service-alerts-and-notices",
    )
    .await;

    let mut stop_scoped = 0;
    let mut route_scoped = 0;
    for entity in &alerts {
        let alert = entity.alert.as_ref().expect("advisory entity carries an alert");
        assert!(!alert.informed_entity.is_empty(), "advisory alert must be scoped");
        for selector in &alert.informed_entity {
            if selector.stop_id.is_some() {
                stop_scoped += 1;
            }
            if selector.route_id.is_some() {
                route_scoped += 1;
            }
        }
    }
    eprintln!(
        "live advisories: {} alerts ({} stop-scoped selectors, {} route-scoped selectors)",
        alerts.len(),
        stop_scoped,
        route_scoped
    );
    assert!(!alerts.is_empty(), "expected at least one live advisory");
}
