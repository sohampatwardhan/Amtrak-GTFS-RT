# Spec State: Containerized Service

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Security remediation amendment approved by the user on 2026-08-14 |
| Requirements | approved | Security remediation amendment approved by the user on 2026-08-14; 39 criteria traced |
| Design | approved | Security remediation amendment approved by the user on 2026-08-14 |
| Tasks | approved | Security remediation task 3.1 and its evidence gates approved by the user on 2026-08-14 |
| Audit | not_run | Optional audit not requested |
| Execution | complete | Task 3.1 passed its implementation, runtime, exact-image scan, and authorized pre/post dependency-audit gates; no image publication or deployment performed |

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
- On 2026-08-14, pre-approval repair round 2 eliminated two false-positive evidence paths: Docker invocation/inspection failures no longer count as startup rejection, and stale scan files cannot survive a failed current scan. Focused guard tests passed, the rebuilt image passed the complete smoke harness, and a fresh [scan summary](../../.security/risk-acceptance/evidence/containerized-service-scan-summary.json) plus [finding table](../../.security/risk-acceptance/evidence/containerized-service-cves-grype.txt) were committed for review. Requirements and Tasks artifact footers were reconciled with their approved state gates.
- On 2026-08-14, the user requested remediation instead of accepting the 401 findings. The approved follow-up replaces Debian/glibc with a package-manager-free musl/scratch runtime, statically links the Rust OpenSSL path, rebuilds validator v8.0.1 reproducibly from pinned source with reviewed dependency overrides, and moves shell/curl smoke operations to a separately pinned helper. The exact 79 MiB image passed the full live/recovery harness and Grype reported **No vulnerabilities found**. The proposed container risk acceptance is closed without approval.
- On 2026-08-14, the user approved the security-remediation amendment and authorized package/version/edge/revision disclosure to OSV, GitHub Advisory, NVD, and CISA KEV. Complete pre/post Cargo inventories (375 packages, 969 edges) produced the same ten inherited warning records and no blocked result or KEV match; the changed fingerprint reflects the direct `sha2` declaration. Four malformed GitHub CVSS enrichments remain explicit partial-source diagnostics. Task 3.1 is complete; the warnings are retained as base-service technical debt and are not described as clean.
