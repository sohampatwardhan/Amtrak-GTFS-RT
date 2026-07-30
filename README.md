# Amtrak GTFS-RT

[![CI](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/actions/workflows/ci.yml/badge.svg)](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/actions/workflows/ci.yml)
[![Validate feeds](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/actions/workflows/validate-feeds.yml/badge.svg)](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/actions/workflows/validate-feeds.yml)

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

Requires a current stable Rust toolchain (the `amtrak-gtfs-rt` dependency uses
edition 2024; developed against 1.96).

```bash
cargo run
```

On startup the service downloads the static GTFS, then begins polling. Within one
poll interval the feeds are available:

```bash
curl -s http://localhost:8080/vehicle-positions.pb --output vp.pb
```

## Deployment

Run it under a process supervisor. The service exits with a non-zero status if any
of its long-lived tasks (poller, static refresher, HTTP server) stops unexpectedly,
so a supervisor configured to restart on failure will bring it back:

```ini
[Service]
ExecStart=/usr/local/bin/amtrak-gtfs-rt-service
Environment=AMTRAK_OUTPUT_DIR=/var/lib/amtrak-gtfs-rt
Restart=on-failure
```

Because the poller and the HTTP layer are decoupled through the output directory,
you can also skip the built-in server entirely and point any static file server, or
sync the directory to object storage behind a CDN.

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

- **Fallback chain.** Sources are tried in order; the first fresh, non-empty batch
  wins. Empty and failing sources are logged and skipped.
- **Last-good serving.** If no source produces data in a cycle, the previous files
  are left in place (their header timestamp reveals their age) — the service never
  serves empty or malformed feeds. A failed static refresh likewise keeps the
  last-good schedule.
- **Atomic group writes.** The three `.pb` files are written to temp files and only
  renamed into place once all three have been written, so a mid-write failure can
  never leave a mix of fresh and stale feeds, and consumers never read a partial file.
- **Version consistency.** `static.zip` is written before the in-memory schedule is
  swapped, so the `feed_version` stamped on RT feeds always matches the static feed
  actually being served.

## Project layout

| File | Responsibility |
|------|----------------|
| `src/config.rs` | Environment-driven configuration |
| `src/sources/mod.rs` | `RtSource` trait and the `RtBatch` normalization model |
| `src/sources/amtrak.rs` | Amtrak source, wrapping the catenary crate |
| `src/static_gtfs.rs` | Static GTFS ingest, shared store, periodic refresh |
| `src/orchestrator.rs` | Poll loop, source selection, protobuf encoding and writing |
| `src/serve.rs` | HTTP layer |
| `src/writer.rs` | Atomic file write primitive |

## Testing

```bash
cargo test                      # unit + integration (offline)
cargo test -- --include-ignored # also runs live tests against Amtrak's endpoints
```

The feeds are verified two ways in the suite: RT protobuf round-trips through the
decoder, and a live end-to-end test fetches real Amtrak data and asserts non-empty,
statically-bound output.

## Validation gate

Spec compliance is enforced separately by
[MobilityData's gtfs-validator](https://github.com/MobilityData/gtfs-validator) and
[gtfs-realtime-validator](https://github.com/MobilityData/gtfs-realtime-validator),
run against feeds generated from live data:

```bash
./scripts/validate-feeds.sh
```

The script generates feeds (or reuses `out/`), fetches and pins both validators,
validates each RT feed type in its own directory, and prints every notice by
severity. Reports land in `validation-reports/`. It needs Java 17 and Maven, and
falls back to Docker automatically if they aren't installed.

**How the gate decides.** It fails when a validator reports an ERROR code that is
not listed in [`validation/baseline.json`](validation/baseline.json). Occurrence
*counts* are not gated — they track how many trains are running and vary
legitimately between runs, whereas a new error *code* is a real regression.

The baseline currently records eleven known RT error codes, most of them inherited
from upstream (trips and stations that appear in live data but not in Amtrak's
published schedule) and two that are fixable here (`E039` `is_deleted` on a
FULL_DATASET feed, `E049` unpopulated header incrementality). Each entry is
annotated with its cause; they are debt to burn down, not permanent exemptions.
Amtrak's static GTFS currently produces **zero** ERROR notices, so the static side
is gated at zero.

CI runs this nightly, on pushes that touch the pipeline, and on demand — not on
pull requests, since an Amtrak outage would otherwise block unrelated work.

### Known issue

`trip-updates.pb` and `vehicle-positions.pb` are currently byte-identical: the
upstream transform emits unified entities carrying both `trip_update` and `vehicle`,
and both feeds are served from that same message. Consumers of either endpoint get
correct data, but each feed carries payload the consumer ignores.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

**AGPL-3.0-only.** This service depends on the AGPL-3.0-licensed
[`catenarytransit/amtrak-gtfs-rt`](https://github.com/catenarytransit/amtrak-gtfs-rt)
crate, which does the core decryption and GTFS matching. Thanks to the Catenary project.
