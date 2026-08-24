# Requirements: Service Advisories → Scoped GTFS-RT Alerts

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Introduction

The feed-producer service will scrape Amtrak's Service Alerts & Notices page and inject its two
advisory classes — **station advisories** (facility notices per station) and **passenger
advisories** (service changes per route) — into each published generation's GTFS-RT **alerts**
feed as correctly scoped alerts, coexisting with the existing train/trip-scoped ASM messages. The
scrape is best-effort and **fails open**: it never blocks a generation from publishing. Discovery
for this feature was approved on 2026-08-17 ([`01_discovery.md`](01_discovery.md)); consumer-side
surfacing of the new alert scopes is out of scope (a follow-up on
[PR #7](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/7)).

### Domain terms

- **Advisory:** an entry on Amtrak's Service Alerts & Notices page, either a *station advisory*
  (tied to a station code, e.g. `ALX`) or a *passenger advisory* (tied to one or more route names).
- **Station code:** the Amtrak station identifier shown in an advisory header (e.g. `ALX`), which is
  the station's GTFS stop identifier.
- **Route name:** the route title in a passenger advisory (e.g. `Amtrak Hartford Line`), which
  matches a GTFS `route_long_name` and resolves to a GTFS `route_id`.
- **Scoped alert:** a GTFS-RT `Alert` whose `informed_entity` names a stop (station advisory) or a
  route (passenger advisory).
- **Generation:** one coherent immutable snapshot the service publishes (static schedule + trip
  updates + vehicle positions + alerts).
- **ASM alert:** the existing train/trip-scoped Amtrak Service Message alerts already in the feed.

### Assumptions

- Amtrak's Service Alerts & Notices page continues to present Station Advisories and Passenger
  Advisories as structured lists with a station code (station advisories) or route name(s)
  (passenger advisories), a title, and effective-date text. (Evolving-source constraint; the page
  layout can change, which is why R5 requires fail-open behavior.)
- Station codes are GTFS stop identifiers and advisory route names match GTFS `route_long_name`
  (confirmed live 2026-08-17: Cascades→60, Piedmont→79, Carolinian→85, Hartford Line→41042, Valley
  Flyer→41044).

## Requirements

### Requirement 1: Station advisory alerts (stop-scoped)

**User Story:** As a feed consumer, I want each station facility advisory as a stop-scoped alert, so
that a station's board can display it.

1. **R1.1** WHEN the Service builds a generation, THE Service SHALL emit each parsed station advisory
   as an alert whose informed-entity stop identifier is the advisory's station code resolved to its
   GTFS stop.
2. **R1.2** IF a station advisory's station code resolves to no GTFS stop, THEN THE Service SHALL
   skip that advisory and record a diagnostic rather than emit an alert against an incorrect stop.

### Requirement 2: Passenger advisory alerts (route-scoped)

**User Story:** As a feed consumer, I want each route-level service advisory as a route-scoped
alert, so that any train or board of the affected route can display it.

1. **R2.1** WHEN the Service builds a generation, THE Service SHALL emit each parsed passenger
   advisory as an alert whose informed entities are the advisory's affected route names resolved to
   GTFS route identifiers.
2. **R2.2** WHEN a passenger advisory affects more than one route, THE Service SHALL include one
   informed entity per resolved route.
3. **R2.3** IF a passenger advisory's route name resolves to no GTFS route, THEN THE Service SHALL
   omit that route and record a diagnostic rather than emit against an incorrect route.

### Requirement 3: Advisory alert content

**User Story:** As a feed consumer, I want each advisory alert to carry its title, detail, and
active window, so that the alert is self-describing.

1. **R3.1** WHEN the Service emits an advisory alert, THE Service SHALL set its header text to the
   advisory title and its description text to the advisory title together with the effective-date
   text.
2. **R3.2** WHERE an advisory's effective dates parse to a definite start-and-end range, THE Service
   SHALL set the alert's active period to that range.
3. **R3.3** IF an advisory's effective dates do not parse to a definite range, THEN THE Service SHALL
   omit the active period and retain the effective-date text in the description.

### Requirement 4: Coherent merge into the generation

**User Story:** As an operator, I want advisory alerts to ride in the same coherent generation as the
existing alerts, so that consumers get one authoritative alerts feed.

1. **R4.1** WHEN the Service publishes a generation, THE Service SHALL include the advisory alerts in
   that generation's alerts feed alongside the existing trip-scoped ASM alerts.
2. **R4.2** WHEN advisory alerts are added to a generation, THE Service SHALL preserve every existing
   trip-scoped ASM alert in that generation.

### Requirement 5: Fail-open scraping

**User Story:** As an operator, I want advisory scraping to never break the feed, so that a website
change or outage cannot stop the service from publishing.

1. **R5.1** IF fetching the advisories page fails, THEN THE Service SHALL publish the generation with
   no advisory alerts and record a diagnostic, without failing generation publication.
2. **R5.2** IF the advisories page cannot be parsed into the expected structure, THEN THE Service
   SHALL emit no advisory alerts and record a diagnostic without failing generation publication.

### Requirement 6: Feed-contract preservation

**User Story:** As a feed consumer, I want advisory alerts delivered through the existing feed
without contract changes, so that current integrations keep working unchanged.

1. **R6.1** THE Service SHALL deliver advisory alerts only through a generation's existing alerts
   artifact, without adding or changing any feed route or the immutable-generation contract.

## Requirements Not Yet Resolved

- **Effective-date parsing scope (Open Decision 1):** which effective-date formats R3.2 parses into
  an active period versus falling back to R3.3. The observable behavior (definite range → active
  period; otherwise text-only) is fixed here; the exact grammar is a design decision.
- **Scrape cadence (Open Decision 2):** how often the page is fetched and whether results are cached
  between generations. R4.1 holds regardless; cadence is design.
- **Detail-page URL (Open Decision 4):** whether the alert also carries the advisory's detail link.

## Approval

Status: **Approved on 2026-08-17.** Six requirements / 13 EARS criteria approved by the user, with
fail-open scraping (R5) as the load-bearing safety property and skip-and-diagnose on unmappable
station/route (R1.2/R2.3). Effective-date grammar, scrape cadence, and detail-URL remain deferred to
design.
