//! Enrichment helpers: friendly route names and per-station local time.
//!
//! These turn raw feed identifiers into rider-facing values. `route_display_name` resolves a GTFS
//! `route_id` (e.g. `40751`) to a human name (`Acela`). `station_tz` and `local_time` render a Unix
//! instant in the *station's own* timezone with the correct daylight-saving offset — Amtrak spans
//! Eastern through Pacific, so a single fixed zone would mislabel most of the network.

use chrono::DateTime;
use chrono_tz::Tz;
use gtfs_structures::{Gtfs, Stop};
use std::fmt;

/// Resolves a GTFS `route_id` to a display name: the route long name, else its short name, else the
/// id itself. Why the fallback chain: Amtrak populates `route_long_name` (`Northeast Regional`,
/// `Acela`) but a defensive path keeps output meaningful if a feed ever omits it.
pub fn route_display_name(gtfs: &Gtfs, route_id: &str) -> String {
    let nonempty = |value: &Option<String>| value.clone().filter(|s| !s.trim().is_empty());
    match gtfs.routes.get(route_id) {
        Some(route) => nonempty(&route.long_name)
            .or_else(|| nonempty(&route.short_name))
            .unwrap_or_else(|| route_id.to_string()),
        None => route_id.to_string(),
    }
}

/// Returns the feed's agency timezone, used only as a fallback when a stop has none. Defaults to
/// `America/New_York` (Amtrak's agency zone) if the feed somehow declares no agency.
pub fn agency_timezone(gtfs: &Gtfs) -> String {
    gtfs.agencies
        .first()
        .map(|agency| agency.timezone.clone())
        .filter(|tz| !tz.trim().is_empty())
        .unwrap_or_else(|| "America/New_York".to_string())
}

/// Resolves the timezone to render a station's times in.
///
/// Returns the station's own `stop_timezone` when present (`is_fallback = false`); otherwise the
/// agency timezone with `is_fallback = true`, so a caller can flag that the time used a fallback
/// zone rather than the station's declared one.
pub fn station_tz(gtfs: &Gtfs, stop: &Stop) -> (String, bool) {
    match stop.timezone.as_ref().filter(|tz| !tz.trim().is_empty()) {
        Some(tz) => (tz.clone(), false),
        None => (agency_timezone(gtfs), true),
    }
}

/// A timezone name that could not be parsed, or a timestamp outside the representable range.
#[derive(Debug)]
pub struct TzError(pub String);

impl fmt::Display for TzError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TzError {}

/// Formats a Unix instant as `HH:MM ZZZ` local wall-clock time in the named IANA zone.
///
/// The zone abbreviation (e.g. `EDT` vs `EST`) reflects the daylight-saving rule in effect on that
/// date, which is exactly the DST correctness the station board requires. Returns [`TzError`] for an
/// unknown zone or an out-of-range timestamp rather than guessing.
pub fn local_time(unix: i64, tz_name: &str) -> Result<String, TzError> {
    let tz: Tz = tz_name
        .parse()
        .map_err(|_| TzError(format!("unknown timezone {tz_name}")))?;
    let instant =
        DateTime::from_timestamp(unix, 0).ok_or_else(|| TzError("timestamp out of range".into()))?;
    Ok(instant.with_timezone(&tz).format("%H:%M %Z").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn gtfs_with(stops_txt: &str, routes_txt: &str) -> Gtfs {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", stops_txt),
            ("routes.txt", routes_txt),
            ("trips.txt", "route_id,service_id,trip_id,trip_short_name\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        Gtfs::from_reader(Cursor::new(bytes)).unwrap()
    }

    // R2.3 support: route ids render as names, with graceful fallback.
    #[test]
    fn route_names_resolve_with_fallback() {
        let gtfs = gtfs_with(
            "stop_id,stop_name,stop_lat,stop_lon,stop_timezone\nNHV,New Haven,41.3,-72.9,America/New_York\n",
            "route_id,agency_id,route_short_name,route_long_name,route_type\n40751,a,,Acela,2\n88,a,,,2\n",
        );
        assert_eq!(route_display_name(&gtfs, "40751"), "Acela");
        assert_eq!(route_display_name(&gtfs, "88"), "88"); // no names -> id
        assert_eq!(route_display_name(&gtfs, "99999"), "99999"); // unknown -> id
    }

    // R6.2: a station without a timezone falls back to the agency zone, flagged.
    #[test]
    fn station_tz_uses_stop_zone_then_falls_back() {
        let gtfs = gtfs_with(
            "stop_id,stop_name,stop_lat,stop_lon,stop_timezone\nDEN,Denver,39.7,-105.0,America/Denver\nBLANK,Blank,0,0,\n",
            "route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,R,Regional,2\n",
        );
        let den = gtfs.stops.get("DEN").unwrap();
        assert_eq!(station_tz(&gtfs, den), ("America/Denver".to_string(), false));
        let blank = gtfs.stops.get("BLANK").unwrap();
        assert_eq!(station_tz(&gtfs, blank), ("America/New_York".to_string(), true));
    }

    // R6.1: the same instant renders in the station's zone with the correct DST abbreviation.
    #[test]
    fn local_time_is_dst_correct_per_zone() {
        // 2026-07-01T16:00:00Z: summer -> EDT (-4) in New York, MDT (-6) in Denver.
        let summer = 1_782_921_600;
        assert_eq!(local_time(summer, "America/New_York").unwrap(), "12:00 EDT");
        assert_eq!(local_time(summer, "America/Denver").unwrap(), "10:00 MDT");
        // 2026-01-01T16:00:00Z: winter -> EST (-5) in New York.
        let winter = 1_767_283_200;
        assert_eq!(local_time(winter, "America/New_York").unwrap(), "11:00 EST");
        // Unknown zone is an error, not a guess.
        assert!(local_time(summer, "Mars/Olympus").is_err());
    }
}
