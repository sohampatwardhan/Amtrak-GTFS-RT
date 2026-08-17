//! Consumer core for the Amtrak **station & train status** queries.
//!
//! Contract: this module tree turns one coherent, immutable GTFS-Realtime *generation* — the four
//! artifacts the feed service already publishes — into two answers: a station's upcoming
//! departures board and a train's live status, each enriched with the train's service alert,
//! per-station local time, and (for a train) its route geometry.
//!
//! Why it lives here: it is compiled only under the `status` Cargo feature and is used solely by
//! the sibling `amtrak-status` binary (`src/bin/amtrak_status.rs`). It never links into the shipped
//! `amtrak-gtfs-rt-service` binary, so the released service and its container image are unchanged.
//!
//! Later tasks populate the submodules: `source` (feed acquisition), `index` (join indexes plus
//! route/timezone/alert enrichment), and `station` / `train` (the two query modes).
//!
//! `dead_code` is allowed while the tree is built incrementally: each module's public surface is
//! consumed by a later task (the CLI in task 5.1), so it is not yet referenced from `main`.
#![allow(dead_code)]

pub mod format;
pub mod index;
pub mod source;
pub mod station;
pub mod train;

#[cfg(test)]
mod live_tests;
