# Spec State: Station & Train Status Queries

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved by the user on 2026-08-17: two-mode consumer, alerts-first, local-generation feed source |
| Requirements | approved | Approved by the user on 2026-08-17: 7 requirements / 24 EARS criteria; spec-check passed |
| Design | approved | Approved by the user on 2026-08-17: standalone consumer, optional `status` feature, chrono-tz tz, 20 properties / 24 criteria |
| Tasks | approved | Approved by the user on 2026-08-17: 7 tasks / 6 stages; spec-check passed |
| Audit | not_run | Not requested |
| Execution | complete | All 7 tasks complete; 75 tests pass (57 service unchanged + 18 new) plus 2 ignored live tests; service/container byte-for-byte unchanged; live verification against Amtrak (train 2159 / NYP) confirmed on-board by the user |

## Change Control

- This feature adds a **consumer/query layer** for per-station departures on top of the existing GTFS-Realtime feeds. It does not change the feed producer's GTFS or GTFS-RT contract, its immutable-generation model, or its direct-peer authorization.
- A change that adds a route to the feed-producer service, alters its authorization, or exposes departures publicly returns to discovery and requires re-approval.
- On 2026-08-17, a throwaway consumer ([`examples/station_departures.rs`](../../examples/station_departures.rs)) validated the end-to-end join for New Haven (NHV): it loaded Amtrak static GTFS, fetched and decrypted live GTFS-RT trip updates via the `amtrak-gtfs-rt` crate, and produced a correct, time-ordered upcoming-departures board. This validation motivates the spec but is not the shippable artifact.
- On 2026-08-17, discovery scope expanded (still pending approval) to a **two-mode query capability** — *by station* (departures board) and *by train number* (train status), mirroring Amtrak's own travel-status widget. Both modes reuse the same feed load; train number maps to GTFS `trip_short_name`.
- On 2026-08-17, **service alerts (ASM delay/status notifications) were elevated to a high-priority, first-class element** for data completeness, per user feedback citing the Acela 2159 delay notice. Alerts are already emitted in our `alerts.pb`; the consumer joins them to trips via `Alert.informed_entity` and attaches them to both the station board and the train view.
- On 2026-08-17, the user **approved discovery** with three directions locked: (1) alerts/status enrichment is sequenced first; (2) the primary feed source is the **local service's immutable generation** (direct-Amtrak only as a dev fallback); (3) the shape stays a standalone consumer with the producer untouched.
- On 2026-08-17, requirements added a **route-geometry** requirement (shapes) after the user asked to include Amtrak `shapes.txt` (confirmed present in the Amtrak GTFS and exposed by `gtfs-structures`).
- On 2026-08-17, the user **re-prioritized**: the top priority is that each train answer is **data-complete** (alerts + live status + geometry from one coherent generation); the **station departures board is the lowest priority**. Interpretation confirmed as consumer-side completeness only — the producer stays untouched. No requirement content changed; only build priority.
- On 2026-08-17, requirements resolved **timezone source** to per-station `stop_timezone` (present for all 646 stations) with agency-timezone fallback (R6.1/R6.2).
- On 2026-08-17, design chose **chrono + chrono-tz** for per-station timezone rendering (already in the resolved graph; adopted as an optional direct dep gated by a `status` feature so the shipped service binary/container are byte-for-byte unchanged). Dependency evidence: `chrono-tz@0.10.4` is inventoried with no finding in the complete `main` and `release` audits; a fresh `change` re-run could not complete its inventory in this environment (`cargo metadata` output exceeded the audit tool's cap), so the complete main/release audits stand as authoritative. Both diagrams render-validated.
- On 2026-08-17, the consumer was implemented spec-driven and **integrated via [PR #7](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/7)** (commit `7b792f8`, branch `station-departures` → `main`). All 7 tasks complete; 75 tests pass + 2 ignored live tests; service/container byte-for-byte unchanged; live-verified against Amtrak (Acela 2159 / NYP). Merge and deployment remain separate authorized steps.
- On 2026-08-17, **station-facility updates (e.g. elevator out of service) were considered and dropped from this spec.** Data gap: the `amtrak-gtfs-rt` alerts are train/trip-scoped ASM messages (`informed_entity.stop_id` always null); only the Pacific Surfliner route scrape carries facility text, and only route-scoped. Comprehensive per-station facility status would need a new upstream source (Amtrak station pages) = producer/data-acquisition scope, which is out of this consumer spec. Revisit later as a separate spec if desired.
