# AGENTS.md

Instructions for coding agents working in this repository.

## Project

Amtrak GTFS-RT (AGPL-3.0-only) publishes live GTFS-Realtime for Amtrak — TripUpdates, VehiclePositions, and Alerts — plus the static GTFS those feeds bind to. Consumers start at `GET /v1/feed-set.json` and then fetch only the four immutable URLs in that manifest.

The target architecture is **three containers, one per feed**, specified in [docs/rearchitecture.md](docs/rearchitecture.md). They share one Rust library. Only VehiclePositions polls Amtrak's fleet feed. The checked-in tree is still the single `amtrak-gtfs-rt-service` binary plus the `advisory-fetcher/` and `track-fetcher/` sidecars. New work implements that document. Do not add a fourth reverse proxy unless the owner asks for one.

Decrypt, trip matching, and protobuf encoding stay in the git dependency [`catenarytransit/amtrak-gtfs-rt`](https://github.com/catenarytransit/amtrak-gtfs-rt), pinned in `Cargo.lock`, called from the shared library. A neutral `RtSource` trait wraps that source so another provider can be added without changing the orchestrator. Do not copy code from Piero Maddaleni's repositories. If an approach of his is reused, credit him in `README.md` and add the license notice that `docs/rearchitecture.md` describes. Repositories of his with no `LICENSE` file are not copied.

Both optional features fail open. A notices failure or a track failure never stops the other feeds from publishing.

- **Alerts.** TransitDocs ASM and Pacific Surfliner advisories come from the one fleet poll. Amtrak station and passenger notices (`AMTRAK_ADVISORIES`, default off) are merged in the Alerts container. `www.amtrak.com` blocks plain HTTP (Akamai). The notices browser runs in the background inside that container. Do not spoof a browser fingerprint, copy Akamai cookies, or use curl-impersonate.
- **TripUpdates.** Platform, track, and gate assignments (`AMTRAK_TRACKS`, default on) stamp `assigned_stop_id` and `platform_code`. The track and gate browser, NJ Transit RailData, and the CTrail Hartford Line board live in the TripUpdates container, on a background scheduler. This container writes the served `static.zip`. A separate track-survey script will supply per-station lead-time profiles through a config file. Until that file exists, keep today's fixed window and the 24-fetch cap.
- **VehiclePositions.** The only process that GETs `getTrainsData`. It matches trains to Amtrak's official static GTFS, writes the vehicle protobuf and an internal fleet snapshot, and serves the public HTTP API.

`cargo run` starts the current single service (`default-run` is `amtrak-gtfs-rt-service`). The `amtrak-status` station and train CLI is a second binary behind the `status` feature and is not in the released container. HTTP handlers serve the last good generation from memory. They must not wait on Chromium or on an upstream fetch. Responses carry `Last-Modified` and `ETag`, and honor `If-None-Match` / `If-Modified-Since` with `304`.

## Architecture

Target containers, from `docs/rearchitecture.md`:

| Container | Fetches and polls | Writes | Serves |
| --- | --- | --- | --- |
| VehiclePositions | One `getTrainsData` poll per cycle, plus the ASM and Surfliner reads inside that same crate call. Parses the static ZIP from the shared volume. Does not download `GTFS.zip` itself. | `vehicle-positions.pb` and the internal fleet snapshot (matched trip updates before track stamping, base alerts, generation id, timestamp, `static_version`) | Public `/livez`, `/readyz`, `/v1/feed-set.json`, and all four generation files. Advances the manifest only when `static.zip` and the three `.pb` files share that generation id, timestamp, and static version. |
| TripUpdates | Amtrak status documents (background Chromium), NJ Transit RailData, Hartford board. Official `GTFS.zip` on the daily refresh, and on a platform rebuild at most every 10 minutes. Near-stop selection reads the fleet snapshot. It does not GET `getTrainsData` again. | `trip-updates.pb`, the served `static.zip` (official schedule, dangling-stop repair, platform stops while tracks are on), `observed-platforms.txt`, the RailData token | Nothing on the host. No public port. |
| Alerts | Amtrak notices HTML, background Chromium that exits between cycles. ASM and Surfliner entities are read from the snapshot, not fetched again. | `alerts.pb` | Nothing on the host. No public port. |

`feed-set.json` stays on `127.0.0.1:8090` in Compose. A generation that is missing any of the four files is not current. The previous complete generation keeps being served.

Checked-in paths, until the split replaces them:

| Path | Role |
| --- | --- |
| `src/main.rs` | Wires config, sources, static bootstrap, and supervised tasks for the single binary |
| `src/config.rs` | Environment configuration, peer allowlist, validator JAR check |
| `src/sources/mod.rs` | `RtSource` and the `RtBatch` model |
| `src/sources/amtrak.rs` | Primary source; wraps the catenary crate |
| `src/sources/advisories.rs` | Optional notices scraper |
| `src/sources/tracks/` | Track and gate stamp, RailData, Hartford, Amtrak status, refresher |
| `src/static_augment.rs` | Adds platform stops to the published static ZIP when tracks are on |
| `src/static_repair.rs` | Drops stop times whose `stop_id` is missing from `stops.txt` |
| `src/static_gtfs.rs` | Exact-byte static validation and the pending/active lifecycle |
| `src/orchestrator.rs` | Source selection, one coherent generation, polling |
| `src/writer.rs` | Immutable generations on disk |
| `src/serve.rs` | axum HTTP API, peer check, and readiness |
| `src/bin/amtrak_status.rs`, `src/bin/status/` | `amtrak-status` CLI (`--features status`) |
| `advisory-fetcher/`, `track-fetcher/` | Python/Playwright browsers. Target: notices move into Alerts, status documents into TripUpdates. |
| `scripts/` | Feed validation, container smoke, release controls |
| `Dockerfile`, `docker-compose.yml` | Today's scratch service image and the Compose stack (service plus both sidecars) |
| `docs/rearchitecture.md` | The three-container design. Follow it for the split. |

There is no checked-in protobuf codegen step. The `gtfs-realtime` crate's build script compiles its `.proto` files with `protoc` at `cargo build` time.

HTTP contract. Do not add other feed URLs.

| Path | Access | Body |
| --- | --- | --- |
| `/livez` | public | Process liveness, independent of feed readiness |
| `/readyz` | controlled | `200` only while the current generation is younger than 300 seconds |
| `/v1/feed-set.json` | controlled | Generation id, timestamp, static version, entity counts, four URLs |
| `/v1/generations/{id}/static.zip` | controlled | Static GTFS for exactly that generation |
| `/v1/generations/{id}/{trip-updates,vehicle-positions,alerts}.pb` | controlled | One GTFS-Realtime product, `application/x-protobuf` |

Controlled routes authorize the direct socket peer. An empty allowlist admits loopback only. `Forwarded` and `X-Forwarded-For` are ignored. A non-loopback bind with no `AMTRAK_ALLOWED_PEER_IPS` refuses to start. `403` is an unauthorized peer, `404` is an unknown generation or artifact, `503` is no current or fresh generation.

Host defaults are bind `127.0.0.1:8080`, output `./out`, and validator JAR `./tools/gtfs-validator-v8.0.1-cli.jar`. The container defaults differ (`/data`, the hardened JAR, still loopback). The full variable table is in the README section "Configuration (environment variables)".

## Setup

- **Rust** stable, 1.85 or newer. The catenary crate uses edition 2024. This repo is developed against 1.96; CI uses `stable`; the release image pins `rust:1.96-alpine`.
- **`protoc`** (`protobuf-compiler` on Debian/Ubuntu).
- **`libssl-dev` and `pkg-config`**. `gtfs-structures` 0.46.1 pulls `reqwest` 0.12 with native TLS, so the build compiles `openssl-sys`. The service's own `reqwest` 0.13 uses rustls.
- **Python 3.12** for the sidecar unit tests. Pins are `advisory-fetcher/requirements.txt` (pytest, playwright) and `track-fetcher/requirements.txt` (pytest, cryptography). Playwright is imported lazily; unit tests do not download or launch a browser.
- **Java 17+** and the official MobilityData GTFS validator CLI 8.0.1 to run the service. Startup checks SHA-256 `19293ddd9b6f954f216d4f12054bd8a3232921751c4484339e339764a91000e2`. The container's repository-hardened JAR is a different digest and is not a substitute on the host. Unit tests do not start the JVM.
- **`jq`** for `scripts/check-release-metadata.sh`.

```bash
sudo apt-get update
sudo apt-get install -y protobuf-compiler libssl-dev pkg-config python3 python-is-python3 python3-pip jq
rustup toolchain install stable --profile minimal --component rustfmt,clippy
rustup default stable
python3 -m pip install --break-system-packages \
  --requirement advisory-fetcher/requirements.txt \
  --requirement track-fetcher/requirements.txt
```

Download the official JAR from the MobilityData v8.0.1 release to `tools/gtfs-validator-v8.0.1-cli.jar` and check the digest above before `cargo run`. That path is gitignored.

## Build, test, and lint

CI (`.github/workflows/ci.yml`) builds and tests the Rust crate without ignored tests, runs both sidecar suites, runs `scripts/test-release-controls.sh`, and checks Compose/Bake config. Release-quality Rust changes also pass fmt and clippy.

```bash
cargo build --locked
cargo test --locked                         # offline unit and integration tests
cargo test --locked --features status       # also the amtrak-status CLI
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings   # see the note below

(cd advisory-fetcher && python -m pytest tests -q)
(cd track-fetcher && python -m pytest tests -q)
scripts/test-release-controls.sh
```

Run the service on loopback (it fetches live Amtrak data once it is up):

```bash
cargo run
```

Station and train CLI, which reads a local generation by default:

```bash
cargo run --features status --bin amtrak-status -- station NYP --limit 10
cargo run --features status --bin amtrak-status -- train 167
```

Today's Compose file builds the scratch service and both browser sidecars, turns advisories and tracks on, and publishes the feed on `127.0.0.1:8090`:

```bash
docker compose up -d --build
```

`AMTRAK_TRACKS=off docker compose up -d --build --scale track-fetcher=0` runs without tracks and without the track sidecar's Chromium. The service image alone, advisories off, is `docker build --tag amtrak-gtfs-rt:local .`. After the split, Compose runs VehiclePositions, TripUpdates, and Alerts, and only VehiclePositions is published on that same host port.

Do not start Compose or the HTTP server in order to run the unit tests. They are offline.

`cargo fmt --all -- --check` is clean. Clippy with `-D warnings` is the bar described in the design notes, and CI does not run it. On Rust 1.99 it currently fails on pre-existing lints in `src/writer.rs`, `src/orchestrator.rs`, `src/sources/mod.rs`, `src/static_gtfs.rs`, `src/bin/status/source.rs`, and `examples/station_departures.rs` (`needless_return`, `useless_conversion`, `double_must_use`, `needless_borrows_for_generic_args`, `unnecessary_sort_by`, `print_literal`). Do not drive-by fix those unless the task is a clippy cleanup. The release image still builds with Rust 1.96.

The MobilityData gate is separate and, by default, fetches live feeds:

```bash
./scripts/validate-feeds.sh
```

It needs Java 17 and Maven, or Docker (automatic fallback). It is not part of pull-request CI.

## Coding conventions

Rust in this crate is edition 2021. Format with rustfmt defaults; there is no `rustfmt.toml` or `clippy.toml`.

- Keep the existing error style: small `Display` types (`ConfigError`, `StoreError`, `RefreshError`) that name the field or stage. Do not introduce `anyhow` or `thiserror` for config and publish errors.
- `TrackConfig`'s `Debug` reports only whether RailData credentials are set. Log lines and error strings must not contain URLs, headers, tokens, or passwords.
- Parse environment through `from_map` so tests inject a map instead of mutating process environment.
- Document public items, including `# Errors` on fallible public functions. Non-trivial modules open with a `//!` comment that states the invariant.
- Silence a lint with `#[expect(clippy::..., reason = "...")]`, not a blanket `allow`.
- The `status` feature stays off the default build. `cargo build --locked --release` must not compile `amtrak-status`.
- Sidecar Python uses `from __future__ import annotations`, the stdlib HTTP server, and pytest functions named `test_*`. There is no ruff or black config; match the surrounding module. Keep Playwright imports inside the functions that launch a browser.

## Testing expectations

- New behavior needs an offline test. Config goes through `from_map`. HTTP tests drive the axum router with an injected peer and clock. Source tests use `sources::mock`.
- Tests that hit the network stay `#[ignore]` with a reason. Run them with `cargo test -- --ignored` or `--include-ignored`. Do not un-ignore them to make CI call Amtrak.
- Ignored today: `sources::amtrak::live_fetch_returns_batch`, the orchestrator live candidate, `sources::advisories_live_tests`, `status::live_tests` (both need live Amtrak), and the manual `static_augment::augment_file` (needs `AUGMENT_IN`).
- Sidecar tests stay offline: no browser, no Amtrak, no RailData.
- `scripts/test-release-controls.sh` is the deterministic release-control suite. It uses local git fixtures only.
- A feed change that can produce a new MobilityData ERROR code needs a real fix or a `validation/baseline.json` entry that records the cause. Do not gate on occurrence counts; those follow how many trains are running. Amtrak's static GTFS is gated at zero ERROR notices.

## Data sources, feeds, and releases

Sources. One caller per upstream. Do not triplicate `getTrainsData`.

- Static GTFS: `https://content.amtrak.com/content/gtfs/GTFS.zip` (`AMTRAK_STATIC_URL`). TripUpdates is the only downloader. The other containers read the ZIP on the shared volume.
- Realtime trains: VehiclePositions is the only process that fetches and decrypts `getTrainsData` on `maps.amtrak.com`, through the catenary crate. TripUpdates selects trains near a stop from that snapshot. It does not GET the map feed again.
- Notices: background browser inside Alerts. Track and gate documents: background browser inside TripUpdates. Until those containers exist, the Playwright sidecars are the supported source. Point `AMTRAK_ADVISORIES_URL` and `AMTRAK_TRACKS_AMTRAK_URL` at them. An empty Amtrak track URL skips that source and logs a warning when tracks are on. A plain HTTP client to `www.amtrak.com` is reset by Akamai. Do not bypass that. No fingerprint spoofing, no curl-impersonate, no copied cookies, no login.
- Hartford Line board: `AMTRAK_TRACKS_HARTFORD_URL` (default the public connecting-train page). Set it empty to skip.
- NJ Transit RailData (`AMTRAK_TRACKS_RAILDATA_BASE`, default `https://raildata.njtransit.com/api/TrainData`) needs `NJT_RAILDATA_USERNAME` and `NJT_RAILDATA_PASSWORD` set together. Without them, NJ stations are skipped. One token is cached at `{AMTRAK_OUTPUT_DIR}/tracks/raildata-token.json` (mode `0600`, never served). A token is requested only when that cache is empty or older than 23 hours, and never more than 10 times in 24 hours. Any failed token request pauses NJ Transit for an hour. Do not point exploratory runs, including `scripts/validate-feeds.sh`, at production RailData credentials: a cold cache spends that daily budget. The `--tracks` flag of the validation script deliberately omits those credentials.

Published feed shape. Consumers already depend on this.

- One generation is one static ZIP plus three protobuf products. Each realtime header carries that generation's static version and timestamp. Do not mix artifacts across generations, and do not build generation URLs by hand when a manifest is available.
- Publication is atomic. Readers see a complete old generation or a complete new one. A source, conversion, validation, or disk failure leaves the current generation in place. Do not rewrite a published generation directory.
- Tracks on (the default) changes the published static feed. For each covered station the service adds a parent `{stop}:station`, re-parents the original stop under it, and adds `{stop}:track:{label}` with `platform_code`. A boarding gate is its own stop, `{stop}:track:{track}:gate:{gate}`, with `platform_code` like `24 (Gate H)`. GTFS-Realtime has no gate field, so the gate travels only through `assigned_stop_id`. `stop_sequence` is set whenever `assigned_stop_id` is. The static version gains a `+tracks.{digest}` suffix, where `digest` is the first 8 hex digits of the platform table. Every ZIP entry other than `stops.txt` is copied byte-for-byte. `AMTRAK_TRACKS=off` publishes the upstream static feed with no platform stops and no suffix.
- Before validation, stop times that name a stop missing from `stops.txt` are dropped and a warning is logged (`stop times name a stop missing from stops.txt; dropped`). Amtrak's 20261018 feed calls at Ocean Springs (`OSM`) without defining it. A feed with no such rows is unchanged.
- The mutable routes `/trip-updates.pb`, `/vehicle-positions.pb`, `/alerts.pb`, `/static.zip`, and `/health` are removed. Do not add them back.

Release workflow:

- User-visible feed, config, or image changes go under `## [Unreleased]` in `CHANGELOG.md` (Keep a Changelog, semver).
- `Cargo.toml` `version` is the released version. A release commit moves Unreleased into `## [X.Y.Z] - YYYY-MM-DD` and must pass `scripts/check-release-metadata.sh` (version, tag form `vX.Y.Z`, changelog heading, `AGPL-3.0-only`, and `THIRD_PARTY_LICENSES.html`).
- Pushing a `v*.*.*` tag starts `.github/workflows/release.yml`. It publishes only the scratch service image, `linux/amd64` and `linux/arm64`, to `ghcr.io/sohampatwardhan/amtrak-gtfs-rt`. It does not publish the browser sidecars; build those from this repo.
- A release is complete only when the GitHub Release carries `image-release.txt` with the tag, revisions, manifest digest, and both platform digests. Until then the previous complete release stays the stable image. Current complete release is `v0.6.2`. Tags `v0.3.0`, `v0.6.0`, and `v0.6.1` never finished publication; do not deploy them or their partial images.
- Pull requests that touch image inputs (`Dockerfile`, `.dockerignore`, `container/`, `src/config.rs`, `Cargo.toml`, `Cargo.lock`, or the smoke, scanner, and policy scripts) run `.github/workflows/release-gates.yml`: a linux/amd64 image build, `scripts/test-container.sh` (live Amtrak fetch), and the pinned Grype zero-match policy. arm64 is still checked only by the release itself.
- `./scripts/validate-feeds.sh` runs on a schedule and on pushes that touch the pipeline, not on every pull request, so an Amtrak outage does not block unrelated work.

## What agents must not do

- Do not commit secrets, `.env` files, RailData tokens, or `raildata-token.json`. Pass `NJT_RAILDATA_USERNAME` and `NJT_RAILDATA_PASSWORD` only through the environment or Cursor's Secrets tab.
- Do not commit `tools/*.jar`, `/out`, `/target`, or `validation-reports/` other than the two release evidence files already tracked.
- Do not change the generation API, platform-stop ids, `platform_code` text (`24 (Gate H)`), or the `+tracks.{digest}` suffix without an explicit consumer-migration note in the README.
- Do not serve the generation directory with a generic file server. That bypasses peer checks, readiness, and manifest-first discovery.
- Do not treat `X-Forwarded-For` or `Forwarded` as the client.
- Do not make a missing notices browser, a RailData outage, or a Hartford failure fail the generation. VehiclePositions still publishes the last complete generation.
- Do not put Chromium in the VehiclePositions image. That image stays scratch and shell-free, as today's root `Dockerfile` runtime stage does. Chromium belongs only in TripUpdates (resident, for status documents) and Alerts (one notices cycle, then exit).
- Do not let an HTTP handler wait on a browser, a map poll, or any other upstream call. Serve the warm cache. A cold browser launch is a scheduler concern, not a request-path concern.
- Do not advance `/v1/feed-set.json` until `static.zip`, `trip-updates.pb`, `vehicle-positions.pb`, and `alerts.pb` for that generation id are all present and the three protobuf headers share its timestamp and static version.
- Do not poll `getTrainsData` from more than one process.
- Do not push tags, publish to GHCR, or run the release workflow unless the user explicitly asks.
- Do not spend NJ Transit's 10-token daily budget on exploratory logins.
- Do not weaken the validator digest check, the baseline gate, or the container vulnerability policy to make a build pass.

## Pull requests and commits

- Branch from `main`. Commit subjects are imperative sentences with a period, specific enough to stand alone (`Publish Amtrak's boarding gates and turn tracks on by default.`).
- `main` is merged with merge commits (`Merge pull request #N`). Do not rewrite published history.
- Update `CHANGELOG.md` for user-visible changes. Leave the `Cargo.toml` version and git tags to an intentional release.
- Run the offline commands in "Build, test, and lint" before pushing. Say which ignored live tests you did not run.
- A PR that changes image inputs should expect the release-gates workflow, which needs network access to Amtrak and can fail when Amtrak is down.

## Cursor Cloud specific instructions

Cloud Agent setup is `bash .cursor/install.sh`, from `.cursor/environment.json`. The script is idempotent and uses Cursor's default image. There is no `.cursor/Dockerfile`, and there is no `start` or `terminals` entry: unit tests do not need the HTTP server, the sidecars, or Docker.

The script installs `protoc`, `libssl-dev`, `pkg-config`, `python`, `jq`, and CA certificates when they are missing; installs Rust stable with `rustfmt` and `clippy` (the image's preinstalled 1.83 cannot compile the edition 2024 dependency); installs both sidecar requirement files; runs `cargo fetch --locked`; and downloads the official GTFS validator JAR to `tools/gtfs-validator-v8.0.1-cli.jar` after the SHA-256 check. It installs OpenJDK 17 only when `java` is missing or older than 17. The default image already has a newer JDK, which satisfies the service's Java 17 minimum.

After install, the commands in "Build, test, and lint" are the verification set. `cargo test --locked` does not run the ignored live tests.

`cargo run` listens on `127.0.0.1:8080` and needs outbound HTTPS to `content.amtrak.com` and `maps.amtrak.com`. With tracks left on and the Hartford URL left at its default, it also calls `hartfordline.com`. It does not call `www.amtrak.com` unless an advisory or Amtrak-track URL points there, and that call will fail closed at Akamai without the sidecars. Advisories stay off unless `AMTRAK_ADVISORIES` is set. That single process is the current tree. The three-container split in `docs/rearchitecture.md` is what new container work implements.

Secrets, only for NJ Transit tracks. Add them in Cursor's Secrets tab. Do not put them in the repo, Compose, or this file.

- `NJT_RAILDATA_USERNAME`
- `NJT_RAILDATA_PASSWORD`

`cargo build`, `cargo test`, and `cargo run` all work with those unset; NJ stations are skipped. Ignored tests and `./scripts/validate-feeds.sh` call live Amtrak endpoints and can fail when Amtrak is unreachable. They are not part of install verification. Do not export RailData credentials for that script.

The release workflow needs GitHub Actions and GHCR credentials that this environment does not have. Do not push a tag to obtain them.
