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
    /// Platform token such as `4` or `A`.
    pub track: String,
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
/// Keys are source-scoped (`njt:NP`, `hartford:NHV`) so two sources reporting the same Amtrak stop
/// never overwrite each other. The lock is held only for in-memory copies, never across an await,
/// which is what keeps a generation's read independent of board latency.
#[derive(Default)]
pub struct AssignmentStore {
    boards: RwLock<HashMap<String, StationBoard>>,
}

impl AssignmentStore {
    /// Creates an empty store.
    pub fn new() -> AssignmentStore {
        AssignmentStore::default()
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
        let mut board = TrackBoard::default();
        for station in boards.values() {
            let age = now.duration_since(station.observed_at).unwrap_or_default();
            if age > max_age {
                continue;
            }
            for row in &station.rows {
                board.insert(row.clone());
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
}
