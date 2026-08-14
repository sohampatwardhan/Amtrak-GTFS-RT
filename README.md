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
trip, and returns ready-made protobuf feeds). Complete static and realtime generations are
persisted immutably and served by a small `axum` HTTP server. A neutral `RtSource` trait wraps the data source so
fallback sources (TransitDocs, RailRat) can be added later without touching the core.

## Endpoints

| Path | Access | Content-Type | Description |
|------|--------|--------------|-------------|
| `/livez` | public | `application/json` | Process liveness, independent of feed readiness |
| `/readyz` | controlled | `application/json` | `200` only while the current generation is less than 300 seconds old |
| `/v1/feed-set.json` | controlled | `application/json` | Current generation ID, timestamp, static version, entity counts, and four immutable URLs |
| `/v1/generations/{id}/static.zip` | controlled | `application/zip` | Static GTFS for exactly generation `{id}` |
| `/v1/generations/{id}/{trip-updates,vehicle-positions,alerts}.pb` | controlled | `application/x-protobuf` | One GTFS-Realtime product from exactly generation `{id}` |

Fetch `/v1/feed-set.json` first and then use only the URLs it returns. Do not construct
generation URLs or substitute a newer ID between artifact requests. Every realtime header is
stamped with the manifest's static version and generation timestamp.

Feed, manifest, and readiness routes authorize the direct socket peer. With no explicit
allowlist, only loopback peers are admitted. `Forwarded` and `X-Forwarded-For` are ignored;
placing a reverse proxy at this authorization boundary requires a separately designed trusted-
proxy policy.

## Running

Requires a current stable Rust toolchain (the `amtrak-gtfs-rt` dependency uses
edition 2024; developed against 1.96) and **`protoc`**, the protobuf compiler —
the `gtfs-realtime` crate generates its Rust bindings from `.proto` files at build
time:

```bash
brew install protobuf          # macOS
sudo apt-get install -y protobuf-compiler   # Debian/Ubuntu
```

```bash
cargo run
```

On startup the service recovers a retained last-good generation, validates a newly fetched static
schedule when needed, and begins polling. Query the manifest rather than a mutable feed URL:

```bash
manifest=$(curl -fsS http://127.0.0.1:8080/v1/feed-set.json)
vehicle_url=$(printf '%s' "$manifest" | jq -r '.urls.vehicle_positions')
curl -fsS "http://127.0.0.1:8080${vehicle_url}" --output vehicle-positions.pb
```

## Container

The service ships as a digest-pinned, non-root, multi-stage image assembled from `scratch`. The
build compiles the locked Rust binary against musl and rebuilds MobilityData validator v8.0.1 from
its SHA-256-pinned source snapshot with reviewed security-version overrides. The complete upstream
validator suite runs during the build, and a fixed reproducible JAR digest gates copy-forward. The
runtime contains only the binary, that JAR, a minimized Corretto Java 17 runtime, musl, zlib, and CA
roots—no shell, package manager, curl, Rust toolchain, `protoc`, or repository source.

```bash
docker build --tag amtrak-gtfs-rt:local .
```

**Image contract:** runs as UID/GID `10001`, entrypoint `/usr/local/bin/amtrak-gtfs-rt-service`
(PID 1), documented port `8080/tcp`, persistent volume `/data`, and a Docker healthcheck that
probes `/livez`. Container defaults differ from the host defaults so the persistence and validator
paths are correct inside the image:

| Variable | Host default | Container default |
|----------|--------------|-------------------|
| `AMTRAK_OUTPUT_DIR` | `./out` | `/data` |
| `AMTRAK_GTFS_VALIDATOR_JAR` | `./tools/gtfs-validator-v8.0.1-cli.jar` | `/opt/amtrak/gtfs-validator-v8.0.1-amtrak-hardened.1-cli.jar` |
| `AMTRAK_BIND_ADDR` | `127.0.0.1:8080` | `127.0.0.1:8080` (loopback-only) |
| `AMTRAK_ALLOWED_PEER_IPS` | empty | empty (loopback only) |

### Run — host networking (simplest safe posture)

With host networking the container shares the host's loopback, so the safe loopback default works
unchanged and only loopback peers are admitted:

```bash
docker run --rm --network host \
  -v amtrak-data:/data \
  amtrak-gtfs-rt:local
# manifest is reachable from the host loopback (an admitted peer):
curl -fsS http://127.0.0.1:8080/v1/feed-set.json
```

### Run — dedicated bridge (explicit network policy required)

A bridge port map is **intentionally unreachable** with the loopback default. Bridged operation
must bind the wildcard address *and* name the exact peer(s) allowed to reach the feed boundary —
there is no wildcard allowlist and no proxy trust. Publish the port on host loopback only:

```bash
docker network create --subnet 172.31.240.0/24 --gateway 172.31.240.1 amtrak-net
docker run --rm --network amtrak-net \
  -p 127.0.0.1:8080:8080 \
  -e AMTRAK_BIND_ADDR=0.0.0.0:8080 \
  -e AMTRAK_ALLOWED_PEER_IPS=172.31.240.1 \
  -v amtrak-data:/data \
  amtrak-gtfs-rt:local
```

`AMTRAK_ALLOWED_PEER_IPS` must be the **exact** source IP the container observes for admitted
traffic. That IP is engine-dependent (the bridge gateway on a Linux bridge; it can differ on
Docker Desktop), so confirm it — a wrong value yields `403`, and the denied request logs its
observed `peer=<ip>` in the container logs. Authorization uses only the direct socket peer;
`Forwarded` and `X-Forwarded-For` are ignored.

### Health, readiness, manifest, and artifacts

```bash
# Docker liveness (public; used by HEALTHCHECK):
docker inspect -f '{{.State.Health.Status}}' <container>
curl -fsS http://127.0.0.1:8080/livez

# Feed readiness — 200 only while the current generation is < 300s old (peer-gated):
curl -fsS http://127.0.0.1:8080/readyz

# Manifest first, then only the URLs it returns (peer-gated):
manifest=$(curl -fsS http://127.0.0.1:8080/v1/feed-set.json)
static_url=$(printf '%s' "$manifest" | jq -r '.urls.static_zip')
curl -fsS "http://127.0.0.1:8080${static_url}" --output static.zip
```

`/livez` is the Docker health signal — process liveness only. `/readyz` is a separate,
peer-gated feed-availability signal. They are deliberately distinct: Docker must not restart a
healthy process that is merely waiting for its first good generation.

### Volume retention and rollback

Generations live only in the mounted volume, so **retain the named volume across upgrades**.
`GenerationStore` recovers the last complete, valid generation on startup and rejects partial
state, so recreating the container over the same volume — even before any successful upstream
refresh — recovers the previous last-good feed set byte-for-byte:

```bash
docker rm -f <container>                       # keep the volume
docker run -d --network amtrak-net -p 127.0.0.1:8080:8080 \
  -e AMTRAK_BIND_ADDR=0.0.0.0:8080 -e AMTRAK_ALLOWED_PEER_IPS=172.31.240.1 \
  -v amtrak-data:/data \
  amtrak-gtfs-rt:local                          # or the prior image tag to roll back
```

Rollback is simply running the preceding image tag against the retained volume; immutable
artifacts are never rewritten and there is no on-disk migration.

### Smoke test

[`scripts/test-container.sh`](scripts/test-container.sh) runs a bounded, fail-closed smoke test
against a built image on an isolated bridge and named volume: it waits for health, readiness, and
the manifest; fetches and independently decodes all four artifacts; asserts denied-peer and
spoofed-header requests get `403`; asserts a wildcard bind without a policy and a non-writable
`/data` refuse to start; and verifies retained-volume recovery. It also records image size and
time-to-health and exports an SBOM plus a CVE report under `validation-reports/container/`. CVE
scanning prefers Docker Scout (needs `docker login`) and falls back to [grype](https://github.com/anchore/grype)
if it is installed (no auth required); unavailable CVE evidence is reported as such, never assumed
clean. The smoke-only shell/curl helper is separately digest-pinned and never enters the production
image. Reviewed release evidence is retained under
[`.security/risk-acceptance/evidence/`](.security/risk-acceptance/evidence/).

```bash
scripts/test-container.sh amtrak-gtfs-rt:local
```

**Out of scope for this image:** anonymous public exposure, reverse-proxy or forwarded-identity
trust, registry publication, and orchestration. The image is built and verified locally; it is not
published or deployed by this repository.

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

The built-in HTTP layer is the supported access boundary. Do not expose the generation directory
through a generic file server: that would bypass peer authorization, readiness semantics, strict
identifier parsing, and manifest-first discovery.

## Configuration (environment variables)

| Variable | Default | Description |
|----------|---------|-------------|
| `AMTRAK_STATIC_URL` | `https://content.amtrak.com/content/gtfs/GTFS.zip` | Static GTFS source |
| `AMTRAK_OUTPUT_DIR` | `./out` | Where feed files are written and served from |
| `AMTRAK_POLL_SECS` | `45` | Realtime poll interval (seconds) |
| `AMTRAK_STATIC_REFRESH_SECS` | `86400` | Static feed refresh interval (seconds) |
| `AMTRAK_FILTER_CAPITAL_CORRIDOR` | `false` | Drop Capital Corridor (route 84); a better feed exists via 511.org |
| `AMTRAK_BIND_ADDR` | `127.0.0.1:8080` | HTTP bind address; non-loopback requires an allowlist |
| `AMTRAK_ALLOWED_PEER_IPS` | empty | Comma-separated exact peer IPs; empty admits loopback only |
| `AMTRAK_GTFS_VALIDATOR_JAR` | `./tools/gtfs-validator-v8.0.1-cli.jar` | Readable, approved MobilityData validator 8.0.1 CLI JAR (official host artifact or repository-hardened container build) |

## Resilience

- **Fallback chain.** Sources are tried in order; the first fresh, non-empty batch wins.
- **Last-good serving.** Source, conversion, validation, static, or publication failure leaves the
  current immutable generation unchanged and the scheduled poller retries later.
- **Atomic publication.** Static GTFS, three realtime feeds, and the manifest become visible only
  after durable generation-directory and current-marker commits. Readers see a complete old or
  complete new generation, never a mixture.
- **Version consistency.** A valid replacement static snapshot remains pending until realtime has
  been built, validated, and committed against it.
- **Freshness semantics.** Readiness becomes false at exactly 300 seconds of generation age while
  liveness remains independently available.

## Project layout

| File | Responsibility |
|------|----------------|
| `src/config.rs` | Environment-driven configuration |
| `src/sources/mod.rs` | `RtSource` trait and the `RtBatch` normalization model |
| `src/sources/amtrak.rs` | Amtrak source, wrapping the catenary crate |
| `src/static_gtfs.rs` | Exact-byte static GTFS validation and pending/active lifecycle |
| `src/orchestrator.rs` | Source selection, coherent generation build/validation, and recoverable polling |
| `src/serve.rs` | Controlled immutable HTTP delivery and freshness health |
| `src/writer.rs` | Durable immutable generation persistence and recovery |

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

## Consumer migration

The mutable `/trip-updates.pb`, `/vehicle-positions.pb`, `/alerts.pb`, `/static.zip`, and `/health`
routes are removed from the generation API. Controlled consumers must switch atomically to
manifest-first discovery, accept `application/x-protobuf` for realtime products, and treat `403`,
`404`, and `503` distinctly: unauthorized peer, unknown immutable generation/artifact, and no
current or fresh generation respectively. Rollback uses the preceding service binary and retained
generation directory; it never rewrites immutable artifacts.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

**AGPL-3.0-only.** This service depends on the AGPL-3.0-licensed
[`catenarytransit/amtrak-gtfs-rt`](https://github.com/catenarytransit/amtrak-gtfs-rt)
crate, which does the core decryption and GTFS matching. Thanks to the Catenary project.
