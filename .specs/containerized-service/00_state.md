# Spec State: Containerized Service

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved 2026-08-14: compatibility-first Debian slim image with Alpine retained as a measured future optimization |
| Requirements | approved | Approved 2026-08-14: 38 criteria covering build, runtime, persistence, authorization, health, and rollback |
| Design | approved | Approved 2026-08-14: digest-pinned multi-stage Debian slim image with explicit network-policy handoff |
| Tasks | approved | Approved 2026-08-14: two dependency-ordered implementation tasks (Dockerfile, smoke harness + runbook) |
| Audit | not_run | Optional audit not requested |
| Execution | in_progress | Execution started 2026-08-14 on branch `containerized-service` |

## Change Control

- This feature adds a container packaging and local container-run boundary without changing the GTFS or GTFS-Realtime HTTP contract.
- A change that alters direct-peer authorization, validator pinning, artifact durability, or public exposure returns to discovery and requires re-approval.
- Image publication, orchestration, and production deployment remain separate decisions unless explicitly added to scope.
- On 2026-08-14, discovery was approved with Debian slim as the default base family; Alpine remains eligible only after complete-image size and native-library compatibility evidence.
- On 2026-08-14, the container requirements were approved without changing the existing feed or access-control contract.
- On 2026-08-14, the container design was approved with immutable base-image pins, validator verification, UID 10001, `/data` persistence, and loopback-safe defaults.
- On 2026-08-14, the two implementation tasks were approved and execution began on branch `containerized-service`.
