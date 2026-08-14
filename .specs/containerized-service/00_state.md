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
| Execution | complete | Both tasks implemented, reviewed, and verified 2026-08-14 on branch `containerized-service`; local container verification complete, rollout blocked pending explicit risk acceptance and the existing dependency-audit decision |

## Change Control

- This feature adds a container packaging and local container-run boundary without changing the GTFS or GTFS-Realtime HTTP contract.
- A change that alters direct-peer authorization, validator pinning, artifact durability, or public exposure returns to discovery and requires re-approval.
- Image publication, orchestration, and production deployment remain separate decisions unless explicitly added to scope.
- On 2026-08-14, discovery was approved with Debian slim as the default base family; Alpine remains eligible only after complete-image size and native-library compatibility evidence.
- On 2026-08-14, the container requirements were approved without changing the existing feed or access-control contract.
- On 2026-08-14, the container design was approved with immutable base-image pins, validator verification, UID 10001, `/data` persistence, and loopback-safe defaults.
- On 2026-08-14, the two implementation tasks were approved and execution began on branch `containerized-service`.
- On 2026-08-14, both tasks were implemented, independently reviewed, and verified; execution is complete. Local container verification passed (build, smoke harness, Rust and feed gates, SBOM). Rollout remains blocked: the Docker Scout CVE report is unavailable without `docker login`, and the base service's dependency-audit block still stands. Image publication and deployment remain out of scope pending user authorization.
- On 2026-08-14, the user chose push + PR; the branch was pushed and [PR #2](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/2) opened against `main` as the authoritative integration record.
- On 2026-08-14, the image CVE-evidence caveat was resolved: the smoke harness gained an auth-free grype fallback and produced a CVE report (33 Critical / 74 High / 146 Medium — not clean; inherited from the JRE's OS packages and the bundled validator JAR). Rollout still requires remediating or risk-accepting those findings. The base-service dependency-audit block was left as-is per user decision (inherited transitive advisories with no upstream fix; unaffected by containerization).
- On 2026-08-14, remediation triage found no fixable Critical image finding. All 10 fixable High findings are embedded in the latest upstream validator JAR (v8.0.1); a smaller runtime would not remediate them and would require a compatibility redesign. A [proposed risk-acceptance record](../../.security/risk-acceptance/containerized-service.md) now documents the image and Cargo findings, compensating controls, expiry, and review triggers. It remains pending explicit owner approval and does not authorize publication or deployment.
