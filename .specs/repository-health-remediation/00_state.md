# Spec State: Repository Health Remediation

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved 2026-08-23: remediate Python CI coverage, release recovery/diagnostics, and stale documentation without absorbing PR #7 |
| Requirements | approved | Re-approved 2026-08-23: seven requirements / 42 criteria including default-branch recovery authorization and explicit PR #8/#9 state |
| Design | approved | Re-approved 2026-08-23: audit fixes cover main-only controls, typed evidence, immutable CI pins, dependency evidence, and per-platform attestation |
| Tasks | approved | Re-approved 2026-08-23: five tasks across four serial dependency stages with integrated verification |
| Audit | fixes_applied | Thorough audit fixes explicitly authorized 2026-08-23 and applied across requirements, design, diagrams, and tasks |
| Execution | delivered | All five tasks and 42 criteria passed; commit `6c107aa` was pushed on `codex/repository-health-remediation` and [PR #13](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/13) opened against `main` |

## Change Control

- The remediation owns repository health on `main`: automated sidecar tests, observable and recoverable release gates, and accurate project/spec state.
- The separate station-and-train-status feature in PR #7 remains out of scope because combining its 4,032-line conflicted change with repository-health work would weaken review and rollback boundaries.
- Runtime product behavior, the zero-vulnerability release policy, immutable Git tags, public network exposure, and deployment remain unchanged unless discovery is re-approved.
- Requirements changes that alter release authorization, evidence retention, or the zero-match policy require discovery re-approval before design resumes.
