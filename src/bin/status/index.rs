//! Join indexes over one loaded generation.
//!
//! Contract: [`FeedIndex::build`] scans a borrowed [`GenerationData`] once and builds the lookups
//! both query modes need, so a later station or train query is a hash lookup rather than a full
//! feed scan. It captures the non-obvious join keys explicitly:
//!
//! - `stop_by_code` — the realtime `StopTimeUpdate.stop_id` is the **Amtrak station code** (e.g.
//!   `NHV`), not the GTFS `stop_id`; stops are indexed by both, uppercased, so either resolves.
//! - `trips_by_number` — the Amtrak **train number** is the GTFS `trip_short_name`.
//! - `alerts_by_trip` — a service alert links to a train through `Alert.informed_entity[].trip`.
//!
//! An alert whose informed entity matches no loaded trip is collected in `unmatched_alerts` and
//! reported as a diagnostic, never silently dropped.

use super::source::GenerationData;
use gtfs_realtime::{Alert, TripUpdate, VehiclePosition};
use gtfs_structures::{Gtfs, Stop, Trip};
use std::collections::HashMap;

/// Precomputed lookups over one generation. Borrows the generation for its lifetime `'g`.
pub struct FeedIndex<'g> {
    /// Identity of the source generation (surfaced on results).
    pub generation_id: &'g str,
    /// Publication time of the source generation (surfaced on results).
    pub generated_at_unix: u64,
    /// The parsed static schedule, retained for route/shape/stop lookups.
    pub gtfs: &'g Gtfs,
    /// Uppercased Amtrak station code (and GTFS stop id) → matching stop(s).
    pub stop_by_code: HashMap<String, Vec<&'g Stop>>,
    /// Amtrak train number (`trip_short_name`) → the trip(s) that carry it.
    pub trips_by_number: HashMap<String, Vec<&'g Trip>>,
    /// Trip id → its realtime trip update (predicted stop times).
    pub update_by_trip: HashMap<String, &'g TripUpdate>,
    /// Trip id → its realtime vehicle position (live location).
    pub vehicle_by_trip: HashMap<String, &'g VehiclePosition>,
    /// Trip id → the service-alert texts affecting it.
    pub alerts_by_trip: HashMap<String, Vec<String>>,
    /// Alert texts whose informed entity matched no loaded trip (reported, not dropped).
    pub unmatched_alerts: Vec<String>,
}

impl<'g> FeedIndex<'g> {
    /// Builds every lookup from one borrowed generation, emitting a diagnostic per unmatched alert.
    pub fn build(data: &'g GenerationData) -> Self {
        let gtfs = &data.static_gtfs;

        let mut stop_by_code: HashMap<String, Vec<&Stop>> = HashMap::new();
        for stop in gtfs.stops.values() {
            let stop_ref: &Stop = stop.as_ref();
            stop_by_code
                .entry(stop_ref.id.to_uppercase())
                .or_default()
                .push(stop_ref);
            if let Some(code) = stop_ref.code.as_ref().filter(|c| !c.is_empty()) {
                stop_by_code
                    .entry(code.to_uppercase())
                    .or_default()
                    .push(stop_ref);
            }
        }

        let mut trips_by_number: HashMap<String, Vec<&Trip>> = HashMap::new();
        for trip in gtfs.trips.values() {
            if let Some(number) = trip.trip_short_name.as_ref().filter(|n| !n.is_empty()) {
                trips_by_number
                    .entry(number.clone())
                    .or_default()
                    .push(trip);
            }
        }

        let mut update_by_trip = HashMap::new();
        for entity in &data.trip_updates.entity {
            if let Some(update) = &entity.trip_update {
                if let Some(trip_id) = update.trip.trip_id.as_ref().filter(|t| !t.is_empty()) {
                    update_by_trip.insert(trip_id.clone(), update);
                }
            }
        }

        let mut vehicle_by_trip = HashMap::new();
        for entity in &data.vehicle_positions.entity {
            if let Some(vehicle) = &entity.vehicle {
                if let Some(trip_id) = vehicle
                    .trip
                    .as_ref()
                    .and_then(|trip| trip.trip_id.as_ref())
                    .filter(|t| !t.is_empty())
                {
                    vehicle_by_trip.insert(trip_id.clone(), vehicle);
                }
            }
        }

        let mut alerts_by_trip: HashMap<String, Vec<String>> = HashMap::new();
        let mut unmatched_alerts = Vec::new();
        for entity in &data.alerts.entity {
            if let Some(alert) = &entity.alert {
                let text = alert_text(alert);
                let mut matched = false;
                for selector in &alert.informed_entity {
                    if let Some(trip_id) = selector.trip.as_ref().and_then(|t| t.trip_id.as_ref()) {
                        if gtfs.trips.contains_key(trip_id) {
                            alerts_by_trip
                                .entry(trip_id.clone())
                                .or_default()
                                .push(text.clone());
                            matched = true;
                        }
                    }
                }
                if !matched {
                    unmatched_alerts.push(text);
                }
            }
        }
        for text in &unmatched_alerts {
            eprintln!(
                "status: alert not matched to any trip in generation {}: {}",
                data.generation_id,
                text.chars().take(80).collect::<String>()
            );
        }

        FeedIndex {
            generation_id: &data.generation_id,
            generated_at_unix: data.generated_at_unix,
            gtfs,
            stop_by_code,
            trips_by_number,
            update_by_trip,
            vehicle_by_trip,
            alerts_by_trip,
            unmatched_alerts,
        }
    }
}

/// Joins an alert's translated description into a single string (Amtrak provides one English text).
fn alert_text(alert: &Alert) -> String {
    alert
        .description_text
        .as_ref()
        .map(|translated| {
            translated
                .translation
                .iter()
                .map(|t| t.text.clone())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtfs_realtime::{
        translated_string::Translation, Alert, EntitySelector, FeedEntity, FeedMessage,
        TranslatedString, TripDescriptor,
    };
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    fn build_gtfs() -> Gtfs {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon,stop_timezone\nNHV,New Haven,41.3,-72.9,America/New_York\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,,Regional,2\n"),
            ("trips.txt", "route_id,service_id,trip_id,trip_short_name\nr,svc,t,199\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,NHV,1\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260817,1\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        Gtfs::from_reader(Cursor::new(archive.finish().unwrap().into_inner())).unwrap()
    }

    fn alert_entity(trip_id: Option<&str>, text: &str) -> FeedEntity {
        FeedEntity {
            id: "a".into(),
            alert: Some(Alert {
                informed_entity: vec![EntitySelector {
                    trip: trip_id.map(|t| TripDescriptor {
                        trip_id: Some(t.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                description_text: Some(TranslatedString {
                    translation: vec![Translation {
                        text: text.to_string(),
                        language: Some("en".to_string()),
                    }],
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn generation(alerts: Vec<FeedEntity>) -> GenerationData {
        GenerationData {
            generation_id: "g1".to_string(),
            generated_at_unix: 100,
            static_gtfs: build_gtfs(),
            trip_updates: FeedMessage::default(),
            vehicle_positions: FeedMessage::default(),
            alerts: FeedMessage {
                entity: alerts,
                ..Default::default()
            },
        }
    }

    // R1.3: index builds the join keys and never silently drops an unmatched alert.
    #[test]
    fn indexes_keys_and_collects_unmatched_alerts() {
        let data = generation(vec![
            alert_entity(Some("t"), "Train t delayed"),
            alert_entity(Some("ZZZ"), "Ghost-trip alert"),
            alert_entity(None, "No-trip alert"),
        ]);
        let index = FeedIndex::build(&data);

        assert!(index.stop_by_code.contains_key("NHV")); // Amtrak code join key
        assert!(index.trips_by_number.contains_key("199")); // train-number join key
        assert_eq!(
            index.alerts_by_trip.get("t").unwrap(),
            &vec!["Train t delayed".to_string()]
        );
        // The ghost-trip and no-trip alerts matched no loaded trip: kept, not dropped.
        assert_eq!(index.unmatched_alerts.len(), 2);
    }
}
