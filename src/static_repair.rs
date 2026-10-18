//! Repairs for upstream static defects that would otherwise reject Amtrak's whole schedule.
//!
//! The GTFS parser refuses a feed whose `stop_times.txt` names a stop that `stops.txt` does not
//! define (`The id OSM is not known`), and the standards validator reports the same rows as
//! `foreign_key_violation` errors. Amtrak has published such a feed: in 20261018 the Mardi Gras
//! Service calls at Ocean Springs (`OSM`) with no `OSM` row in `stops.txt`. Rather than reject
//! every later schedule, the service drops those stop times, which keeps each trip's other calls,
//! and logs which stops were missing. The transform is deterministic, so unchanged upstream bytes
//! still compare equal.

use crate::static_augment::{fail, read_entry, replace_entry, AugmentError, UTF8_BOM};
use std::collections::{BTreeMap, HashSet};

const STOPS_FILE: &str = "stops.txt";
const STOP_TIMES_FILE: &str = "stop_times.txt";

/// `upstream` without stop times whose stop is missing from `stops.txt`, or `None` when nothing
/// needs repair or the archive cannot be read (the parse that follows then reports why).
pub fn repair_static(upstream: &[u8]) -> Option<Vec<u8>> {
    match drop_dangling_stop_times(upstream) {
        Ok(repaired) => repaired,
        Err(error) => {
            tracing::warn!(%error, stage = "static", "static repair skipped");
            None
        }
    }
}

fn drop_dangling_stop_times(upstream: &[u8]) -> Result<Option<Vec<u8>>, AugmentError> {
    let stops = read_entry(upstream, STOPS_FILE)?;
    let mut reader = csv_reader(&stops);
    let id = column(&mut reader, STOPS_FILE, "stop_id")?;
    let mut known = HashSet::new();
    for record in reader.records() {
        let record = record.map_err(|error| fail("stops.txt row", error))?;
        known.insert(record.get(id).unwrap_or_default().trim().to_string());
    }

    let stop_times = read_entry(upstream, STOP_TIMES_FILE)?;
    let mut reader = csv_reader(&stop_times);
    let stop = column(&mut reader, STOP_TIMES_FILE, "stop_id")?;
    let headers = reader
        .headers()
        .map_err(|error| fail("stop_times.txt header", error))?
        .clone();
    let mut kept = Vec::new();
    let mut dropped: BTreeMap<String, usize> = BTreeMap::new();
    for record in reader.records() {
        let record = record.map_err(|error| fail("stop_times.txt row", error))?;
        let stop_id = record.get(stop).unwrap_or_default().trim();
        if known.contains(stop_id) {
            kept.push(record);
        } else {
            *dropped.entry(stop_id.to_string()).or_default() += 1;
        }
    }
    if dropped.is_empty() {
        return Ok(None);
    }
    for (stop_id, rows) in &dropped {
        tracing::warn!(
            stop = stop_id.as_str(),
            rows,
            stage = "static",
            "stop times name a stop missing from stops.txt; dropped"
        );
    }

    let mut writer = csv::WriterBuilder::new()
        .flexible(true)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_writer(Vec::new());
    writer
        .write_record(&headers)
        .map_err(|error| fail("stop_times.txt write", error))?;
    for record in &kept {
        writer
            .write_record(record)
            .map_err(|error| fail("stop_times.txt write", error))?;
    }
    let rewritten = writer
        .into_inner()
        .map_err(|error| fail("stop_times.txt write", error))?;
    replace_entry(upstream, STOP_TIMES_FILE, &rewritten).map(Some)
}

fn csv_reader(raw: &[u8]) -> csv::Reader<&[u8]> {
    let body = raw.strip_prefix(UTF8_BOM).unwrap_or(raw);
    // Short rows omitting trailing empty fields are legal GTFS.
    csv::ReaderBuilder::new().flexible(true).from_reader(body)
}

fn column(reader: &mut csv::Reader<&[u8]>, file: &str, name: &str) -> Result<usize, AugmentError> {
    reader
        .headers()
        .map_err(|error| fail(file, error))?
        .iter()
        .position(|header| header.trim() == name)
        .ok_or_else(|| AugmentError(format!("{file} has no {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::static_augment::{augment_static, PlatformTable};
    use gtfs_structures::Gtfs;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /// Amtrak's 20261018 shape: the Mardi Gras Service calls at `OSM`, which `stops.txt` omits.
    fn mardi_gras_zip(stop_times: &str) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        let files = [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\n51,Amtrak,https://www.amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_code,stop_name,stop_url,stop_timezone,stop_lat,stop_lon\r\n\
BIX,BIX,Biloxi Amtrak Station,https://www.amtrak.com/stations/bix,America/Chicago,30.398,-88.892\r\n\
NHV,NHV,New Haven Union Station,https://www.amtrak.com/stations/nhv,America/New_York,41.297,-72.926\r\n\
PAG,PAG,Pascagoula Amtrak Station,https://www.amtrak.com/stations/pag,America/Chicago,30.367,-88.556\r\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\n42996,51,,Mardi Gras Service,2\n"),
            ("trips.txt", "route_id,service_id,trip_id,trip_short_name,direction_id,shape_id,trip_headsign\n42996,svc,312328,26,1,,Mobile Amtrak Sta\n"),
            ("stop_times.txt", stop_times),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20261019,1\n"),
            ("feed_info.txt", "feed_publisher_name,feed_publisher_url,feed_lang,feed_version\nAmtrak,http://www.amtrak.com,en,20261018\n"),
        ];
        for (name, contents) in files {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    const DANGLING: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence,pickup_type,drop_off_type,timepoint\r\n\
312328,19:39:00,19:39:00,BIX,4,0,0,1\r\n\
312328,19:47:00,19:52:00,OSM,5,0,0,1\r\n\
312328,20:11:00,20:11:00,PAG,6,0,0,1\r\n";

    #[test]
    fn stop_times_naming_a_missing_stop_are_dropped() {
        let upstream = mardi_gras_zip(DANGLING);
        let Err(error) = Gtfs::from_reader(Cursor::new(upstream.clone())) else {
            panic!("the fixture must reproduce the upstream parse failure");
        };
        assert_eq!(error.to_string(), "The id OSM is not known");

        let repaired = repair_static(&upstream).unwrap();
        let gtfs = Gtfs::from_reader(Cursor::new(repaired.clone())).unwrap();
        let calls: Vec<(&str, u32)> = gtfs.trips["312328"]
            .stop_times
            .iter()
            .map(|time| (time.stop.id.as_str(), time.stop_sequence))
            .collect();
        assert_eq!(calls, [("BIX", 4), ("PAG", 6)]);
        assert_eq!(repair_static(&upstream), Some(repaired.clone()));
        assert_eq!(
            read_entry(&repaired, STOPS_FILE),
            read_entry(&upstream, STOPS_FILE)
        );
    }

    #[test]
    fn a_repaired_feed_takes_platform_stops() {
        let repaired = repair_static(&mardi_gras_zip(DANGLING)).unwrap();
        let table = PlatformTable::parse("NHV=1").unwrap();
        let gtfs =
            Gtfs::from_reader(Cursor::new(augment_static(&repaired, &table).unwrap())).unwrap();
        assert!(gtfs.stops.contains_key("NHV:track:1"));
    }

    #[test]
    fn a_consistent_feed_is_left_alone() {
        let upstream = mardi_gras_zip(
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
312328,19:39:00,19:39:00,BIX,4\n312328,20:11:00,20:11:00, PAG ,6\n",
        );
        assert_eq!(repair_static(&upstream), None);
        assert_eq!(repair_static(b"not a zip"), None);
    }
}
