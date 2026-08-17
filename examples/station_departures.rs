//! Throwaway consumer: upcoming Amtrak departures at one station.
//!
//! Proves the "departures at a station" join end to end without touching the
//! service: it pulls Amtrak's public static GTFS, fetches+decrypts the live
//! GTFS-RT trip updates via catenary's `amtrak-gtfs-rt` crate, and joins them
//! on the station code carried in each `StopTimeUpdate.stop_id`.
//!
//! Run:
//!   cargo run --example station_departures            # defaults to NHV
//!   cargo run --example station_departures -- BOS 15  # station code, max rows
//!
//! Times are printed in UTC plus a timezone-free "in N min" so the result is
//! unambiguous without pulling a timezone database into this exploratory tool.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const STATIC_URL: &str = "https://content.amtrak.com/content/gtfs/GTFS.zip";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let station_code = args.next().unwrap_or_else(|| "NHV".to_string()).to_uppercase();
    let limit: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(20);

    eprintln!("Loading Amtrak static GTFS ...");
    let gtfs = gtfs_structures::Gtfs::from_url_async(STATIC_URL).await?;

    // code (uppercased stop_id or stop_code) -> display name, from static stops.
    let mut station_names: HashMap<String, String> = HashMap::new();
    for stop in gtfs.stops.values() {
        if let Some(name) = &stop.name {
            station_names.insert(stop.id.to_uppercase(), name.clone());
            if let Some(code) = &stop.code {
                station_names.insert(code.to_uppercase(), name.clone());
            }
        }
    }
    let station_label = station_names
        .get(&station_code)
        .cloned()
        .unwrap_or_else(|| station_code.clone());

    eprintln!("Fetching live GTFS-RT trip updates ...");
    let client = reqwest::Client::new();
    let feeds = amtrak_gtfs_rt::fetch_amtrak_gtfs_rt(&gtfs, &client).await?;

    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;

    struct Departure {
        time: i64,
        scheduled: bool,
        trip_id: String,
        route_id: String,
        headsign: String,
    }
    let mut departures: Vec<Departure> = Vec::new();

    for entity in &feeds.trip_updates.entity {
        let Some(update) = &entity.trip_update else {
            continue;
        };
        for stu in &update.stop_time_update {
            let Some(stop_id) = &stu.stop_id else { continue };
            if !stop_id.eq_ignore_ascii_case(&station_code) {
                continue;
            }
            // Prefer the real departure event; fall back to arrival for a terminus.
            let (event, is_departure) = match (&stu.departure, &stu.arrival) {
                (Some(dep), _) => (dep, true),
                (None, Some(arr)) => (arr, false),
                (None, None) => continue,
            };
            let Some(time) = event.time else { continue };

            let trip_id = update.trip.trip_id.clone().unwrap_or_default();
            let rt_route = update.trip.route_id.clone().unwrap_or_default();
            let (headsign, route_id) = match gtfs.trips.get(&trip_id) {
                Some(trip) => (
                    trip.trip_headsign.clone().unwrap_or_default(),
                    trip.route_id.clone(),
                ),
                None => (String::new(), rt_route),
            };

            departures.push(Departure {
                time,
                scheduled: is_departure,
                trip_id,
                route_id,
                headsign,
            });
        }
    }

    // Upcoming only, soonest first.
    departures.retain(|d| d.time >= now - 60);
    departures.sort_by_key(|d| d.time);

    println!();
    println!("Upcoming departures at {station_code} ({station_label})");
    println!(
        "as of {} UTC — {} live trip updates touch this station",
        hhmm_utc(now),
        departures.len()
    );
    println!("{:-<72}", "");
    println!(
        "{:<8} {:>7}  {:<7} {:<10} {}",
        "time", "in", "kind", "route", "trip / headsign"
    );
    println!("{:-<72}", "");

    if departures.is_empty() {
        println!("(no live departures found for {station_code} right now)");
    }
    for d in departures.iter().take(limit) {
        let mins = (d.time - now) / 60;
        let rel = if mins <= 0 {
            "now".to_string()
        } else {
            format!("{mins}m")
        };
        let label = if d.headsign.is_empty() {
            d.trip_id.clone()
        } else {
            format!("{} → {}", d.trip_id, d.headsign)
        };
        println!(
            "{:<8} {:>7}  {:<7} {:<10} {}",
            hhmm_utc(d.time),
            rel,
            if d.scheduled { "depart" } else { "arrive" },
            d.route_id,
            label
        );
    }

    Ok(())
}

/// Formats a unix timestamp as `HH:MM` in UTC without a date/time dependency.
fn hhmm_utc(unix: i64) -> String {
    let secs_of_day = unix.rem_euclid(86_400);
    format!("{:02}:{:02}", secs_of_day / 3600, (secs_of_day % 3600) / 60)
}
