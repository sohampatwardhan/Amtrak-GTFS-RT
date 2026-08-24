//! Best-effort scraper for Amtrak's Service Alerts & Notices page.
//!
//! Turns the page's **Station Advisories** (facility notices per station) and **Passenger
//! Advisories** (service changes per route) into scoped GTFS-RT alert entities:
//!
//! - a station advisory becomes an alert whose `informed_entity.stop_id` is the advisory's station
//!   code resolved to its GTFS stop (the code *is* the Amtrak stop id);
//! - a passenger advisory becomes an alert whose `informed_entity.route_id`s are the advisory's
//!   route names resolved via GTFS `route_long_name`, one selector per affected route.
//!
//! Everything here is **fail-open**: a fetch failure, an unexpected DOM, an unmappable station or
//! route, or an unparseable date degrades to *fewer or zero advisories plus a diagnostic* — never a
//! panic and never an error. Callers merge the returned entities into the alerts feed; the
//! generation publishes regardless.
//!
//! `dead_code` is allowed while the tree is built incrementally: the public functions here are
//! consumed by the `WithAdvisories` decorator (task 3.1), not yet referenced from the service.
#![allow(dead_code)]

use super::{RtBatch, RtSource, SourceError};
use crate::config::AdvisoryConfig;
use async_trait::async_trait;
use gtfs_realtime::{
    translated_string::Translation, Alert, EntitySelector, FeedEntity, TimeRange, TranslatedString,
};
use gtfs_structures::Gtfs;
use scraper::{Html, Selector};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::Mutex;

/// Cached advisory alerts plus the instant they were fetched, for TTL reuse.
struct Cached {
    fetched_at: Instant,
    alerts: Vec<FeedEntity>,
}

/// An [`RtSource`] decorator that appends best-effort scoped advisory alerts to the inner source's
/// batch. It is strictly **additive and fail-open**: the inner batch (and its errors) pass through
/// unchanged, and advisory scraping never fails the `fetch`. A TTL cache bounds page fetches and
/// serves the last-good advisories through a transient scrape failure.
pub struct WithAdvisories<S> {
    inner: S,
    client: reqwest::Client,
    config: AdvisoryConfig,
    cache: Mutex<Option<Cached>>,
}

impl<S> WithAdvisories<S> {
    /// Wraps `inner`, scraping advisories from `config.url` no more than once per `config.ttl`.
    pub fn new(inner: S, config: AdvisoryConfig) -> Self {
        Self {
            inner,
            client: reqwest::Client::new(),
            config,
            cache: Mutex::new(None),
        }
    }

    /// Returns the current advisory alerts, using the cache within the TTL and re-scraping
    /// otherwise. On a scrape that yields nothing (failure or genuinely empty), it serves the
    /// last-good cached set; it never errors.
    async fn current_advisories(&self, gtfs: &Gtfs) -> Vec<FeedEntity> {
        let mut guard = self.cache.lock().await;
        if let Some(cached) = guard.as_ref() {
            if cached.fetched_at.elapsed() < self.config.ttl {
                return cached.alerts.clone();
            }
        }
        let fresh = fetch_advisory_alerts(&self.client, gtfs, &self.config.url).await;
        let alerts = if fresh.is_empty() {
            guard.as_ref().map(|c| c.alerts.clone()).unwrap_or_default()
        } else {
            fresh
        };
        *guard = Some(Cached {
            fetched_at: Instant::now(),
            alerts: alerts.clone(),
        });
        alerts
    }
}

#[async_trait]
impl<S: RtSource> RtSource for WithAdvisories<S> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn fetch(&self, gtfs: &Gtfs) -> Result<RtBatch, SourceError> {
        let mut batch = self.inner.fetch(gtfs).await?;
        let mut advisories = self.current_advisories(gtfs).await;
        batch.alerts.entity.append(&mut advisories);
        Ok(batch)
    }
}

/// Lookup tables from the static GTFS, built once per scrape.
pub struct AdvisoryIndex {
    /// Uppercased Amtrak station code (and GTFS stop id) → GTFS `stop_id`.
    stop_by_code: HashMap<String, String>,
    /// GTFS `route_long_name` → GTFS `route_id`.
    route_by_name: HashMap<String, String>,
}

impl AdvisoryIndex {
    /// Builds the station-code and route-name lookups from the static schedule.
    pub fn build(gtfs: &Gtfs) -> Self {
        let mut stop_by_code = HashMap::new();
        for stop in gtfs.stops.values() {
            stop_by_code
                .entry(stop.id.to_uppercase())
                .or_insert_with(|| stop.id.clone());
            if let Some(code) = stop.code.as_ref().filter(|c| !c.is_empty()) {
                stop_by_code
                    .entry(code.to_uppercase())
                    .or_insert_with(|| stop.id.clone());
            }
        }
        let mut route_by_name = HashMap::new();
        for route in gtfs.routes.values() {
            if let Some(name) = route.long_name.as_ref().filter(|n| !n.trim().is_empty()) {
                route_by_name
                    .entry(name.trim().to_string())
                    .or_insert_with(|| route.id.clone());
            }
        }
        Self {
            stop_by_code,
            route_by_name,
        }
    }
}

/// Fetches the advisories page and parses both advisory classes into scoped alert entities.
///
/// Fail-open: any fetch/parse failure logs a diagnostic and returns an empty `Vec` (never errors).
pub async fn fetch_advisory_alerts(
    client: &reqwest::Client,
    gtfs: &Gtfs,
    url: &str,
) -> Vec<FeedEntity> {
    let Some(html) = fetch_html(client, url).await else {
        tracing::warn!("advisories page fetch failed; emitting no advisory alerts");
        return Vec::new();
    };
    let index = AdvisoryIndex::build(gtfs);
    let mut out = parse_station_advisories(&html, &index);
    out.extend(parse_passenger_advisories(&html, &index));
    out
}

/// GET the page body, or `None` on any transport/status failure (credential-free).
async fn fetch_html(client: &reqwest::Client, url: &str) -> Option<String> {
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().await.ok()
}

/// Parses the page's Station Advisories into stop-scoped alert entities.
///
/// A station code that resolves to no GTFS stop is skipped with a diagnostic (never misfiled).
pub fn parse_station_advisories(html: &str, index: &AdvisoryIndex) -> Vec<FeedEntity> {
    let document = Html::parse_document(html);
    let row = selector("li.na-service-alert__stations_ul_li");
    let header = selector(".na-service-alert__stations_ul_li_header");
    let link = selector(".na-service-alert__stations_ul_li_details_alert_link");
    let date = selector(".na-service-alert__stations_ul_li_details_alert_date");

    let mut out = Vec::new();
    for li in document.select(&row) {
        let header_text = text_of(li.select(&header).next());
        let Some(code) = extract_station_code(&header_text) else {
            continue;
        };
        let link_el = li.select(&link).next();
        let title = text_of(link_el).trim().to_string();
        let url = link_el.and_then(|e| e.value().attr("data-href")).map(str::to_string);
        let effective = text_of(li.select(&date).next()).trim().to_string();

        match index.stop_by_code.get(&code.to_uppercase()) {
            Some(stop_id) => out.push(build_alert(
                &format!("advisory-station-{code}"),
                &title,
                &effective,
                url,
                vec![EntitySelector {
                    stop_id: Some(stop_id.clone()),
                    ..Default::default()
                }],
            )),
            None => tracing::warn!(code = %code, "station advisory code not in GTFS; skipped"),
        }
    }
    out
}

/// Parses the page's Passenger Advisories into route-scoped alert entities.
///
/// Multi-route advisories emit one selector per resolved route; a route name that resolves to no
/// GTFS route is dropped with a diagnostic, and an advisory with no resolved route emits no alert.
pub fn parse_passenger_advisories(html: &str, index: &AdvisoryIndex) -> Vec<FeedEntity> {
    let document = Html::parse_document(html);
    let option = selector("div.na-service-alert__option");
    let heading = selector("h3");
    let tooltip = selector(".tooltip__text_content");
    let link = selector(".na-service-alert__option_title");
    let date = selector(".na-service-alert__option_date");

    let mut out = Vec::new();
    for opt in document.select(&option) {
        // Route names: the tooltip entries (multi-route) if present, else the single <h3> title.
        let mut names: Vec<String> = opt
            .select(&tooltip)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            let h3 = text_of(opt.select(&heading).next()).trim().to_string();
            if !h3.is_empty() && h3 != "Multiple Routes" {
                names.push(h3);
            }
        }

        let link_el = opt.select(&link).next();
        let title = text_of(link_el).trim().to_string();
        let url = link_el.and_then(|e| e.value().attr("data-href")).map(str::to_string);
        let effective = text_of(opt.select(&date).next()).trim().to_string();

        let mut selectors = Vec::new();
        for name in &names {
            match index.route_by_name.get(name) {
                Some(route_id) => selectors.push(EntitySelector {
                    route_id: Some(route_id.clone()),
                    ..Default::default()
                }),
                None => tracing::warn!(route = %name, "passenger advisory route not in GTFS; dropped"),
            }
        }
        if selectors.is_empty() {
            continue;
        }
        out.push(build_alert(
            &format!("advisory-passenger-{}", names.join(",")),
            &title,
            &effective,
            url,
            selectors,
        ));
    }
    out
}

/// Builds one advisory alert entity: header = title, description = title + effective text, an
/// `active_period` only when the effective text parses to a definite range, and the given scope.
fn build_alert(
    id: &str,
    title: &str,
    effective: &str,
    url: Option<String>,
    informed_entity: Vec<EntitySelector>,
) -> FeedEntity {
    let description = if effective.is_empty() {
        title.to_string()
    } else {
        format!("{title} ({effective})")
    };
    let active_period = match parse_effective_period(effective) {
        Some((start, end)) => vec![TimeRange {
            start: Some(start as u64),
            end: Some(end as u64),
        }],
        None => Vec::new(),
    };
    FeedEntity {
        id: id.to_string(),
        is_deleted: Some(false),
        alert: Some(Alert {
            active_period,
            informed_entity,
            header_text: Some(translated(title)),
            description_text: Some(translated(&description)),
            url: url.map(|u| translated(&u)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Parses effective-date text into a definite `(start, end)` unix range, or `None` to omit the
/// active period. Recognizes exactly two fully-resolved calendar dates (with a year, propagated
/// from a later endpoint to an earlier year-less one); anything else — a single date, more than two
/// dates, or no year — returns `None` so the caller keeps the text and omits the period.
pub fn parse_effective_period(text: &str) -> Option<(i64, i64)> {
    let dates = scan_full_dates(text);
    if dates.len() == 2 && dates[0] <= dates[1] {
        Some((to_unix(dates[0]), to_unix(dates[1])))
    } else {
        None
    }
}

/// Scans "Month Day[, Year]" occurrences in order and returns fully-resolved `(year, month, day)`
/// tuples, propagating the last seen year to earlier year-less entries. Empty if no year appears.
fn scan_full_dates(text: &str) -> Vec<(i64, u32, u32)> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    let mut partial: Vec<(u32, u32, Option<i64>)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Some(month) = month_num(tokens[i]) {
            if let Some(day) = tokens
                .get(i + 1)
                .and_then(|t| t.parse::<u32>().ok())
                .filter(|d| (1..=31).contains(d))
            {
                let year = tokens
                    .get(i + 2)
                    .and_then(|t| t.parse::<i64>().ok())
                    .filter(|y| (2000..=2100).contains(y));
                partial.push((month, day, year));
                i += if year.is_some() { 3 } else { 2 };
                continue;
            }
        }
        i += 1;
    }
    let Some(fallback_year) = partial.iter().rev().find_map(|(_, _, y)| *y) else {
        return Vec::new();
    };
    partial
        .into_iter()
        .map(|(m, d, y)| (y.unwrap_or(fallback_year), m, d))
        .collect()
}

/// English month name (any case) → 1..=12.
fn month_num(token: &str) -> Option<u32> {
    let months = [
        "january", "february", "march", "april", "may", "june", "july", "august", "september",
        "october", "november", "december",
    ];
    let lower = token.to_ascii_lowercase();
    months.iter().position(|m| *m == lower).map(|i| i as u32 + 1)
}

/// Unix seconds for midnight UTC of a `(year, month, day)` date (Howard Hinnant's civil algorithm).
fn to_unix(date: (i64, u32, u32)) -> i64 {
    let (mut y, m, d) = (date.0, date.1 as i64, date.2 as i64);
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) * 86400
}

/// Parses a CSS selector known at author time to be valid.
fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static advisory selector is valid CSS")
}

/// Collected text of an optional element.
fn text_of(element: Option<scraper::ElementRef>) -> String {
    element.map(|e| e.text().collect::<String>()).unwrap_or_default()
}

/// Extracts a parenthesized station code, e.g. `"Alexandria, VA (ALX)"` → `"ALX"`.
fn extract_station_code(header: &str) -> Option<String> {
    let open = header.rfind('(')?;
    let close = header[open..].find(')')? + open;
    let code = header[open + 1..close].trim();
    (!code.is_empty()).then(|| code.to_string())
}

/// A single-language ("en") translated string.
fn translated(text: &str) -> TranslatedString {
    TranslatedString {
        translation: vec![Translation {
            text: text.to_string(),
            language: Some("en".to_string()),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AdvisoryConfig;
    use crate::sources::mock::{Behavior, MockSource};
    use crate::sources::{RtBatch, RtSource};
    use gtfs_realtime::FeedMessage;
    use std::io::{Cursor, Write};
    use std::time::Duration;
    use zip::write::SimpleFileOptions;

    impl<S> WithAdvisories<S> {
        fn with_primed_cache(inner: S, config: AdvisoryConfig, alerts: Vec<FeedEntity>) -> Self {
            Self {
                inner,
                client: reqwest::Client::new(),
                config,
                cache: Mutex::new(Some(Cached {
                    fetched_at: Instant::now(),
                    alerts,
                })),
            }
        }
    }

    fn cfg(url: &str) -> AdvisoryConfig {
        AdvisoryConfig {
            url: url.to_string(),
            ttl: Duration::from_secs(3600),
            enabled: true,
        }
    }
    fn entity(id: &str) -> FeedEntity {
        FeedEntity {
            id: id.to_string(),
            alert: Some(Alert::default()),
            ..Default::default()
        }
    }
    fn asm_batch() -> RtBatch {
        RtBatch {
            trip_updates: FeedMessage::default(),
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage {
                entity: vec![entity("asm")],
                ..Default::default()
            },
        }
    }

    // R4.1/R4.2: advisory alerts are appended and the inner ASM alerts are preserved.
    #[tokio::test]
    async fn advisories_merge_and_preserve_asm() {
        let inner = MockSource {
            name: "amtrak",
            behavior: Behavior::Ok(asm_batch()),
        };
        let deco = WithAdvisories::with_primed_cache(inner, cfg("http://unused"), vec![entity("adv")]);
        let batch = deco.fetch(&gtfs()).await.unwrap();
        assert_eq!(batch.alerts.entity.len(), 2);
        assert!(batch.alerts.entity.iter().any(|e| e.id == "asm")); // R4.2 preserved
        assert!(batch.alerts.entity.iter().any(|e| e.id == "adv")); // R4.1 appended
    }

    // R5.1/R6.1: a failed advisory fetch leaves the inner batch unchanged (zero advisories, no error).
    #[tokio::test]
    async fn fetch_failure_leaves_inner_batch_unchanged() {
        let inner = MockSource {
            name: "amtrak",
            behavior: Behavior::Ok(asm_batch()),
        };
        let deco = WithAdvisories::new(inner, cfg("http://127.0.0.1:1/nope"));
        let batch = deco.fetch(&gtfs()).await.unwrap();
        assert_eq!(batch.alerts.entity.len(), 1);
        assert_eq!(batch.alerts.entity[0].id, "asm");
    }

    fn gtfs() -> Gtfs {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\nALX,Alexandria,38.8,-77.05\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\n60,a,,Amtrak Cascades,2\n41042,a,,Amtrak Hartford Line,2\n41044,a,,Valley Flyer,2\n"),
            ("trips.txt", "route_id,service_id,trip_id,trip_short_name\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        Gtfs::from_reader(Cursor::new(archive.finish().unwrap().into_inner())).unwrap()
    }

    const STATION_HTML: &str = r#"
    <ul class="na-service-alert__stations_ul">
      <li class="na-service-alert__stations_ul_li">
        <span class="na-service-alert__stations_ul_li_header"> Alexandria, VA (ALX) </span>
        <div class="na-service-alert__stations_ul_li_details">
          <div class="na-service-alert__stations_ul_li_details_alert">
            <a class="na-service-alert__stations_ul_li_details_alert_link" data-href="/alert/alx.html">Alexandria Station Will No Longer Accept Checked Baggage</a>
            <span class="na-service-alert__stations_ul_li_details_alert_date">Effective April 20, 2026</span>
          </div>
        </div>
      </li>
      <li class="na-service-alert__stations_ul_li">
        <span class="na-service-alert__stations_ul_li_header"> Nowhere, ZZ (ZZZ) </span>
        <div class="na-service-alert__stations_ul_li_details_alert">
          <a class="na-service-alert__stations_ul_li_details_alert_link" data-href="/x">Ghost station notice</a>
          <span class="na-service-alert__stations_ul_li_details_alert_date">Effective May 1, 2026</span>
        </div>
      </li>
    </ul>"#;

    const PASSENGER_HTML: &str = r#"
    <div class="na-service-alert__option">
      <h3>Amtrak Cascades</h3>
      <div class="na-service-alert__option_block">
        <a class="na-service-alert__option_title" data-href="/alert/cascades.html">Amtrak Cascades Replaced by Bus</a>
        <span class="na-service-alert__option_date"> August 17, 2026 6:15 AM </span>
      </div>
    </div>
    <div class="na-service-alert__option">
      <div class="na-service-alert__option-wrapper">
        <h3>Multiple Routes</h3>
        <div class="tooltip"><div class="tooltip__text">
          <p class="tooltip__text_content">Amtrak Hartford Line</p>
          <p class="tooltip__text_content">Valley Flyer</p>
        </div></div>
      </div>
      <div class="na-service-alert__option_block">
        <a class="na-service-alert__option_title" data-href="/alert/hl.html">Hartford Line and Valley Flyer Schedule Changes</a>
        <span class="na-service-alert__option_date"> Effective Monday - Friday, April 21 - October 30, 2026 </span>
      </div>
    </div>
    <div class="na-service-alert__option">
      <h3>Ghost Route</h3>
      <div class="na-service-alert__option_block">
        <a class="na-service-alert__option_title" data-href="/g">Ghost route advisory</a>
        <span class="na-service-alert__option_date"> Effective July 1, 2026 </span>
      </div>
    </div>"#;

    fn stop_id(entity: &FeedEntity) -> Option<&str> {
        entity.alert.as_ref()?.informed_entity.first()?.stop_id.as_deref()
    }
    fn route_ids(entity: &FeedEntity) -> Vec<String> {
        entity
            .alert
            .as_ref()
            .map(|a| a.informed_entity.iter().filter_map(|s| s.route_id.clone()).collect())
            .unwrap_or_default()
    }

    // R1.1/R1.2: station advisory -> stop-scoped alert; unmapped code skipped.
    #[test]
    fn station_advisories_are_stop_scoped() {
        let index = AdvisoryIndex::build(&gtfs());
        let alerts = parse_station_advisories(STATION_HTML, &index);
        assert_eq!(alerts.len(), 1); // ZZZ dropped
        assert_eq!(stop_id(&alerts[0]), Some("ALX"));
        let alert = alerts[0].alert.as_ref().unwrap();
        // R3.1 content; R3.3 single date -> no active_period.
        let header = &alert.header_text.as_ref().unwrap().translation[0].text;
        assert!(header.contains("Checked Baggage"));
        assert!(alert.description_text.as_ref().unwrap().translation[0].text.contains("April 20, 2026"));
        assert!(alert.active_period.is_empty());
        assert_eq!(alert.url.as_ref().unwrap().translation[0].text, "/alert/alx.html");
    }

    // R2.1/R2.2/R2.3/R3.2: passenger advisories -> route-scoped; multi-route fan-out; unmapped
    // dropped; a definite range -> active_period.
    #[test]
    fn passenger_advisories_are_route_scoped() {
        let index = AdvisoryIndex::build(&gtfs());
        let alerts = parse_passenger_advisories(PASSENGER_HTML, &index);
        assert_eq!(alerts.len(), 2); // Ghost Route dropped (no resolvable route)

        let cascades = &alerts[0];
        assert_eq!(route_ids(cascades), vec!["60".to_string()]);

        let multi = &alerts[1];
        let mut ids = route_ids(multi);
        ids.sort();
        assert_eq!(ids, vec!["41042".to_string(), "41044".to_string()]); // R2.2
        // R3.2: "April 21 - October 30, 2026" parses to a definite range.
        assert_eq!(multi.alert.as_ref().unwrap().active_period.len(), 1);
    }

    // R5.2: unparseable / unexpected HTML yields zero advisories, never a panic.
    #[test]
    fn broken_html_yields_zero_advisories() {
        let index = AdvisoryIndex::build(&gtfs());
        assert!(parse_station_advisories("<html><body>nope", &index).is_empty());
        assert!(parse_passenger_advisories("<div>garbage</div>", &index).is_empty());
        assert!(parse_station_advisories("", &index).is_empty());
    }

    // R3.2/R3.3: effective-date parsing — definite range vs. everything else.
    #[test]
    fn effective_period_parsing() {
        // Range with a propagated year.
        assert!(parse_effective_period("Effective April 21 - October 30, 2026").is_some());
        // Full two dates.
        assert!(parse_effective_period("April 20, 2026 - May 1, 2026").is_some());
        // Single date -> None.
        assert!(parse_effective_period("Effective April 20, 2026").is_none());
        // No year anywhere -> None.
        assert!(parse_effective_period("Effective April 21 - October 30").is_none());
        // More than two dates -> None.
        assert!(parse_effective_period("August 3 - 6 and August 24 - 27, 2026").is_none());
        // Sanity: 1970-01-01 is unix 0.
        assert_eq!(to_unix((1970, 1, 1)), 0);
        assert_eq!(to_unix((2026, 4, 20)), 1_776_643_200);
    }
}
