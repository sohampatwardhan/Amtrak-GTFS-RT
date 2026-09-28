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
use async_trait::async_trait;
use gtfs_realtime::trip_update::stop_time_update::ScheduleRelationship;
use gtfs_realtime::trip_update::StopTimeUpdate;
use gtfs_structures::{Gtfs, Trip};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const TRACK_MARK: &str = ":track:";

/// Decorator that stamps fresh track assignments onto the inner source's trip updates.
///
/// Fail-open: inner errors propagate unchanged, and missing or expired assignments leave the batch
/// as the inner source produced it. `fetch` reads the shared store only, so its latency does not
/// depend on the boards.
pub struct WithTracks<S> {
    inner: S,
    store: Arc<AssignmentStore>,
    max_age: Duration,
}

impl<S> WithTracks<S> {
    /// Wraps `inner`, reading assignments no older than `max_age` from `store`.
    pub fn new(inner: S, store: Arc<AssignmentStore>, max_age: Duration) -> Self {
        Self {
            inner,
            store,
            max_age,
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
        let board = self.store.fresh_board(SystemTime::now(), self.max_age);
        if !board.is_empty() {
            apply_track_assignments(&mut batch, gtfs, &board);
        }
        Ok(batch)
    }
}

/// Writes `{stop_id}:track:{track}` onto stop times whose train and station match `board`.
///
/// The scheduled `stop_id` is kept. `stop_sequence` is filled from the static trip when it is
/// missing and the stop occurs once. Skipped stops are left alone: a platform assignment on a
/// skipped stop would fail validation and drop the update.
pub fn apply_track_assignments(batch: &mut RtBatch, gtfs: &Gtfs, board: &TrackBoard) {
    for entity in &mut batch.trip_updates.entity {
        let Some(update) = entity.trip_update.as_mut() else {
            continue;
        };
        let Some(trip_id) = update.trip.trip_id.as_deref() else {
            continue;
        };
        let Some(trip) = gtfs.trips.get(trip_id) else {
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
            stamp_stop(stu, trip, gtfs, &train_number, board);
        }
    }
}

fn stamp_stop(
    stu: &mut StopTimeUpdate,
    trip: &Trip,
    gtfs: &Gtfs,
    train_number: &str,
    board: &TrackBoard,
) {
    if !accepts_platform_assignment(stu) {
        return;
    }
    let Some(referenced) = referenced_stop_id(stu, trip) else {
        return;
    };
    let Some(canonical) = canonical_stop_id(gtfs, referenced) else {
        return;
    };
    let Some(track) = board.track(train_number, &canonical) else {
        return;
    };
    if stu.stop_sequence.is_none() {
        let sequences: Vec<_> = trip
            .stop_times
            .iter()
            .filter(|time| time.stop.id == canonical)
            .map(|time| time.stop_sequence)
            .collect();
        if sequences.len() != 1 {
            return;
        }
        stu.stop_sequence = sequences.first().copied();
    }
    let mut properties = stu.stop_time_properties.take().unwrap_or_default();
    properties.assigned_stop_id = Some(format!("{canonical}{TRACK_MARK}{track}"));
    stu.stop_time_properties = Some(properties);
}

fn accepts_platform_assignment(stu: &StopTimeUpdate) -> bool {
    let relationship = stu
        .schedule_relationship
        .unwrap_or(ScheduleRelationship::Scheduled as i32);
    if relationship == ScheduleRelationship::Skipped as i32 {
        return false;
    }
    let events = stu.arrival.is_some() || stu.departure.is_some();
    events || relationship == ScheduleRelationship::NoData as i32
}

fn referenced_stop_id<'a>(stu: &'a StopTimeUpdate, trip: &'a Trip) -> Option<&'a str> {
    if let Some(stop_id) = stu.stop_id.as_deref() {
        return Some(stop_id);
    }
    let sequence = stu.stop_sequence?;
    trip.stop_times
        .iter()
        .find(|time| time.stop_sequence == sequence)
        .map(|time| time.stop.id.as_str())
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

/// Whether `assigned_id` is a static stop or a `{scheduled_stop}:track:{label}` overlay.
///
/// A static `assigned_stop_id` must equal `stop_id` when both are set (GTFS-RT). The overlay may
/// differ: `stop_id` stays the scheduled station, which is what this feed's consumers already join on.
pub(crate) fn stop_assignment_is_valid(
    assigned_id: &str,
    update: &StopTimeUpdate,
    trip: &Trip,
    gtfs: &Gtfs,
) -> bool {
    if update.stop_sequence.is_none() {
        return false;
    }
    if gtfs.stops.contains_key(assigned_id) {
        return update
            .stop_id
            .as_ref()
            .is_none_or(|stop_id| stop_id == assigned_id);
    }
    let Some((station, _)) = split_track_assignment(assigned_id) else {
        return false;
    };
    if !gtfs.stops.contains_key(station) {
        return false;
    }
    let sequence_stop = update.stop_sequence.and_then(|sequence| {
        trip.stop_times
            .iter()
            .find(|time| time.stop_sequence == sequence)
            .map(|time| time.stop.id.as_str())
    });
    let scheduled = update.stop_id.as_deref().or(sequence_stop);
    scheduled == Some(station)
        && update
            .stop_id
            .as_ref()
            .is_none_or(|stop_id| stop_id == station)
}

/// Splits `{stop_id}:track:{label}` when the label is a platform token.
pub fn split_track_assignment(assigned: &str) -> Option<(&str, &str)> {
    let (station, track) = assigned.rsplit_once(TRACK_MARK)?;
    if station.is_empty() || !is_track_label(track) {
        return None;
    }
    Some((station, track))
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

    fn sample_gtfs() -> Gtfs {
        let mut gtfs = Gtfs::default();
        let stop = Arc::new(Stop {
            id: "NWK".into(),
            code: Some("NWK".into()),
            ..Default::default()
        });
        gtfs.stops.insert("NWK".into(), stop.clone());
        gtfs.trips.insert(
            "trip-67".into(),
            Trip {
                id: "trip-67".into(),
                trip_short_name: Some("067".into()),
                stop_times: vec![StopTime::from(
                    RawStopTime {
                        stop_sequence: 3,
                        ..Default::default()
                    },
                    stop,
                )],
                ..Default::default()
            },
        );
        gtfs
    }

    fn batch() -> RtBatch {
        RtBatch {
            trip_updates: gtfs_realtime::FeedMessage {
                entity: vec![FeedEntity {
                    id: "e".into(),
                    trip_update: Some(TripUpdate {
                        trip: TripDescriptor {
                            trip_id: Some("trip-67".into()),
                            ..Default::default()
                        },
                        stop_time_update: vec![StopTimeUpdate {
                            stop_id: Some("NWK".into()),
                            stop_sequence: Some(3),
                            arrival: Some(trip_update::StopTimeEvent {
                                time: Some(10),
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..RtBatch::empty()
        }
    }

    #[tokio::test]
    async fn an_empty_store_leaves_the_inner_batch_unchanged() {
        let gtfs = sample_gtfs();
        let inner = MockSource {
            name: "amtrak",
            behavior: Behavior::Ok(batch()),
        };
        let source = WithTracks::new(
            inner,
            Arc::new(AssignmentStore::new()),
            Duration::from_secs(300),
        );
        let fetched = source.fetch(&gtfs).await.unwrap();
        let stu = &fetched.trip_updates.entity[0]
            .trip_update
            .as_ref()
            .unwrap()
            .stop_time_update[0];
        assert!(stu.stop_time_properties.is_none());
        assert_eq!(source.name(), "amtrak");
    }
}
