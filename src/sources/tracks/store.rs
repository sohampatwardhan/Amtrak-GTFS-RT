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

/// Rank used when two sources name the same train and stop. Higher replaces lower.
///
/// Amtrak's own status ranks first only while its document is `fresh` (fetched no more than the
/// store's max age ago). A retained Amtrak row from an older document still publishes until
/// departure, but below NJ Transit and the Hartford board, so a live board reading of a track
/// change wins over a value the sidecar can no longer confirm.
pub fn source_precedence(source_key: &str, fresh: bool) -> u8 {
    if source_key.starts_with("amtrak:") {
        if fresh {
            3
        } else {
            0
        }
    } else if source_key.starts_with("njt:") {
        2
    } else {
        1
    }
}

/// Thread-safe store of the latest successful board per source station.
///
/// Keys are source-scoped (`njt:NP`, `hartford:NHV`, `amtrak:167:2026-10-17`) so two sources
/// reporting the same Amtrak stop never overwrite each other's storage. A read merges them by
/// [`source_precedence`]. The lock is held only for in-memory copies, never across an await,
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

    /// Merges rows that carry their own [`Assignment::retain_until`].
    ///
    /// A row for the same train and stop is replaced. A row that this fetch does not mention is
    /// kept until its `retain_until` passes, which is how a track Amtrak showed once stays up
    /// through departure even if a later document omits it. Rows with no `retain_until` are left
    /// for [`Self::replace_station`].
    ///
    /// `observed_at` is when the source document was fetched, and it becomes the age reference
    /// for [`source_precedence`]. A document older than the one already stored is ignored, so a
    /// re-served snapshot cannot roll a track back.
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
            if observed_at < station.observed_at {
                return;
            }
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
        let mut ordered: Vec<_> = boards
            .iter()
            .map(|(key, station)| {
                let age = now.duration_since(station.observed_at).unwrap_or_default();
                (
                    source_precedence(key, age <= max_age),
                    key.as_str(),
                    age,
                    station,
                )
            })
            .collect();
        ordered.sort_by_key(|(rank, key, _, _)| (*rank, *key));
        let mut board = TrackBoard::default();
        for (_, _, age, station) in ordered {
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

    const MAX_AGE: Duration = Duration::from_secs(300);

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn fresh_amtrak_beats_njt_which_beats_hartford() {
        let store = AssignmentStore::new();
        store.replace_station("hartford:NHV", vec![row("67", "NYP", "7")], at(1_000));
        assert_eq!(
            store.fresh_board(at(1_100), MAX_AGE).track("67", "NYP"),
            Some("7")
        );
        store.replace_station("njt:NY", vec![row("67", "NYP", "4")], at(1_000));
        assert_eq!(
            store.fresh_board(at(1_100), MAX_AGE).track("67", "NYP"),
            Some("4")
        );
        let amtrak = row_until("67", "NYP", "9", at(5_000));
        store.upsert_retained("amtrak:67:2099-01-01", vec![amtrak], at(1_000));
        assert_eq!(
            store.fresh_board(at(1_100), MAX_AGE).track("67", "NYP"),
            Some("9")
        );
    }

    #[test]
    fn a_stale_amtrak_track_yields_to_a_live_board_but_is_kept_until_departure() {
        let store = AssignmentStore::new();
        let amtrak = row_until("67", "NYP", "9", at(5_000));
        store.upsert_retained("amtrak:67:2099-01-01", vec![amtrak], at(1_000));
        store.replace_station("njt:NY", vec![row("67", "NYP", "4")], at(1_400));
        // Amtrak's document is 400 s old, past the 300 s limit; NJ Transit's reading is live.
        assert_eq!(
            store.fresh_board(at(1_400), MAX_AGE).track("67", "NYP"),
            Some("4")
        );
        // Once NJ Transit's reading expires too, the retained Amtrak track still publishes.
        assert_eq!(
            store.fresh_board(at(1_800), MAX_AGE).track("67", "NYP"),
            Some("9")
        );
        assert_eq!(
            store.fresh_board(at(5_000), MAX_AGE).track("67", "NYP"),
            None
        );
    }

    #[test]
    fn amtrak_rows_survive_an_omitting_document_and_ignore_an_older_one() {
        let store = AssignmentStore::new();
        let key = "amtrak:67:2099-01-01";
        store.upsert_retained(key, vec![row_until("67", "NYP", "9", at(2_000))], at(1_000));
        store.upsert_retained(
            key,
            vec![row_until("67", "NYP", "10", at(2_000))],
            at(1_100),
        );
        assert_eq!(
            store.fresh_board(at(1_100), MAX_AGE).track("67", "NYP"),
            Some("10")
        );
        // A later document that omits the stop keeps the track already seen.
        store.upsert_retained(key, vec![], at(1_200));
        assert_eq!(
            store.fresh_board(at(1_200), MAX_AGE).track("67", "NYP"),
            Some("10")
        );
        // A document fetched before the stored one does not roll the track back.
        store.upsert_retained(key, vec![row_until("67", "NYP", "9", at(2_000))], at(1_050));
        assert_eq!(
            store.fresh_board(at(1_200), MAX_AGE).track("67", "NYP"),
            Some("10")
        );
        store.prune(at(2_000), MAX_AGE);
        assert!(store.fresh_board(at(1_200), MAX_AGE).is_empty());
    }

    fn row_until(train: &str, stop: &str, track: &str, until: SystemTime) -> Assignment {
        Assignment {
            retain_until: Some(until),
            ..row(train, stop, track)
        }
    }
}
