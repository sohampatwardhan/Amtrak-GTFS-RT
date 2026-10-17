//! Amtrak's own train-status documents.
//!
//! The Amtrak site and app load
//! `GET /dotcom/travel-service/statuses/{train}?service-date={YYYY-MM-DD}` and read
//! `stops[].departure.track.number`, falling back to `stops[].arrival.track.number`. That host
//! resets non-browser clients, so the service does not call it itself. A sidecar browser fetches
//! the trains that are near a stop and serves the JSON here. A track is kept until the stop's
//! latest predicted time plus the configured grace, including when a later document omits it.
//!
//! The sidecar stamps each bundled document with `fetchedAt`, the time it read that document
//! from Amtrak. That, not the time the service read the bundle, is the document's age, so a
//! document the sidecar keeps re-serving after Amtrak stops answering does not look new.

use super::store::Assignment;
use super::{is_track_label, normalize_train_number};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One train-day's assignments, already keyed for the [`super::store::AssignmentStore`].
pub struct StatusGroup {
    /// `amtrak:{train}:{service-date}`.
    pub source_key: String,
    pub rows: Vec<Assignment>,
    /// The document's `fetchedAt`. `None` for a bare status document, which is as old as the read.
    pub fetched_at: Option<SystemTime>,
}

/// Parses a status document or a sidecar bundle (`{"documents":[...]}`).
///
/// # Errors
///
/// Returns a message when `body` is not a status document or a bundle of them. A document that
/// simply has no tracks is success with no groups, so a quiet hour does not look like a failure.
pub fn groups_from_status(
    body: &str,
    grace: Duration,
    now: SystemTime,
) -> Result<Vec<StatusGroup>, String> {
    let value: Value = serde_json::from_str(body.trim()).map_err(|error| error.to_string())?;
    let documents = match value.get("documents") {
        Some(Value::Array(documents)) => documents.clone(),
        Some(_) => return Err("documents is not an array".into()),
        None if value.get("data").is_some() => vec![value],
        None => return Err("status document has no data".into()),
    };
    let mut groups = Vec::new();
    for document in documents {
        let Some(trains) = document.get("data").and_then(Value::as_array) else {
            return Err("status document has no data".into());
        };
        let fetched_at = document
            .get("fetchedAt")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339)
            .map(|fetched_at| fetched_at.min(now));
        for train in trains {
            if let Some(mut group) = group_from_train(train, grace, now) {
                group.fetched_at = fetched_at;
                groups.push(group);
            }
        }
    }
    Ok(groups)
}

fn group_from_train(train: &Value, grace: Duration, now: SystemTime) -> Option<StatusGroup> {
    let service = train.get("travelService")?;
    let train_number = normalize_train_number(service.get("number")?.as_str()?)?;
    let date = service
        .get("date")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let stops = train.get("stops")?.as_array()?;
    let mut rows = Vec::new();
    for stop in stops {
        let Some(code) = stop
            .get("station")
            .and_then(|station| station.get("code"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let code = code.trim().to_uppercase();
        if code.is_empty() || code.contains(':') {
            continue;
        }
        let Some(track) = chosen_track(stop) else {
            continue;
        };
        let retain_until = latest_instant(stop)
            .map(|instant| instant + grace)
            .unwrap_or_else(|| now + grace);
        rows.push(Assignment {
            train_number: train_number.clone(),
            stop_id: code,
            track,
            retain_until: Some(retain_until),
        });
    }
    if rows.is_empty() {
        return None;
    }
    Some(StatusGroup {
        source_key: format!("amtrak:{train_number}:{date}"),
        rows,
        fetched_at: None,
    })
}

/// Departure track when it is a platform token, otherwise the arrival track.
fn chosen_track(stop: &Value) -> Option<String> {
    stop.get("departure")
        .and_then(track_number)
        .or_else(|| stop.get("arrival").and_then(track_number))
}

fn track_number(side: &Value) -> Option<String> {
    let number = side.get("track")?.get("number")?;
    let text = match number {
        Value::String(text) => text.trim().to_uppercase(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    is_track_label(&text).then_some(text)
}

fn latest_instant(stop: &Value) -> Option<SystemTime> {
    ["departure", "arrival"]
        .into_iter()
        .filter_map(|side| stop.get(side))
        .flat_map(|side| {
            [
                side.pointer("/schedule/dateTime"),
                side.pointer("/statusInfo/dateTime"),
            ]
        })
        .filter_map(|value| value.and_then(Value::as_str))
        .filter_map(parse_rfc3339)
        .max()
}

/// `YYYY-MM-DDTHH:MM:SS` with `Z` or `±HH:MM`. Fractional seconds are ignored.
fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    if text.len() < 20 || text.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let year: i32 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: u32 = text.get(11..13)?.parse().ok()?;
    let minute: u32 = text.get(14..16)?.parse().ok()?;
    let second: u32 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = text.get(19..)?;
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped
            .find(|character: char| !character.is_ascii_digit())
            .unwrap_or(stripped.len());
        rest = stripped.get(digits..)?;
    }
    let offset_secs: i32 = if rest == "Z" || rest.is_empty() {
        0
    } else if rest.len() >= 6 && (rest.starts_with('+') || rest.starts_with('-')) {
        let sign: i32 = if rest.starts_with('+') { 1 } else { -1 };
        let hours: i32 = rest.get(1..3)?.parse().ok()?;
        let minutes: i32 = rest.get(4..6)?.parse().ok()?;
        sign * (hours * 3600 + minutes * 60)
    } else {
        return None;
    };
    let days = civil_days(year, month, day)?;
    let local = days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    let unix = local - i64::from(offset_secs);
    if unix < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::from_secs(unix as u64))
}

/// Days since 1970-01-01 for a valid civil date. Invalid dates (February 31) return `None`.
fn civil_days(year: i32, month: u32, day: u32) -> Option<i64> {
    let leap = |year: i32| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let lengths = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > lengths[month as usize - 1] {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + u64::from(day_of_year);
    Some(i64::from(era) * 146_097 + day_of_era as i64 - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_keeps_published_platforms_and_drops_placeholders() {
        let body = include_str!("../../../fixtures/tracks/amtrak_status.json");
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let groups = groups_from_status(body, Duration::from_secs(900), now).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].source_key, "amtrak:167:2099-01-01");
        let track = |stop: &str| {
            groups[0]
                .rows
                .iter()
                .find(|row| row.stop_id == stop)
                .map(|row| row.track.as_str())
        };
        assert_eq!(track("PVD"), Some("1"));
        assert_eq!(track("KIN"), Some("2"));
        assert_eq!(track("BUF"), Some("MAIN"));
        assert_eq!(track("BOS"), None);
        let pvd = groups[0]
            .rows
            .iter()
            .find(|row| row.stop_id == "PVD")
            .unwrap();
        // Departure 2099-01-01T17:08:00-05:00 is 22:08:00Z, plus the 900s grace.
        let expected = parse_rfc3339("2099-01-01T22:08:00Z").unwrap() + Duration::from_secs(900);
        assert_eq!(pvd.retain_until, Some(expected));
    }

    #[test]
    fn a_single_status_document_parses_without_a_bundle() {
        let body = r#"{"data":[{"travelService":{"number":"54","date":"2099-01-02"},"stops":[{"station":{"code":"nyp"},"arrival":{"track":{"number":10},"schedule":{"dateTime":"2099-01-02T12:00:00Z"}}}]}]}"#;
        let groups = groups_from_status(body, Duration::from_secs(60), UNIX_EPOCH).unwrap();
        assert_eq!(groups[0].source_key, "amtrak:54:2099-01-02");
        assert_eq!(groups[0].rows[0].stop_id, "NYP");
        assert_eq!(groups[0].rows[0].track, "10");
        assert_eq!(groups[0].fetched_at, None);
    }

    #[test]
    fn bundled_documents_carry_their_own_fetch_time() {
        let body = r#"{"documents":[{"fetchedAt":"2099-01-02T11:50:00Z","data":[{"travelService":{"number":"54","date":"2099-01-02"},"stops":[{"station":{"code":"NYP"},"departure":{"track":{"number":"10"}}}]}]}]}"#;
        let now = parse_rfc3339("2099-01-02T12:00:00Z").unwrap();
        let groups = groups_from_status(body, Duration::from_secs(60), now).unwrap();
        assert_eq!(groups[0].fetched_at, parse_rfc3339("2099-01-02T11:50:00Z"));
        // A sidecar clock ahead of the service's is clamped to the read time.
        let early = now - Duration::from_secs(3_600);
        let groups = groups_from_status(body, Duration::from_secs(60), early).unwrap();
        assert_eq!(groups[0].fetched_at, Some(early));
    }

    #[test]
    fn html_and_empty_objects_are_rejected() {
        let now = UNIX_EPOCH;
        assert!(groups_from_status("<html>", Duration::from_secs(1), now).is_err());
        assert!(groups_from_status("{}", Duration::from_secs(1), now).is_err());
        assert!(groups_from_status(r#"{"documents":[]}"#, Duration::from_secs(1), now).is_ok());
    }

    #[test]
    fn civil_dates_round_trip_the_unix_epoch() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(UNIX_EPOCH));
        assert_eq!(
            parse_rfc3339("2026-10-17T16:57:42-04:00"),
            parse_rfc3339("2026-10-17T20:57:42Z")
        );
        assert!(parse_rfc3339("2099-02-31T00:00:00Z").is_none());
    }
}
