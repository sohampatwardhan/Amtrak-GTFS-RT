# Spec State: Service Advisories → Scoped GTFS-RT Alerts

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved by the user on 2026-08-17: producer-side fail-open advisory scraper; consumer surfacing out of scope |
| Requirements | approved | Approved by the user on 2026-08-17: 6 requirements / 13 EARS criteria; spec-check passed |
| Design | approved | Approved by the user on 2026-08-17: WithAdvisories decorator, fail-open TTL cache, 13 properties / 13 criteria |
| Tasks | approved | Approved by the user on 2026-08-17: 5 tasks / 4 stages; spec-check passed |
| Audit | not_run | Not requested |
| Execution | complete | All 5 tasks complete; 64 tests pass + 3 ignored; service builds (locked release), default-off. Live check proved the direct fetch is Akamai-blocked (IP-independent) → shipped fail-open + default-off scaffolding; the real fetch moves to a separate `advisory-fetcher` (Playwright sidecar) spike spec. |

## Change Control

- This feature **changes the shipped feed-producer service**: it scrapes Amtrak's Service Alerts & Notices page and injects **stop-scoped** (station advisories) and **route-scoped** (passenger advisories) GTFS-RT alerts into the published generation's alerts feed. It therefore re-opens the service's container-scan, dependency-audit, and release gates.
- The pivotal placement decision was made by the user on 2026-08-17: **producer emits** (scrape in the service) — not a consumer-side scrape. Consumer-side surfacing of the new route/stop-scoped alerts is a separate follow-up that depends on the station-departures consumer ([PR #7](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/7)).
- A change that alters the immutable-generation contract, the direct-peer authorization, or makes the scrape a hard dependency of generation success returns to discovery.
- Precedent in-repo: the `amtrak-gtfs-rt` crate already scrapes the Pacific Surfliner website into GTFS-RT alerts using `scraper` 0.22; this feature mirrors that pattern at national scale in our own service.
- On 2026-08-18, execution completed and integrated via **[PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8)** (commit `f2e1075`, branch `service-advisories` → `main`). Live verification proved the direct in-service fetch is Akamai-blocked (IP-independent; confirmed after ruling out a Proton VPN confound), so the feature ships **fail-open and default-off**; the real data acquisition moves to a separate `advisory-fetcher` (Playwright sidecar) spec beginning with an Akamai-bypass spike.
