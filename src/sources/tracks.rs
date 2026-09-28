//! Live platform/track numbers for Amtrak trip updates.
//!
//! Amtrak's static GTFS publishes one stop per station and no platform children, so a spec
//! `assigned_stop_id` that pointed at a different static stop cannot be used. When
//! `AMTRAK_TRACKS` is on, matching stop-time updates keep their scheduled `stop_id` and gain
//! `stop_time_properties.assigned_stop_id` of the form `{stop_id}:track:{track}` (for example
//! `NWK:track:4`). The orchestrator accepts that form only when `{stop_id}` is the scheduled
//! stop. Predictions are left untouched when no track is known.
//!
//! Two public boards supply the tracks, both fail-open:
//!
//! - NJ Transit's DepartureVision JSON API (`raildata.njtransit.com`). Amtrak rows use a `TRAIN_ID`
//!   prefixed with `A` (`A67`, `A067`). The public session bootstrap is the same one the
//!   DepartureVision page uses. If it fails and `NJT_RAILDATA_USERNAME` / `NJT_RAILDATA_PASSWORD`
//!   are set, the official `getTrainScheduleJSON19Rec` method is tried instead.
//! - The Hartford Line connecting-train board at New Haven Union (`NHV`), parsed from the public
//!   HTML table. Rows whose service is `Amtrak` contribute their train number and track.
//!
//! The Rust image does not gain a browser. A fetch or parse failure keeps the last good board, or
//! none, and the inner realtime batch still publishes.

use super::{RtBatch, RtSource, SourceError};
use crate::config::TrackConfig;
use aes::Aes192;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use cipher::BlockEncrypt;
use gtfs_realtime::trip_update::stop_time_update::ScheduleRelationship;
use gtfs_realtime::trip_update::StopTimeUpdate;
use gtfs_structures::{Gtfs, Trip};
use pbkdf2::pbkdf2_hmac;
use scraper::{Html, Selector};
use serde_json::Value;
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

/// AES-192 key shipped in the public DepartureVision client. Not an operator secret.
const SPA_AES_KEY: &str = "$AZPcAwSNIgcPCEoTSa0OAw#";
/// PBKDF2 password shipped in the public DepartureVision client (`ry.js`). Not an operator secret.
const SPA_PBKDF2_PASSWORD: &str = "63hsyd653gs6c8h3id8bqn";
const TRACK_MARK: &str = ":track:";

/// One train's track at one Amtrak stop.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Assignment {
    train_number: String,
    stop_id: String,
    track: String,
}

/// Lookup of normalized train number + uppercase Amtrak stop id → track label.
#[derive(Clone, Debug, Default)]
pub struct TrackBoard {
    assignments: HashMap<(String, String), String>,
}

impl TrackBoard {
    fn insert(&mut self, assignment: Assignment) {
        self.assignments.insert(
            (assignment.train_number, assignment.stop_id.to_uppercase()),
            assignment.track,
        );
    }

    fn track(&self, train_number: &str, stop_id: &str) -> Option<&str> {
        self.assignments
            .get(&(train_number.to_string(), stop_id.to_uppercase()))
            .map(String::as_str)
    }

    fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }
}

#[derive(Clone)]
struct CachedBoards {
    fetched_at: Instant,
    njt: Vec<Assignment>,
    hartford: Vec<Assignment>,
    njt_token: Option<String>,
}

/// Decorator that stamps live tracks onto the inner source's trip updates.
///
/// Fail-open: inner errors propagate, and a board failure never does.
pub struct WithTracks<S> {
    inner: S,
    client: reqwest::Client,
    config: TrackConfig,
    cache: Mutex<Option<CachedBoards>>,
}

impl<S> WithTracks<S> {
    /// Wraps `inner`. Board fetches are skipped until `AMTRAK_TRACKS` is enabled; callers should
    /// not construct this wrapper when the feature is off.
    pub fn new(inner: S, config: TrackConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("amtrak-gtfs-rt-tracks")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner,
            client,
            config,
            cache: Mutex::new(None),
        }
    }

    async fn current_board(&self) -> TrackBoard {
        let previous = {
            let guard = self.cache.lock().await;
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < self.config.ttl {
                    return board_from(&cached.njt, &cached.hartford);
                }
            }
            guard.as_ref().cloned()
        };
        let (njt, hartford, token) = self.refresh(previous.as_ref()).await;
        let board = board_from(&njt, &hartford);
        *self.cache.lock().await = Some(CachedBoards {
            fetched_at: Instant::now(),
            njt,
            hartford,
            njt_token: token,
        });
        board
    }

    async fn refresh(
        &self,
        previous: Option<&CachedBoards>,
    ) -> (Vec<Assignment>, Vec<Assignment>, Option<String>) {
        let mut token = previous.and_then(|cached| cached.njt_token.clone());
        let previous_njt = previous.map(|cached| cached.njt.as_slice()).unwrap_or(&[]);
        let njt = match self.fetch_njt(&mut token, previous_njt).await {
            Ok(rows) => rows,
            Err(()) => {
                tracing::warn!("njt track board unavailable; keeping last assignments");
                previous
                    .map(|cached| cached.njt.clone())
                    .unwrap_or_default()
            }
        };
        let hartford = match self.fetch_hartford().await {
            Ok(rows) => rows,
            Err(()) => {
                tracing::warn!("hartford track board unavailable; keeping last assignments");
                previous
                    .map(|cached| cached.hartford.clone())
                    .unwrap_or_default()
            }
        };
        (njt, hartford, token)
    }

    async fn fetch_njt(
        &self,
        token: &mut Option<String>,
        previous: &[Assignment],
    ) -> Result<Vec<Assignment>, ()> {
        if self.config.njt_stations.is_empty() {
            return Ok(Vec::new());
        }
        if let Ok(rows) = self.fetch_njt_public(token, previous).await {
            return Ok(rows);
        }
        if self.config.njt_username.is_some() {
            return self.fetch_njt_official(previous).await;
        }
        Err(())
    }

    async fn fetch_njt_public(
        &self,
        token: &mut Option<String>,
        previous: &[Assignment],
    ) -> Result<Vec<Assignment>, ()> {
        if token.is_none() {
            *token = Some(self.bootstrap_token().await?);
        }
        match self
            .public_schedules(token.as_deref().unwrap_or_default(), previous)
            .await
        {
            Ok(rows) => Ok(rows),
            Err(()) => {
                *token = Some(self.bootstrap_token().await?);
                self.public_schedules(token.as_deref().unwrap_or_default(), previous)
                    .await
            }
        }
    }

    async fn bootstrap_token(&self) -> Result<String, ()> {
        let origin = self.config.njt_spa_origin.trim_end_matches('/');
        let script = http_text(&self.client, &format!("{origin}/block1.js")).await?;
        let blob = quoted_dotted_blob(&script).ok_or(())?;
        let plain = decrypt_spa_blob(blob).ok_or(())?;
        let challenge = spa_challenge_plain(&plain, &iso8601_millis(SystemTime::now())).ok_or(())?;
        let cipher = aes192_ecb_base64(SPA_AES_KEY.as_bytes(), challenge.as_bytes()).ok_or(())?;
        let (content_type, body) = multipart_body(&[("BaseInfo", &cipher)]);
        let base = self.config.njt_api_base.trim_end_matches('/');
        let response = http_post(
            &self.client,
            &format!("{base}/Signdata/getBaseInfo2"),
            &content_type,
            body,
        )
        .await?;
        let value: Value = serde_json::from_str(&response).map_err(|_| ())?;
        let authenticated = value
            .get("Authenticated")
            .and_then(|item| item.as_str())
            .unwrap_or("");
        if !authenticated.eq_ignore_ascii_case("true") {
            return Err(());
        }
        value
            .get("UserToken")
            .and_then(|item| item.as_str())
            .filter(|token| !token.is_empty())
            .map(str::to_string)
            .ok_or(())
    }

    async fn public_schedules(
        &self,
        token: &str,
        previous: &[Assignment],
    ) -> Result<Vec<Assignment>, ()> {
        let mut rows = Vec::new();
        let mut any_ok = false;
        for station in &self.config.njt_stations {
            match self.public_station(token, station).await {
                Ok(mut station_rows) => {
                    any_ok = true;
                    rows.append(&mut station_rows);
                }
                Err(()) => {
                    tracing::warn!("njt station track fetch failed");
                    rows.extend(remembered_for_station(
                        previous,
                        &self.config.station_map,
                        station,
                    ));
                }
            }
        }
        if any_ok {
            Ok(rows)
        } else {
            Err(())
        }
    }

    async fn public_station(&self, token: &str, station: &str) -> Result<Vec<Assignment>, ()> {
        let (content_type, body) = multipart_body(&[("token", token), ("station", station)]);
        let base = self.config.njt_api_base.trim_end_matches('/');
        let response = http_post(
            &self.client,
            &format!("{base}/TrainData/getTrainSchedule19Rec"),
            &content_type,
            body,
        )
        .await?;
        assignments_for_station(&response, station, &self.config.station_map)
    }

    async fn fetch_njt_official(&self, previous: &[Assignment]) -> Result<Vec<Assignment>, ()> {
        let Some(username) = self.config.njt_username.as_deref() else {
            return Err(());
        };
        let Some(password) = self.config.njt_password.as_deref() else {
            return Err(());
        };
        let mut rows = Vec::new();
        let mut any_ok = false;
        for station in &self.config.njt_stations {
            let body = form_encode(&[
                ("username", username),
                ("password", password),
                ("station", station),
            ]);
            match http_post(
                &self.client,
                &self.config.njt_official_url,
                "application/x-www-form-urlencoded",
                body,
            )
            .await
            {
                Ok(response) => {
                    match assignments_for_station(&response, station, &self.config.station_map) {
                        Ok(mut station_rows) => {
                            any_ok = true;
                            rows.append(&mut station_rows);
                        }
                        Err(()) => {
                            tracing::warn!("njt official track response was unusable");
                            rows.extend(remembered_for_station(
                                previous,
                                &self.config.station_map,
                                station,
                            ));
                        }
                    }
                }
                Err(()) => {
                    tracing::warn!("njt official track fetch failed");
                    rows.extend(remembered_for_station(
                        previous,
                        &self.config.station_map,
                        station,
                    ));
                }
            }
        }
        if any_ok {
            Ok(rows)
        } else {
            Err(())
        }
    }

    async fn fetch_hartford(&self) -> Result<Vec<Assignment>, ()> {
        let Some(url) = self.config.hartford_url.as_deref() else {
            return Ok(Vec::new());
        };
        let html = http_text(&self.client, url).await?;
        if !html.contains("status-table") {
            return Err(());
        }
        Ok(parse_hartford_board(&html)
            .into_iter()
            .map(|(train_number, track)| Assignment {
                train_number,
                stop_id: self.config.hartford_stop_id.clone(),
                track,
            })
            .collect())
    }
}

fn remembered_for_station(
    previous: &[Assignment],
    station_map: &[(String, String)],
    station: &str,
) -> Vec<Assignment> {
    let Some(stop_id) = station_map.iter().find_map(|(code, amtrak)| {
        code.eq_ignore_ascii_case(station)
            .then_some(amtrak.as_str())
    }) else {
        return Vec::new();
    };
    previous
        .iter()
        .filter(|row| row.stop_id.eq_ignore_ascii_case(stop_id))
        .cloned()
        .collect()
}

fn board_from(njt: &[Assignment], hartford: &[Assignment]) -> TrackBoard {
    let mut board = TrackBoard::default();
    for assignment in njt.iter().chain(hartford.iter()) {
        board.insert(assignment.clone());
    }
    board
}

#[async_trait]
impl<S: RtSource> RtSource for WithTracks<S> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn fetch(&self, gtfs: &Gtfs) -> Result<RtBatch, SourceError> {
        let mut batch = self.inner.fetch(gtfs).await?;
        let board = self.current_board().await;
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

/// Amtrak departures from one NJT schedule document, keyed by the configured Amtrak stop.
fn assignments_for_station(
    body: &str,
    njt_station: &str,
    station_map: &[(String, String)],
) -> Result<Vec<Assignment>, ()> {
    let Some(stop_id) = station_map.iter().find_map(|(code, amtrak)| {
        (code.eq_ignore_ascii_case(njt_station)).then_some(amtrak.clone())
    }) else {
        return Err(());
    };
    let value = schedule_value(body)?;
    let items = items_of(&value).ok_or(())?;
    let mut rows = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(train_id) = object_field(object, "TRAIN_ID") else {
            continue;
        };
        let Some(train_number) = amtrak_number_from_njt(train_id) else {
            continue;
        };
        let Some(track) = object_field(object, "TRACK") else {
            continue;
        };
        let track = track.trim().to_uppercase();
        if !is_track_label(&track) {
            continue;
        }
        rows.push(Assignment {
            train_number,
            stop_id: stop_id.clone(),
            track,
        });
    }
    Ok(rows)
}

fn amtrak_number_from_njt(train_id: &str) -> Option<String> {
    let trimmed = train_id.trim();
    let rest = trimmed
        .strip_prefix('A')
        .or_else(|| trimmed.strip_prefix('a'))?;
    if rest.is_empty() || !rest.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    normalize_train_number(rest)
}

/// Amtrak rows from the Hartford Line connecting-train table: `(train number, track)`.
pub fn parse_hartford_board(html: &str) -> Vec<(String, String)> {
    let document = Html::parse_document(html);
    let Ok(rows) = Selector::parse("#status-table tbody tr") else {
        return Vec::new();
    };
    let Ok(cells) = Selector::parse("td") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in document.select(&rows) {
        let fields: Vec<String> = row
            .select(&cells)
            .map(|cell| {
                cell.text()
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        if fields.len() < 6 || !fields[1].eq_ignore_ascii_case("amtrak") {
            continue;
        }
        let Some(train_number) = normalize_train_number(&fields[2]) else {
            continue;
        };
        let track = fields[5].trim().to_uppercase();
        if !is_track_label(&track) {
            continue;
        }
        out.push((train_number, track));
    }
    out
}

fn schedule_value(body: &str) -> Result<Value, ()> {
    let text = json_text(body).ok_or(())?;
    let mut value: Value = serde_json::from_str(&text).map_err(|_| ())?;
    if items_of(&value).is_none() {
        if let Some(object) = value.as_object() {
            for nested in object.values() {
                let Some(text) = nested.as_str() else {
                    continue;
                };
                if let Ok(inner) = serde_json::from_str::<Value>(text) {
                    if items_of(&inner).is_some() {
                        value = inner;
                        break;
                    }
                }
            }
        }
    }
    if value
        .get("errorMessage")
        .and_then(|item| item.as_str())
        .is_some()
    {
        return Err(());
    }
    Ok(value)
}

fn items_of(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Array(items) => Some(items),
        Value::Object(object) => object
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("items"))
            .and_then(|(_, item)| item.as_array()),
        _ => None,
    }
}

fn object_field<'a>(object: &'a serde_json::Map<String, Value>, name: &str) -> Option<&'a str> {
    object
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.as_str())
}

fn json_text(body: &str) -> Option<String> {
    let trimmed = body.trim();
    let slice = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        trimmed
    } else {
        let start = trimmed.find(['{', '['])?;
        let end = trimmed.rfind(['}', ']'])?;
        if end < start {
            return None;
        }
        &trimmed[start..=end]
    };
    Some(unescape_xml(slice))
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn quoted_dotted_blob(script: &str) -> Option<&str> {
    let mut rest = script;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let end = after.find('"')?;
        let candidate = &after[..end];
        if candidate.matches('.').count() == 3 {
            return Some(candidate);
        }
        rest = &after[end + 1..];
    }
    None
}

#[allow(deprecated)] // cipher 0.4 still types nonces as generic-array 0.14.
fn decrypt_spa_blob(blob: &str) -> Option<String> {
    let parts: Vec<&str> = blob.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let iterations_text = String::from_utf8(b64_decode(parts[0])?).ok()?;
    let iterations: u32 = iterations_text.parse().ok()?;
    let salt = b64_decode(parts[1])?;
    let iv = b64_decode(parts[2])?;
    let ciphertext = b64_decode(parts[3])?;
    let mut key = [0_u8; 32];
    pbkdf2_hmac::<Sha256>(SPA_PBKDF2_PASSWORD.as_bytes(), &salt, iterations, &mut key);
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let nonce = Nonce::from_slice(&iv);
    let plain = cipher.decrypt(nonce, ciphertext.as_ref()).ok()?;
    String::from_utf8(plain).ok()
}

fn spa_challenge_plain(decoded: &str, timestamp: &str) -> Option<String> {
    let words: Vec<&str> = decoded.split_whitespace().collect();
    if words.len() < 7 {
        return None;
    }
    Some(format!(
        "timestamp=={timestamp}||{}=={}||{}=={}||",
        words[1], words[4], words[2], words[6]
    ))
}

#[allow(deprecated)] // cipher 0.4 still types blocks as generic-array 0.14.
fn aes192_ecb_base64(key: &[u8], plaintext: &[u8]) -> Option<String> {
    if key.len() != 24 {
        return None;
    }
    let cipher = Aes192::new_from_slice(key).ok()?;
    let mut padded = pkcs7_pad(plaintext, 16);
    let (chunks, _) = padded.as_chunks_mut::<16>();
    for chunk in chunks {
        let block = cipher::generic_array::GenericArray::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }
    Some(STANDARD.encode(padded))
}

fn pkcs7_pad(data: &[u8], block: usize) -> Vec<u8> {
    let pad = block - (data.len() % block);
    let mut out = Vec::with_capacity(data.len() + pad);
    out.extend_from_slice(data);
    out.extend(std::iter::repeat_n(u8::try_from(pad).unwrap_or(16), pad));
    out
}

fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let mut padded = input.trim().replace('-', "+").replace('_', "/");
    match STANDARD.decode(&padded) {
        Ok(bytes) => Some(bytes),
        Err(_) => {
            while !padded.len().is_multiple_of(4) {
                padded.push('=');
            }
            STANDARD.decode(padded).ok()
        }
    }
}

fn iso8601_millis(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (year, month, day, hour, minute, second) = civil_utc(duration.as_secs());
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    )
}

/// Howard Hinnant's civil-from-days conversion, UTC.
fn civil_utc(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let tod = u32::try_from(secs % 86_400).unwrap_or(0);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = u64::try_from(z - era * 146_097).unwrap_or(0);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let mut year = i64::try_from(yoe).unwrap_or(0) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    (
        i32::try_from(year).unwrap_or(1970),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
        hour,
        minute,
        second,
    )
}

fn multipart_body(fields: &[(&str, &str)]) -> (String, String) {
    let boundary = "amtraktrackboundary";
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

async fn http_text(client: &reqwest::Client, url: &str) -> Result<String, ()> {
    let response = client.get(url).send().await.map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    response.text().await.map_err(|_| ())
}

async fn http_post(
    client: &reqwest::Client,
    url: &str,
    content_type: &str,
    body: String,
) -> Result<String, ()> {
    let response = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body)
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    response.text().await.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{Behavior, MockSource};
    use gtfs_realtime::{trip_update, FeedEntity, TripDescriptor, TripUpdate};
    use gtfs_structures::{RawStopTime, Stop, StopTime};
    use std::sync::Arc;

    const AES_VECTOR: &str = "MdhGzS3ZKM97bBsPxAEJLIrDcK0+ncgQ1ZWUGjlLpUMxapaykTesEsAY6+Hq4Z+so4JXQK4OPYtMrI1h4mYghlJ+ixWxZYhbPNc4U7je0kM=";
    const SPA_BLOB: &str = "NTAwMDAw.2yqe14dapBz99cNILfXMmgZ5Enc1DP+zmGZF3q0NX9Y=.87H+RjrKvgMVjl8k.50sVtNjn4tzEN1RRg4DGP6E4sfonvA2c+4S1CdB3nDioyU4KaPR2FVDCU+/+ZqEMo/pr8uIAZeZDEnopHg==";

    fn map() -> Vec<(String, String)> {
        vec![("NP".into(), "NWK".into())]
    }

    #[test]
    fn train_numbers_strip_amtrak_prefix_and_leading_zeros() {
        assert_eq!(normalize_train_number("A67").as_deref(), Some("67"));
        assert_eq!(normalize_train_number("a067").as_deref(), Some("67"));
        assert_eq!(normalize_train_number("066").as_deref(), Some("66"));
        assert_eq!(normalize_train_number("141").as_deref(), Some("141"));
        assert_eq!(amtrak_number_from_njt("7885"), None);
        assert_eq!(amtrak_number_from_njt("A2228").as_deref(), Some("2228"));
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
        assert_eq!(split_track_assignment("NWK:track:4"), Some(("NWK", "4")));
        assert!(split_track_assignment("NWK:track:TBD").is_none());
    }

    #[test]
    fn njt_fixture_keeps_amtrak_tracks_only() {
        let body = include_str!("../../fixtures/tracks/njt_newark.json");
        let rows = assignments_for_station(body, "NP", &map()).unwrap();
        let tracks: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.train_number.as_str(),
                    row.track.as_str(),
                    row.stop_id.as_str(),
                )
            })
            .collect();
        assert_eq!(
            tracks,
            vec![("67", "4", "NWK"), ("67", "2", "NWK"), ("169", "3", "NWK")]
        );
        let mut board = TrackBoard::default();
        for row in rows {
            board.insert(row);
        }
        assert_eq!(board.track("67", "NWK"), Some("2"));
        assert_eq!(board.track("169", "NWK"), Some("3"));
    }

    #[test]
    fn official_xml_wrapper_parses() {
        let body = include_str!("../../fixtures/tracks/njt_official_wrapped.xml");
        let rows = assignments_for_station(body, "NP", &map()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].train_number, "190");
        assert_eq!(rows[0].track, "4");
    }

    #[test]
    fn hartford_fixture_keeps_amtrak_rows() {
        let html = include_str!("../../fixtures/tracks/hartford_board.html");
        let rows = parse_hartford_board(html);
        assert_eq!(
            rows,
            vec![
                ("141".into(), "3".into()),
                ("2151".into(), "1".into()),
                ("2190".into(), "2".into()),
                ("66".into(), "1".into()),
            ]
        );
    }

    #[test]
    fn departurevision_crypto_matches_the_public_client() {
        let plain = "timestamp==2026-09-28T03:36:17.227Z||username==DV2024||password==ydm3fP(v||";
        let encoded = aes192_ecb_base64(SPA_AES_KEY.as_bytes(), plain.as_bytes()).unwrap();
        assert_eq!(encoded, AES_VECTOR);
        let decoded = decrypt_spa_blob(SPA_BLOB).unwrap();
        assert_eq!(decoded, "train username password rn DV2024 sw ydm3fP(v");
        let challenge = spa_challenge_plain(&decoded, "2026-09-28T03:36:17.227Z").unwrap();
        assert_eq!(challenge, plain);
    }

    #[test]
    fn utc_epoch_formats_as_iso8601() {
        assert_eq!(
            iso8601_millis(UNIX_EPOCH + Duration::from_secs(0)),
            "1970-01-01T00:00:00.000Z"
        );
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

    #[test]
    fn apply_stamps_track_and_fills_stop_sequence() {
        let gtfs = sample_gtfs();
        let mut batch = RtBatch {
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
            vehicle_positions: gtfs_realtime::FeedMessage::default(),
            alerts: gtfs_realtime::FeedMessage::default(),
        };
        let mut board = TrackBoard::default();
        board.insert(Assignment {
            train_number: "67".into(),
            stop_id: "NWK".into(),
            track: "4".into(),
        });
        apply_track_assignments(&mut batch, &gtfs, &board);
        let stu = &batch.trip_updates.entity[0]
            .trip_update
            .as_ref()
            .unwrap()
            .stop_time_update[0];
        assert_eq!(stu.stop_id.as_deref(), Some("NWK"));
        assert_eq!(stu.stop_sequence, Some(3));
        assert_eq!(
            stu.stop_time_properties
                .as_ref()
                .and_then(|properties| properties.assigned_stop_id.as_deref()),
            Some("NWK:track:4")
        );
    }

    #[tokio::test]
    async fn unreachable_boards_leave_the_inner_batch_unchanged() {
        let gtfs = sample_gtfs();
        let batch = RtBatch {
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
        };
        let inner = MockSource {
            name: "amtrak",
            behavior: Behavior::Ok(batch),
        };
        let mut config = TrackConfig::from_map(|_| None).unwrap();
        config.enabled = true;
        config.njt_api_base = "http://127.0.0.1:1".into();
        config.njt_spa_origin = "http://127.0.0.1:1".into();
        config.hartford_url = Some("http://127.0.0.1:1/board".into());
        config.ttl = Duration::from_secs(60);
        let source = WithTracks::new(inner, config);
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
