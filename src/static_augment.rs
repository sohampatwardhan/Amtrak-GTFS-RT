//! Platform stops added to the published static feed so track assignments are spec-conformant.
//!
//! GTFS-Realtime's `StopTimeProperties.assigned_stop_id` must name a stop in the static
//! `stops.txt`. Amtrak's static feed has one plain stop per station and no platforms, and that
//! stop is referenced directly by `stop_times.txt`, so it cannot itself become a parent station.
//! For each covered station this module therefore adds a new parent station (`{stop}:station`),
//! re-parents the original stop under it, and adds one platform stop per configured track
//! (`{stop}:track:{label}`, with `platform_code`). The platform and the scheduled stop are then
//! siblings of one station, which is the relationship the reference describes for platform
//! assignments. Nothing else in the upstream feed changes.
//!
//! The transform is deterministic: every non-`stops.txt` entry is copied byte-for-byte, and the
//! rewritten `stops.txt` has a fixed timestamp, so the same upstream bytes and table always
//! produce the same output. Any failure returns [`AugmentError`] and the caller publishes the
//! upstream bytes instead.

use crate::sources::tracks::is_track_label;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

const STOPS_FILE: &str = "stops.txt";
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// Covered stations and their real platform tracks, in configured order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlatformTable {
    stations: BTreeMap<String, Vec<String>>,
}

impl PlatformTable {
    /// Parses `STOP=labels;STOP=labels`, where labels are comma-separated platform tokens or
    /// numeric ranges (`NWK=A,1-5`). Stop ids are uppercased; duplicate labels are dropped.
    ///
    /// # Errors
    ///
    /// Returns a message naming the malformed entry, range, or label.
    pub fn parse(raw: &str) -> Result<PlatformTable, String> {
        let mut stations = BTreeMap::new();
        for entry in raw
            .split(';')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let Some((stop, labels)) = entry.split_once('=') else {
                return Err(format!("entry {entry:?} must be STOP=tracks"));
            };
            let stop = stop.trim().to_uppercase();
            if stop.is_empty() || stop.contains(':') {
                return Err(format!("entry {entry:?} has an invalid stop id"));
            }
            let mut tracks: Vec<String> = Vec::new();
            for part in labels
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
            {
                for label in expand(part)? {
                    if !tracks.contains(&label) {
                        tracks.push(label);
                    }
                }
            }
            if tracks.is_empty() {
                return Err(format!("entry {entry:?} lists no tracks"));
            }
            stations.insert(stop, tracks);
        }
        Ok(PlatformTable { stations })
    }

    /// First 8 hex digits of SHA-256 over the canonical form (`NHV=1,2;NWK=A,1`), used to make
    /// the static version change exactly when the table changes.
    pub fn digest(&self) -> String {
        let hash = Sha256::digest(self.canonical().as_bytes());
        hash.iter()
            .take(4)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether `label` is a configured track of covered stop `stop_id`.
    pub fn contains(&self, stop_id: &str, label: &str) -> bool {
        self.stations
            .get(&stop_id.to_uppercase())
            .is_some_and(|tracks| tracks.iter().any(|track| track == label))
    }

    /// Covered stop ids in sorted order.
    pub fn station_ids(&self) -> impl Iterator<Item = &str> {
        self.stations.keys().map(String::as_str)
    }

    fn canonical(&self) -> String {
        self.stations
            .iter()
            .map(|(stop, tracks)| format!("{stop}={}", tracks.join(",")))
            .collect::<Vec<_>>()
            .join(";")
    }
}

fn expand(part: &str) -> Result<Vec<String>, String> {
    let upper = part.to_uppercase();
    if let Some((start, end)) = upper.split_once('-') {
        let start: u32 = start
            .trim()
            .parse()
            .map_err(|_| format!("range {part:?} is not numeric"))?;
        let end: u32 = end
            .trim()
            .parse()
            .map_err(|_| format!("range {part:?} is not numeric"))?;
        if start == 0 || start > end || end > 999 {
            return Err(format!("range {part:?} must be ascending within 1-999"));
        }
        return Ok((start..=end).map(|number| number.to_string()).collect());
    }
    if !is_track_label(&upper) {
        return Err(format!("{part:?} is not a platform track label"));
    }
    Ok(vec![upper])
}

/// Parent station id added for covered stop `stop_id`.
pub fn parent_station_id(stop_id: &str) -> String {
    format!("{stop_id}:station")
}

/// Platform stop id for track `label` at covered stop `stop_id`.
pub fn platform_stop_id(stop_id: &str, label: &str) -> String {
    format!("{stop_id}:track:{label}")
}

/// Why augmentation could not produce a feed; the caller falls back to upstream bytes.
#[derive(Debug, Eq, PartialEq)]
pub struct AugmentError(pub String);

impl std::fmt::Display for AugmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AugmentError {}

/// Returns `upstream` with platform structure added for every covered station it contains.
///
/// A covered stop that is absent, not a plain stop, or already parented is skipped with a
/// warning. Every other archive entry is copied without recompression.
///
/// # Errors
///
/// Returns [`AugmentError`] for an unreadable archive or `stops.txt`, a missing `stop_id`
/// column, or a generated id that collides with an existing stop.
pub fn augment_static(upstream: &[u8], table: &PlatformTable) -> Result<Vec<u8>, AugmentError> {
    let mut archive =
        ZipArchive::new(Cursor::new(upstream)).map_err(|error| fail("archive", error))?;
    let mut stops = Vec::new();
    archive
        .by_name(STOPS_FILE)
        .map_err(|error| fail("stops.txt", error))?
        .read_to_end(&mut stops)
        .map_err(|error| fail("stops.txt", error))?;
    let rewritten = augment_stops(&stops, table)?;

    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::default());
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|error| fail("entry", error))?;
        if entry.name() == STOPS_FILE {
            drop(entry);
            writer
                .start_file(STOPS_FILE, options)
                .map_err(|error| fail("write", error))?;
            writer
                .write_all(&rewritten)
                .map_err(|error| fail("write", error))?;
        } else {
            writer
                .raw_copy_file(entry)
                .map_err(|error| fail("copy", error))?;
        }
    }
    Ok(writer
        .finish()
        .map_err(|error| fail("finish", error))?
        .into_inner())
}

fn fail(stage: &str, error: impl std::fmt::Display) -> AugmentError {
    AugmentError(format!("{stage}: {error}"))
}

fn augment_stops(raw: &[u8], table: &PlatformTable) -> Result<Vec<u8>, AugmentError> {
    let body = raw.strip_prefix(UTF8_BOM).unwrap_or(raw);
    let mut reader = csv::ReaderBuilder::new().from_reader(body);
    let mut headers: Vec<String> = reader
        .headers()
        .map_err(|error| fail("stops.txt header", error))?
        .iter()
        .map(|header| header.trim().to_string())
        .collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| fail("stops.txt row", error))?;
        rows.push(record.iter().map(str::to_string).collect());
    }
    for column in ["location_type", "parent_station", "platform_code"] {
        if !headers.iter().any(|header| header == column) {
            headers.push(column.to_string());
        }
    }
    for row in &mut rows {
        row.resize(headers.len(), String::new());
    }
    let column = |name: &str| headers.iter().position(|header| header == name);
    let id = column("stop_id").ok_or_else(|| AugmentError("stops.txt has no stop_id".into()))?;
    let location = column("location_type").unwrap_or_default();
    let parent = column("parent_station").unwrap_or_default();
    let platform = column("platform_code").unwrap_or_default();
    let name = column("stop_name");
    let existing: HashSet<String> = rows.iter().map(|row| row[id].clone()).collect();

    let mut added = Vec::new();
    for stop in table.station_ids() {
        let Some(index) = rows.iter().position(|row| row[id] == stop) else {
            tracing::warn!(stop, "covered station absent from static feed; skipped");
            continue;
        };
        let plain = matches!(rows[index][location].trim(), "" | "0");
        if !plain || !rows[index][parent].trim().is_empty() {
            tracing::warn!(
                stop,
                "covered station is not a plain unparented stop; skipped"
            );
            continue;
        }
        let station_id = parent_station_id(stop);
        let labels = table.stations.get(stop).cloned().unwrap_or_default();
        let generated = std::iter::once(station_id.clone())
            .chain(labels.iter().map(|label| platform_stop_id(stop, label)));
        for generated_id in generated {
            if existing.contains(&generated_id) {
                return Err(AugmentError(format!(
                    "generated stop id {generated_id} already exists"
                )));
            }
        }
        let template = rows[index].clone();
        rows[index][parent].clone_from(&station_id);

        let mut station = template.clone();
        clear_except(
            &mut station,
            &headers,
            &[
                "stop_name",
                "stop_lat",
                "stop_lon",
                "stop_timezone",
                "stop_url",
            ],
        );
        station[id].clone_from(&station_id);
        station[location] = "1".into();
        added.push(station);
        for label in &labels {
            let mut row = template.clone();
            clear_except(
                &mut row,
                &headers,
                &["stop_name", "stop_lat", "stop_lon", "stop_timezone"],
            );
            row[id] = platform_stop_id(stop, label);
            if let Some(name) = name {
                row[name] = format!("{} Track {label}", template[name]);
            }
            row[location] = "0".into();
            row[parent].clone_from(&station_id);
            row[platform].clone_from(label);
            added.push(row);
        }
    }

    let mut writer = csv::WriterBuilder::new()
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record(&headers)
        .map_err(|error| fail("stops.txt write", error))?;
    for row in rows.iter().chain(added.iter()) {
        writer
            .write_record(row)
            .map_err(|error| fail("stops.txt write", error))?;
    }
    writer
        .into_inner()
        .map_err(|error| fail("stops.txt write", error))
}

fn clear_except(row: &mut [String], headers: &[String], keep: &[&str]) {
    for (value, header) in row.iter_mut().zip(headers) {
        if !keep.contains(&header.as_str()) {
            value.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtfs_structures::{Gtfs, LocationType};

    const STOPS: &str = "stop_id,stop_code,stop_name,stop_url,stop_timezone,stop_lat,stop_lon\n\
NWK,NWK,Newark,https://www.amtrak.com/stations/nwk,America/New_York,40.734706,-74.16475\n\
NYP,NYP,\"Ny Moynihan Train Hall At Penn Station\",https://www.amtrak.com/stations/nyp,America/New_York,40.750327,-73.994459\n";

    pub(crate) fn fixture_zip(stops: &[u8]) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let files: [(&str, &[u8]); 6] = [
            ("agency.txt", b"agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", stops),
            ("routes.txt", b"route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,R,Regional,2\n"),
            ("trips.txt", b"route_id,service_id,trip_id,trip_short_name\nr,svc,t,67\n"),
            ("stop_times.txt", b"trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,NWK,1\nt,10:20:00,10:20:00,NYP,2\n"),
            ("calendar_dates.txt", b"service_id,date,exception_type\nsvc,20260813,1\n"),
        ];
        for (name, contents) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn table() -> PlatformTable {
        PlatformTable::parse("NWK=A,1-2;NYP=5;TRE=1").unwrap()
    }

    fn entries(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = ZipArchive::new(Cursor::new(zip)).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index_raw(index).unwrap();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                (entry.name().to_string(), bytes)
            })
            .collect()
    }

    #[test]
    fn table_parses_ranges_letters_and_digest() {
        let table = PlatformTable::parse(" nwk = A, 1-3 ,2 ; NHV=1-4,8,10 ").unwrap();
        assert!(table.contains("NWK", "A"));
        assert!(table.contains("nwk", "3"));
        assert!(!table.contains("NWK", "4"));
        assert!(table.contains("NHV", "10"));
        assert_eq!(table.station_ids().collect::<Vec<_>>(), vec!["NHV", "NWK"]);
        assert_eq!(table.digest().len(), 8);
        assert_eq!(
            table.digest(),
            PlatformTable::parse("NHV=1,2,3,4,8,10;NWK=A,1,2,3")
                .unwrap()
                .digest()
        );
        assert_ne!(
            table.digest(),
            PlatformTable::parse("NWK=A,1-3").unwrap().digest()
        );
        for bad in ["NWK", "NWK=", "NWK=TBD", "NWK=5-1", "NWK=0-3", "N:W=1"] {
            assert!(PlatformTable::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn augmentation_adds_platform_structure_only() {
        let upstream = fixture_zip(STOPS.as_bytes());
        let augmented = augment_static(&upstream, &table()).unwrap();
        let gtfs = Gtfs::from_reader(Cursor::new(augmented.clone())).unwrap();

        let station = &gtfs.stops["NWK:station"];
        assert_eq!(station.location_type, LocationType::StopArea);
        assert_eq!(station.name.as_deref(), Some("Newark"));
        assert_eq!(
            gtfs.stops["NWK"].parent_station.as_deref(),
            Some("NWK:station")
        );
        assert_eq!(gtfs.stops["NWK"].code.as_deref(), Some("NWK"));
        for label in ["A", "1", "2"] {
            let platform = &gtfs.stops[&platform_stop_id("NWK", label)];
            assert_eq!(platform.location_type, LocationType::StopPoint);
            assert_eq!(platform.parent_station.as_deref(), Some("NWK:station"));
            assert_eq!(platform.platform_code.as_deref(), Some(label));
            assert_eq!(
                platform.name.as_deref(),
                Some(format!("Newark Track {label}").as_str())
            );
            assert!(platform.latitude.is_some());
        }
        assert!(gtfs.stops.contains_key("NYP:track:5"));
        // TRE is covered but absent upstream, so nothing is added for it.
        assert!(!gtfs.stops.keys().any(|id| id.starts_with("TRE")));
        assert_eq!(gtfs.stops.len(), 2 + 1 + 3 + 1 + 1);
        assert_eq!(gtfs.trips["t"].stop_times[0].stop.id, "NWK");

        let before = entries(&upstream);
        let after = entries(&augmented);
        assert_eq!(
            before.iter().map(|entry| &entry.0).collect::<Vec<_>>(),
            after.iter().map(|entry| &entry.0).collect::<Vec<_>>()
        );
        for (old, new) in before.iter().zip(&after) {
            if old.0 != STOPS_FILE {
                assert_eq!(old.1, new.1, "{} changed", old.0);
            }
        }
    }

    #[test]
    fn augmentation_is_deterministic() {
        let upstream = fixture_zip(STOPS.as_bytes());
        assert_eq!(
            augment_static(&upstream, &table()).unwrap(),
            augment_static(&upstream, &table()).unwrap()
        );
    }

    #[test]
    fn byte_order_mark_is_ignored() {
        let mut stops = UTF8_BOM.to_vec();
        stops.extend_from_slice(STOPS.as_bytes());
        let augmented = augment_static(&fixture_zip(&stops), &table()).unwrap();
        let gtfs = Gtfs::from_reader(Cursor::new(augmented)).unwrap();
        assert!(gtfs.stops.contains_key("NWK:track:A"));
    }

    #[test]
    fn parented_or_non_plain_stations_are_skipped() {
        let stops = "stop_id,stop_name,stop_lat,stop_lon,location_type,parent_station\n\
P,Parent,40,-74,1,\n\
NWK,Newark,40,-74,0,P\n\
NYP,Penn,40,-73,1,\n\
X,Stop,40,-73,0,P\n";
        let augmented = augment_static(&fixture_zip(stops.as_bytes()), &table()).unwrap();
        let gtfs = Gtfs::from_reader(Cursor::new(augmented)).unwrap();
        assert!(!gtfs
            .stops
            .keys()
            .any(|id| id.contains(":track:") || id.contains(":station")));
    }

    #[test]
    fn collisions_and_missing_stop_ids_fail() {
        let stops =
            "stop_id,stop_name,stop_lat,stop_lon\nNWK,Newark,40,-74\nNWK:track:A,Clash,40,-74\n";
        assert!(augment_static(&fixture_zip(stops.as_bytes()), &table()).is_err());
        let stops = "stop_name,stop_lat,stop_lon\nNewark,40,-74\n";
        assert!(augment_static(&fixture_zip(stops.as_bytes()), &table()).is_err());
        assert!(augment_static(b"not a zip", &table()).is_err());
    }

    /// Augments a real feed for manual validator runs:
    /// `AUGMENT_IN=GTFS.zip AUGMENT_OUT=out.zip cargo test -- --ignored augment_file`.
    #[test]
    #[ignore = "manual: augments the file named by AUGMENT_IN"]
    fn augment_file() {
        let input = std::env::var("AUGMENT_IN").expect("AUGMENT_IN");
        let output = std::env::var("AUGMENT_OUT").expect("AUGMENT_OUT");
        let table = PlatformTable::parse("NYP=1-21;NWK=A,1-5;NHV=1-4,8,10,12,14").unwrap();
        let augmented = augment_static(&std::fs::read(input).unwrap(), &table).unwrap();
        std::fs::write(output, augmented).unwrap();
    }
}
