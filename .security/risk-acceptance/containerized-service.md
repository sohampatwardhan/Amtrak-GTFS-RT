# Proposed Risk Acceptance: Containerized Service Dependencies

| Field | Value |
|---|---|
| Status | **Proposed — not yet accepted** |
| Prepared | 2026-08-14 |
| Scope | Container image `sha256:7d6eb07c8c220607de64ec9fbef8d6aeabda94b076ad8c687f4bb4a3365517e1` and the Cargo resolution recorded by the release dependency audit |
| Proposed review deadline | 2026-11-12 (90 days), or earlier on any review trigger below |
| Risk owner / approver | Pending explicit owner designation and approval |
| Deployment authority | Not granted by this record |

## Decision requested

Approve a time-bounded exception for the inherited dependency findings described below while the
service remains subject to its existing runtime controls and monitoring. This draft does **not**
mark the risk accepted, turn an unavailable audit gate green, authorize image publication, or
authorize deployment. Those decisions remain explicit owner actions.

## Evidence and remediation assessment

The local image was scanned with grype 0.117.0 on 2026-08-14. The retained machine-readable report
is [`validation-reports/container/cves-grype.json`](../../validation-reports/container/cves-grype.json)
and the human-readable report is
[`validation-reports/container/cves-grype.txt`](../../validation-reports/container/cves-grype.txt).
The scan contains 401 matches: 33 Critical, 74 High, 146 Medium, 10 Low, 122 Negligible, and 16
Unknown.

The Critical and High findings were classified by fix state:

| Severity | Fixed | Not fixed | Won't fix | Assessment |
|---|---:|---:|---:|---|
| Critical | 0 | 12 | 21 | All 33 matches are Debian OS-package findings with no available package fix. |
| High | 10 | 22 | 42 | The 10 fixable matches are Java libraries embedded in the validator JAR; the remaining 64 have no available package fix. |

The 10 fixable High matches affect `commons-beanutils` 1.9.2, `commons-compress` 1.20, `gson`
2.8.6, `httpcore5` 5.0.2, and `httpcore5-h2` 5.0.2 inside MobilityData's validator JAR. The image
already pins MobilityData GTFS Validator v8.0.1 by SHA-256, and v8.0.1 was confirmed on 2026-08-14
as the [latest upstream release](https://github.com/MobilityData/gtfs-validator/releases/tag/v8.0.1).
Replacing libraries inside the pinned vendor artifact would create an unsupported derivative and
invalidate the verified artifact identity. The appropriate remediation is an upstream validator
release containing updated libraries.

A smaller or distroless runtime was assessed and deferred for this exception cycle. It could reduce
the OS-package attack surface by removing packages, but it would not remediate the fixable findings
inside the vendor JAR. The current image also requires Java, `shasum`, glibc compatibility, and a
health probe. Replacing those facilities requires a separate design change and complete regression
evidence; it should be evaluated as follow-up hardening rather than represented as an available
package update.

The separate release dependency audit is retained at
[`release.md`](../dependency-audit/release.md) and [`release.json`](../dependency-audit/release.json).
It resolves 375 packages and reports 10 warnings, 0 blocking findings, and an `unavailable` gate
because required enrichment sources were partial/unavailable. The warnings are transitive
advisories involving `atomic-polyfill`, `derivative`, `fxhash`, `gcc`, `rust-crypto`,
`rustc-serialize`, and `time`. Reachability is not assessed, so this record does not claim that the
affected code paths are unreachable.

## Residual risk

- A vulnerable OS or validator-JAR code path could compromise service availability, integrity, or
  confidentiality if it is reachable through the fixed upstream data-processing path or runtime
  environment.
- The critical `rust-crypto` advisory and the other Cargo warnings are transitive and currently
  lack authoritative in-tree remediation; their runtime reachability remains unknown.
- Scanner severity is not proof of exploitability, but lack of demonstrated reachability is also
  not proof of safety. The residual risk is therefore material and must be owned explicitly.
- Rebuilding from the same Dockerfile can produce a different image because Debian package
  resolution occurs at build time. This acceptance applies only to the image digest above; a new
  digest requires a new scan and review.

## Compensating controls

- The runtime is non-root at fixed UID/GID 10001; the service binary and validator JAR are
  root-owned and not writable by that user.
- The default bind address is loopback-only. Bridged operation fails closed unless an exact peer
  allowlist is configured; forwarded identity headers do not bypass direct-peer authorization.
- The container exposes only the service HTTP port, persists data under `/data`, and contains no
  compiler, source tree, build cache, or package manager index.
- The validator JAR is pinned and verified by SHA-256 both during the image build and at service
  startup.
- The smoke harness verifies liveness, readiness, peer denial, spoofed-header denial, artifact
  decoding, incomplete-generation rejection, and offline last-good recovery.
- Each release candidate must retain an SBOM and CVE report. Unavailable evidence must continue to
  fail closed rather than being reported as clean.

## Conditions and review triggers

If accepted, this exception expires at the earliest of the proposed review deadline or any of the
following events:

1. MobilityData publishes a validator release that updates the affected embedded libraries.
2. Debian publishes fixed packages for any accepted Critical or High image finding.
3. The image digest, base-image digest, validator digest, Cargo lockfile, network exposure, peer
   authorization model, or upstream input source changes.
4. A finding enters CISA KEV, credible exploitation is reported, or reachability analysis shows an
   affected path is exercised by this service.
5. The dependency audit becomes complete and changes the severity or disposition of a finding.

At each trigger, rebuild if applicable, regenerate the SBOM and vulnerability reports, rerun the
container smoke harness and release dependency audit, and either remediate or obtain a new explicit
acceptance. Publication and deployment still require their own authorization.

## Approval record

| Decision | Name / role | Date | Notes |
|---|---|---|---|
| Pending | — | — | No risk has been accepted by creating this draft. |
