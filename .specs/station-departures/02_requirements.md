# Requirements: Station & Train Status Queries

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Introduction

This feature adds a **standalone consumer** of the existing Amtrak GTFS-Realtime feeds that answers
two rider-facing questions — *next departures at a station* and *status of a train by number* — each
enriched with the relevant **service alert** (delay/status notification) and, for a train, its
geographic route path. It does not change the feed-producer service; it reads a single coherent
snapshot the producer already publishes and computes answers from it. Discovery for this feature was
approved on 2026-08-17 ([`01_discovery.md`](01_discovery.md)).

### Domain terms

- **Generation:** one immutable snapshot published by the local service, comprising a static GTFS
  schedule plus real-time trip updates, vehicle positions, and alerts that are mutually coherent.
- **Station code:** the Amtrak station identifier carried in the real-time feed's
  `StopTimeUpdate.stop_id` (e.g. `NHV` for New Haven Union Station).
- **Train number:** the Amtrak train number, represented in the static schedule as a trip's
  `trip_short_name` (e.g. `2159`).
- **Service alert:** an Amtrak Service Message (delay/status notification) carried in the alerts
  feed, linked to a trip through the alert's informed-entity.
- **Route path (shape):** the ordered sequence of geographic points defining a trip's physical
  route, from the static schedule's shapes data.
- **Query time:** the wall-clock instant a query is evaluated.

### Assumptions

- The Amtrak GTFS-Realtime feeds continue to expose trip updates, vehicle positions, and
  ASM-derived alerts, and the static schedule continues to include stop times, trip short names,
  routes, shapes, and per-station timezones (`stop_timezone`). (Evolving-technology constraint;
  confirmed against live data on 2026-08-17: all 646 stations carry `stop_timezone`.)
- Station-level departure boards, per-trip status, and alert rendering are consumer concerns, not
  producer concerns.

### Implementation priority

Per user direction (2026-08-17), the top priority is that each **train answer is data-complete**:
its service alerts (Requirement 1), its live status — position, remaining stops, and overall delay
(Requirement 3), and its route geometry (Requirement 4) — all drawn from one coherent generation
(Requirement 5), rendered in correct local time (Requirement 6), and failing loudly on missing data
(Requirement 7). The **station departures board (Requirement 2) is the lowest priority** and is
sequenced last. Requirement numbering reflects specification grouping, not build order; the task
plan sequences work by this priority.

## Requirements

### Requirement 1: Service alert enrichment

**User Story:** As a feed consumer, I want each train in a result to carry its Amtrak service
message, so that I see the train's status and delay reason and not merely its times.

1. **R1.1** WHEN the Consumer presents a train in any result, THE Consumer SHALL attach the text of
   every service alert whose informed-entity identifies that train's trip.
2. **R1.2** IF a train has no matching service alert, THEN THE Consumer SHALL present that train with
   no alert text rather than omitting the train.
3. **R1.3** IF a loaded alert's informed-entity matches no trip in the current generation, THEN THE
   Consumer SHALL emit a diagnostic identifying the unmatched alert rather than discarding it
   silently.
4. **R1.4** WHEN a train has more than one matching service alert, THE Consumer SHALL present all of
   their messages for that train.

### Requirement 2: Station departures board

**User Story:** As a feed consumer, I want the upcoming departures at a station ordered by time, so
that I can see which Amtrak trains leave next and when.

1. **R2.1** WHEN the Consumer is given a station identifier that resolves to a station, THE Consumer
   SHALL list that station's departures whose time is at or after the query time, ordered ascending
   by time.
2. **R2.2** WHEN a train terminates at the requested station with an arrival but no departure, THE
   Consumer SHALL include that train labeled as an arrival.
3. **R2.3** WHEN the Consumer presents a departure row, THE Consumer SHALL include the human-readable
   route name, the train number, the trip headsign, and an indication of whether the time is
   real-time or scheduled.
4. **R2.4** IF a stop for a train is marked canceled, THEN THE Consumer SHALL label that entry as
   canceled rather than omitting it.
5. **R2.5** IF the given station identifier resolves to no station, THEN THE Consumer SHALL report
   the identifier as unresolved rather than returning an empty board indistinguishable from
   "no departures."

### Requirement 3: Train status by number

**User Story:** As a feed consumer, I want a train's live status by its Amtrak train number, so that
I can see where it is and its predicted remaining stops.

1. **R3.1** WHEN the Consumer is given a train number that resolves to an active trip, THE Consumer
   SHALL report that trip's current geographic position when a vehicle position is available for it.
2. **R3.2** WHEN the Consumer reports a train's status, THE Consumer SHALL list the train's remaining
   stops with their predicted arrival and departure times ordered by stop sequence.
3. **R3.3** WHEN the Consumer reports a train's status, THE Consumer SHALL include the train's
   overall delay whenever it is derivable from the feed.
4. **R3.4** IF a train number resolves to more than one active trip on the service day, THEN THE
   Consumer SHALL present every matching trip with distinguishing origin, destination, or direction.
5. **R3.5** IF a train number resolves to no active trip, THEN THE Consumer SHALL report that the
   train is not currently running rather than failing obscurely.

### Requirement 4: Trip route geometry

**User Story:** As a feed consumer, I want a train's geographic route path, so that I can place its
position along the route or draw it on a map.

1. **R4.1** WHEN the Consumer reports a train's status and the trip references a route path present
   in the static schedule, THE Consumer SHALL make available that trip's ordered sequence of
   geographic points.
2. **R4.2** IF a trip references a route path absent from the static schedule, THEN THE Consumer
   SHALL report the train's status without geometry and SHALL indicate that geometry is unavailable.

### Requirement 5: Coherent local-generation data source

**User Story:** As an operator, I want every answer computed from one coherent published snapshot,
so that schedule, predictions, positions, and alerts agree with each other.

1. **R5.1** THE Consumer SHALL obtain the static schedule, trip updates, vehicle positions, and
   alerts for a query from a single generation.
2. **R5.2** WHEN the local service exposes a current generation, THE Consumer SHALL read that
   generation's artifacts as identified by the service's feed-set manifest.
3. **R5.3** WHEN the Consumer presents any result, THE Consumer SHALL include the source generation's
   publication timestamp.
4. **R5.4** IF the local service has no current generation available, THEN THE Consumer SHALL report
   data-unavailable rather than emitting a partial result.
5. **R5.5** WHERE a direct-Amtrak source is explicitly selected instead of the local service, THE
   Consumer SHALL obtain all feeds directly from Amtrak for that query.

### Requirement 6: Local-time presentation

**User Story:** As a rider-facing consumer, I want times shown in the local timezone of the station
they belong to, so that departure and arrival times are directly meaningful across Amtrak's
multiple timezones.

1. **R6.1** WHEN the Consumer presents a clock time for a station, THE Consumer SHALL render it in
   that station's timezone from the static schedule with the correct daylight-saving offset for the
   date.
2. **R6.2** IF a station carries no timezone in the static schedule, THEN THE Consumer SHALL render
   its times in the feed's agency timezone and SHALL indicate that a fallback timezone was used.

### Requirement 7: Feed acquisition and decoding failure

**User Story:** As a feed consumer, I want a clear failure instead of a misleading partial answer
when the data cannot be obtained, so that I never trust an incomplete result.

1. **R7.1** IF acquiring or decoding any required feed fails, THEN THE Consumer SHALL report the
   failure and SHALL NOT emit a partial or stale result.

## Requirements Not Yet Resolved

- **Station identifier surface (Open Decision 2):** whether the Consumer accepts a GTFS `stop_id`
  and/or a name match in addition to the Amtrak station code, and how co-located stations (e.g. New
  Haven Union `NHV` vs. State St `STS`) are disambiguated. R2.1/R2.5 hold regardless; the accepted
  identifier forms are settled in design.
- **Alert matching breadth (Open Decision 4):** whether R1 attaches only trip-scoped alerts or also
  route-/stop-scoped alerts. Requirements assume trip-scoped attachment; broader matching is a
  design decision that must not change the observable behavior of R1.1–R1.4.
- **Timezone source (Open Decision 5): RESOLVED (2026-08-17).** Local time comes from each station's
  `stop_timezone` in the static schedule; confirmed present and populated for all 646 Amtrak stations
  ([`Stop.timezone`](https://docs.rs/gtfs-structures)). Agency timezone is a defensive fallback only
  (R6.2). Selecting a tz database to convert IANA zone + unix time is a design/dependency detail.

## Approval

Status: **Approved on 2026-08-17.** Seven requirements / 24 EARS criteria approved by the user, with
build priority "complete train answers first, station board last" and per-station timezone resolved.
Station-identifier forms (R2) and alert-matching breadth (R1) remain deferred to design.
