# Discovery: Real-Time Track Assignments

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Problem and Outcome

NJ Transit's DepartureVision data and the CTrail Hartford Line connecting-train board both publish the
track (platform) an Amtrak train will use at stations those operators share with Amtrak. The service's
GTFS-Realtime feed cannot carry that today, so riders using standard transit apps see a station but
no platform.

[PR #17](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/17) attempted this but has four defects
this feature replaces:

1. It writes `assigned_stop_id = "NWK:track:4"` while keeping `stop_id = "NWK"`. The GTFS-Realtime
   reference requires `assigned_stop_id` to refer to a `stop_id` in `stops.txt`, and requires
   `stop_id` to match it when both are set. Spec-following consumers cannot resolve the value.
2. Its primary NJ Transit path replays the DepartureVision web client's obfuscated session handshake
   with keys copied from NJT's JavaScript, bypassing NJT's registered RailData access.
3. When a board fetch fails, the last assignments are kept forever and keyed only by train number, so
   a later day's train can receive an earlier day's track.
4. Board refreshes run inside the generation path with sequential 12-second timeouts, so an NJT outage
   can delay every generation by minutes.

**Outcome:** trip updates carry spec-conformant platform assignments at the covered stations, from
authorized sources, that are never stale and never slow the feed.

## Users and Current Workaround

- **Transit-app consumers** (Transit, OpenTripPlanner-based apps) reading the published feed set.
  Today they get no platform and riders check station boards.
- **The operator's own tools** (`amtrak-status`, Trackside) reading the same feed.

## Scope and Non-Goals

**In scope**

- When tracks are enabled, the published `static.zip` gains a fixed, configured set of platform stops
  for each covered station: a new parent station, the original Amtrak stop re-parented under it, and
  one child platform stop per track with `platform_code`. All other Amtrak rows are unchanged.
- Trip-update stop times at covered stations set `assigned_stop_id` to the platform stop, populate
  `stop_sequence`, and omit `stop_id`, as the reference prefers. Skipped stops are never assigned.
- NJ Transit data comes only from the registered RailData API (`getToken` + `getTrainSchedule19Rec`).
  The copied DepartureVision bootstrap and its embedded keys are removed.
- The Hartford Line board continues to supply New Haven Union (`NHV`).
- Assignments expire after a bounded age. Boards refresh in a background task; generation reads the
  latest result without waiting on the network.
- `amtrak-status` renders the platform from the assigned stop's `platform_code`.
- Feed validation in CI also runs with tracks enabled against fixtures.

**Non-goals**

- Rewriting `VehiclePosition.stop_id` to the platform. The reference says it "should" reflect the
  assignment; leaving the station stop is conformant and avoids changing a field consumers join on.
- A separate JSON track endpoint.
- Learning new platforms automatically from observed data.
- Newark Airport, Princeton Junction, and New Brunswick until live RailData responses confirm Amtrak
  tracks are published there. Adding a station is a configuration change.

## Constraints and Success Measures

- **Static integrity:** the standards validator must accept the exact bytes that are published, with
  zero `ERROR` notices, and the static version must stay stable between Amtrak feed changes.
- **Fail-open:** with tracks disabled, or when augmentation or a board fails, the service publishes
  what it publishes today.
- **NJT limits:** RailData tokens last 24 hours and `getToken` is limited to 10 calls per day, so the
  token must be cached and survive restarts.
- **Credentials:** RailData credentials come from the environment, are never logged, and are never
  committed.
- **Measure:** a fixture generation with tracks enabled passes the static validator and the
  GTFS-Realtime validator with no new `ERROR` codes, and every assignment resolves to a platform stop
  whose parent is the scheduled stop's parent.

## Approaches Considered

| Approach | Benefits | Costs / risks | Decision |
|---|---|---|---|
| Fixed configured platform table; deterministic augmentation of each upstream snapshot | Spec-conformant; static version changes only with the Amtrak feed or the table; consumers load platforms once | Needs a curated track list per station; an unlisted track is not published | **Chosen** |
| Add platform stops as tracks are first observed | No curated list | Every new track changes the static version and forces consumers to reload static data; unbounded growth | Rejected |
| Generate tracks 1–N for every station | No curation | Publishes platforms that do not exist | Rejected |
| Keep DepartureVision bootstrap or use the legacy `NJTTrainData.asmx` service | No registration | Access-control and terms risk; the SOAP service is legacy | Rejected |
| Refresh boards inside each generation | Simple | A slow board delays the whole feed | Rejected |

## Chosen Direction

Keep PR #17's board parsers and track-label rules, and replace its output model and NJT access. A
platform table (defaults plus environment overrides) drives a deterministic transform of the upstream
static ZIP. The transformed bytes are what the validator checks and what the feed set publishes. The
static version becomes the upstream `feed_version` joined to a short digest of the platform table. A
background refresher owns the RailData token and both boards and publishes a timestamped assignment
snapshot; the generation path reads it, drops entries past their age, and stamps only tracks that
exist in the table.

## Architecture and Flow Outline

1. **Static path:** Amtrak `GTFS.zip` → platform augmentation (tracks enabled) → standards validation →
   published `static.zip` and the parsed GTFS used for matching.
2. **Board path:** background refresher → RailData token cache (persisted in `/data`) → per-station
   `getTrainSchedule19Rec` and the Hartford board → timestamped assignment snapshot.
3. **Generation path:** Amtrak realtime batch → read the assignment snapshot → stamp platform stop ids
   on matching stop times → existing orchestrator validation → publish.

## Failure and Verification Strategy

- Augmentation failure falls back to the unmodified upstream snapshot and disables stamping for that
  snapshot. Board or token failures leave assignments to expire. Neither fails a generation.
- Unit and fixture tests cover the transform, the version derivation, stamping, expiry, token caching,
  and parser behavior. The static validator and the GTFS-Realtime validator run on a tracks-enabled
  fixture generation in CI.

## Open Decisions

- Default track list per station, with evidence, and the parent-station id convention. Resolved in
  design.

## Approval

Status: **Approved on 2026-09-28** (the user granted standing approval for every spec-driven gate).
