//! Feed acquisition: obtain exactly one coherent immutable generation.
//!
//! Contract: a [`FeedSource`] yields one [`GenerationData`] whose static schedule, trip updates,
//! vehicle positions, and alerts all come from the *same* generation, or it fails without emitting
//! a partial result. Coherence is the whole point — a station board or train status computed from
//! mismatched schedule and prediction snapshots would be silently wrong.
//!
//! Two sources exist. [`LocalServiceSource`] reads the local feed service's published generation
//! (the default, honoring the immutable-generation contract). [`AmtrakDirectSource`] is a
//! dev/offline fallback that fetches static GTFS and the decrypted realtime feeds straight from
//! Amtrak. The HTTP surface is abstracted behind a private [`HttpGet`] trait so the failure paths
//! (`503` → unavailable, fetch/decode errors → fail-closed) are unit-testable without a socket.

use async_trait::async_trait;
use gtfs_realtime::FeedMessage;
use gtfs_structures::Gtfs;
use prost::Message;
use std::fmt;
use std::io::Cursor;
use std::time::{SystemTime, UNIX_EPOCH};

/// One coherent immutable generation: the static schedule plus the three realtime feeds, tagged
/// with the generation's identity and publication time so every derived answer can cite its source.
pub struct GenerationData {
    /// Opaque generation identifier from the producing service (or `amtrak-direct` for the fallback).
    pub generation_id: String,
    /// Unix seconds when the generation was published; surfaced on every result for staleness.
    pub generated_at_unix: u64,
    /// Parsed static GTFS schedule (stops, trips, routes, shapes, per-stop timezones).
    pub static_gtfs: Gtfs,
    /// Realtime trip updates (predicted stop times).
    pub trip_updates: FeedMessage,
    /// Realtime vehicle positions (live location).
    pub vehicle_positions: FeedMessage,
    /// Realtime service alerts (ASM delay/status messages).
    pub alerts: FeedMessage,
}

/// Closed, credential-free reason a generation could not be produced.
///
/// The variants stay coarse on purpose: messages never embed a URL, header, or credential, matching
/// the producing service's fail-closed style. `Unavailable` is distinct from `Fetch` so callers can
/// tell "no generation published yet" apart from "the request failed".
#[derive(Debug)]
pub enum SourceError {
    /// The service has no current generation (HTTP 503 / absent).
    Unavailable,
    /// A transport-level failure (request failed, non-200 status other than 503).
    Fetch(String),
    /// An artifact could not be decoded (bad zip or protobuf, malformed manifest).
    Decode(String),
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => write!(f, "no current generation is available"),
            Self::Fetch(reason) => write!(f, "feed fetch failed: {reason}"),
            Self::Decode(reason) => write!(f, "feed decode failed: {reason}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// Acquires exactly one coherent generation, or fails without a partial result.
#[async_trait]
pub trait FeedSource {
    /// Loads one immutable generation's four artifacts.
    async fn load(&self) -> Result<GenerationData, SourceError>;
}

/// Minimal HTTP GET used by [`LocalServiceSource`], abstracted so tests can inject canned
/// status/body pairs instead of standing up a server. Returns the status code and raw body.
#[async_trait]
trait HttpGet: Send + Sync {
    async fn get(&self, url: &str) -> Result<(u16, Vec<u8>), String>;
}

/// Production `HttpGet` over a shared `reqwest` client. Error text is fixed and credential-free so a
/// failing request can never leak the requested URL.
struct ReqwestGet(reqwest::Client);

#[async_trait]
impl HttpGet for ReqwestGet {
    async fn get(&self, url: &str) -> Result<(u16, Vec<u8>), String> {
        let response = self
            .0
            .get(url)
            .send()
            .await
            .map_err(|_| "request failed".to_string())?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .map_err(|_| "response body read failed".to_string())?
            .to_vec();
        Ok((status, bytes))
    }
}

/// Reads one immutable generation from the local feed service via its `/v1/feed-set.json` manifest.
pub struct LocalServiceSource {
    /// Base URL of the local service, e.g. `http://127.0.0.1:8080`.
    pub base_url: String,
    /// Shared HTTP client.
    pub client: reqwest::Client,
}

impl LocalServiceSource {
    /// Creates a source pointed at `base_url` with a fresh client.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl FeedSource for LocalServiceSource {
    async fn load(&self) -> Result<GenerationData, SourceError> {
        load_from_local(&self.base_url, &ReqwestGet(self.client.clone())).await
    }
}

/// Manifest fields the consumer needs: the generation identity and the four artifact URLs.
struct Manifest {
    generation_id: String,
    generated_at_unix: u64,
    static_zip: String,
    trip_updates: String,
    vehicle_positions: String,
    alerts: String,
}

/// Parses the `/v1/feed-set.json` body into the fields the consumer needs, or a `Decode` error
/// naming the missing/invalid field.
fn parse_manifest(body: &[u8]) -> Result<Manifest, SourceError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| SourceError::Decode("manifest is not JSON".into()))?;
    let string_field = |v: &serde_json::Value, key: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SourceError::Decode(format!("manifest missing {key}")))
    };
    let urls = value
        .get("urls")
        .ok_or_else(|| SourceError::Decode("manifest missing urls".into()))?;
    Ok(Manifest {
        generation_id: string_field(&value, "generation_id")?,
        generated_at_unix: value
            .get("generated_at_unix")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| SourceError::Decode("manifest missing generated_at_unix".into()))?,
        static_zip: string_field(urls, "static_zip")?,
        trip_updates: string_field(urls, "trip_updates")?,
        vehicle_positions: string_field(urls, "vehicle_positions")?,
        alerts: string_field(urls, "alerts")?,
    })
}

/// Resolves a possibly-relative artifact URL from the manifest against the service base URL.
fn resolve(base_url: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{}{}", base_url.trim_end_matches('/'), path)
    }
}

/// Decodes the four raw artifacts into a [`GenerationData`], mapping any failure to `Decode`.
fn decode_generation(
    generation_id: String,
    generated_at_unix: u64,
    static_zip: &[u8],
    trip_updates: &[u8],
    vehicle_positions: &[u8],
    alerts: &[u8],
) -> Result<GenerationData, SourceError> {
    let static_gtfs = Gtfs::from_reader(Cursor::new(static_zip.to_vec()))
        .map_err(|_| SourceError::Decode("static GTFS zip".into()))?;
    let decode_pb = |bytes: &[u8], name: &str| {
        FeedMessage::decode(bytes).map_err(|_| SourceError::Decode(format!("{name} protobuf")))
    };
    Ok(GenerationData {
        generation_id,
        generated_at_unix,
        static_gtfs,
        trip_updates: decode_pb(trip_updates, "trip-updates")?,
        vehicle_positions: decode_pb(vehicle_positions, "vehicle-positions")?,
        alerts: decode_pb(alerts, "alerts")?,
    })
}

/// Loads and decodes one generation from a local service reachable through `http`.
///
/// A `503` on the manifest is reported as [`SourceError::Unavailable`]; any other non-200, transport
/// failure, or decode failure fails closed with no partial result.
async fn load_from_local<G: HttpGet>(
    base_url: &str,
    http: &G,
) -> Result<GenerationData, SourceError> {
    let manifest_url = resolve(base_url, "/v1/feed-set.json");
    let (status, body) = http.get(&manifest_url).await.map_err(SourceError::Fetch)?;
    match status {
        200 => {}
        503 => return Err(SourceError::Unavailable),
        other => return Err(SourceError::Fetch(format!("manifest status {other}"))),
    }
    let manifest = parse_manifest(&body)?;

    let fetch = |path: String| async move {
        let (status, bytes) = http.get(&resolve(base_url, &path)).await.map_err(SourceError::Fetch)?;
        match status {
            200 => Ok(bytes),
            503 => Err(SourceError::Unavailable),
            other => Err(SourceError::Fetch(format!("artifact status {other}"))),
        }
    };
    let static_zip = fetch(manifest.static_zip).await?;
    let trip_updates = fetch(manifest.trip_updates).await?;
    let vehicle_positions = fetch(manifest.vehicle_positions).await?;
    let alerts = fetch(manifest.alerts).await?;

    decode_generation(
        manifest.generation_id,
        manifest.generated_at_unix,
        &static_zip,
        &trip_updates,
        &vehicle_positions,
        &alerts,
    )
}

/// Dev/offline fallback: fetches static GTFS and the decrypted realtime feeds directly from Amtrak.
///
/// Used only when explicitly selected (`--source amtrak`) and by the live verification test. It
/// synthesizes a generation with id `amtrak-direct` and the fetch time as the timestamp, since
/// Amtrak's upstream is not versioned into immutable generations.
pub struct AmtrakDirectSource {
    /// URL of Amtrak's static GTFS `.zip`.
    pub static_url: String,
    /// Shared HTTP client passed to the `amtrak-gtfs-rt` crate.
    pub client: reqwest::Client,
}

#[async_trait]
impl FeedSource for AmtrakDirectSource {
    async fn load(&self) -> Result<GenerationData, SourceError> {
        let static_gtfs = Gtfs::from_url_async(&self.static_url)
            .await
            .map_err(|_| SourceError::Fetch("static GTFS fetch failed".into()))?;
        let batch = amtrak_gtfs_rt::fetch_amtrak_gtfs_rt(&static_gtfs, &self.client)
            .await
            .map_err(|_| SourceError::Fetch("amtrak realtime fetch failed".into()))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        Ok(GenerationData {
            generation_id: "amtrak-direct".to_string(),
            generated_at_unix: now,
            static_gtfs,
            trip_updates: batch.trip_updates,
            vehicle_positions: batch.vehicle_positions,
            alerts: batch.alerts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Injectable HTTP stub mapping exact URLs to `(status, body)`; a missing URL is a fetch error.
    struct MockGet {
        responses: HashMap<String, (u16, Vec<u8>)>,
    }

    #[async_trait]
    impl HttpGet for MockGet {
        async fn get(&self, url: &str) -> Result<(u16, Vec<u8>), String> {
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| "no mock response".to_string())
        }
    }

    fn minimal_static_zip() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (name, contents) in [
            ("agency.txt", "agency_id,agency_name,agency_url,agency_timezone\na,Amtrak,https://amtrak.com,America/New_York\n"),
            ("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\ns,Station,42,-71\n"),
            ("routes.txt", "route_id,agency_id,route_short_name,route_long_name,route_type\nr,a,R,Regional,2\n"),
            ("trips.txt", "route_id,service_id,trip_id\nr,svc,t\n"),
            ("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nt,10:00:00,10:00:00,s,1\n"),
            ("calendar_dates.txt", "service_id,date,exception_type\nsvc,20260817,1\n"),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    fn empty_feed() -> Vec<u8> {
        FeedMessage::default().encode_to_vec()
    }

    fn manifest_json() -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "generation_id": "7-3",
            "generated_at_unix": 1_786_000_000_u64,
            "urls": {
                "static_zip": "/v1/generations/7-3/static.zip",
                "trip_updates": "/v1/generations/7-3/trip-updates.pb",
                "vehicle_positions": "/v1/generations/7-3/vehicle-positions.pb",
                "alerts": "/v1/generations/7-3/alerts.pb",
            }
        }))
        .unwrap()
    }

    fn full_mock() -> MockGet {
        let base = "http://svc";
        let mut responses = HashMap::new();
        responses.insert(format!("{base}/v1/feed-set.json"), (200, manifest_json()));
        responses.insert(format!("{base}/v1/generations/7-3/static.zip"), (200, minimal_static_zip()));
        responses.insert(format!("{base}/v1/generations/7-3/trip-updates.pb"), (200, empty_feed()));
        responses.insert(format!("{base}/v1/generations/7-3/vehicle-positions.pb"), (200, empty_feed()));
        responses.insert(format!("{base}/v1/generations/7-3/alerts.pb"), (200, empty_feed()));
        MockGet { responses }
    }

    // R5.1, R5.2: one manifest-identified generation loads all four artifacts coherently.
    #[tokio::test]
    async fn loads_one_generation_from_the_manifest() {
        let data = load_from_local("http://svc", &full_mock()).await.unwrap();
        assert_eq!(data.generation_id, "7-3");
        assert_eq!(data.generated_at_unix, 1_786_000_000);
        assert!(data.static_gtfs.stops.contains_key("s"));
    }

    // R5.4: an absent current generation (503 on the manifest) is Unavailable, not a partial result.
    #[tokio::test]
    async fn manifest_503_is_unavailable() {
        let mut mock = full_mock();
        mock.responses
            .insert("http://svc/v1/feed-set.json".into(), (503, Vec::new()));
        assert!(matches!(
            load_from_local("http://svc", &mock).await,
            Err(SourceError::Unavailable)
        ));
    }

    // R7.1: a failed artifact fetch fails closed (no partial GenerationData).
    #[tokio::test]
    async fn artifact_fetch_failure_fails_closed() {
        let mut mock = full_mock();
        mock.responses
            .remove("http://svc/v1/generations/7-3/alerts.pb");
        assert!(matches!(
            load_from_local("http://svc", &mock).await,
            Err(SourceError::Fetch(_))
        ));
    }

    // R7.1: an undecodable artifact fails closed with a Decode error.
    #[tokio::test]
    async fn artifact_decode_failure_fails_closed() {
        let mut mock = full_mock();
        mock.responses.insert(
            "http://svc/v1/generations/7-3/trip-updates.pb".into(),
            (200, b"not a protobuf".to_vec()),
        );
        assert!(matches!(
            load_from_local("http://svc", &mock).await,
            Err(SourceError::Decode(_))
        ));
    }
}
