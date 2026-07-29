# Amtrak GTFS-RT — Design Spec

- **Date:** 2026-07-29
- **Status:** Approved (brainstorming), pending user spec review
- **Author:** Soham + Claude

## 1. Goal

Produce **spec-valid, live GTFS-Realtime** for Amtrak — TripUpdates, VehiclePositions,
and Alerts — plus the **static GTFS** it binds to, consumable by real third-party
transit apps (Transit, Transitland, OpenTripPlanner).

The realtime feed is the prize. The static feed exists to make RT valid: every RT
`trip_id`/`stop_id`/`route_id` must resolve against the static feed, or consumers
silently drop the entity.

### Non-goals (v1)

- Public hosting / CDN, monitoring dashboards, historical archival.
- Full fallback-source implementations (interface only — see scope).
- A consumer-facing API beyond serving the standard GTFS + `.pb` artifacts.

## 2. Locked decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | **Primary RT source = Amtrak `getTrainsData`**; fallback = TransitDocs/ASM, then RailRat | Closest to source; one call yields positions + per-station timings. Fallbacks give graceful degradation when Amtrak's endpoint is flaky. |
| D2 | **Static source = Amtrak's official `GTFS.zip`** (`content.amtrak.com/content/gtfs/GTFS.zip`) | Verified live, ~19 MB, `Last-Modified` same-day — actively maintained. No third-party repo or scraping needed for static. Treated as a swappable, versioned input. |
| D3 | **Language = Rust** | The proven reference (catenary) is Rust; staying in-language lets us reuse its exact crates for the two hard algorithms (AES decrypt, date-offset detection) rather than re-porting and re-validating. Single static binary, tiny footprint, cheap 24/7 poller. Perf/concurrency is *not* the driver — the workload is small (~hundreds of trains, poll every 30–60 s). |
| D4 | **Depend on catenary's `amtrak-gtfs-rt` crate** (git/crates dependency) **behind our own `RtSource` seam** | Fastest path to a correct feed; inherits upstream fixes to Amtrak's quirks. The seam isolates their API so fallback sources and a future fork are contained changes. |
| D5 | **Serving = poller writes files; dumb static server (or object storage) serves them** | Decouples serving from polling (neither can crash the other); standard GTFS-RT distribution model; trivially upgrades to object storage/CDN later. |

## 3. Verified facts (2026-07-29)

- `https://content.amtrak.com/content/gtfs/GTFS.zip` → `200`, `application/zip`,
  `Content-Length: 19282625`, has `ETag` + `Last-Modified` (same day). Usable for
  ETag-conditional daily refresh.
- catenary `amtrak-gtfs-rt` = **v0.9.1**, edition 2024, **AGPL-3.0**. Deps of note:
  `amtk 0.1.0` (decrypt), `gtfs-realtime 0.2.0`, `gtfs-structures 0.46.1`,
  `prost 0.14`, `reqwest 0.13`, `geojson 1.0`, `chrono`/`chrono-tz`, `scraper`.
- catenary key items to build on: `fetch_amtrak_gtfs_rt_joined(&Gtfs)`,
  `feature_to_gtfs_unified()`, `detect_date_offset_from_delay()`,
  `feature_to_amtrak_arrival_structs()`, `filter_capital_corridor()`,
  `make_gtfs_header()`, `amtk::decrypt()`.
- Endpoints: positions `https://maps.amtrak.com/services/MapDataService/trains/getTrainsData`;
  alerts (via catenary) `https://asm-backend.transitdocs.com/map`.

## 4. Licensing (decision to confirm)

catenary's crate is **AGPL-3.0**. Depending on it and offering the feed as a network
service triggers AGPL's network-use clause: **our service source must also be released
under AGPL-3.0**. This is consistent with an open transit-feed project and with
catenary's ethos, and is the assumed default here. Confirm before first commit of code.
If AGPL is unacceptable, fall back to D4's "fresh Rust impl, catenary as reference"
variant (still uses `amtk`/`gtfs-realtime`, which have their own licenses to check).

## 5. Architecture — five isolated units

A single Cargo crate with focused modules (workspace only if it grows).

### 5.1 `static_gtfs`
- **Does:** Download `GTFS.zip` (`reqwest`), ETag-conditional; parse into a
  `gtfs_structures::Gtfs`; refresh daily; expose the current `Gtfs` + its
  `feed_version` as a swappable, versioned input.
- **Interface:** `current() -> Arc<StaticFeed>` where `StaticFeed { gtfs: Gtfs, feed_version: String }`.
- **Depends on:** `reqwest`, `gtfs-structures`.

### 5.2 `sources`
- **Does:** Define the neutral seam so downstream never knows the source.
- **Interface:**
  ```rust
  trait RtSource {
      async fn fetch(&self, feed: &StaticFeed) -> Result<RtBatch>;
      fn name(&self) -> &'static str;
  }
  // RtBatch { trip_updates: Vec<FeedEntity>, vehicles: Vec<FeedEntity>, alerts: Vec<FeedEntity>, observed_at: DateTime<Utc> }
  ```
- **Implementations:**
  - `AmtrakSource` (v1): delegates to catenary `fetch_amtrak_gtfs_rt_joined(&gtfs)`,
    adapting its output into `RtBatch`.
  - `TransitDocsSource`, `RailRatSource` (deferred): stubs implementing the trait;
    real impls in v1.1. Their transforms are ours (catenary's is Amtrak-GeoJSON-specific).

### 5.3 `orchestrator`
- **Does:** `tokio` interval loop (default 45 s). Runs the source chain
  (Amtrak → TransitDocs → RailRat) until one yields a fresh, non-empty batch;
  otherwise keeps last-good. Encodes each entity group into a `FeedMessage`
  (header stamped with the active `feed_version`), `prost` `encode_to_vec()`,
  writes to the output dir atomically (temp file + rename).
- **Outputs:** `trip-updates.pb`, `vehicle-positions.pb`, `alerts.pb`, plus a
  refreshed `static.zip`.
- **Interface:** `run(config, static_gtfs, sources, writer)`.

### 5.4 `serve`
- **Does:** Expose the output dir over HTTP with correct content-types
  (`application/protobuf`, `application/zip`). v1 = a minimal `axum` reader over the
  dir, or defer to any static server / object storage. Read-only; never mutates state.

### 5.5 cross-cutting
- **`config`:** poll interval, output dir, source order, Capital Corridor filter on/off,
  static refresh interval.
- **health:** log **match rate** (matched trains / observed trains) each cycle as the
  primary health signal; log source used and batch age.

## 6. Data flow

```
daily:      GTFS.zip --(ETag)--> Gtfs (StaticFeed)
every 45s:  RtSource.fetch(&StaticFeed)
              -> RtBatch (trip_updates, vehicles, alerts)
              -> encode 3x FeedMessage (feed_version stamped)
              -> atomic write *.pb + static.zip
consumers:  HTTP GET static.zip + *.pb
```

## 7. Error handling

- Source fails / empty / stale → try next source in the chain.
- **All sources fail → serve last-good `.pb`** (header `timestamp` reveals age).
  Never serve empty or malformed feeds.
- Static refresh fails → keep last-good `Gtfs`.
- Decrypt failure → treated as a source failure (advance the chain).
- Unmatched trains → logged and **dropped** (never emit a `trip_id` absent from static).

## 8. Testing & validation (how we earn "valid for real apps")

- **Unit (`cargo test`):** decrypt against a captured `getTrainsData` fixture;
  `Station0..N` arrival parsing; date-offset detection (overnight/multi-day trains);
  matcher edge cases; encoder produces well-formed `FeedMessage`.
- **Golden integration:** captured `getTrainsData` sample → expected entity set.
- **Validation gate (CI):** static feed through a GTFS validator; RT `.pb` through
  MobilityData's GTFS-Realtime validator. Both must pass.

## 9. v1 scope

**In:** Amtrak primary source + alerts (via catenary/TransitDocs), all three RT feed
types, static ingest with daily refresh, files-to-disk serving, configurable Capital
Corridor filter, match-rate health logging.

**Deferred (v1.1+):** full `TransitDocsSource`/`RailRatSource` position fallback,
hosting/CDN, monitoring dashboards, historical archival.

## 10. Open risks / to verify at implementation start

- **AGPL license acceptance** (§4) — confirm before writing dependent code.
- **Crate consumability** — `amtrak-gtfs-rt` may not be on crates.io; expect a git
  dependency. Confirm its public functions are callable as a library (edition 2024
  requires a recent Rust toolchain).
- **Rust toolchain** — edition 2024 → need current stable Rust.
- **Amtrak endpoint stability** — `getTrainsData` is known to be intermittently flaky;
  the fallback chain and last-good serving exist for this reason.

## 11. First implementation steps

1. Confirm AGPL is acceptable; pick the license for this repo accordingly.
2. Scaffold the Cargo crate; add `amtrak-gtfs-rt` (git dep), `gtfs-structures`,
   `gtfs-realtime`, `prost`, `reqwest`, `tokio`, `axum`.
3. Spike: load `GTFS.zip` → `Gtfs`, call `fetch_amtrak_gtfs_rt_joined`, encode one
   `trip-updates.pb`, validate it. Prove the core path end-to-end before building the
   orchestrator/serve layers.
