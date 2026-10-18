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
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tokio::sync::Notify;
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

    /// Adds one platform. Returns whether the table changed.
    ///
    /// Stop ids are uppercased. A label that is not a platform token, or a stop id that contains
    /// `:`, is ignored. Duplicate labels are ignored.
    pub fn insert_track(&mut self, stop_id: &str, label: &str) -> bool {
        let stop = stop_id.trim().to_uppercase();
        let label = label.trim().to_uppercase();
        if stop.is_empty()
            || stop.contains(':')
            || !(2..=5).contains(&stop.len())
            || !stop
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
            || !is_track_label(&label)
        {
            return false;
        }
        let tracks = self.stations.entry(stop).or_default();
        if tracks.iter().any(|track| track == &label) {
            return false;
        }
        tracks.push(label);
        true
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

    /// Number of platform labels across all stations.
    pub fn label_count(&self) -> usize {
        self.stations.values().map(Vec::len).sum()
    }

    /// Adds every label of `other` after this table's own.
    fn extend(&mut self, other: &PlatformTable) {
        for (stop, tracks) in &other.stations {
            let existing = self.stations.entry(stop.clone()).or_default();
            for track in tracks {
                if !existing.contains(track) {
                    existing.push(track.clone());
                }
            }
        }
    }

    /// Canonical form (`NHV=1,2;NWK=A,1`), also the on-disk record of observed platforms.
    pub fn canonical(&self) -> String {
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

/// Most observed labels kept, so a misbehaving source cannot grow the static feed without bound.
pub const MAX_OBSERVED_PLATFORMS: usize = 1000;

struct PlatformState {
    configured: PlatformTable,
    /// Labels seen at runtime that are not configured. This delta is what `path` records.
    observed: PlatformTable,
    /// Plain stop ids of the active static snapshot. Only these stations are observed.
    known: Option<HashSet<String>>,
    /// Observations changed since the file was last written.
    dirty: bool,
    cap_reported: bool,
}

/// Configured platforms plus tracks observed at runtime, shared by the stamper and static refresh.
///
/// Observed labels are kept apart from configured ones and only the observed delta is written to
/// `path`, so removing a track from `AMTRAK_TRACKS_PLATFORMS` takes effect and deleting the file
/// resets observations. A newly observed track is signaled on [`Self::changed`] so static refresh
/// can publish the platform stop without waiting for the daily Amtrak feed poll.
#[derive(Clone)]
pub struct SharedPlatforms {
    inner: Arc<RwLock<PlatformState>>,
    changed: Arc<Notify>,
    path: Option<PathBuf>,
}

impl SharedPlatforms {
    /// Shares `configured` with no observations. `path` is where new observations are recorded.
    #[cfg(test)]
    pub fn new(configured: PlatformTable, path: Option<PathBuf>) -> SharedPlatforms {
        SharedPlatforms::with_observed(configured, PlatformTable::default(), path)
    }

    /// Shares `configured` plus the observations recorded at `path`.
    ///
    /// A missing file is no observations. A file that does not parse is renamed to `*.invalid`
    /// and ignored, so a corrupt record cannot fail startup or be silently overwritten. Recorded
    /// labels that are now configured are dropped from the delta.
    pub fn load(configured: PlatformTable, path: PathBuf) -> SharedPlatforms {
        let observed = match std::fs::read_to_string(&path) {
            Ok(saved) => match PlatformTable::parse(saved.trim()) {
                Ok(saved) => saved,
                Err(error) => {
                    let mut invalid = path.clone().into_os_string();
                    invalid.push(".invalid");
                    let renamed = std::fs::rename(&path, &invalid);
                    tracing::warn!(
                        %error,
                        renamed = renamed.is_ok(),
                        path = %path.display(),
                        "ignoring unreadable platform observations"
                    );
                    PlatformTable::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => PlatformTable::default(),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "cannot read platform observations");
                PlatformTable::default()
            }
        };
        SharedPlatforms::with_observed(configured, observed, Some(path))
    }

    fn with_observed(
        configured: PlatformTable,
        saved: PlatformTable,
        path: Option<PathBuf>,
    ) -> SharedPlatforms {
        let mut observed = PlatformTable::default();
        for (stop, tracks) in &saved.stations {
            for track in tracks {
                if !configured.contains(stop, track)
                    && observed.label_count() < MAX_OBSERVED_PLATFORMS
                {
                    observed.insert_track(stop, track);
                }
            }
        }
        let dirty = observed != saved;
        SharedPlatforms {
            inner: Arc::new(RwLock::new(PlatformState {
                configured,
                observed,
                known: None,
                dirty,
                cap_reported: false,
            })),
            changed: Arc::new(Notify::new()),
            path,
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, PlatformState> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, PlatformState> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Configured and observed platforms together, configured labels first.
    pub fn snapshot(&self) -> PlatformTable {
        let state = self.read();
        let mut table = state.configured.clone();
        table.extend(&state.observed);
        table
    }

    /// Tables to augment static with, in order of preference.
    ///
    /// With observations, the configured table alone follows the full one, so a single bad
    /// observation that makes augmentation or validation fail costs only the observed platforms,
    /// not every platform stop.
    pub fn candidates(&self) -> Vec<PlatformTable> {
        let full = self.snapshot();
        let configured = self.read().configured.clone();
        if full == configured {
            vec![full]
        } else {
            vec![full, configured]
        }
    }

    /// Limits observation to `stop_ids` (an active static snapshot's stops). Generated platform
    /// and parent ids, which contain `:`, are skipped.
    pub fn set_known_stations<'a>(&self, stop_ids: impl IntoIterator<Item = &'a str>) {
        let known = stop_ids
            .into_iter()
            .filter(|stop| !stop.contains(':'))
            .map(str::to_uppercase)
            .collect();
        self.write().known = Some(known);
    }

    /// Records one observed platform. Returns whether it was new.
    ///
    /// Configured labels, labels already observed, stations outside the static feed, and anything
    /// past [`MAX_OBSERVED_PLATFORMS`] are ignored. Nothing is written to disk here; see
    /// [`Self::persist`].
    pub fn observe(&self, stop_id: &str, label: &str) -> bool {
        let stop = stop_id.trim().to_uppercase();
        let label = label.trim().to_uppercase();
        {
            let mut state = self.write();
            if state.configured.contains(&stop, &label) || state.observed.contains(&stop, &label) {
                return false;
            }
            if state
                .known
                .as_ref()
                .is_some_and(|known| !known.contains(&stop))
            {
                return false;
            }
            if state.observed.label_count() >= MAX_OBSERVED_PLATFORMS {
                if !state.cap_reported {
                    state.cap_reported = true;
                    tracing::warn!(
                        limit = MAX_OBSERVED_PLATFORMS,
                        "observed platform limit reached; new tracks are not added"
                    );
                }
                return false;
            }
            if !state.observed.insert_track(&stop, &label) {
                return false;
            }
            state.dirty = true;
        }
        self.changed.notify_one();
        true
    }

    /// Writes the observed delta to `path` if it changed, atomically and outside the table lock.
    /// A failure is logged and retried on the next call.
    pub async fn persist(&self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let contents = {
            let mut state = self.write();
            if !state.dirty {
                return;
            }
            state.dirty = false;
            state.observed.canonical()
        };
        let target = path.clone();
        let written = tokio::task::spawn_blocking(move || write_atomically(&target, &contents))
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error)));
        if let Err(error) = written {
            self.write().dirty = true;
            tracing::warn!(%error, path = %path.display(), "could not save platform observations");
        }
    }

    /// Wakes [`crate::static_gtfs::run_snapshot_refresh`] after a new platform is observed.
    pub fn changed(&self) -> Arc<Notify> {
        self.changed.clone()
    }
}

fn write_atomically(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
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
    // GTFS producers may omit trailing empty fields, so short rows are legal; they are padded to
    // the header width below rather than failing the whole augmentation.
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(body);
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
    fn an_observed_track_is_added_once_and_changes_the_digest() {
        let mut table = PlatformTable::parse("NWK=1").unwrap();
        let before = table.digest();
        assert!(!table.insert_track("NWK", "1"));
        assert!(table.insert_track("pvd", "1"));
        assert!(table.insert_track("BUF", "MAIN"));
        assert!(!table.insert_track("PVD", "TBD"));
        assert!(!table.insert_track("bad:stop", "1"));
        assert_ne!(table.digest(), before);
        assert_eq!(table.canonical(), "BUF=MAIN;NWK=1;PVD=1");
        let augmented = augment_static(&fixture_zip(STOPS.as_bytes()), &table).unwrap();
        let gtfs = Gtfs::from_reader(Cursor::new(augmented)).unwrap();
        assert!(gtfs.stops.contains_key("NWK:track:1"));
        assert!(!gtfs.stops.contains_key("PVD:track:1"));
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "amtrak-platforms-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("observed-platforms.txt")
    }

    #[tokio::test]
    async fn only_the_observed_delta_is_recorded_and_reloaded() {
        let path = temp_path("delta");
        let shared = SharedPlatforms::load(PlatformTable::parse("NWK=1,4").unwrap(), path.clone());
        shared.set_known_stations(["NWK", "PVD", "PVD:track:1"]);
        assert!(!shared.observe("NWK", "4"));
        assert!(shared.observe("pvd", "1"));
        assert!(!shared.observe("PVD", "1"));
        assert!(
            !shared.observe("XYZ", "2"),
            "station outside the static feed"
        );
        assert!(!shared.observe("PVD", "TBD"));
        shared.persist().await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "PVD=1");
        assert_eq!(shared.snapshot().canonical(), "NWK=1,4;PVD=1");
        assert_eq!(shared.candidates().len(), 2);
        assert_eq!(shared.candidates()[1].canonical(), "NWK=1,4");

        // Removing NWK=4 from the configuration takes effect; PVD=1 is still observed. A recorded
        // label that is now configured leaves the delta.
        let reloaded = SharedPlatforms::load(PlatformTable::parse("PVD=1").unwrap(), path.clone());
        assert_eq!(reloaded.snapshot().canonical(), "PVD=1");
        assert_eq!(reloaded.candidates().len(), 1);
        reloaded.persist().await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn an_unreadable_record_is_set_aside_and_ignored() {
        let path = temp_path("invalid");
        std::fs::write(&path, "PVD=1;garbage").unwrap();
        let shared = SharedPlatforms::load(PlatformTable::parse("NWK=1").unwrap(), path.clone());
        assert_eq!(shared.snapshot().canonical(), "NWK=1");
        assert!(!path.exists());
        let mut invalid = path.into_os_string();
        invalid.push(".invalid");
        assert!(PathBuf::from(invalid).exists());
    }

    #[test]
    fn observations_are_capped() {
        let shared = SharedPlatforms::new(PlatformTable::default(), None);
        let mut added = 0;
        for station in 0..100 {
            for track in 1..=20 {
                if shared.observe(&format!("S{station:03}"), &track.to_string()) {
                    added += 1;
                }
            }
        }
        assert_eq!(added, MAX_OBSERVED_PLATFORMS);
        assert_eq!(shared.snapshot().label_count(), MAX_OBSERVED_PLATFORMS);
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
    fn rows_omitting_trailing_fields_are_accepted() {
        let stops = "stop_id,stop_name,stop_lat,stop_lon,stop_timezone,stop_url\n\
NWK,Newark,40.73,-74.16\n\
NYP,Penn,40.75,-73.99,America/New_York,https://www.amtrak.com/stations/nyp\n";
        let augmented = augment_static(&fixture_zip(stops.as_bytes()), &table()).unwrap();
        let gtfs = Gtfs::from_reader(Cursor::new(augmented)).unwrap();
        assert!(gtfs.stops.contains_key("NWK:track:A"));
        assert_eq!(
            gtfs.stops["NWK"].parent_station.as_deref(),
            Some("NWK:station")
        );
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
