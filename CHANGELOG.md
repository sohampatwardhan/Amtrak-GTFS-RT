# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Planned, not yet implemented:

- `TransitDocsSource` / `RailRatSource` positional fallback implementations (the
  `RtSource` trait is already in place for them).
- ETag-conditional static refresh (currently a full re-download each cycle).
- MobilityData GTFS / GTFS-Realtime validator gate in CI.

## [0.1.0] - 2026-07-29

First working release. Serves live Amtrak GTFS-Realtime feeds, verified end-to-end
against Amtrak's production endpoints (145 trip updates, 145 vehicle positions,
31 alerts in a live poll).

### Added

- **GTFS-Realtime feeds** served over HTTP: `/trip-updates.pb`,
  `/vehicle-positions.pb`, and `/alerts.pb` as `application/protobuf`.
- **Static GTFS** ingest from Amtrak's official
  `https://content.amtrak.com/content/gtfs/GTFS.zip`, re-served at `/static.zip`
  and refreshed on an interval (default daily).
- **`/health`** liveness endpoint.
- **`RtSource` trait** normalizing any provider's data into an `RtBatch`, so
  fallback sources can be added without touching the orchestrator. `AmtrakSource`
  is the sole v1 implementation, delegating decryption, per-station arrival
  parsing, trip matching, and multi-day date-offset handling to
  [`catenarytransit/amtrak-gtfs-rt`](https://github.com/catenarytransit/amtrak-gtfs-rt).
- **Source fallback chain**: sources are tried in order and the first fresh,
  non-empty batch wins; empty and failing sources are logged and skipped.
- **`feed_version` stamping**: every RT feed header carries the active static
  feed's version so consumers can confirm the two match.
- **Atomic writes**: the three `.pb` files are written to temp files and renamed
  as a group, so a failure partway through can never leave a mix of fresh and
  stale feeds — and consumers never read a partial file.
- **Last-good serving**: a poll cycle with no fresh data leaves the previous
  files untouched rather than serving empty or malformed feeds. A failed static
  refresh likewise keeps the last-good schedule.
- **Non-zero exit** when a long-lived task dies, so process supervisors using
  `Restart=on-failure` actually restart the service.
- **Configuration** via environment variables: `AMTRAK_STATIC_URL`,
  `AMTRAK_OUTPUT_DIR`, `AMTRAK_POLL_SECS`, `AMTRAK_STATIC_REFRESH_SECS`,
  `AMTRAK_FILTER_CAPITAL_CORRIDOR`, `AMTRAK_BIND_ADDR`.
- **Optional Capital Corridor filter** (route 84), which has a better dedicated
  feed via 511.org.
- 21 tests, including live integration tests against Amtrak's real endpoints
  (run with `cargo test -- --include-ignored`).

[Unreleased]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.1.0
