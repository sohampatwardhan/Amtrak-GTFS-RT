//! `amtrak-status` — command-line client for Amtrak station & train status queries.
//!
//! Contract: reads one coherent immutable generation from the local feed service (default) or from
//! Amtrak directly, then answers `station <code>` and `train <number>` queries, rendering times in
//! each station's local zone and always naming the source generation. Compiled only under
//! `--features status`, so it is excluded from the default/release build of the feed service.
//!
//! Usage:
//!   amtrak-status station <code> [--limit N] [--source local|amtrak] [--base-url URL]
//!   amtrak-status train <number>            [--source local|amtrak] [--base-url URL]
//!
//! Exit codes: `0` success (including a resolved "no such station/train" answer); `1` a feed
//! fetch/decode failure; `2` a usage error; `3` the service has no current generation.

mod status;

use status::format::local_time;
use status::index::FeedIndex;
use status::source::{AmtrakDirectSource, FeedSource, LocalServiceSource, SourceError};
use status::station::{station_query, DepartureRow, StationResult, StopKind};
use status::train::{train_query, TrainResult, TrainStatus};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";
const AMTRAK_STATIC_URL: &str = "https://content.amtrak.com/content/gtfs/GTFS.zip";

#[tokio::main]
async fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()).await);
}

/// Which feed source the query reads from.
enum SourceKind {
    Local,
    Amtrak,
}

/// A parsed query.
enum Mode {
    Station { code: String, limit: usize },
    Train { number: String },
}

/// Fully parsed CLI configuration.
struct Config {
    source: SourceKind,
    base_url: String,
    mode: Mode,
}

/// Drives one CLI invocation and returns its process exit code.
async fn run(args: Vec<String>) -> i32 {
    let config = match parse_args(&args) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };

    let source: Box<dyn FeedSource> = match config.source {
        SourceKind::Local => Box::new(LocalServiceSource::new(config.base_url)),
        SourceKind::Amtrak => Box::new(AmtrakDirectSource {
            static_url: AMTRAK_STATIC_URL.to_string(),
            client: reqwest::Client::new(),
        }),
    };

    let data = match source.load().await {
        Ok(data) => data,
        Err(error) => {
            eprintln!("{}", source_error_message(&error));
            return source_error_exit(&error);
        }
    };

    let index = FeedIndex::build(&data);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();

    let output = match config.mode {
        Mode::Station { code, limit } => render_station_result(station_query(&index, &code, now), limit),
        Mode::Train { number } => render_train_result(train_query(&index, &number, now)),
    };
    print!("{output}");
    0
}

/// Maps a load failure to a distinct process exit code: `3` for an absent generation (R5.4), `1`
/// for a transport/decode failure (R7.1).
fn source_error_exit(error: &SourceError) -> i32 {
    match error {
        SourceError::Unavailable => 3,
        SourceError::Fetch(_) | SourceError::Decode(_) => 1,
    }
}

/// Human-readable, credential-free message for a load failure, keeping "no generation" distinct
/// from a request failure.
fn source_error_message(error: &SourceError) -> String {
    match error {
        SourceError::Unavailable => {
            "data-unavailable: the feed service has no current generation".to_string()
        }
        other => format!("error: {other}"),
    }
}

/// Formats a Unix instant as `HH:MM UTC`, used for the source generation timestamp (R5.3).
fn utc_stamp(unix: i64) -> String {
    local_time(unix, "UTC").unwrap_or_else(|_| "??:??".to_string())
}

/// Renders a station query result, showing at most `limit` rows. Times are in each row's station
/// timezone (R6.1); an unresolved identifier renders a distinct message (R2.5).
fn render_station_result(result: StationResult, limit: usize) -> String {
    match result {
        StationResult::Unresolved { identifier } => {
            format!("unresolved: '{identifier}' did not match any Amtrak station\n")
        }
        StationResult::Board {
            generation_id,
            generated_at_unix,
            station_code,
            station_name,
            rows,
        } => {
            let mut out = String::new();
            out.push_str(&format!("Departures at {station_code} ({station_name})\n"));
            out.push_str(&format!(
                "generation {generation_id} @ {} — {} upcoming\n",
                utc_stamp(generated_at_unix as i64),
                rows.len()
            ));
            for row in rows.iter().take(limit) {
                out.push_str(&render_departure_row(row));
            }
            out
        }
    }
}

/// Renders one departure-board row and any alerts beneath it.
fn render_departure_row(row: &DepartureRow) -> String {
    let when = local_time(row.time_unix, &row.station_tz).unwrap_or_else(|_| "??:??".to_string());
    let fallback = if row.tz_is_fallback { "*" } else { "" };
    let kind = if row.canceled {
        "CANCELED"
    } else if row.kind == StopKind::Departure {
        "depart"
    } else {
        "arrive"
    };
    let mut out = format!(
        "  {when}{fallback}  {kind:<8} {:<20} {:<6} → {}\n",
        row.route_name, row.train_number, row.headsign
    );
    for alert in &row.alerts {
        out.push_str(&format!("      ! {alert}\n"));
    }
    out
}

/// Renders a train query result. A not-running number renders a distinct message (R3.5).
fn render_train_result(result: TrainResult) -> String {
    match result {
        TrainResult::NotRunning { train_number } => {
            format!("train {train_number} is not currently running\n")
        }
        TrainResult::Trains {
            generation_id,
            generated_at_unix,
            trains,
        } => {
            let mut out = format!(
                "generation {generation_id} @ {}\n",
                utc_stamp(generated_at_unix as i64)
            );
            for train in &trains {
                out.push_str(&render_train_status(train));
            }
            out
        }
    }
}

/// Renders one active train's status block.
fn render_train_status(train: &TrainStatus) -> String {
    let mut out = format!(
        "Train {} — {} → {} (from {})\n",
        train.train_number, train.route_name, train.destination, train.origin
    );
    if let Some((lat, lon)) = train.position {
        out.push_str(&format!("  position: {lat:.4}, {lon:.4}\n"));
    }
    if let Some(delay) = train.overall_delay_secs {
        out.push_str(&format!("  delay: {} min\n", delay / 60));
    }
    match &train.shape {
        Some(points) => out.push_str(&format!("  route geometry: {} points\n", points.len())),
        None => out.push_str("  route geometry: unavailable\n"),
    }
    for alert in &train.alerts {
        out.push_str(&format!("  ! {alert}\n"));
    }
    out.push_str("  remaining stops:\n");
    for stop in &train.remaining_stops {
        let when = stop
            .departure_unix
            .or(stop.arrival_unix)
            .map(|u| local_time(u, &stop.tz).unwrap_or_else(|_| "??:??".to_string()))
            .unwrap_or_else(|| "—".to_string());
        let canceled = if stop.canceled { " (canceled)" } else { "" };
        out.push_str(&format!(
            "    {when}  {} ({}){canceled}\n",
            stop.stop_name, stop.stop_code
        ));
    }
    out
}

/// Parses CLI arguments into a [`Config`], or returns a usage message.
fn parse_args(args: &[String]) -> Result<Config, String> {
    let usage = "usage: amtrak-status <station <code> [--limit N] | train <number>> \
                 [--source local|amtrak] [--base-url URL]";
    let mut source = SourceKind::Local;
    let mut base_url = DEFAULT_BASE_URL.to_string();
    let mut limit = 20_usize;
    let mut positionals = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--source" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("local") => source = SourceKind::Local,
                    Some("amtrak") => source = SourceKind::Amtrak,
                    _ => return Err(format!("--source must be local or amtrak\n{usage}")),
                }
            }
            "--base-url" => {
                i += 1;
                base_url = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| format!("--base-url requires a value\n{usage}"))?;
            }
            "--limit" => {
                i += 1;
                limit = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| format!("--limit requires a number\n{usage}"))?;
            }
            other => positionals.push(other.to_string()),
        }
        i += 1;
    }

    let mode = match positionals.as_slice() {
        [subcommand, value] if subcommand == "station" => Mode::Station {
            code: value.clone(),
            limit,
        },
        [subcommand, value] if subcommand == "train" => Mode::Train {
            number: value.clone(),
        },
        _ => return Err(usage.to_string()),
    };

    Ok(Config {
        source,
        base_url,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: StopKind, time: i64, canceled: bool, alerts: Vec<String>) -> DepartureRow {
        DepartureRow {
            time_unix: time,
            kind,
            is_realtime: true,
            canceled,
            route_name: "Acela".into(),
            train_number: "2159".into(),
            headsign: "Washington".into(),
            station_tz: "America/New_York".into(),
            tz_is_fallback: false,
            alerts,
        }
    }

    // R5.3 + R6.1 + R2.3 + R2.4 + R1.1: board names its generation, renders local time, and labels
    // a canceled row and an alert.
    #[test]
    fn station_board_renders_generation_local_time_and_labels() {
        let board = StationResult::Board {
            generation_id: "7-3".into(),
            generated_at_unix: 1_782_921_600, // 16:00 UTC
            station_code: "NYP".into(),
            station_name: "New York Penn".into(),
            rows: vec![
                row(StopKind::Departure, 1_782_921_600, false, vec!["5 min late".into()]),
                row(StopKind::Departure, 1_782_925_200, true, vec![]),
            ],
        };
        let out = render_station_result(board, 20);
        assert!(out.contains("generation 7-3 @ 16:00 UTC")); // R5.3
        assert!(out.contains("12:00 EDT")); // R6.1 local time, DST-correct
        assert!(out.contains("depart"));
        assert!(out.contains("Acela"));
        assert!(out.contains("! 5 min late")); // R1.1
        assert!(out.contains("CANCELED")); // R2.4
    }

    // R2.5: an unresolved station renders a distinct message.
    #[test]
    fn unresolved_station_message_is_distinct() {
        let out = render_station_result(
            StationResult::Unresolved {
                identifier: "ZZZ".into(),
            },
            20,
        );
        assert!(out.contains("unresolved") && out.contains("ZZZ"));
    }

    // R3.5: a not-running train renders a distinct message.
    #[test]
    fn not_running_train_message_is_distinct() {
        let out = render_train_result(TrainResult::NotRunning {
            train_number: "9999".into(),
        });
        assert!(out.contains("9999") && out.contains("not currently running"));
    }

    // R5.4 + R7.1: load failures map to distinct, non-zero exit codes.
    #[test]
    fn source_errors_map_to_distinct_exit_codes() {
        assert_eq!(source_error_exit(&SourceError::Unavailable), 3);
        assert_eq!(source_error_exit(&SourceError::Fetch("x".into())), 1);
        assert_eq!(source_error_exit(&SourceError::Decode("x".into())), 1);
        assert!(source_error_message(&SourceError::Unavailable).contains("data-unavailable"));
    }

    #[test]
    fn arg_parsing_accepts_modes_and_flags_and_rejects_garbage() {
        let ok = parse_args(&[
            "station".into(),
            "NYP".into(),
            "--source".into(),
            "amtrak".into(),
            "--limit".into(),
            "5".into(),
        ])
        .unwrap();
        assert!(matches!(ok.mode, Mode::Station { limit: 5, .. }));
        assert!(matches!(ok.source, SourceKind::Amtrak));
        assert!(parse_args(&["bogus".into()]).is_err());
    }
}
