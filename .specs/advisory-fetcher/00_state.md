# Spec State: Advisory Fetcher (Playwright sidecar)

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved 2026-08-18; spike-first Playwright sidecar, Pi/OOM-bounded |
| Requirements | approved | Approved 2026-08-18; 8 requirements / 22 criteria, Pi 5 confirmed on-device |
| Design | approved | Approved 2026-08-18; spike-gated Playwright sidecar, HTTP snapshot, 1 GB cap |
| Tasks | approved | Approved 2026-08-18; 10 tasks / 8 stages, spike-gated, throwaway Pi container |
| Audit | not_run | Not requested |
| Execution | complete | All 10 tasks verified; final review clean (2 findings fixed). Spike bypass 10/10; 34 tests; arm64 image serves + fetches; no resident browser, ~50 MiB idle, auto-restart. PR #8 merged to main; join yields 13 scoped alerts (9 stop + 6 route). **Delivered: PR #9** (push+PR, operator-selected) |

## Current Repository Status

- PR #8 (service consumption) and PR #9 (this fetcher) are merged to `main`; no current document should describe either as unmerged.
- Source integration does not mean the sidecar is deployed. Advisories remain default-off until an operator runs the component and configures the service URL.
- `v0.2.0` is the latest completed release; `v0.3.0` publication is incomplete. No production deployment, anonymous exposure, proxy trust, or orchestration is authorized.
- PR #7 remains a separate open/conflicting stream outside this feature.

## Change Control

- This is a **new, standalone component** (advisory-fetcher/), not part of the Rust service. It uses a real browser (Playwright) to earn Amtrak's Akamai sensor cookie, fetch the Service Alerts & Notices page, and write a **snapshot** the feed-producer service consumes via `AdvisoryConfig` (established by [service-advisories PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8)). It runs in its own container image; the service keeps its scratch/musl posture.
- **Spike-first history.** The load-bearing Akamai-bypass assumption was initially unproven and therefore gated implementation. Execution later proved it 10/10 on device; that result supersedes the original uncertainty without deleting the historical gate.
- The snapshot-consumption contract from merged PR #8 remains fail-open and default-off. This component enables advisories only when an operator explicitly deploys and configures it.
