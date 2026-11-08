# Design Document

> **Superseded on 2026-11-07** by [`docs/one-container.md`](../../../docs/one-container.md). The owner chose one container with every helper inside it. This spec records how the three-container roles were built, and the one-container plan removes them.

## Overview

Three processes link `amtrak-gtfs-rt-service`. `AMTRAK_ROLE` selects the role. Unset means the combined process, so `cargo run` and the existing image smoke test stay valid. Compose runs the three roles. VehiclePositions polls once, writes `fleet/{id}`, and publishes a generation only when `contrib/{id}/trip-updates.pb` and `contrib/{id}/alerts.pb` exist. HTTP reads that generation from memory.

This covers requirements 1.1–1.4, 2.1–2.4, 3.1–3.7, 4.1–4.6, and 5.1–5.5.

gtfs-mcp and Trackside are the two consumers. Both read `feed-set.json`, then `official-static.zip`, `platform-addon.zip`, and the three protobufs. `static.zip` remains for callers that already load the combined file. New public names are additions. Existing URLs, content types, manifest fields, and protobuf header meanings stay.

`static.zip` stays the combined schedule. `official-static.zip` is the download. `platform-addon.zip` is schema `amtrak-platform-addon/v1`: `addon.txt` lists the files to copy over the official zip. Those files are the repaired `stop_times.txt` and the platform `stops.txt` when they differ. The Java validator stays in the vehicle-positions image. Vehicle-positions keeps the public port. A Caddy proxy is optional and off unless Compose is started with `--profile proxy`. The deployment does not assume nginx or Tailscale Serve in front.

```mermaid
---
config:
  layout: elk
---
flowchart LR
  client["Client"] --> direct["127.0.0.1:8090"]
  client --> proxy["Caddy profile proxy"]
  direct --> vp["vehicle-positions"]
  proxy --> vp
  vp --> feeds["feed-set and artifacts"]
```

```mermaid
---
config:
  layout: elk
---
flowchart LR
  maps["getTrainsData"] --> vp["VehiclePositions"]
  vp --> fleet["fleet snapshot"]
  fleet --> tu["TripUpdates"]
  fleet --> al["Alerts"]
  tu --> contrib["contrib files"]
  al --> contrib
  contrib --> vp
  vp --> http["Public /v1"]
```

## Architecture

VehiclePositions owns the poll, the static refresh (the image already has the validator), the HTTP server, and the early-refresh queue. TripUpdates owns the track browser, NJ Transit, Hartford, and trip-update stamping. Alerts owns the notices browser and alert merging. The optional Caddy service is not one of the three required containers.

```mermaid
sequenceDiagram
  participant C as Client
  participant V as VehiclePositions
  participant T as TripUpdates
  participant A as Alerts
  C->>V: GET trip-updates.pb?train=167
  V-->>C: cached protobuf
  V->>V: enqueue 167
  Note over V: debounce then write early-refresh.json
  T->>T: hot poll reads the file and the lead-time profile
  T->>V: contrib trip-updates.pb
  A->>V: contrib alerts.pb
  V->>V: publish generation when both exist
```

Track scheduling states:

```mermaid
---
config:
  layout: elk
---
stateDiagram-v2
  [*] --> Waiting
  Waiting --> FullPoll: base interval
  Waiting --> HotPoll: hot tick
  Waiting --> Early: debounced hint
  FullPoll --> Waiting
  HotPoll --> Waiting
  Early --> Waiting: cap and min gap applied
```

## Components and Interfaces

`TrackScheduler` records hints and releases trains. `LeadProfile::load` reads `amtrak-station-lead-time/v1`. `write_fleet` and `write_contribution` are the disk contract. `serve::artifact` adds `ETag`, `Last-Modified`, and `304`, and spawns the interest hook.

```mermaid
classDiagram
  class TrackScheduler {
    +enqueue_train(train, now)
    +enqueue_station(station, now)
    +take_early(now) Vec
    +base_due(now) bool
  }
  class LeadProfile {
    +load(path, before, hot) LeadProfile
    +window_for(station) StationWindow
  }
  class FleetMeta {
    +id: String
    +generated_at_unix: u64
    +static_version: String
    +addon_version: String
  }
  TrackScheduler --> FleetMeta : hints never block HTTP
```

The Python track poller loads the same profile and reads `tracks/early-refresh.json`. Its base interval is `AMTRAK_TRACKS_BASE_INTERVAL_SECS` (default 900). Between those runs it still fetches hot trains, so a station such as PVD keeps its tighter hot window.

## Data Models

```mermaid
erDiagram
  FLEET ||--o| CONTRIB_TRIPS : decorated_by
  FLEET ||--o| CONTRIB_ALERTS : decorated_by
  FLEET ||--|| STATIC_ZIP : matched_against
  GENERATION ||--|| FLEET : published_from
  PROFILE ||--o{ STATION : adopts
```

A profile station is adopted only when `confidence` is `observed` and `compared_to_default` is `wider`. `sparse`, `no_track`, `none`, `covered`, and unknown stations keep the global lookahead and hot window. The lookback stays global.

```mermaid
---
config:
  layout: elk
---
stateDiagram-v2
  [*] --> Snapshot
  Snapshot --> WaitingOnSiblings: fleet written
  WaitingOnSiblings --> Published: trip updates and alerts exist
  WaitingOnSiblings --> Snapshot: a newer poll replaces CURRENT
  Published --> Snapshot: next poll
```

## Error Handling

A bad profile, a failed browser, or a failed stamp does not fail the HTTP server. The assembler logs and leaves the current generation in place. A denied peer is `403`. An unknown generation is `404`. No current generation is `503`.

```mermaid
---
config:
  layout: elk
---
flowchart TB
  poll["Fleet poll"] --> wrote{"Snapshot written"}
  wrote -->|no| keep["Keep current generation"]
  wrote -->|yes| wait["Wait for both contribs"]
  wait --> valid{"Validation accepts"}
  valid -->|yes| publish["Move current marker"]
  valid -->|no| keep
```

## Testing Strategy

Rust tests cover debounce, coalescing, the 24-train cap, the minimum gap, profile adoption, the cohort marker, `304`, a warm p99 under 100 milliseconds, and a trip-update response that returns while a two-second hook is still running. Python tests cover the same profile rules and the early-refresh cap. Live Amtrak tests stay ignored.
