//! Live platform/track numbers for Amtrak trip updates.
//!
//! NJ Transit's RailData API and the CTrail Hartford Line board publish the track an Amtrak train
//! will use at stations those operators share with Amtrak. When `AMTRAK_TRACKS` is on, a background
//! [`refresher`] reads them into an expiring [`store`], and [`WithTracks`] stamps matching
//! stop-time updates during generation without making any network request itself.
//!
//! NJ Transit data is read only through a registered RailData account ([`raildata`]); the service
//! contains no copy of the DepartureVision web client's session handshake.

pub mod hartford;
pub mod raildata;
pub mod refresher;
pub mod store;

pub use store::{AssignmentStore, TrackBoard};

use super::{RtBatch, RtSource, SourceError};
use crate::static_augment::{platform_stop_id, PlatformTable};
use async_trait::async_trait;
use gtfs_realtime::trip_update::stop_time_update::ScheduleRelationship;
use gtfs_realtime::trip_update::StopTimeUpdate;
use gtfs_structures::{Gtfs, Trip};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// Decorator that stamps fresh track assignments onto the inner source's trip updates.
///
/// Fail-open: inner errors propagate unchanged, and missing or expired assignments leave the batch
/// as the inner source produced it. `fetch` reads the shared store only, so its latency does not
/// depend on the boards.
pub struct WithTracks<S> {
    inner: S,
    store: Arc<AssignmentStore>,
    table: Arc<PlatformTable>,
    max_age: Duration,
    unlisted: Mutex<HashSet<(String, String)>>,
}

impl<S> WithTracks<S> {
    /// Wraps `inner`, stamping tracks from `store` that are no older than `max_age` and that name
    /// a platform configured in `table` (and therefore present in the augmented static feed).
    pub fn new(
        inner: S,
        store: Arc<AssignmentStore>,
        table: Arc<PlatformTable>,
        max_age: Duration,
    ) -> Self {
        Self {
            inner,
            store,
            table,
            max_age,
            unlisted: Mutex::new(HashSet::new()),
        }
    }
}

#[async_trait]
impl<S: RtSource> RtSource for WithTracks<S> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn fetch(&self, gtfs: &Gtfs) -> Result<RtBatch, SourceError> {
        let mut batch = self.inner.fetch(gtfs).await?;
        let now = SystemTime::now();
        let board = self.store.fresh_board(now, self.max_age);
        if !board.is_empty() {
            apply_track_assignments(&mut batch, gtfs, &board, &self.table, now, &self.unlisted);
        }
        Ok(batch)
    }
}

/// Earliest predicted event, relative to generation time, that may still receive a track.
const EVENT_WINDOW_BEFORE: u64 = 3600;
/// Latest predicted event, relative to generation time, that may receive a track. Boards list only
/// imminent departures, so this also keeps the other run of a train that runs across two days
/// (same train number, a day apart) from receiving today's track.
const EVENT_WINDOW_AFTER: u64 = 12 * 3600;

/// Points matching stop times at the reported platform stop, as GTFS-Realtime requires.
///
/// A stamped stop time gets `assigned_stop_id = {stop}:track:{label}`, which is a real stop in the
/// augmented static feed and a sibling of the scheduled stop under one parent station. The
/// reference requires `stop_sequence` whenever `assigned_stop_id` is set and prefers `stop_id` to be
/// omitted (it must otherwise equal the assigned stop), so `stop_sequence` is filled from the
/// static trip and `stop_id` is cleared. A stop time is stamped only when it is not skipped, has a
/// predicted time inside the event window, visits its station exactly once in the trip, has a
/// board track for its train and station, and that track is configured; an unconfigured track is
/// logged once per station and label.
pub fn apply_track_assignments(
    batch: &mut RtBatch,
    gtfs: &Gtfs,
    board: &TrackBoard,
    table: &PlatformTable,
    now: SystemTime,
    unlisted: &Mutex<HashSet<(String, String)>>,
) {
    let now = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    for entity in &mut batch.trip_updates.entity {
        let Some(update) = entity.trip_update.as_mut() else {
            continue;
        };
        let Some(trip) = update
            .trip
            .trip_id
            .as_deref()
            .and_then(|trip_id| gtfs.trips.get(trip_id))
        else {
            continue;
        };
        let Some(train_number) = trip
            .trip_short_name
            .as_deref()
            .and_then(normalize_train_number)
        else {
            continue;
        };
        for stu in &mut update.stop_time_update {
            stamp_stop(stu, trip, gtfs, &train_number, board, table, now, unlisted);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stamp_stop(
    stu: &mut StopTimeUpdate,
    trip: &Trip,
    gtfs: &Gtfs,
    train_number: &str,
    board: &TrackBoard,
    table: &PlatformTable,
    now: u64,
    unlisted: &Mutex<HashSet<(String, String)>>,
) {
    let skipped = stu.schedule_relationship == Some(ScheduleRelationship::Skipped as i32);
    if skipped || !event_in_window(stu, now) {
        return;
    }
    let Some((stop_id, sequence)) = scheduled_visit(stu, trip, gtfs) else {
        return;
    };
    let Some(track) = board.track(train_number, &stop_id) else {
        return;
    };
    let platform = platform_stop_id(&stop_id, track);
    if !table.contains(&stop_id, track) || !gtfs.stops.contains_key(&platform) {
        let mut seen = unlisted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if seen.insert((stop_id.clone(), track.to_string())) {
            tracing::warn!(stop = %stop_id, track, "board track is not a configured platform; not published");
        }
        return;
    }
    stu.stop_sequence = Some(sequence);
    stu.stop_id = None;
    let mut properties = stu.stop_time_properties.take().unwrap_or_default();
    properties.assigned_stop_id = Some(platform);
    stu.stop_time_properties = Some(properties);
}

fn event_in_window(stu: &StopTimeUpdate, now: u64) -> bool {
    let earliest = now.saturating_sub(EVENT_WINDOW_BEFORE);
    let latest = now.saturating_add(EVENT_WINDOW_AFTER);
    [stu.arrival.as_ref(), stu.departure.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|event| event.time)
        .filter_map(|time| u64::try_from(time).ok())
        .any(|time| (earliest..=latest).contains(&time))
}

/// The scheduled stop and its `stop_sequence`, only when the station occurs once in the trip.
///
/// The occurrence check runs even when the update already carries `stop_sequence`: a board reading
/// names a train and a station, not a visit, so a trip that calls at the station twice is
/// ambiguous and stays unassigned.
fn scheduled_visit(stu: &StopTimeUpdate, trip: &Trip, gtfs: &Gtfs) -> Option<(String, u32)> {
    let stop_id = match (stu.stop_sequence, stu.stop_id.as_deref()) {
        (Some(sequence), referenced) => {
            let visit = trip
                .stop_times
                .iter()
                .find(|time| time.stop_sequence == sequence)?;
            if referenced.is_some_and(|id| id != visit.stop.id) {
                return None;
            }
            visit.stop.id.clone()
        }
        (None, Some(referenced)) => canonical_stop_id(gtfs, referenced)?,
        (None, None) => return None,
    };
    let mut visits = trip
        .stop_times
        .iter()
        .filter(|time| time.stop.id == stop_id);
    let sequence = visits.next()?.stop_sequence;
    if visits.next().is_some() {
        return None;
    }
    Some((stop_id, sequence))
}

fn canonical_stop_id(gtfs: &Gtfs, stop_id: &str) -> Option<String> {
    if let Some(stop) = gtfs.stops.get(stop_id) {
        return Some(stop.id.clone());
    }
    gtfs.stops
        .values()
        .find(|stop| {
            stop.id.eq_ignore_ascii_case(stop_id)
                || stop
                    .code
                    .as_deref()
                    .is_some_and(|code| code.eq_ignore_ascii_case(stop_id))
        })
        .map(|stop| stop.id.clone())
}

/// Platform token: `4`, `14`, `2A`, or a single letter. Words such as `TBD` are rejected.
pub fn is_track_label(track: &str) -> bool {
    let bytes = track.as_bytes();
    if bytes.is_empty() || bytes.len() > 4 || !bytes.iter().all(|byte| byte.is_ascii_alphanumeric())
    {
        return false;
    }
    let split_at = track
        .find(|character: char| character.is_ascii_uppercase())
        .unwrap_or(track.len());
    let (digits, letters) = track.split_at(split_at);
    if !letters
        .chars()
        .all(|character| character.is_ascii_uppercase())
        || letters.len() > 1
    {
        return false;
    }
    if digits.is_empty() {
        return letters.len() == 1;
    }
    digits.chars().all(|character| character.is_ascii_digit())
        && !digits.starts_with('0')
        && digits.len() <= 3
}

/// Normalizes an Amtrak train number. A single leading `A` is removed when the rest is numeric
/// (NJT's Amtrak prefix). Leading zeros are removed (`A067` and `066` both become `67`).
pub fn normalize_train_number(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let numeric = if let Some(rest) = trimmed
        .strip_prefix('A')
        .or_else(|| trimmed.strip_prefix('a'))
    {
        if !rest.is_empty() && rest.chars().all(|character| character.is_ascii_digit()) {
            rest
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    if !numeric.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let stripped = numeric.trim_start_matches('0');
    Some(if stripped.is_empty() {
        "0".to_string()
    } else {
        stripped.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::store::Assignment;
    use super::*;
    use crate::sources::mock::{Behavior, MockSource};
    use gtfs_realtime::{trip_update, FeedEntity, TripDescriptor, TripUpdate};
    use gtfs_structures::{RawStopTime, Stop, StopTime};

    #[test]
    fn train_numbers_strip_amtrak_prefix_and_leading_zeros() {
        assert_eq!(normalize_train_number("A67").as_deref(), Some("67"));
        assert_eq!(normalize_train_number("a067").as_deref(), Some("67"));
        assert_eq!(normalize_train_number("066").as_deref(), Some("66"));
        assert_eq!(normalize_train_number("141").as_deref(), Some("141"));
    }

    #[test]
    fn track_labels_accept_numbers_and_a_single_letter() {
        assert!(is_track_label("4"));
        assert!(is_track_label("14"));
        assert!(is_track_label("2A"));
        assert!(is_track_label("A"));
        assert!(!is_track_label(""));
        assert!(!is_track_label("0"));
        assert!(!is_track_label("TBD"));
        assert!(!is_track_label("12AB"));
    }

    const NOW: u64 = 1_800_000_000;

    fn stop(id: &str, parent: Option<&str>) -> Arc<Stop> {
        Arc::new(Stop {
            id: id.into(),
            code: Some(id.into()),
            parent_station: parent.map(str::to_string),
            ..Default::default()
        })
    }

    /// NWK (child of NWK:station) with platforms 1 and 4; trip 67 calls at NWK then NYP; trip 99
    /// calls at NWK twice.
    fn sample_gtfs() -> Gtfs {
        let mut gtfs = Gtfs::default();
        let nwk = stop("NWK", Some("NWK:station"));
        let nyp = stop("NYP", None);
        for stop in [
            nwk.clone(),
            nyp.clone(),
            stop("NWK:station", None),
            stop("NWK:track:1", Some("NWK:station")),
            stop("NWK:track:4", Some("NWK:station")),
        ] {
            gtfs.stops.insert(stop.id.clone(), stop);
        }
        let visit = |sequence: u32, stop: &Arc<Stop>| {
            StopTime::from(
                RawStopTime {
                    stop_sequence: sequence,
                    ..Default::default()
                },
                stop.clone(),
            )
        };
        gtfs.trips.insert(
            "trip-67".into(),
            Trip {
                id: "trip-67".into(),
                trip_short_name: Some("067".into()),
                stop_times: vec![visit(3, &nwk), visit(4, &nyp)],
                ..Default::default()
            },
        );
        gtfs.trips.insert(
            "trip-99".into(),
            Trip {
                id: "trip-99".into(),
                trip_short_name: Some("99".into()),
                stop_times: vec![visit(1, &nwk), visit(2, &nyp), visit(3, &nwk)],
                ..Default::default()
            },
        );
        gtfs
    }

    fn update(trip_id: &str, stus: Vec<StopTimeUpdate>) -> RtBatch {
        RtBatch {
            trip_updates: gtfs_realtime::FeedMessage {
                entity: vec![FeedEntity {
                    id: "e".into(),
                    trip_update: Some(TripUpdate {
                        trip: TripDescriptor {
                            trip_id: Some(trip_id.into()),
                            ..Default::default()
                        },
                        stop_time_update: stus,
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..RtBatch::empty()
        }
    }

    fn at(stop_id: Option<&str>, sequence: Option<u32>, time: i64) -> StopTimeUpdate {
        StopTimeUpdate {
            stop_id: stop_id.map(str::to_string),
            stop_sequence: sequence,
            departure: Some(trip_update::StopTimeEvent {
                time: Some(time),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn board(train: &str, stop: &str, track: &str) -> TrackBoard {
        let mut board = TrackBoard::default();
        board.insert(Assignment {
            train_number: train.into(),
            stop_id: stop.into(),
            track: track.into(),
        });
        board
    }

    fn table() -> PlatformTable {
        PlatformTable::parse("NWK=1,4,5").unwrap()
    }

    fn stamp(batch: &mut RtBatch, board: &TrackBoard) {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(NOW);
        apply_track_assignments(
            batch,
            &sample_gtfs(),
            board,
            &table(),
            now,
            &Mutex::default(),
        );
    }

    fn assigned(batch: &RtBatch, index: usize) -> &StopTimeUpdate {
        &batch.trip_updates.entity[0]
            .trip_update
            .as_ref()
            .unwrap()
            .stop_time_update[index]
    }

    fn assignment_of(stu: &StopTimeUpdate) -> Option<&str> {
        stu.stop_time_properties
            .as_ref()
            .and_then(|properties| properties.assigned_stop_id.as_deref())
    }

    #[test]
    fn a_fresh_track_points_at_the_platform_stop() {
        let mut batch = update("trip-67", vec![at(Some("nwk"), None, NOW as i64 + 600)]);
        stamp(&mut batch, &board("67", "NWK", "4"));
        let stu = assigned(&batch, 0);
        assert_eq!(assignment_of(stu), Some("NWK:track:4"));
        assert_eq!(stu.stop_sequence, Some(3));
        assert_eq!(stu.stop_id, None);
    }

    #[test]
    fn a_sequence_only_update_is_stamped() {
        let mut batch = update("trip-67", vec![at(None, Some(3), NOW as i64)]);
        stamp(&mut batch, &board("67", "NWK", "1"));
        assert_eq!(assignment_of(assigned(&batch, 0)), Some("NWK:track:1"));
    }

    #[test]
    fn a_station_visited_twice_is_never_stamped() {
        let mut batch = update(
            "trip-99",
            vec![
                at(Some("NWK"), Some(1), NOW as i64),
                at(None, Some(3), NOW as i64),
            ],
        );
        stamp(&mut batch, &board("99", "NWK", "4"));
        assert_eq!(assignment_of(assigned(&batch, 0)), None);
        assert_eq!(assignment_of(assigned(&batch, 1)), None);
        assert_eq!(assigned(&batch, 0).stop_id.as_deref(), Some("NWK"));
    }

    #[test]
    fn skipped_stops_are_left_alone() {
        let mut stu = at(Some("NWK"), Some(3), NOW as i64);
        stu.schedule_relationship = Some(ScheduleRelationship::Skipped as i32);
        let mut batch = update("trip-67", vec![stu]);
        stamp(&mut batch, &board("67", "NWK", "4"));
        assert_eq!(assignment_of(assigned(&batch, 0)), None);
    }

    #[test]
    fn events_outside_the_window_are_left_alone() {
        for time in [NOW as i64 - 3_601, NOW as i64 + 12 * 3600 + 1] {
            let mut batch = update("trip-67", vec![at(Some("NWK"), Some(3), time)]);
            stamp(&mut batch, &board("67", "NWK", "4"));
            assert_eq!(assignment_of(assigned(&batch, 0)), None, "{time}");
        }
        let mut no_time = at(Some("NWK"), Some(3), 0);
        no_time.departure = None;
        let mut batch = update("trip-67", vec![no_time]);
        stamp(&mut batch, &board("67", "NWK", "4"));
        assert_eq!(assignment_of(assigned(&batch, 0)), None);
    }

    #[test]
    fn unconfigured_or_unpublished_tracks_are_not_stamped() {
        // 9 is not in the table; 5 is in the table but its platform stop is not in the feed
        // (as when augmentation fell back to upstream bytes).
        for track in ["9", "5"] {
            let mut batch = update("trip-67", vec![at(Some("NWK"), Some(3), NOW as i64)]);
            stamp(&mut batch, &board("67", "NWK", track));
            let stu = assigned(&batch, 0);
            assert_eq!(assignment_of(stu), None, "{track}");
            assert_eq!(stu.stop_id.as_deref(), Some("NWK"));
        }
    }

    #[test]
    fn mismatched_stop_id_and_sequence_are_left_alone() {
        let mut batch = update("trip-67", vec![at(Some("NYP"), Some(3), NOW as i64)]);
        stamp(&mut batch, &board("67", "NWK", "4"));
        assert_eq!(assignment_of(assigned(&batch, 0)), None);
    }

    #[tokio::test]
    async fn the_decorator_reads_only_fresh_store_rows() {
        let gtfs = sample_gtfs();
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let batch = update("trip-67", vec![at(Some("NWK"), Some(3), now)]);
        let store = Arc::new(AssignmentStore::new());
        let source = |store: Arc<AssignmentStore>| {
            WithTracks::new(
                MockSource {
                    name: "amtrak",
                    behavior: Behavior::Ok(batch.clone()),
                },
                store,
                Arc::new(table()),
                Duration::from_secs(300),
            )
        };
        let row = Assignment {
            train_number: "67".into(),
            stop_id: "NWK".into(),
            track: "4".into(),
        };
        store.replace_station(
            "njt:NP",
            vec![row.clone()],
            SystemTime::now() - Duration::from_secs(301),
        );
        let stale = source(store.clone()).fetch(&gtfs).await.unwrap();
        assert_eq!(assignment_of(assigned(&stale, 0)), None);

        store.replace_station("njt:NP", vec![row], SystemTime::now());
        let decorated = source(store);
        let fresh = decorated.fetch(&gtfs).await.unwrap();
        assert_eq!(assignment_of(assigned(&fresh, 0)), Some("NWK:track:4"));
        assert_eq!(decorated.name(), "amtrak");
    }
}
