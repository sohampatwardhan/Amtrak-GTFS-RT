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
    let Some(stop) = index.stop_by_code.get(&code).and_then(|stops| stops.first()) else {
        return StationResult::Unresolved {
            identifier: identifier.to_string(),
        };
    };
    let station_name = stop.name.clone().unwrap_or_else(|| code.clone());
    let (station_tz, tz_is_fallback) = station_tz(index.gtfs, stop);

    let mut rows = Vec::new();
    for (trip_id, update) in &index.update_by_trip {
        for stu in &update.stop_time_update {
            if stu.stop_id.as_deref().unwrap_or_default().to_uppercase() != code {
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
    use super::*;
    use super::super::source::GenerationData;
    use gtfs_realtime::{
        trip_update::{StopTimeEvent, StopTimeUpdate},
        translated_string::Translation,
        Alert, EntitySelector, FeedEntity, FeedMessage, TranslatedString, TripDescriptor, TripUpdate,
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
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon,stop_timezone\nNYP,New York Penn,40.75,-73.99,America/New_York\n"),
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

    fn update(trip_id: &str, arrival: Option<i64>, departure: Option<i64>, canceled: bool) -> FeedEntity {
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
        let StationResult::Board { generation_id, generated_at_unix, station_name, rows, .. } =
            station_query(&index, "nyp", NOW)
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
