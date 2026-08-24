//! Train-status query: "where is train N and how is it doing?"
//!
//! Contract: [`train_query`] resolves an Amtrak train number (GTFS `trip_short_name`) to the
//! trip(s) *active in the loaded generation* and reports each one's live status — current position,
//! remaining stops with predicted times, overall delay when the feed carries one, route/headsign,
//! its route geometry, and its service alerts. A number that resolves to no active trip is reported
//! as `NotRunning` (not an obscure failure); a number with several active trips returns them all,
//! each carrying origin/destination so they can be told apart.

use super::format::{agency_timezone, route_display_name, station_tz};
use super::index::FeedIndex;
use gtfs_structures::Trip;

/// One remaining stop on a train's journey, with predicted times in the stop's local zone.
pub struct StopStatus {
    /// Amtrak station code as carried in the realtime feed.
    pub stop_code: String,
    /// Human-readable station name (resolved from the static schedule).
    pub stop_name: String,
    /// Predicted arrival (Unix seconds), when the feed provides one.
    pub arrival_unix: Option<i64>,
    /// Predicted departure (Unix seconds), when the feed provides one.
    pub departure_unix: Option<i64>,
    /// Whether the stop is canceled for this train.
    pub canceled: bool,
    /// IANA timezone to render this stop's times in.
    pub tz: String,
}

/// Live status of one active trip serving a train number.
pub struct TrainStatus {
    /// Amtrak train number (`trip_short_name`).
    pub train_number: String,
    /// GTFS trip id of the active trip.
    pub trip_id: String,
    /// Friendly route name (e.g. `Acela`).
    pub route_name: String,
    /// Trip headsign.
    pub headsign: String,
    /// Scheduled origin station name (distinguishes same-number trips).
    pub origin: String,
    /// Scheduled destination station name (distinguishes same-number trips).
    pub destination: String,
    /// Current position as `(lat, lon)`, when a vehicle position is available.
    pub position: Option<(f64, f64)>,
    /// Overall delay in seconds, when derivable from the feed.
    pub overall_delay_secs: Option<i64>,
    /// Remaining stops (time at/after the query time), earliest first.
    pub remaining_stops: Vec<StopStatus>,
    /// Ordered `(lat, lon)` route path, when the trip references a known shape.
    pub shape: Option<Vec<(f64, f64)>>,
    /// True when the trip's shape is absent from the static schedule.
    pub shape_unavailable: bool,
    /// Service-alert texts affecting this train.
    pub alerts: Vec<String>,
}

/// Outcome of a train-number query.
pub enum TrainResult {
    /// One or more active trips found, tagged with the source generation.
    Trains {
        generation_id: String,
        generated_at_unix: u64,
        trains: Vec<TrainStatus>,
    },
    /// The number matched no trip active in this generation.
    NotRunning { train_number: String },
}

/// Answers a train-number query against a loaded generation index.
pub fn train_query(index: &FeedIndex, train_number: &str, now_unix: i64) -> TrainResult {
    let active: Vec<&Trip> = index
        .trips_by_number
        .get(train_number)
        .map(|trips| {
            trips
                .iter()
                .copied()
                .filter(|trip| {
                    index.update_by_trip.contains_key(&trip.id)
                        || index.vehicle_by_trip.contains_key(&trip.id)
                })
                .collect()
        })
        .unwrap_or_default();

    if active.is_empty() {
        return TrainResult::NotRunning {
            train_number: train_number.to_string(),
        };
    }

    TrainResult::Trains {
        generation_id: index.generation_id.to_string(),
        generated_at_unix: index.generated_at_unix,
        trains: active
            .into_iter()
            .map(|trip| build_status(index, trip, now_unix))
            .collect(),
    }
}

/// Assembles the live status for one active trip.
fn build_status(index: &FeedIndex, trip: &Trip, now_unix: i64) -> TrainStatus {
    let gtfs = index.gtfs;
    let (origin, destination) = endpoints(trip);

    let position = index
        .vehicle_by_trip
        .get(&trip.id)
        .and_then(|vehicle| vehicle.position.as_ref())
        .map(|p| (p.latitude as f64, p.longitude as f64));

    let mut overall_delay_secs = None;
    let mut remaining_stops = Vec::new();
    if let Some(update) = index.update_by_trip.get(&trip.id) {
        for stu in &update.stop_time_update {
            let arrival_unix = stu.arrival.as_ref().and_then(|e| e.time);
            let departure_unix = stu.departure.as_ref().and_then(|e| e.time);
            if overall_delay_secs.is_none() {
                overall_delay_secs = stu
                    .departure
                    .as_ref()
                    .and_then(|e| e.delay)
                    .or_else(|| stu.arrival.as_ref().and_then(|e| e.delay))
                    .map(i64::from);
            }
            let effective = departure_unix.or(arrival_unix);
            if effective.map(|t| t >= now_unix).unwrap_or(false) {
                let code = stu.stop_id.clone().unwrap_or_default();
                let (stop_name, tz) = resolve_stop(index, &code);
                remaining_stops.push(StopStatus {
                    stop_code: code,
                    stop_name,
                    arrival_unix,
                    departure_unix,
                    canceled: stu.schedule_relationship == Some(1),
                    tz,
                });
            }
        }
    }
    remaining_stops.sort_by_key(|s| s.departure_unix.or(s.arrival_unix).unwrap_or(i64::MAX));

    let (shape, shape_unavailable) = match trip
        .shape_id
        .as_ref()
        .and_then(|shape_id| gtfs.shapes.get(shape_id))
    {
        Some(points) => {
            let mut ordered: Vec<_> = points.iter().collect();
            ordered.sort_by_key(|p| p.sequence);
            (
                Some(ordered.into_iter().map(|p| (p.latitude, p.longitude)).collect()),
                false,
            )
        }
        None => (None, true),
    };

    TrainStatus {
        train_number: trip.trip_short_name.clone().unwrap_or_default(),
        trip_id: trip.id.clone(),
        route_name: route_display_name(gtfs, &trip.route_id),
        headsign: trip.trip_headsign.clone().unwrap_or_default(),
        origin,
        destination,
        position,
        overall_delay_secs,
        remaining_stops,
        shape,
        shape_unavailable,
        alerts: index
            .alerts_by_trip
            .get(&trip.id)
            .cloned()
            .unwrap_or_default(),
    }
}

/// Origin and destination station names from the trip's static stop sequence.
fn endpoints(trip: &Trip) -> (String, String) {
    let name = |st: &gtfs_structures::StopTime| st.stop.name.clone().unwrap_or_default();
    (
        trip.stop_times.first().map(name).unwrap_or_default(),
        trip.stop_times.last().map(name).unwrap_or_default(),
    )
}

/// Resolves an Amtrak station code to `(name, tz)` using the index, falling back to the code and
/// agency timezone if the station is unknown.
fn resolve_stop(index: &FeedIndex, code: &str) -> (String, String) {
    if let Some(stop) = index.stop_by_code.get(&code.to_uppercase()).and_then(|s| s.first()) {
        let name = stop.name.clone().unwrap_or_else(|| code.to_string());
        let (tz, _fallback) = station_tz(index.gtfs, stop);
        return (name, tz);
    }
    (code.to_string(), agency_timezone(index.gtfs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::source::GenerationData;
    use gtfs_realtime::{
        trip_update::{StopTimeEvent, StopTimeUpdate},
        translated_string::Translation,
        Alert, EntitySelector, FeedEntity, FeedMessage, Position, TranslatedString, TripDescriptor,
        TripUpdate, VehiclePosition,
    };
    use gtfs_structures::Gtfs;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    const NOW: i64 = 1_000;
    const FUTURE: i64 = 5_000;

    fn build_gtfs() -> Gtfs {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon,stop_timezone\nNHV,New Haven,41.3,-72.9,America/New_York\nNYP,New York Penn,40.75,-73.99,America/New_York\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\n40751,a,,Acela,2\n"),
            // t carries shape sh1; tb has no shape (exercises the missing-geometry path).
            ("trips.txt", "route_id,service_id,trip_id,trip_short_name,shape_id,trip_headsign\n40751,svc,t,2159,sh1,Washington\n40751,svc,tb,2159,,Washington\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,NHV,1\nt,11:00:00,11:00:00,NYP,2\ntb,10:00:00,10:00:00,NHV,1\ntb,11:00:00,11:00:00,NYP,2\n"),
            ("shapes.txt", "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence\nsh1,41.3,-72.9,1\nsh1,40.75,-73.99,2\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260817,1\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        Gtfs::from_reader(Cursor::new(archive.finish().unwrap().into_inner())).unwrap()
    }

    fn trip_update(trip_id: &str) -> FeedEntity {
        FeedEntity {
            id: trip_id.into(),
            trip_update: Some(TripUpdate {
                trip: TripDescriptor {
                    trip_id: Some(trip_id.into()),
                    route_id: Some("40751".into()),
                    ..Default::default()
                },
                stop_time_update: vec![StopTimeUpdate {
                    stop_id: Some("NYP".into()),
                    departure: Some(StopTimeEvent {
                        time: Some(FUTURE),
                        delay: Some(300),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn vehicle(trip_id: &str) -> FeedEntity {
        FeedEntity {
            id: format!("v-{trip_id}"),
            vehicle: Some(VehiclePosition {
                trip: Some(TripDescriptor {
                    trip_id: Some(trip_id.into()),
                    ..Default::default()
                }),
                position: Some(Position {
                    latitude: 41.05,
                    longitude: -73.5,
                    ..Default::default()
                }),
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
            generation_id: "g9".into(),
            generated_at_unix: 4242,
            static_gtfs: build_gtfs(),
            trip_updates: FeedMessage {
                entity: vec![trip_update("t"), trip_update("tb")],
                ..Default::default()
            },
            vehicle_positions: FeedMessage {
                entity: vec![vehicle("t")],
                ..Default::default()
            },
            alerts: FeedMessage {
                entity: vec![alert("t", "Operating 5 minutes late")],
                ..Default::default()
            },
        }
    }

    // R3.1/3.2/3.3/3.4/4.1/4.2/1.1/1.4/5.3: a full train answer with a duplicate same-number trip.
    #[test]
    fn train_status_is_complete_and_lists_all_active_trips() {
        let data = generation();
        let index = FeedIndex::build(&data);
        let result = train_query(&index, "2159", NOW);
        let TrainResult::Trains { generation_id, generated_at_unix, trains } = result else {
            panic!("expected active trains");
        };
        assert_eq!(generation_id, "g9"); // R5.3 source tagging
        assert_eq!(generated_at_unix, 4242);
        assert_eq!(trains.len(), 2); // R3.4 both active trips returned

        let with_shape = trains.iter().find(|t| t.trip_id == "t").unwrap();
        assert_eq!(with_shape.route_name, "Acela");
        let (lat, lon) = with_shape.position.unwrap(); // R3.1 (f32 feed -> f64, compare with tolerance)
        assert!((lat - 41.05).abs() < 1e-4 && (lon + 73.5).abs() < 1e-4);
        assert_eq!(with_shape.overall_delay_secs, Some(300)); // R3.3
        assert_eq!(with_shape.remaining_stops.len(), 1); // R3.2 upcoming NYP
        assert_eq!(with_shape.remaining_stops[0].stop_name, "New York Penn");
        assert_eq!(with_shape.shape.as_ref().unwrap().len(), 2); // R4.1 geometry
        assert!(!with_shape.shape_unavailable);
        assert_eq!(with_shape.alerts, vec!["Operating 5 minutes late".to_string()]); // R1.1

        // R4.2: the shapeless trip reports no geometry, flagged, with the rest intact.
        let no_shape = trains.iter().find(|t| t.trip_id == "tb").unwrap();
        assert!(no_shape.shape.is_none());
        assert!(no_shape.shape_unavailable);
        assert!(no_shape.position.is_none()); // no vehicle for tb
        assert!(no_shape.alerts.is_empty()); // R1.2 no alert -> still present, empty
    }

    // R3.5: an unknown/inactive number is reported as not running, not an error.
    #[test]
    fn unknown_train_number_is_not_running() {
        let data = generation();
        let index = FeedIndex::build(&data);
        assert!(matches!(
            train_query(&index, "9999", NOW),
            TrainResult::NotRunning { .. }
        ));
    }

    // R3.2: a stop already in the past is not listed as remaining.
    #[test]
    fn past_stops_are_excluded_from_remaining() {
        let data = generation();
        let index = FeedIndex::build(&data);
        // Query time after the NYP departure -> no remaining stops.
        let TrainResult::Trains { trains, .. } = train_query(&index, "2159", FUTURE + 1) else {
            panic!("expected trains");
        };
        assert!(trains.iter().all(|t| t.remaining_stops.is_empty()));
    }
}
