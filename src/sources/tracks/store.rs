//! Expiring, per-station track assignments shared by the board refresher and the stamper.
//!
//! Track numbers are same-day data: a board observed ten minutes ago may describe a train that has
//! already left, and a board from yesterday describes a different train that happens to share its
//! number. The store therefore keeps each source station's rows together with the time they were
//! observed, replaces them only when that station is fetched successfully, and lets readers drop
//! anything older than the configured maximum age. A failed fetch never refreshes the timestamp.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, SystemTime};

/// One train's track at one Amtrak stop, as reported by a board.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assignment {
    /// Normalized Amtrak train number (`67`, not `A067`).
    pub train_number: String,
    /// Amtrak GTFS `stop_id` of the station.
    pub stop_id: String,
    /// Platform token such as `4`, `A`, or `MAIN`.
    pub track: String,
    /// When set, the row stays publishable until this instant even if its board is not refreshed
    /// again. Amtrak sets this to the stop's departure (plus a grace). Other boards leave it
    /// empty and expire with the station's observation age.
    pub retain_until: Option<SystemTime>,
}

/// Lookup of (normalized train number, uppercase Amtrak stop id) → track label.
#[derive(Clone, Debug, Default)]
pub struct TrackBoard {
    assignments: HashMap<(String, String), String>,
}

impl TrackBoard {
    /// Adds one assignment; a later row for the same train and stop replaces an earlier one.
    pub fn insert(&mut self, assignment: Assignment) {
        self.assignments.insert(
            (assignment.train_number, assignment.stop_id.to_uppercase()),
            assignment.track,
        );
    }

    /// Track reported for `train_number` at `stop_id`, if any.
    pub fn track(&self, train_number: &str, stop_id: &str) -> Option<&str> {
        self.assignments
            .get(&(train_number.to_string(), stop_id.to_uppercase()))
            .map(String::as_str)
    }

    /// Whether the board holds no assignments.
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }
}

struct StationBoard {
    observed_at: SystemTime,
    rows: Vec<Assignment>,
}

/// Thread-safe store of the latest successful board per source station.
///
/// Keys are source-scoped (`njt:NP`, `hartford:NHV`, `amtrak:167:2026-10-17`) so two sources
/// reporting the same Amtrak stop never overwrite each other's storage. A read merges them with a
/// fixed precedence: Amtrak's own status, then NJ Transit RailData, then every other board
/// (the Hartford Line board). The lock is held only for in-memory copies, never across an await,
/// which is what keeps a generation's read independent of board latency.

/// Rank used when two sources name the same train and stop. Higher replaces lower.
pub fn source_precedence(source_key: &str) -> u8 {
    if source_key.starts_with("amtrak:") {
        3
    } else if source_key.starts_with("njt:") {
        2
    } else {
        1
    }
}
#[derive(Default)]
pub struct AssignmentStore {
    boards: RwLock<HashMap<String, StationBoard>>,
}

impl AssignmentStore {
    /// Creates an empty store.
    pub fn new() -> AssignmentStore {
        AssignmentStore::default()
    }

    /// Merges rows that carry their own [`Assignment::retain_until`].
    ///
    /// A row for the same train and stop is replaced. A row that this fetch does not mention is
    /// kept until its `retain_until` passes, which is how a track Amtrak showed once stays up
    /// through departure even if a later document omits it. Rows with no `retain_until` are left
    /// for [`Self::replace_station`].
    pub fn upsert_retained(
        &self,
        source_key: &str,
        rows: Vec<Assignment>,
        observed_at: SystemTime,
    ) {
        let mut boards = self
            .boards
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let empty = {
            let station = boards
                .entry(source_key.to_string())
                .or_insert_with(|| StationBoard {
                    observed_at,
                    rows: Vec::new(),
                });
            station.observed_at = observed_at;
            for row in rows {
                if let Some(existing) = station.rows.iter_mut().find(|existing| {
                    existing.train_number == row.train_number
                        && existing.stop_id.eq_ignore_ascii_case(&row.stop_id)
                }) {
                    *existing = row;
                } else {
                    station.rows.push(row);
                }
            }
            station.rows.retain(|row| match row.retain_until {
                Some(until) => until > observed_at,
                None => true,
            });
            station.rows.is_empty()
        };
        if empty {
            boards.remove(source_key);
        }
    }

    /// Drops rows that can no longer be published, so a long run does not keep yesterday's tracks.
    pub fn prune(&self, now: SystemTime, max_age: Duration) {
        let mut boards = self
            .boards
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        boards.retain(|_, station| {
            let age = now.duration_since(station.observed_at).unwrap_or_default();
            station.rows.retain(|row| match row.retain_until {
                Some(until) => now < until,
                None => age <= max_age,
            });
            !station.rows.is_empty()
        });
    }

    /// Replaces one source station's rows after a successful fetch.
    ///
    /// `observed_at` is the time the board was read; it becomes the age reference for every row.
    pub fn replace_station(
        &self,
        source_key: &str,
        rows: Vec<Assignment>,
        observed_at: SystemTime,
    ) {
        let mut boards = self
            .boards
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        boards.insert(source_key.to_string(), StationBoard { observed_at, rows });
    }

    /// Builds a lookup from boards observed no more than `max_age` before `now`.
    ///
    /// Boards with an `observed_at` in the future (clock adjustments) are treated as fresh; boards
    /// past the limit are skipped, which is how a failed refresh lets its station expire.
    pub fn fresh_board(&self, now: SystemTime, max_age: Duration) -> TrackBoard {
        let boards = self
            .boards
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut ordered: Vec<_> = boards.iter().collect();
        ordered.sort_by_key(|(key, _)| (source_precedence(key), key.as_str()));
        let mut board = TrackBoard::default();
        for (_, station) in ordered {
            let age = now.duration_since(station.observed_at).unwrap_or_default();
            for row in &station.rows {
                let publish = match row.retain_until {
                    Some(until) => now < until,
                    None => age <= max_age,
                };
                if publish {
                    board.insert(row.clone());
                }
            }
        }
        board
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(train: &str, stop: &str, track: &str) -> Assignment {
        Assignment {
            train_number: train.into(),
            stop_id: stop.into(),
            track: track.into(),
            retain_until: None,
        }
    }

    #[test]
    fn a_successful_refresh_replaces_only_that_station() {
        let store = AssignmentStore::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        store.replace_station(
            "njt:NP",
            vec![row("67", "NWK", "4"), row("169", "NWK", "3")],
            t0,
        );
        store.replace_station("hartford:NHV", vec![row("141", "NHV", "8")], t0);
        store.replace_station("njt:NP", vec![row("67", "NWK", "2")], t0);
        let board = store.fresh_board(t0, Duration::from_secs(300));
        assert_eq!(board.track("67", "NWK"), Some("2"));
        assert_eq!(board.track("169", "NWK"), None);
        assert_eq!(board.track("141", "nhv"), Some("8"));
    }

    #[test]
    fn stations_expire_when_not_refreshed() {
        let store = AssignmentStore::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        store.replace_station("njt:NP", vec![row("67", "NWK", "4")], t0);
        store.replace_station(
            "hartford:NHV",
            vec![row("141", "NHV", "8")],
            t0 + Duration::from_secs(200),
        );
        let later = t0 + Duration::from_secs(301);
        let board = store.fresh_board(later, Duration::from_secs(300));
        assert_eq!(board.track("67", "NWK"), None);
        assert_eq!(board.track("141", "NHV"), Some("8"));
        assert!(store
            .fresh_board(t0 + Duration::from_secs(10_000), Duration::from_secs(300))
            .is_empty());
    }

    #[test]
    fn amtrak_replaces_njt_and_stays_until_departure() {
        let store = AssignmentStore::new();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        store.replace_station("njt:NY", vec![row("67", "NYP", "4")], t0);
        store.replace_station("hartford:NHV", vec![row("141", "NHV", "8")], t0);
        let kept = row_until("67", "NYP", "9", t0 + Duration::from_secs(1_000));
        store.upsert_retained("amtrak:67:2099-01-01", vec![kept], t0);
        let board = store.fresh_board(t0 + Duration::from_secs(400), Duration::from_secs(300));
        assert_eq!(board.track("67", "NYP"), Some("9"));
        assert_eq!(board.track("141", "NHV"), None);
        store.upsert_retained(
            "amtrak:67:2099-01-01",
            vec![row_until(
                "67",
                "NYP",
                "10",
                t0 + Duration::from_secs(1_000),
            )],
            t0 + Duration::from_secs(500),
        );
        let board = store.fresh_board(t0 + Duration::from_secs(500), Duration::from_secs(300));
        assert_eq!(board.track("67", "NYP"), Some("10"));
        // A later document that omits the stop keeps the track already seen.
        store.upsert_retained(
            "amtrak:67:2099-01-01",
            vec![],
            t0 + Duration::from_secs(600),
        );
        // An empty upsert removes the key when nothing remains... wait, empty vec plus retain
        // filter: existing row retain is t0+1000, observed_at is t0+600, row stays.
        let board = store.fresh_board(t0 + Duration::from_secs(600), Duration::from_secs(300));
        assert_eq!(board.track("67", "NYP"), Some("10"));
        let gone = store.fresh_board(t0 + Duration::from_secs(1_000), Duration::from_secs(300));
        assert_eq!(gone.track("67", "NYP"), None);
    }

    fn row_until(train: &str, stop: &str, track: &str, until: SystemTime) -> Assignment {
        Assignment {
            retain_until: Some(until),
            ..row(train, stop, track)
        }
    }
}
