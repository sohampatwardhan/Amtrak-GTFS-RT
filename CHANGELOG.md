# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Immutable feed generations** containing one static GTFS snapshot, separated
  TripUpdates, VehiclePositions, Alerts, and a manifest that identifies their
  shared generation and static version.
- **Manifest-first API** at `/v1/feed-set.json` with generation-pinned artifact
  URLs, plus independent `/livez` and freshness-aware `/readyz` health routes.
- **Durable recovery** through an atomically replaced current marker and validated
  generation scan, preserving the last-good feed set across process restarts and
  upstream outages.
- **Internal peer access policy** with loopback-only defaults, exact-IP allowlists,
  fail-closed transport-peer identity, and no trust in forwarding headers.
- **Feed validation gate** running content-pinned MobilityData `gtfs-validator`
  8.0.1 and a commit-pinned `gtfs-realtime-validator` against exactly one
  manifest-selected feed set. It independently decodes all three realtime feeds
  and rejects malformed, expired, or unapproved validator exceptions.
- **CI workflows**: `ci.yml` (build and offline tests on every push and pull
  request) and `validate-feeds.yml` (the validation gate, nightly plus on
  pipeline changes and on demand — kept off pull requests so an Amtrak outage
  cannot block unrelated work).
- **Spec-driven delivery record** under `.specs/amtrak-gtfs-rt-service`, including
  approved requirements, design, task execution evidence, and rollout decision.

### Changed

- Replaced mutable top-level feed files with immutable generation routes.
- Split mixed upstream entities into type-correct GTFS-Realtime products, removed
  invalid FULL_DATASET deletions, normalized headers, and filtered unresolved
  static references.
- Static GTFS replacements now pass the same pinned standards validator at
  runtime before they can participate in a committed generation.
- Long-lived task supervision now cancels and drains sibling activities when the
  poller, static refresher, or HTTP server exits or fails.

### Known issues

- Production rollout remains blocked until a fresh dependency audit has complete,
  shippable evidence. The current audit includes an unavailable CISA KEV source
  and a critical advisory inherited through the upstream `amtrak-gtfs-rt` crate.

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
