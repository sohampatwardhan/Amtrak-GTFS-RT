# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.1] - 2026-10-19

### Security

- The rebuilt MobilityData validator now bundles jsoup 1.23.2 instead of 1.23.1, fixing
  GHSA-65r4-943x-97jj (High). The hardened validator JAR's pinned SHA-256 is now
  `bc3ec6fb672fe45e96fd28e559301c075c752d10172e7b90e5ec3c1f29011a15`, and the service accepts that
  digest. jsoup 1.23.2 adds no runtime dependency; its POM imports `netty-bom` 4.2.17.Final, whose
  checksum is now in the dependency verification metadata.

### Notes

- `v0.6.0` never completed publication: its release stopped at the zero-match vulnerability gate
  on the jsoup advisory above. Its changes ship in this release; see the 0.6.0 section for the
  full list (Amtrak's own track numbers through the `track-fetcher` sidecar, RailData at every
  shared station, static feeds that name an undefined stop, bounded generation retention).

## [0.6.0] - 2026-10-19

### Added

- **RailData coverage for every station Amtrak and NJ Transit both serve.** The default poll is
  New York Penn, Newark Penn, Newark Airport, Metropark, New Brunswick, Princeton Junction,
  Trenton, and Philadelphia 30th Street (`NY`, `NP`, `NA`, `MP`, `NB`, `PJ`, `TR`, `PH` mapped to
  `NYP`, `NWK`, `EWR`, `MET`, `NBK`, `PJC`, `TRE`, `PHL`). A track from that board is published
  even when it is not in the seeded New York, Newark, and New Haven platform lists. All eight
  stations share one cached token.
- **Amtrak's own track numbers** (still default-off). Set `AMTRAK_TRACKS_AMTRAK_URL` to the
  `track-fetcher` sidecar and `AMTRAK_TRACKS=on`. The sidecar reads
  `GET /dotcom/travel-service/statuses/{train}?service-date={YYYY-MM-DD}` — the document the
  Amtrak app uses for `departure.track.number`, falling back to `arrival.track.number` — for
  trains with a stop inside a 30-minute lookahead and a 10-minute lookback. A train whose next
  stop is within about 12 minutes and still has no track is fetched every minute; the rest of
  that window rotates under a 90-second cooldown with at most 24 fetches a cycle, using one
  resident headless browser. A track stays published until the stop's latest time plus
  `AMTRAK_TRACKS_RETAIN_GRACE_SECS` (default 900), including when a later document omits it.
  Amtrak wins over NJ Transit and the Hartford board while its document is fresh; an older
  Amtrak track yields to a live board reading. An Amtrak track that is not already in
  `AMTRAK_TRACKS_PLATFORMS`, at a station in the static feed, is recorded in
  `observed-platforms.txt` (observed labels only, at most 1000) and becomes a platform stop on a
  static rebuild at most every 10 minutes; if the validator rejects it, static is rebuilt with
  the configured platforms alone. The sidecar backs off browser relaunches, times out each
  request, blocks hosts outside `amtrak.com`, serves `503` when every fetch fails, and fails
  `/healthz` when its poll loop stalls; in Compose it runs read-only with no capabilities and a
  768 MB memory limit. Unset the URL to leave this source off. `docker buildx bake` does not build
  the sidecar; CI builds it on amd64 and arm64.
- **Time- and count-bounded generation retention.** A superseded generation is deleted once it
  has been superseded for `AMTRAK_GENERATION_RETAIN_SECS` (default 600), measured from the newer
  generation's manifest mtime so the window holds across restarts. `AMTRAK_GENERATION_MAX`
  (default 20, current included) caps how many are kept on disk and in memory. The current feed
  set is never deleted.
- **Byte-capped generation retention.** `AMTRAK_GENERATION_BUDGET_BYTES` (default 256 MiB, `0` to
  disable) deletes the oldest immutable generations once unique on-disk bytes exceed the cap.
  The generation referenced by the current feed set is never deleted. Unchanged static GTFS is
  stored once under `blobs/<sha256>` and hard-linked from each `generations/<id>/static.zip`, so
  served URLs stay `/v1/generations/{id}/static.zip`.
- **Static validator cache.** `AMTRAK_VALIDATOR_CACHE` (default on) skips the Java GTFS validator
  when a ZIP's SHA-256 already has an accept or standards-reject result. Tool failures and
  timeouts are not cached.
- **Advisory render limits.** The fetcher re-renders the notices list on its existing 15-minute
  poll, stops each page once the HTML size is stable instead of waiting on page timers, and
  reuses detail pages for `DETAIL_TTL_SECS` (default 24 hours).
- **`amtrak-status` delay deltas.** Train and station output now annotate each predicted time
  with how it compares to the static schedule — `(on time)`, `(12 min late)`, `(3 min early)` —
  and the train-level `delay:` line uses the same label. The delay prefers the feed's own
  GTFS-Realtime `delay` when present and otherwise derives it as predicted time minus scheduled
  time, with the scheduled instant rebuilt in the stop's timezone and aligned to the service
  day nearest the prediction. Amtrak's feed publishes predicted times without `delay` values,
  so the derivation is what normally produces the number; previously the `delay:` line could
  only appear when a feed supplied `delay` explicitly, and per-stop times carried no delay
  information at all.

### Changed

- A static GTFS feed that does not parse now reports why, for example
  `static parse failed: The id OSM is not known` for a stop time that names a stop missing from
  `stops.txt`, instead of a bare `static parse failed`. A rejected daily static refresh logs the
  same reason. Parse errors come from in-memory bytes and never include a URL or credential.
- The README is reorganized: GitHub callouts for warnings and requirements, diagrams for the
  architecture, the manifest-first request flow, the Compose stack, platform stops, and the static
  snapshot lifecycle, a table of the stations where Amtrak's own status posted a track, and a
  section for the `amtrak-status` CLI.
- The container Java runtime's `jlink` set now includes `java.sql` (Gson loads `java.sql.Time`
  while parsing GTFS dates; without it the CLI does not produce a report) and strips debug
  symbols. `java.desktop` remains because notice introspection loads `java.beans`. The validator
  process is started with `-Djava.awt.headless=true`.
- The advisory-fetcher image installs Playwright's Debian 12 Chromium libraries and
  `chromium-headless-shell` directly, instead of `playwright install --with-deps`, which also
  pulled xvfb and the large font packs. Compose sizes `/dev/shm` at 64 MiB because the browser
  is launched with `--disable-dev-shm-usage`.

### Fixed

- **Amtrak's static schedule is accepted again when it names an undefined stop.** In Amtrak's current feed
  (20261018) the Mardi Gras Service calls at Ocean Springs (`OSM`), which `stops.txt` does not
  define, so every static refresh failed with `The id OSM is not known` and the service kept the
  last accepted schedule. With tracks on, this also meant no track was ever stamped. Stop times
  naming a stop that `stops.txt` lacks are now dropped before parsing and validation, with a
  warning naming the stop; the rest of each trip is kept.
- **RailData login errors no longer spend the daily token budget.** NJ Transit answers failed
  logins with HTTP 500 and a JSON `errorMessage` (for example `Missing user account.`); these were
  treated as transport errors, so the service requested a new token for every station and used all
  10 of the day's requests within a minute. Such answers are now classified as rejected credentials
  (or as NJ Transit's daily limit when the message says so), and any failed token request pauses
  NJ Transit requests for an hour.
- **Turning tracks on or off takes effect at startup.** A restart recovered the static feed saved
  by the previous run, so platform stops appeared (or disappeared) only at the next daily static
  refresh and every track was dropped until then. When the recovered static was built for a
  different track configuration, the first static refresh now runs immediately.
- **`BROWSER_MODE=xvfb-headful` works in both browser sidecars.** Neither image had full
  Chromium or Xvfb, and nothing started an X server, so the documented fallback failed at launch.
  Build with `--build-arg BROWSER_BUNDLE=full` (or `FETCHER_BROWSER_BUNDLE=full` for Compose and
  bake) to add both; the fetcher then starts Xvfb itself. The default image is unchanged, and
  selecting headful mode on it stops at startup with a message naming the build argument. CI
  builds both bundles of both images on amd64 and arm64.
- Startup no longer reads every generation on the volume into memory. It loads the current
  generation, prunes the rest by directory metadata, and then loads only the survivors. A volume
  with hundreds of generations left by an older release previously pushed the process past
  5 GB RSS within a minute of starting.

## [0.5.0] - 2026-09-28

### Added

- **Live platform and track assignments** (default-off). With `AMTRAK_TRACKS=on`, the published
  static feed gains a parent station and one platform stop per configured track (`{stop}:track:{label}`
  with `platform_code`) at each covered station, and trip updates point
  `stop_time_properties.assigned_stop_id` at the platform stop with `stop_sequence` set and
  `stop_id` omitted, as the GTFS-Realtime reference requires. Tracks come from NJ Transit's
  registered RailData API (New York Penn, Newark Penn, Newark Airport, Metropark, New Brunswick,
  Princeton Junction, Trenton) and the CTrail Hartford
  Line board (New Haven Union). Defaults cover New York Penn, Newark Penn, and New Haven Union;
  other tracks are added with `AMTRAK_TRACKS_PLATFORMS`. Boards refresh in the background,
  readings expire after five minutes, and the RailData token is cached so the service stays within
  NJ Transit's 10 token requests a day. The static version gains a `+tracks.{digest}` suffix while
  tracks are on.

## [0.4.0] - 2026-09-27

### Added

- **Advisory detail text in `alerts.pb`.** The advisory-fetcher now snapshots each
  advisory's detail page, and the service copies its paragraphs, effective date, and
  PSN into the alert's `description_text`. A missing detail page keeps the title-only
  description.
- **`amtrak-status` consumer** for station departures and train status, reading
  either the local feed set or Amtrak directly.
- **Root-level packaging.** `docker compose up -d --build` at the repository root
  builds and runs the scratch service and the advisory-fetcher together with
  advisories on, published on host loopback port 8090. `docker buildx bake` builds
  both images without starting them. Chromium stays out of the service image.

### Changed

- CI now also runs the advisory-fetcher's offline tests, the release-control
  fixtures, and a Compose/Bake config check, and release scan failures upload the
  reports that explain them.

### Notes

- `v0.3.0` never completed publication; its changes ship in this release.

## [0.3.0] - 2026-08-18

### Added

- **Service advisories as scoped GTFS-RT alerts** (default-off). When enabled, the
  service turns Amtrak's Service Alerts & Notices into alert entities: station
  advisories become stop-scoped (`stop_id`) and passenger advisories become
  route-scoped (`route_id`). It is fail-open — an unreachable source, unexpected
  markup, or an unmappable station never fails feed generation — and is configured
  through `AMTRAK_ADVISORIES` and `AMTRAK_ADVISORIES_URL`.
- **`advisory-fetcher` sidecar** (`advisory-fetcher/`): a standalone Python +
  Playwright container that runs a headless Chromium to defeat Amtrak's Akamai bot
  gate (which blocks plain requests) and serves the advisories HTML over HTTP for
  the service to consume. It is memory-efficient (launch-per-poll, a single browser
  per cycle with none resident between polls, subresource-blocking) and fail-open,
  and ships as its own image so the service keeps its scratch/musl,
  no-vulnerabilities posture. Verified end-to-end on a Raspberry Pi 5.

## [0.2.0] - 2026-08-14

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
- **Public multi-platform container release** for `linux/amd64` and `linux/arm64`
  through GitHub Container Registry, with immutable-digest installation support.
- **Release licensing evidence** containing the project AGPL text, third-party
  notices, per-platform SPDX SBOMs, vulnerability reports, and provenance.

### Changed

- Replaced the Debian/JRE container with a package-manager-free, non-root scratch
  runtime and a source-built MobilityData validator whose Gradle distribution,
  dependency graph, source archive, and final JAR are checksum-verified. The
  exact final image has a retained SPDX SBOM and zero-match Grype report.
- Replaced mutable top-level feed files with immutable generation routes.
- Split mixed upstream entities into type-correct GTFS-Realtime products, removed
  invalid FULL_DATASET deletions, normalized headers, and filtered unresolved
  static references.
- Static GTFS replacements now pass the same pinned standards validator at
  runtime before they can participate in a committed generation.
- Long-lived task supervision now cancels and drains sibling activities when the
  poller, static refresher, or HTTP server exits or fails.

### Known issues

- The complete Cargo audit retains nine inherited advisory warnings with no
  confirmed CISA KEV match. They are not treated as clean, although none blocks
  this container-only remediation.

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

[Unreleased]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/compare/v0.6.1...HEAD
[0.6.1]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.6.1
[0.6.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.6.0
[0.5.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.5.0
[0.4.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.4.0
[0.3.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.2.0
[0.1.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.1.0
