# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Feed validation gate** (`scripts/validate-feeds.sh`) running MobilityData's
  `gtfs-validator` 8.0.1 and `gtfs-realtime-validator` against feeds generated
  from live data. Fails on any ERROR code absent from
  `validation/baseline.json`; occurrence counts are not gated, since they track
  how many trains are running. Each RT feed type is validated in its own
  directory so the batch processor does not treat them as successive iterations
  of one feed. Runs on Java 17 + Maven, or Docker as a fallback.
- **CI workflows**: `ci.yml` (build and offline tests on every push and pull
  request) and `validate-feeds.yml` (the validation gate, nightly plus on
  pipeline changes and on demand — kept off pull requests so an Amtrak outage
  cannot block unrelated work).
- **`validation/baseline.json`** recording the eleven known GTFS-Realtime error
  codes with their causes, as debt to burn down.

### Known issues

- `trip-updates.pb` and `vehicle-positions.pb` are byte-identical: the upstream
  transform emits unified entities carrying both `trip_update` and `vehicle`.
  Both endpoints serve correct data, but each carries payload its consumer
  ignores.
- `E039` (`is_deleted` present on a FULL_DATASET feed) and `E049` (header
  incrementality not populated) are emitted by our own pipeline and are fixable
  here.

### Planned

- `TransitDocsSource` / `RailRatSource` positional fallback implementations (the
  `RtSource` trait is already in place for them).
- ETag-conditional static refresh (currently a full re-download each cycle).

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
