# Discovery: Service Advisories → Scoped GTFS-RT Alerts

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Problem and Outcome

The service's GTFS-Realtime **alerts** feed today carries only **train/trip-scoped** messages: the
Amtrak Service Messages (ASM) delay notifications produced by the `amtrak-gtfs-rt` crate
(`informed_entity.trip`), plus route-scoped Pacific Surfliner website advisories for that one
California route. Two whole classes of advisory that Amtrak publishes on
[amtrak.com/service-alerts-and-notices](https://www.amtrak.com/service-alerts-and-notices) are
**absent** from the feed:

- **Station Advisories** — facility notices tied to a station, e.g. *"Alexandria, VA (ALX) — Station
  Will No Longer Accept Checked Baggage"*, *"Brattleboro, VT (BRA) — New Location"*.
- **Passenger Advisories** — service changes tied to one or more routes, e.g. *"Amtrak Cascades
  Replaced by Bus Between Vancouver, WA and Portland"*, *"Piedmont and Carolinian Service
  Adjustments"*, *"Amtrak Hartford Line and Valley Flyer Schedule Changes"*.

The outcome is that the **feed-producer service scrapes that page and injects these advisories as
correctly scoped GTFS-RT alerts** into each published generation's alerts feed — station advisories
as **stop-scoped** (`informed_entity.stop_id`) and passenger advisories as **route-scoped**
(`informed_entity.route_id`) — alongside the existing trip-scoped ASM alerts. Because they land in
the standard `alerts` feed, every downstream consumer (including the station & train status tool)
surfaces them for free: station advisories on a station's board, passenger advisories on any train
or board of the affected route.

## Users and Current Workaround

The user is any **feed consumer** who wants station-facility and route-level service advisories, not
just per-train delay messages. Today the advisories exist only as HTML on Amtrak's website; a
consumer would have to scrape and parse that page itself and re-map station names and route names to
GTFS ids. Amtrak's own site presents them under two tabs (`Station Advisories`, `Passenger
Advisories`) with a structured list per advisory.

## Scope and Non-Goals

In scope:

- Fetch and parse the Service Alerts & Notices page's **Station Advisories** and **Passenger
  Advisories** sections.
- Map each advisory to a GTFS-RT `Alert`: **station code → GTFS `stop_id`** for station advisories
  (`informed_entity.stop_id`); **route name → GTFS `route_id`** via `route_long_name` for passenger
  advisories (`informed_entity.route_id`), emitting one selector per affected route for multi-route
  advisories.
- Populate `header_text` (advisory title), `description_text` (title plus the effective-date text),
  and the detail-page `url`; derive `active_period` from the effective dates when they parse
  unambiguously, and omit it otherwise rather than guess.
- **Merge** these alerts into the alerts feed the service already publishes, within the **same
  immutable generation** as the static schedule and real-time feeds, coexisting with the existing
  ASM trip-scoped alerts.
- **Fail open:** a fetch or parse failure of the advisories page must **not** fail generation
  publication — the generation still publishes with the ASM alerts, and the failure is a diagnostic.

Out of scope (this increment):

- **Consumer-side alert matching** for the new scopes: the station & train status tool currently
  matches alerts by trip only; broadening it to route/stop scope is a separate follow-up that
  depends on [PR #7](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/7). This spec is
  producer-only — it *emits* the scoped alerts.
- Any advisory tab beyond Station and Passenger (e.g. a systemwide/current-disruptions tab, if one
  exists) — deferred (see Open Decisions).
- Changing the immutable-generation contract, the direct-peer authorization, or the existing feed
  routes.

## Constraints and Success Measures

Constraints:

- Reuse the in-repo precedent: the `amtrak-gtfs-rt` crate scrapes the Pacific Surfliner website with
  **`scraper` 0.22** (`Html::parse_document` + CSS `Selector`s); this feature uses the same crate
  (already resolved transitively) and pattern in our own service.
- Preserve the service's fail-closed *generation* discipline while making the advisory scrape
  **supplementary** — advisories are best-effort enrichment, never a gate on publishing a generation.
- Do not weaken validation, persistence, or access controls; the alerts feed simply gains entities.

Success measures:

- For a live run, a known station advisory (e.g. `ALX`) appears as a stop-scoped alert on `ALX`, and
  a known passenger advisory (e.g. Hartford Line / Valley Flyer) appears as route-scoped alerts on
  routes `41042` / `41044`.
- Route-name and station-code mapping is exact and tested against the static GTFS (confirmed live:
  Amtrak Cascades→60, Piedmont→79, Carolinian→85, Hartford Line→41042, Valley Flyer→41044; station
  codes are the GTFS stop identifiers).
- A simulated markup change or fetch failure yields **zero** advisory alerts and a still-published
  generation — never a crash or a failed generation.
- Multi-route advisories emit one `informed_entity` per affected route.

## Approaches Considered

| Approach | Benefits | Costs / risks | Decision |
|---|---|---|---|
| **A. Scrape in the service** — a new best-effort advisory source that parses the page and merges scoped alerts into `alerts.pb`. | Single source of truth in the published feed; every consumer benefits with no client work; mirrors the crate's existing Surfliner scraper pattern; keeps producer/consumer separation (consumers still just read the feed). | Changes the shipped v0.2.0 service → re-opens container-scan / dependency-audit / release gates. HTML scraping is brittle (breaks on Amtrak markup changes) and must fail open. Adds `scraper` as a direct dependency (already transitive). | **Chosen** (user selected producer-emits) |
| **B. Upstream PR to `amtrak-gtfs-rt`** — add national advisory scraping to catenary's crate. | Benefits every downstream user of the crate; the crate already scrapes the Surfliner site. | Out of our control (external repo, review latency); a large upstream change; we cannot gate our release on it. | Rejected |
| **C. Separate sidecar service** — a new service that augments the feed with advisories. | Keeps the producer untouched. | Extra operational surface and a second deployable; the producer already owns and publishes the alerts feed, so splitting it is artificial. | Rejected |

Library note: the `scraper` 0.22 capability (CSS-selector HTML parsing) is confirmed by the
`amtrak-gtfs-rt` crate's own working use of it, not from memory; the exact selectors and current
`scraper` API are a design-phase concern.

## Chosen Direction

Add a **best-effort national advisory scraper to the service** as a new alert source that runs when
a generation is built: fetch the Service Alerts & Notices page, parse the Station and Passenger
advisory sections, map station codes → `stop_id` and route names → `route_id`, build scoped
GTFS-RT `Alert` entities, and **merge** them into the generation's alerts feed alongside the ASM
trip alerts. The scrape is supplementary and **fails open** — any fetch/parse failure logs a
diagnostic and yields zero advisory alerts without blocking the generation. This keeps the feed the
single source of truth (Approach A) and mirrors the crate's proven Surfliner-scraper pattern.

## Architecture and Flow Outline

```
Amtrak Service Alerts & Notices page (HTML)
        │  (best-effort fetch; fail-open)
        ▼
┌──────────────── feed-producer service (generation build) ─────────────┐
│  AmtrakSource ── ASM trip-scoped alerts ─┐                             │
│                                          ├─► merge alerts              │
│  AdvisorySource (new) ──────────────────┘                             │
│    parse Station Advisories  → Alert{ informed_entity.stop_id = code } │
│    parse Passenger Advisories→ Alert{ informed_entity.route_id = id* } │
│    (station code → stop_id; route_long_name → route_id; fail-open)     │
│                                          │                             │
│                                          ▼                             │
│                validate → publish one immutable generation             │
└───────────────────────────────────────────────────────────────────────┘
        ▼
   alerts.pb now carries trip-, stop-, and route-scoped alerts
```

The static schedule, trip updates, vehicle positions, and the merged alerts remain one coherent
immutable generation. The new source mirrors the existing decorator pattern used to wrap the Amtrak
source.

## Failure and Verification Strategy

- **Fail open:** a fetch, decode, or parse failure of the advisories page yields zero advisory
  alerts and a diagnostic; the generation still publishes with the ASM alerts. Advisory scraping is
  never a generation gate.
- **Markup robustness:** parsing is tolerant — an unexpected DOM yields zero advisories, never a
  panic; verified with fixture HTML (a captured snapshot and a deliberately-broken variant).
- **Mapping correctness:** station-code → `stop_id` and route-name → `route_id` mappings are tested
  against the static GTFS; an unmappable station/route is skipped with a diagnostic, not emitted
  against a wrong id.
- **Coexistence:** ASM trip alerts are preserved; advisory alerts are additive.
- **Live check:** a known station and a known route advisory appear as correctly scoped alerts in a
  freshly built generation.

## Open Decisions

1. **Effective-date parsing → `active_period`.** Formats vary widely (*"Effective April 20, 2026"*,
   *"Effective August 3 - 6 and August 24 - 27, 2026"*, *"Effective Monday - Friday, April 21 -
   October 30, 2026"*). Parse into `active_period` where unambiguous, else include the text in
   `description_text` and omit `active_period`. Exact parsing scope is a design decision.
2. **Scrape cadence.** Advisories change slowly; run the scrape on the existing static-refresh timer
   / per-generation, with caching to avoid hammering the page. To be settled in design.
3. **Third advisory tab.** Whether a systemwide/"current disruptions" tab exists and should be
   included later.
4. **Detail-page URL.** Whether to populate `Alert.url` with the advisory's detail link.
5. **Consumer surfacing (out of scope here).** Broadening the station & train status tool's alert
   matching to route/stop scope is a dependent follow-up on [PR #7](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/7).

## Approval

Status: **Approved on 2026-08-17.** The user approved the producer-side, fail-open advisory-scraper
boundary: the service scrapes Amtrak's Service Alerts & Notices page and injects stop-scoped station
advisories and route-scoped passenger advisories into the alerts feed, coexisting with ASM trip
alerts. Consumer-side surfacing stays out of scope (a PR #7 follow-up). Open decisions 1–4 are
deferred to requirements/design.
