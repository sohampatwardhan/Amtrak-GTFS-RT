# Discovery: Station & Train Status Queries

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Problem and Outcome

The service publishes coherent, immutable GTFS-Realtime generations (static schedule + trip
updates + vehicle positions + alerts) but offers no way to answer the questions Amtrak's own
travel-status pages answer — *"Search By Train Station"*, *"Search By Train Number"*, and the
**delay/service notifications** shown alongside them:

- **By station:** *"what are the next Amtrak departures from this station?"* — join the static
  `stop_times` schedule with real-time `TripUpdate.stop_time_update` predictions for one station,
  ordered by time.
- **By train number:** *"where is train N and how is it doing?"* — locate the trip whose Amtrak
  train number (GTFS `trip_short_name`) is N and report its live status: current position, delay,
  and upcoming stops with predicted times.
- **Status / alerts (highest priority for completeness):** surface the Amtrak Service Message tied
  to a train — e.g. *"As of 10:09 AM ET, Acela 2159 has departed Boston (BOS) and is currently
  operating approximately 25 minutes late."* This text lives in the GTFS-RT **`alerts`** feed
  (`Alert.description_text`, `informed_entity` → the trip). Without it, a board or train view shows
  a time but not the *reason/status*, so it is data-incomplete.

The desired outcome is a **station & train status query capability** — a standalone consumer that
turns the existing feeds into an accurate, time-ordered station departures board and a per-train
status view, **each enriched with the relevant service alerts**, with friendly route names and
correct local times. A throwaway consumer already proved the station join end to end against live
data (New Haven / NHV); this spec turns that proof into a maintainable, tested component covering
both query modes and their alerts, without changing the feed producer.

## Users and Current Workaround

The primary user is a **downstream integrator or operator** who has the feeds but wants
station-level and train-level answers *with their status*. Today they must fetch the static GTFS
`.zip`, the `trip-updates.pb`, the `vehicle-positions.pb`, and the `alerts.pb`, decode all of them,
and discover the non-obvious join keys before they can answer any of the questions:

1. the real-time station `stop_id` is actually the **Amtrak station code** (e.g. `NHV`), not the
   GTFS `stop_id`;
2. the Amtrak **train number** is the GTFS `trip_short_name` (e.g. route 88 / Northeast Regional
   runs trains `#199`, `#150`, `#86`), which the crate also tags into each entity id as
   `{date}-{trainnum}`; and
3. a **service alert** is linked to a train through `Alert.informed_entity` (route + trip
   descriptor), so it must be joined back to the same trip to appear on that train's status and on
   the boards of stations the train serves.

Amtrak's website exposes exactly these lookups (the `travel-status` "Search By Train Station /
Train Number" form and the `train-status.html` delay notifications); this feature brings the same
answers, with status, to consumers of our GTFS-RT feeds.

The validation tool ([`examples/station_departures.rs`](../../examples/station_departures.rs)) automated the station lookup for one station
but prints UTC and raw `route_id`s, ignores alerts, covers only the station mode, and talks directly
to Amtrak — deliberately minimal, not a product.

## Scope and Non-Goals

In scope:

- **Alerts / status (high priority):** join the `alerts` feed to trips via `informed_entity` and
  attach the service-message text to the matching train (train mode) and to affected departures
  (station mode). Data completeness for both modes depends on this.
- **Station mode:** resolve a caller-supplied station identifier (Amtrak code such as `NHV`, and/or
  GTFS `stop_id`, and/or a name match) and produce an ordered list of **upcoming** departures:
  time, route (friendly name), trip identity/headsign, real-time vs. scheduled status, and any alert
  on that train.
- **Train-number mode:** resolve a caller-supplied Amtrak train number (GTFS `trip_short_name`) for
  the active service day and produce that train's status: current position (from
  `vehicle_positions`), overall delay, remaining stops with predicted times, and its alert text.
- A **shared consumer core** that loads the feeds once (static + trip updates + vehicle positions +
  alerts) and serves both modes.
- Correct **local-time** presentation (Eastern with DST for the NEC; ideally per-stop timezone).
- A reusable library boundary plus at least one thin entry point (CLI) exercising both modes.
- Automated tests over fixture feeds so the join paths (including alert attachment) are verifiable
  offline.

Out of scope (this increment):

- Any change to the feed-producer service, its routes, or its authorization model.
- A long-running HTTP query service or public exposure (candidate *next* increment, see Open
  Decisions).
- Multi-agency / non-Amtrak feeds; trip planning or routing between stations; historical/past
  departures; authoring or editing alerts (we render upstream alerts, not create them).

## Constraints and Success Measures

Constraints:

- Reuse the proven data path: `gtfs-structures` for static GTFS and the `amtrak-gtfs-rt` crate for
  decrypted real-time — no re-implementation of decryption, trip matching, or ASM-alert parsing.
- Join keys are fixed by the upstream data and must be explicit and tested: station mode joins on
  the **Amtrak station code** in `StopTimeUpdate.stop_id`; train mode joins on **`trip_short_name`**
  (the Amtrak train number); alerts join on **`Alert.informed_entity`** (route + trip descriptor)
  back to the trip.
- Must not add runtime dependencies to the shipped service binary; consumer-only dependencies are
  acceptable and are subject to the project's dependency-audit posture.
- Honor the producer's model: when consuming the local service, read a **single immutable
  generation** so static schedule, trip updates, vehicle positions, and alerts are mutually
  coherent.

Success measures:

- For a live station (NHV baseline), the departures board matches independent observation of Amtrak
  departures in ordering and within a small time tolerance, **and** a delayed train's alert (e.g.
  Acela 2159's delay notification) appears on its row.
- For a live train number, the status view identifies the correct trip, current position, next-stop
  predictions, **and** the same alert text Amtrak shows on `train-status.html`.
- Edge cases handled without dropping or mislabeling rows: terminating trips (arrival-only),
  canceled stops, a train number with more than one same-day trip, and a train with no alert.
- Route IDs render as names (e.g. `40751` → `Acela`, `88` → `Northeast Regional`, `90` →
  `Vermonter`, `41042` → `Hartford Line`, `41044` → `Valley Flyer`).
- All join paths — station, train, and alert attachment — are covered by offline fixture tests.

## Approaches Considered

| Approach | Benefits | Costs / risks | Decision |
|---|---|---|---|
| **A. Standalone consumer (library + CLI)** — a new module/binary that ingests the four feeds and answers station + train-number queries with alerts; no server. | Matches standard GTFS practice (feed producer stays a pure republisher; departure boards, per-trip status, and alert rendering are consumer concerns, as in OpenTripPlanner / Transitland / Navitia). Zero risk to the shipped v0.2.0 service. Directly productionizes the validated prototype. Testable offline. | Not a hosted API; each caller runs it. Needs a clear library boundary to be reusable. | **Chosen** |
| **B. New routes on the feed producer** (`/v1/stations/{code}/departures`, `/v1/trains/{number}`). | One deployable; reuses the running generation + access control. | Violates producer/consumer separation — grafts query logic onto a raw-feed distributor. Changes the shipped, released service and re-opens its authorization/exposure surface. Heavier gate (returns to that service's change control). | Rejected for this increment |
| **C. Separate query microservice** — a standalone service that consumes the feeds and serves a departures/train-status API. | Keeps producer/consumer separation (it is essentially Approach A that also serves HTTP); hostable; cacheable. | More operational surface (a second service, its own auth/exposure decisions) than is justified before the core queries are productionized. | Deferred — natural evolution of A once the library exists |

Library/API capability notes: the static path (`gtfs_structures::Gtfs::from_url_async`) and the
real-time path (`amtrak_gtfs_rt::fetch_amtrak_gtfs_rt`, returning trip updates, vehicle positions,
*and* ASM-derived alerts) were both exercised live during validation, so their availability is
confirmed by running code, not memory. The "boards, trip status, and alerts are consumer concerns"
framing reflects long-standing practice in the named GTFS consumers; the design phase will confirm
any specific API shape it borrows.

## Chosen Direction

Build a **standalone consumer** with a shared core (feed acquisition → decode → index) that serves
two query modes — station departures and train-number status — **each enriched with joined service
alerts**, behind a well-factored library with a thin CLI entry point. It is kept entirely separate
from the feed-producer service. This honors standard GTFS producer/consumer separation, keeps the
released service untouched, and productionizes the validated station join while adding the
train-number and alert joins on the same loaded feeds. Alert enrichment is treated as a first-class
requirement, not an add-on, because it is what makes each answer data-complete. A hosted query API
(Approach C) remains a clean follow-on because all queries live in a reusable library rather than in
route handlers.

## Architecture and Flow Outline

```
                         station code (e.g. NHV)          train number (e.g. 2159)
                                   │                                │
      ┌──────────────── station & train status (consumer) ─────────┼───────────┐
      │  shared core: acquire feeds once                            │           │
      │     static GTFS      ◀── local service generation OR Amtrak URL         │
      │     trip-updates     ◀── same generation OR live crate                  │
      │     vehicle-positions◀── same generation OR live crate                  │
      │     alerts           ◀── same generation OR live crate                  │
      │     build indexes: stop-code↔stop, trip_short_name↔trip(s),             │
      │                     alert.informed_entity↔trip                          │
      │                                   │                                │    │
      │        station mode ◀─────────────┘                                │    │
      │          filter trip_updates by stop code → departure/arrival;     │    │
      │          enrich route_id→name, trip→headsign, + attach alert;      │    │
      │          order upcoming                                            │    │
      │                                                                    │    │
      │        train mode ◀────────────────────────────────────────────────┘   │
      │          trip_short_name → trip(s) today → trip_update + vehicle        │
      │          position + attached alert; remaining stops + predicted times   │
      └───────────────────────────┬───────────────────────────────┬────────────┘
                                   ▼                               ▼
              departures board (with status)         train status view (with delay notice)
```

The feed-producer service is unchanged and sits upstream as one of the two possible feed sources.

## Failure and Verification Strategy

- **No live data / feed fetch fails:** surface a clear error; never emit a partial or stale result
  silently.
- **Unknown identifier:** an unmapped station code or a train number with no active trip is reported
  as unresolved, not as an empty board that looks like "no service."
- **Missing alert:** a train with no service message renders normally (no alert row), and an alert
  whose `informed_entity` matches no loaded trip is not silently dropped without a diagnostic.
- **Coherence:** when reading the local service, static schedule, trip updates, vehicle positions,
  and alerts come from the **same immutable generation**.
- **Verification:** offline fixture tests for station resolution, the station join
  (terminus/arrival-only and canceled stops), train-number resolution (including a duplicate
  same-day number), and alert attachment via `informed_entity`; plus a live check against NHV and a
  live delayed train (Acela 2159 pattern) cross-checked against Amtrak's own status page.

## Open Decisions

1. **Primary feed source** — **Resolved (2026-08-17):** consume the **local service's generation
   artifacts** (`/v1/feed-set.json` → the four artifacts of one immutable generation), which
   respects the immutable-generation contract and the loopback/direct-peer auth. Direct-from-Amtrak
   is retained only as an optional dev/offline fallback, not the default.
2. **Station identifier surface** — accept Amtrak code only, or also GTFS `stop_id` and/or a
   name/fuzzy match; and how to disambiguate co-located stations (e.g. New Haven Union `NHV` vs.
   State St `STS`).
3. **Train-number resolution** — how to handle a `trip_short_name` that maps to more than one trip
   on the active service day (return all, or disambiguate by direction/origin); and whether to
   accept the `{date}-{number}` entity id form as an alternate input.
4. **Alert matching breadth** — attach only trip-scoped alerts (`informed_entity` trip match), or
   also route-scoped/stop-scoped alerts; and how to present multiple alerts on one train.
5. **Timezone source** — fixed Eastern vs. per-stop timezone from `stops.txt`/agency for correctness
   beyond the NEC.
6. **Output surface breadth** — CLI now, with a library boundary ready for a future hosted API
   (Approach C).

## Approval

Status: **Approved on 2026-08-17.** The user approved the two-mode station & train status capability
as a standalone consumer, with **alerts/status enrichment sequenced first** for data completeness and
the **local service immutable generation** as the primary feed source (direct-Amtrak as optional dev
fallback). Remaining open decisions (2–6) are deferred to requirements/design.
