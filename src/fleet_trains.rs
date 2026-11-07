//! The train list the TripUpdates track poller reads instead of polling Amtrak.
//!
//! VehiclePositions is the only process that fetches `getTrainsData`. It writes this list
//! next to each fleet snapshot so the embedded track poller can pick trains near a stop
//! without a second map poll. Only matched trains appear: track stamps apply only to
//! matched trip updates.

use gtfs_realtime::FeedMessage;
use serde_json::{json, Value};

/// Builds `trains.json` from matched trip updates.
///
/// `train_number` maps a trip id to its public train number (`trips.txt`
/// `trip_short_name`). Each stop lists its arrival and departure instants as unix
/// seconds, preferring the predicted time the trip update carries. A trip with no
/// train number, no start date, or no timed stop is left out.
pub fn train_list(
    trip_updates: &FeedMessage,
    train_number: impl Fn(&str) -> Option<String>,
) -> Value {
    let mut trains = Vec::new();
    for entity in &trip_updates.entity {
        let Some(update) = entity.trip_update.as_ref() else {
            continue;
        };
        let Some(number) = update.trip.trip_id.as_deref().and_then(&train_number) else {
            continue;
        };
        let Some(service_date) = update.trip.start_date.as_deref().and_then(iso_date) else {
            continue;
        };
        let stops: Vec<Value> = update
            .stop_time_update
            .iter()
            .filter_map(|stop| {
                let code = stop.stop_id.as_deref()?.trim();
                let times: Vec<i64> = [stop.arrival.as_ref(), stop.departure.as_ref()]
                    .into_iter()
                    .flatten()
                    .filter_map(|event| event.time)
                    .collect();
                (!code.is_empty() && !times.is_empty())
                    .then(|| json!({ "code": code.to_ascii_uppercase(), "times_unix": times }))
            })
            .collect();
        if stops.is_empty() {
            continue;
        }
        trains.push(json!({
            "train_number": number,
            "service_date": service_date,
            "stops": stops,
        }));
    }
    json!({ "schema": "amtrak-fleet-trains/v1", "trains": trains })
}

/// `20261107` becomes `2026-11-07`; anything else is rejected.
fn iso_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (raw.len() == 8 && raw.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| format!("{}-{}-{}", &raw[..4], &raw[4..6], &raw[6..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtfs_realtime::trip_update::{StopTimeEvent, StopTimeUpdate};
    use gtfs_realtime::{FeedEntity, TripDescriptor, TripUpdate};

    fn event(time: i64) -> Option<StopTimeEvent> {
        Some(StopTimeEvent {
            time: Some(time),
            ..Default::default()
        })
    }

    fn update(trip_id: &str, start_date: &str, stops: Vec<StopTimeUpdate>) -> FeedEntity {
        FeedEntity {
            id: trip_id.into(),
            trip_update: Some(TripUpdate {
                trip: TripDescriptor {
                    trip_id: Some(trip_id.into()),
                    start_date: Some(start_date.into()),
                    ..Default::default()
                },
                stop_time_update: stops,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn stop(code: &str, arrival: Option<i64>, departure: Option<i64>) -> StopTimeUpdate {
        StopTimeUpdate {
            stop_id: Some(code.into()),
            arrival: arrival.and_then(event),
            departure: departure.and_then(event),
            ..Default::default()
        }
    }

    fn numbers(trip_id: &str) -> Option<String> {
        match trip_id {
            "trip-163" => Some("163".into()),
            "trip-66" => Some("66".into()),
            _ => None,
        }
    }

    #[test]
    fn lists_matched_trains_with_station_times() {
        let feed = FeedMessage {
            entity: vec![update(
                "trip-163",
                "20261107",
                vec![
                    stop("nhv", Some(1_794_081_000), Some(1_794_081_300)),
                    stop("NYP", Some(1_794_088_000), None),
                ],
            )],
            ..Default::default()
        };
        assert_eq!(
            train_list(&feed, numbers),
            json!({
                "schema": "amtrak-fleet-trains/v1",
                "trains": [{
                    "train_number": "163",
                    "service_date": "2026-11-07",
                    "stops": [
                        { "code": "NHV", "times_unix": [1_794_081_000, 1_794_081_300] },
                        { "code": "NYP", "times_unix": [1_794_088_000] },
                    ],
                }],
            })
        );
    }

    #[test]
    fn skips_trips_without_a_number_date_or_timed_stop() {
        let feed = FeedMessage {
            entity: vec![
                update("unknown-trip", "20261107", vec![stop("NYP", Some(1), None)]),
                update("trip-66", "11/07/2026", vec![stop("BOS", Some(1), None)]),
                update("trip-163", "20261107", vec![stop("NYP", None, None)]),
            ],
            ..Default::default()
        };
        assert_eq!(train_list(&feed, numbers)["trains"], json!([]));
    }
}
