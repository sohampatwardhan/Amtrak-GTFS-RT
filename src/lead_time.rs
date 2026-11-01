//! Per-station track lead times from an `amtrak-station-lead-time/v1` profile.
//!
//! The survey script in the station-coverage work writes this file. This module
//! only reads it. Stations that are missing, `sparse`, `no_track`, or `none`
//! keep the global window. A station marked `covered` is not narrowed. An
//! `observed` station whose `compared_to_default` is `wider` uses
//! `suggested_window_before_secs` and, when present, `suggested_hot_window_secs`.

#![deny(missing_docs)]

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Schema string a profile file must carry.
pub const SCHEMA: &str = "amtrak-station-lead-time/v1";

/// Lookahead and hot window for one station after the profile is applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationWindow {
    /// How far ahead a stop at this station may be and still be fetched.
    pub window_before: Duration,
    /// How close the next stop must be to be fetched on the hot cadence.
    pub hot_window: Duration,
}

/// Loaded profile. An empty profile applies the globals everywhere.
///
/// # Examples
///
/// ```
/// use amtrak_gtfs_rt_service::lead_time::LeadProfile;
/// use std::time::Duration;
///
/// let profile = LeadProfile::empty(Duration::from_secs(1800), Duration::from_secs(720));
/// let window = profile.window_for("PVD");
/// assert_eq!(window.window_before, Duration::from_secs(1800));
/// ```
#[derive(Clone, Debug)]
pub struct LeadProfile {
    global_before: Duration,
    global_hot: Duration,
    stations: HashMap<String, StationWindow>,
}

impl LeadProfile {
    /// A profile that never overrides the globals.
    ///
    /// # Arguments
    ///
    /// * `window_before` - Global lookahead.
    /// * `hot_window` - Global hot window.
    ///
    /// # Returns
    ///
    /// A profile whose `window_for` always returns those globals.
    pub fn empty(window_before: Duration, hot_window: Duration) -> Self {
        Self {
            global_before: window_before,
            global_hot: hot_window,
            stations: HashMap::new(),
        }
    }

    /// Loads a profile file.
    ///
    /// # Arguments
    ///
    /// * `path` - JSON file whose `schema` is [`SCHEMA`].
    /// * `global_before` - Lookahead used when a station is not adopted.
    /// * `global_hot` - Hot window used when a station is not adopted.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, is not the expected
    /// schema, or is not a JSON object. A station row that cannot be adopted
    /// is skipped, not an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use amtrak_gtfs_rt_service::lead_time::{LeadProfile, SCHEMA};
    /// use std::time::Duration;
    ///
    /// let dir = std::env::temp_dir().join(format!("lead-{}-doc", std::process::id()));
    /// let _ = std::fs::create_dir_all(&dir);
    /// let path = dir.join("pvd.profile.json");
    /// std::fs::write(
    ///     &path,
    ///     format!(
    ///         r#"{{"schema":"{SCHEMA}","stations":[{{"station":"PVD","confidence":"observed","compared_to_default":"wider","suggested_window_before_secs":2400,"suggested_hot_window_secs":480}}]}}"#
    ///     ),
    /// )
    /// .unwrap();
    /// let profile = LeadProfile::load(&path, Duration::from_secs(1800), Duration::from_secs(720))
    ///     .unwrap();
    /// assert_eq!(profile.window_for("PVD").window_before, Duration::from_secs(2400));
    /// let _ = std::fs::remove_dir_all(&dir);
    /// ```
    pub fn load(
        path: &Path,
        global_before: Duration,
        global_hot: Duration,
    ) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(&text, global_before, global_hot)
    }

    /// Parses profile JSON.
    ///
    /// # Arguments
    ///
    /// * `text` - File body.
    /// * `global_before` - Fallback lookahead.
    /// * `global_hot` - Fallback hot window.
    ///
    /// # Errors
    ///
    /// Returns an error when `schema` is not [`SCHEMA`] or the body is not an object.
    pub fn parse(
        text: &str,
        global_before: Duration,
        global_hot: Duration,
    ) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let object = value.as_object().ok_or("profile is not an object")?;
        let schema = object
            .get("schema")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if schema != SCHEMA {
            return Err(format!("profile schema is {schema}, expected {SCHEMA}"));
        }
        let mut stations = HashMap::new();
        let rows = object
            .get("stations")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for row in rows {
            if let Some((code, window)) = adopt(row, global_before, global_hot) {
                stations.insert(code, window);
            }
        }
        Ok(Self {
            global_before,
            global_hot,
            stations,
        })
    }

    /// Window for `station`.
    ///
    /// # Arguments
    ///
    /// * `station` - Amtrak stop code. Matching is case-insensitive.
    ///
    /// # Returns
    ///
    /// The adopted window, or the globals when the station is not adopted.
    pub fn window_for(&self, station: &str) -> StationWindow {
        self.stations
            .get(&station.trim().to_uppercase())
            .copied()
            .unwrap_or(StationWindow {
                window_before: self.global_before,
                hot_window: self.global_hot,
            })
    }

    /// How many stations use a non-global window.
    ///
    /// # Returns
    ///
    /// The count of adopted stations.
    pub fn adopted_len(&self) -> usize {
        self.stations.len()
    }
}

fn adopt(
    row: &Value,
    global_before: Duration,
    global_hot: Duration,
) -> Option<(String, StationWindow)> {
    let object = row.as_object()?;
    let code = object.get("station")?.as_str()?.trim().to_uppercase();
    if code.is_empty() {
        return None;
    }
    let confidence = object
        .get("confidence")
        .and_then(Value::as_str)
        .unwrap_or("");
    let compared = object
        .get("compared_to_default")
        .and_then(Value::as_str)
        .unwrap_or("");
    if matches!(confidence, "sparse" | "no_track" | "none" | "") || compared != "wider" {
        return None;
    }
    if confidence != "observed" {
        return None;
    }
    let before = positive_secs(object.get("suggested_window_before_secs"))?;
    let hot = positive_secs(object.get("suggested_hot_window_secs")).unwrap_or(global_hot);
    let _ = global_before;
    Some((
        code,
        StationWindow {
            window_before: before,
            hot_window: hot,
        },
    ))
}

fn positive_secs(value: Option<&Value>) -> Option<Duration> {
    let seconds = value?.as_u64()?;
    if seconds == 0 {
        return None;
    }
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> LeadProfile {
        LeadProfile::parse(body, Duration::from_secs(1800), Duration::from_secs(720)).unwrap()
    }

    #[test]
    fn wider_observed_station_overrides_and_sparse_does_not() {
        let profile = parse(
            r#"{
              "schema": "amtrak-station-lead-time/v1",
              "stations": [
                {"station":"pvd","confidence":"observed","compared_to_default":"wider",
                 "suggested_window_before_secs":600,"suggested_hot_window_secs":180},
                {"station":"NYP","confidence":"sparse","compared_to_default":"wider",
                 "suggested_window_before_secs":9000,"suggested_hot_window_secs":60},
                {"station":"BOS","confidence":"no_track","compared_to_default":"unknown",
                 "suggested_window_before_secs":9000,"suggested_hot_window_secs":60},
                {"station":"WAS","confidence":"observed","compared_to_default":"covered",
                 "suggested_window_before_secs":600,"suggested_hot_window_secs":60}
              ]
            }"#,
        );
        assert_eq!(profile.adopted_len(), 1);
        let pvd = profile.window_for("PVD");
        assert_eq!(pvd.window_before, Duration::from_secs(600));
        assert_eq!(pvd.hot_window, Duration::from_secs(180));
        assert_eq!(
            profile.window_for("NYP").window_before,
            Duration::from_secs(1800)
        );
        assert_eq!(
            profile.window_for("missing").hot_window,
            Duration::from_secs(720)
        );
        assert_eq!(
            profile.window_for("WAS").window_before,
            Duration::from_secs(1800)
        );
    }

    #[test]
    fn wrong_schema_is_an_error() {
        let error = LeadProfile::parse(
            "{\"schema\":\"other\",\"stations\":[]}",
            Duration::from_secs(1),
            Duration::from_secs(1),
        );
        assert!(error.is_err());
    }
}
