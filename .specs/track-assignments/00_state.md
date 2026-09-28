# Spec State: Real-Time Track Assignments

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved 2026-09-28 under the user's standing approval of all gates: rework PR #17 to platform stops in `static.zip`, registered RailData only, expiring assignments, background refresh |
| Requirements | approved | Approved 2026-09-28 under standing approval: eight requirements / 48 criteria (revised 2026-09-28 in design: event-time window R3.11, status-tool station resolution R8.2, feed validation moved to the scheduled job R8.3–R8.4) covering platform stops, static integrity, conformant stamping, authorized RailData access, freshness, timing, sources, and tools/CI |
| Design | approved | Approved 2026-09-28 under standing approval: deterministic `stops.txt` augmentation, registered RailData client with persisted token budget, background refresher, expiring store, spec-conformant stamping and orchestrator checks; ten properties cover all 48 criteria |
| Tasks | approved | Approved 2026-09-28 under standing approval: seven tasks across five dependency stages covering all 48 criteria |
| Audit | not_run | Not requested |
| Execution | complete | 2026-09-28: seven tasks and 48 criteria verified; preflight hardening self-approved under delegated authority; independent final review passed after one fix (flexible CSV reader) |

## Change Control

- The feature replaces PR #17's output model and NJ Transit access on the same branch; its board parsers and track-label rules are kept.
- Publishing a transformed `static.zip` changes the service's contract that `static.zip` is Amtrak's bytes. The change applies only when tracks are enabled.
- Adding stations or tracks is configuration, not a discovery change. Changing where tracks appear in the feed requires discovery re-approval.
