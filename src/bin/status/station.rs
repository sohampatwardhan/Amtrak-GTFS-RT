//! Station departures query: "what leaves this station next?"
//!
//! Contract: [`station_query`] resolves a station identifier (an Amtrak station code or a GTFS stop
//! id, case-insensitive) and returns the upcoming departures at it — every realtime stop update at
//! that station with a time at or after the query time, ordered soonest first. A terminating train
//! (arrival, no departure) is kept and labeled an arrival; a canceled stop is kept and labeled
//! canceled; a train's service alerts ride along on its row. An unresolvable identifier is reported
//! as `Unresolved` so it is never mistaken for "no departures".

use super::format::{route_display_name, station_tz};
use super::index::FeedIndex;
use chrono::{DateTime, LocalResult, TimeZone};
use chrono_tz::Tz;
use gtfs_realtime::trip_update::{StopTimeEvent, StopTimeUpdate};
use gtfs_realtime::TripUpdate;
use gtfs_structures::Gtfs;

/// Whether a board row is a departure or an arrival-only (terminating) stop.
#[derive(Debug, PartialEq, Eq)]
pub enum StopKind {
    Departure,
    Arrival,
}

/// One row of a station departures board.
pub struct DepartureRow {
    /// Predicted time of the event (Unix seconds).
    pub time_unix: i64,
    /// Departure, or arrival-only for a train terminating here.
    pub kind: StopKind,
    /// Whether the time is a real-time prediction (vs. a purely scheduled time).
    pub is_realtime: bool,
    /// Whether this stop is canceled for the train.
    pub canceled: bool,
    /// Delay in seconds at this stop (positive = late), when derivable — see [`stop_delay_secs`].
    pub delay_secs: Option<i64>,
    /// Friendly route name (e.g. `Acela`).
    pub route_name: String,
    /// Amtrak train number.
    pub train_number: String,
    /// Trip headsign.
    pub headsign: String,
    /// IANA timezone the time should be rendered in (the station's zone).
    pub station_tz: String,
    /// Whether `station_tz` is an agency fallback rather than the station's own zone.
    pub tz_is_fallback: bool,
    /// Service-alert texts affecting this train.
    pub alerts: Vec<String>,
    /// Platform/track: the `platform_code` of the stop time's assigned platform stop, if any.
    pub track: Option<String>,
}

/// Outcome of a station query.
pub enum StationResult {
    /// The station resolved; `rows` are its upcoming departures, soonest first.
    Board {
        generation_id: String,
        generated_at_unix: u64,
        station_code: String,
        station_name: String,
        rows: Vec<DepartureRow>,
    },
    /// The identifier matched no station.
    Unresolved { identifier: String },
}

/// Answers a station departures query against a loaded generation index.
pub fn station_query(index: &FeedIndex, identifier: &str, now_unix: i64) -> StationResult {
    let code = identifier.to_uppercase();
    let Some(stop) = index
        .stop_by_code
        .get(&code)
        .and_then(|stops| stops.first())
    else {
        return StationResult::Unresolved {
            identifier: identifier.to_string(),
        };
    };
    let station_name = stop.name.clone().unwrap_or_else(|| code.clone());
    let (station_tz, tz_is_fallback) = station_tz(index.gtfs, stop);

    let mut rows = Vec::new();
    for (trip_id, update) in &index.update_by_trip {
        for stu in &update.stop_time_update {
            let at_station = scheduled_stop_id(index.gtfs, trip_id, stu)
                .is_some_and(|stop_id| stop_id.to_uppercase() == code);
            if !at_station {
                continue;
            }
            let (kind, event) = match (&stu.departure, &stu.arrival) {
                (Some(dep), _) => (StopKind::Departure, Some(dep)),
                (None, Some(arr)) => (StopKind::Arrival, Some(arr)),
                (None, None) => continue,
            };
            let Some(time_unix) = event.and_then(|e| e.time).filter(|t| *t >= now_unix) else {
                continue;
            };
            let (route_name, train_number, headsign) = trip_meta(index.gtfs, trip_id, update);
            rows.push(DepartureRow {
                time_unix,
                kind,
                is_realtime: true,
                canceled: stu.schedule_relationship == Some(1),
                delay_secs: stop_delay_secs(index.gtfs, trip_id, stu, &station_tz),
                route_name,
                train_number,
                headsign,
                station_tz: station_tz.clone(),
                tz_is_fallback,
                alerts: index
                    .alerts_by_trip
                    .get(trip_id)
                    .cloned()
                    .unwrap_or_default(),
                track: track_from_update(index.gtfs, stu),
            });
        }
    }
    rows.sort_by_key(|row| row.time_unix);

    StationResult::Board {
        generation_id: index.generation_id.to_string(),
        generated_at_unix: index.generated_at_unix,
        station_code: code,
        station_name,
        rows,
    }
}

/// The stop a stop-time update refers to: its `stop_id`, or the static trip's stop at its
/// `stop_sequence` when `stop_id` is absent.
///
/// A platform assignment clears `stop_id` (GTFS-Realtime prefers only `stop_sequence` alongside
/// `assigned_stop_id`), so the sequence is authoritative whenever the id is missing; without this a
/// stamped stop would silently drop off its station's board.
pub(super) fn scheduled_stop_id(
    gtfs: &Gtfs,
    trip_id: &str,
    stu: &gtfs_realtime::trip_update::StopTimeUpdate,
) -> Option<String> {
    if let Some(stop_id) = stu.stop_id.as_deref() {
        return Some(stop_id.to_string());
    }
    let sequence = stu.stop_sequence?;
    gtfs.trips
        .get(trip_id)?
        .stop_times
        .iter()
        .find(|time| time.stop_sequence == sequence)
        .map(|time| time.stop.id.clone())
}

/// Delay in seconds for one stop-time update — positive when the train is late at that stop.
///
/// Derivation, in order:
/// 1. The event's own GTFS-Realtime `delay`, when the feed carries one.
/// 2. Predicted `time` minus the trip's scheduled time from the static GTFS.
///
/// Path 2 is the one that normally fires for Amtrak: its feed publishes predicted times but no
/// `delay` values, so without it no delay can ever be shown. The event a delay describes is the
/// first of departure, then arrival, that carries a delay or a time — the same event the
/// displayed time comes from — and the scheduled side uses that event's kind (a departure
/// prediction is compared with the scheduled departure).
///
/// GTFS scheduled times are seconds into the service day, so the scheduled instant is rebuilt
/// from the prediction's calendar date in the stop's timezone and then shifted by whole days
/// to the day nearest the prediction. The nearest-day shift is what keeps a past-midnight
/// schedule entry (`25:10:00`) or a prediction minutes before midnight from producing a
/// spurious ~24 h delay; it also bounds a derived delay to ±12 h.
pub(super) fn stop_delay_secs(
    gtfs: &Gtfs,
    trip_id: &str,
    stu: &StopTimeUpdate,
    tz_name: &str,
) -> Option<i64> {
    let (event, is_departure) = delay_event(stu)?;
    if let Some(delay) = event.delay {
        return Some(i64::from(delay));
    }
    let predicted = event.time?;
    let scheduled = scheduled_stop_time(gtfs, trip_id, stu)?;
    let scheduled_secs = if is_departure {
        scheduled.departure_time.or(scheduled.arrival_time)
    } else {
        scheduled.arrival_time.or(scheduled.departure_time)
    }?;
    let scheduled_unix = align_scheduled(predicted, scheduled_secs, tz_name)?;
    Some(predicted - scheduled_unix)
}

/// The stop-time event a delay describes: the first of departure, then arrival, that carries a
/// delay or a predicted time. An update whose events carry neither has no describable delay.
fn delay_event(stu: &StopTimeUpdate) -> Option<(&StopTimeEvent, bool)> {
    let bearing = |event: &StopTimeEvent| event.delay.is_some() || event.time.is_some();
    if let Some(departure) = &stu.departure {
        if bearing(departure) {
            return Some((departure, true));
        }
    }
    if let Some(arrival) = &stu.arrival {
        if bearing(arrival) {
            return Some((arrival, false));
        }
    }
    None
}

/// The trip's static stop time a stop-time update refers to: matched by `stop_sequence` when
/// the update carries one, else by the resolved stop id (or that stop's station code, for feeds
/// whose stop times key on codes rather than ids).
fn scheduled_stop_time<'a>(
    gtfs: &'a Gtfs,
    trip_id: &str,
    stu: &StopTimeUpdate,
) -> Option<&'a gtfs_structures::StopTime> {
    let trip = gtfs.trips.get(trip_id)?;
    if let Some(sequence) = stu.stop_sequence {
        if let Some(stop_time) = trip
            .stop_times
            .iter()
            .find(|time| time.stop_sequence == sequence)
        {
            return Some(stop_time);
        }
    }
    let stop_id = scheduled_stop_id(gtfs, trip_id, stu)?;
    trip.stop_times
        .iter()
        .find(|time| time.stop.id == stop_id)
        .or_else(|| {
            trip.stop_times
                .iter()
                .find(|time| time.stop.code.as_deref() == Some(stop_id.as_str()))
        })
}

/// Rebuilds the Unix instant of a GTFS scheduled time (`secs` into the service day, possibly
/// past 24:00) on the service day nearest `predicted_unix`, in the stop's timezone.
fn align_scheduled(predicted_unix: i64, scheduled_secs: u32, tz_name: &str) -> Option<i64> {
    let tz: Tz = tz_name.parse().ok()?;
    let date = DateTime::from_timestamp(predicted_unix, 0)?
        .with_timezone(&tz)
        .date_naive();
    let midnight = match tz.from_local_datetime(&date.and_hms_opt(0, 0, 0)?) {
        LocalResult::Single(instant) => instant.timestamp(),
        // A zone whose midnight repeats (fall-back at 00:00) starts its service day at the
        // earlier reading.
        LocalResult::Ambiguous(earlier, _) => earlier.timestamp(),
        LocalResult::None => return None,
    };
    let candidate = midnight + i64::from(scheduled_secs);
    // Shift by whole days so the scheduled instant is the one nearest the prediction.
    Some(candidate + (predicted_unix - candidate + 43_200).div_euclid(86_400) * 86_400)
}

/// Track shown for a stop time: the `platform_code` of its assigned platform stop in the static
/// feed, which is where the service publishes the track number.
pub(super) fn track_from_update(
    gtfs: &Gtfs,
    stu: &gtfs_realtime::trip_update::StopTimeUpdate,
) -> Option<String> {
    let assigned = stu
        .stop_time_properties
        .as_ref()?
        .assigned_stop_id
        .as_deref()?;
    gtfs.stops
        .get(assigned)?
        .platform_code
        .clone()
        .filter(|code| !code.trim().is_empty())
}

/// Resolves a trip's display route name, train number, and headsign, preferring the static schedule
/// and falling back to the realtime trip descriptor's route when the trip is not in the schedule.
fn trip_meta(gtfs: &Gtfs, trip_id: &str, update: &TripUpdate) -> (String, String, String) {
    match gtfs.trips.get(trip_id) {
        Some(trip) => (
            route_display_name(gtfs, &trip.route_id),
            trip.trip_short_name.clone().unwrap_or_default(),
            trip.trip_headsign.clone().unwrap_or_default(),
        ),
        None => (
            route_display_name(gtfs, update.trip.route_id.as_deref().unwrap_or_default()),
            String::new(),
            String::new(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::source::GenerationData;
    use super::*;
    use gtfs_realtime::{
        translated_string::Translation,
        trip_update::{StopTimeEvent, StopTimeUpdate},
        Alert, EntitySelector, FeedEntity, FeedMessage, TranslatedString, TripDescriptor,
        TripUpdate,
    };
    use gtfs_structures::Gtfs;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    const NOW: i64 = 1_000;

    fn build_gtfs() -> Gtfs {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon,stop_timezone,location_type,parent_station,platform_code\nNYP,New York Penn,40.75,-73.99,America/New_York,0,NYP:station,\nNYP:station,New York Penn,40.75,-73.99,America/New_York,1,,\nNYP:track:4,New York Penn Track 4,40.75,-73.99,America/New_York,0,NYP:station,4\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\n40751,a,,Acela,2\n88,a,,Northeast Regional,2\n"),
            ("trips.txt", "route_id,service_id,trip_id,trip_short_name,trip_headsign\n40751,svc,t_dep,2159,Washington\n88,svc,t_arr,171,New York\n40751,svc,t_cancel,2160,Washington\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt_dep,11:00:00,11:00:00,NYP,1\nt_arr,12:00:00,12:00:00,NYP,1\nt_cancel,13:00:00,13:00:00,NYP,1\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260817,1\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        Gtfs::from_reader(Cursor::new(archive.finish().unwrap().into_inner())).unwrap()
    }

    fn update(
        trip_id: &str,
        arrival: Option<i64>,
        departure: Option<i64>,
        canceled: bool,
    ) -> FeedEntity {
        let event = |time: Option<i64>| {
            time.map(|t| StopTimeEvent {
                time: Some(t),
                ..Default::default()
            })
        };
        FeedEntity {
            id: trip_id.into(),
            trip_update: Some(TripUpdate {
                trip: TripDescriptor {
                    trip_id: Some(trip_id.into()),
                    ..Default::default()
                },
                stop_time_update: vec![StopTimeUpdate {
                    stop_id: Some("NYP".into()),
                    arrival: event(arrival),
                    departure: event(departure),
                    schedule_relationship: canceled.then_some(1),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn alert(trip_id: &str, text: &str) -> FeedEntity {
        FeedEntity {
            id: format!("a-{trip_id}"),
            alert: Some(Alert {
                informed_entity: vec![EntitySelector {
                    trip: Some(TripDescriptor {
                        trip_id: Some(trip_id.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                description_text: Some(TranslatedString {
                    translation: vec![Translation {
                        text: text.into(),
                        language: Some("en".into()),
                    }],
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn generation() -> GenerationData {
        GenerationData {
            generation_id: "gS".into(),
            generated_at_unix: 777,
            static_gtfs: build_gtfs(),
            trip_updates: FeedMessage {
                entity: vec![
                    update("t_dep", Some(4_900), Some(5_000), false), // departure
                    update("t_arr", Some(4_000), None, false),        // arrival-only (terminus)
                    update("t_cancel", None, Some(6_000), true),      // canceled departure
                ],
                ..Default::default()
            },
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage {
                entity: vec![alert("t_dep", "Delay: 5 minutes late")],
                ..Default::default()
            },
        }
    }

    // R2.1/2.2/2.3/2.4/1.1/5.3: an ordered board with an arrival-only terminus, a canceled stop,
    // complete row fields, and an attached alert.
    #[test]
    fn board_is_ordered_and_labels_edge_cases() {
        let data = generation();
        let index = FeedIndex::build(&data);
        let StationResult::Board {
            generation_id,
            generated_at_unix,
            station_name,
            rows,
            ..
        } = station_query(&index, "nyp", NOW)
        else {
            panic!("expected a board");
        };
        assert_eq!(generation_id, "gS"); // R5.3
        assert_eq!(generated_at_unix, 777);
        assert_eq!(station_name, "New York Penn");
        assert_eq!(rows.len(), 3);

        // R2.1: soonest first — arrival(4000), departure(5000), canceled departure(6000).
        assert_eq!(rows[0].time_unix, 4_000);
        assert_eq!(rows[1].time_unix, 5_000);
        assert_eq!(rows[2].time_unix, 6_000);

        assert_eq!(rows[0].kind, StopKind::Arrival); // R2.2 terminus
        assert_eq!(rows[1].kind, StopKind::Departure);
        assert!(rows[2].canceled); // R2.4 kept + labeled

        // R2.3: complete row fields on the departure.
        assert_eq!(rows[1].route_name, "Acela");
        assert_eq!(rows[1].train_number, "2159");
        assert_eq!(rows[1].headsign, "Washington");
        assert!(rows[1].is_realtime);
        // R1.1: the departing train's alert rides on its row.
        assert_eq!(rows[1].alerts, vec!["Delay: 5 minutes late".to_string()]);
        assert_eq!(rows[1].track, None);
    }

    #[test]
    fn a_platform_assignment_keeps_its_station_and_shows_the_track() {
        use gtfs_realtime::trip_update::stop_time_update::StopTimeProperties;
        let mut data = generation();
        let stamped = &mut data.trip_updates.entity[0]
            .trip_update
            .as_mut()
            .unwrap()
            .stop_time_update[0];
        stamped.stop_id = None;
        stamped.stop_sequence = Some(1);
        stamped.stop_time_properties = Some(StopTimeProperties {
            assigned_stop_id: Some("NYP:track:4".into()),
            ..Default::default()
        });
        let index = FeedIndex::build(&data);
        let StationResult::Board { rows, .. } = station_query(&index, "nyp", NOW) else {
            panic!("expected a board");
        };
        let departure = rows.iter().find(|row| row.train_number == "2159").unwrap();
        assert_eq!(departure.track.as_deref(), Some("4"));
        assert_eq!(rows.len(), 3);

        let unknown = StopTimeUpdate {
            stop_time_properties: Some(StopTimeProperties {
                assigned_stop_id: Some("NYP:track:99".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(track_from_update(&data.static_gtfs, &unknown).is_none());
    }

    // Delay derivation: the feed carries a predicted time but no `delay`, so the row's delay is
    // derived against the static schedule — 11:00 EDT scheduled, 11:12 EDT predicted.
    #[test]
    fn board_row_delay_is_derived_from_the_schedule() {
        let data = GenerationData {
            generation_id: "gD".into(),
            generated_at_unix: 777,
            static_gtfs: build_gtfs(),
            trip_updates: FeedMessage {
                // 2026-08-17 15:12 UTC = 11:12 EDT; t_dep is scheduled out of NYP at 11:00.
                entity: vec![update("t_dep", None, Some(1_786_979_520), false)],
                ..Default::default()
            },
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage::default(),
        };
        let index = FeedIndex::build(&data);
        // Query at 2026-08-17 14:50 UTC, before the predicted departure.
        let StationResult::Board { rows, .. } = station_query(&index, "nyp", 1_786_978_200) else {
            panic!("expected a board");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].delay_secs, Some(720));
    }

    // R2.5: an unresolvable identifier is distinct from an empty board.
    #[test]
    fn unresolved_identifier_is_distinct() {
        let data = generation();
        let index = FeedIndex::build(&data);
        assert!(matches!(
            station_query(&index, "ZZZ", NOW),
            StationResult::Unresolved { .. }
        ));
    }
}
