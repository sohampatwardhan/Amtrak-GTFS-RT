# Amtrak GTFS-RT

A small Rust service that produces **live GTFS-Realtime feeds for Amtrak** — TripUpdates,
VehiclePositions, and Alerts — plus the static GTFS they bind to, for use in third-party
transit apps (Transit, Transitland, OpenTripPlanner).

It loads Amtrak's official static `GTFS.zip`, then on an interval delegates the
decrypt → parse → match → encode work to the
[`catenarytransit/amtrak-gtfs-rt`](https://github.com/catenarytransit/amtrak-gtfs-rt)
crate (which fetches and decrypts Amtrak's `getTrainsData`, matches each train to a GTFS
trip, and returns ready-made protobuf feeds). The feeds are written atomically to disk and
served by a small `axum` HTTP server. A neutral `RtSource` trait wraps the data source so
fallback sources (TransitDocs, RailRat) can be added later without touching the core.

## Endpoints

| Path | Content-Type | Description |
|------|--------------|-------------|
| `/trip-updates.pb` | `application/protobuf` | GTFS-RT TripUpdates (delays/predictions) |
| `/vehicle-positions.pb` | `application/protobuf` | GTFS-RT VehiclePositions (live train locations) |
| `/alerts.pb` | `application/protobuf` | GTFS-RT Alerts (service disruptions) |
| `/static.zip` | `application/zip` | The static GTFS the RT feeds bind to |
| `/health` | `text/plain` | Liveness check (`ok`) |

Each RT feed's header is stamped with the static feed's `feed_version` so consumers can
confirm the realtime and static feeds match.

## Running

```bash
cargo run
```

Then, e.g.:

```bash
curl -s http://localhost:8080/vehicle-positions.pb --output vp.pb
```

## Configuration (environment variables)

| Variable | Default | Description |
|----------|---------|-------------|
| `AMTRAK_STATIC_URL` | `https://content.amtrak.com/content/gtfs/GTFS.zip` | Static GTFS source |
| `AMTRAK_OUTPUT_DIR` | `./out` | Where feed files are written and served from |
| `AMTRAK_POLL_SECS` | `45` | Realtime poll interval (seconds) |
| `AMTRAK_STATIC_REFRESH_SECS` | `86400` | Static feed refresh interval (seconds) |
| `AMTRAK_FILTER_CAPITAL_CORRIDOR` | `false` | Drop Capital Corridor (route 84); a better feed exists via 511.org |
| `AMTRAK_BIND_ADDR` | `0.0.0.0:8080` | HTTP bind address |

## Resilience

- Sources are tried in order; the first fresh, non-empty batch wins. If none produce
  data in a cycle, the last-good files on disk are left in place (their header timestamp
  reveals their age) — the service never serves empty or partial feeds.
- The static feed refresh keeps the last-good schedule if a refresh fails.

## Validation

The feeds are verified in two ways in the test suite: RT protobuf round-trips through the
decoder, and a live end-to-end test fetches real Amtrak data and confirms non-empty,
statically-bound output. For a full spec-compliance gate, run
[MobilityData's GTFS-Realtime Validator](https://github.com/MobilityData/gtfs-realtime-validator)
against the served `.pb` URLs and a GTFS validator against `static.zip` (recommended in CI).

## License

**AGPL-3.0.** This service depends on the AGPL-3.0-licensed
[`catenarytransit/amtrak-gtfs-rt`](https://github.com/catenarytransit/amtrak-gtfs-rt)
crate, which does the core decryption and GTFS matching. Thanks to the Catenary project.
