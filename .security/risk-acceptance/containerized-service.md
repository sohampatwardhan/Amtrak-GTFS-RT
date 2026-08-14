# Closed Risk Acceptance: Containerized Service Dependencies

| Field | Value |
|---|---|
| Status | **Closed — remediation superseded the proposed exception** |
| Prepared / closed | 2026-08-14 |
| Remediated image | `amtrak-gtfs-rt@sha256:c2bc846fb8af357015fef2fecb6280f901d6189d298773f832b2d0032a0c2a56` (`linux/arm64`, 83,213,742 bytes uncompressed) |
| Deployment authority | Not granted by this record |

## Resolution

No dependency risk acceptance is requested for the container image. The 401-match Debian/JRE
image and its proposed exception were replaced by a package-manager-free scratch runtime. The
delivered filesystem contains the service, a minimized Corretto Java 17 runtime, musl, zlib, CA
roots, and a hardened validator JAR; it contains no shell, BusyBox, curl, apk database, glibc,
libssl, or libcrypto.

MobilityData validator v8.0.1 is rebuilt from source commit
`d74d7177f9f7c6bc7adc69508bb939362f2cf770`. Its source archive is gated by SHA-256
`651872b1a7abbde5b999d7261f875532eebaee22a9d7ce4946b8f764cdf7b8a3`, reviewed overrides update
the affected embedded libraries, the complete upstream test suite runs during the image build,
and archive normalization makes the output byte-reproducible. The accepted JAR SHA-256 is
`24ca7e890ca15bfbb36fa889fcb16200f7276995b7e6ec75551a8b7175e818d7`; startup verifies it using
the service's internal SHA-256 implementation.

## Evidence

- Grype 0.117.0 scanned the exact image above against valid database schema v6.1.9 (built
  2026-08-14T06:39:10Z) and reported **No vulnerabilities found**. The SPDX 2.3 SBOM contains 90
  packages, including OpenJDK and the embedded Maven libraries, so the result is not based on an
  empty inventory.
  The retained [scan summary](evidence/containerized-service-scan-summary.json),
  [raw Grype JSON](evidence/containerized-service-cves-grype.json),
  [SPDX SBOM](evidence/containerized-service-sbom.spdx.json), and
  [finding table](evidence/containerized-service-cves-grype.txt) identify that result and allow
  independent inventory review without access to the local image.
- A standalone Grype scan of the rebuilt validator JAR reported zero matches.
- Two independent clean validator builds produced the same JAR digest.
- The upstream validator build completed 69 Gradle tasks, including its complete test suite.
- The repository Rust suite passed (57 passed, 2 live tests ignored), with formatting, Clippy
  (`-D warnings`), documentation, and smoke-script syntax checks also passing.
- The complete live container harness passed: healthy in 13 seconds, 79 MiB reported by the
  harness, all four artifacts independently decoded, authorization guards passed, and offline
  last-good recovery was byte-identical. The harness generated a fresh SBOM and zero-match Grype
  JSON/table for the exact image.

The authorized [pre-remediation](../dependency-audit/pre-remediation.md) and
[post-remediation](../dependency-audit/post-remediation.md) Cargo audits each resolved 375
packages and 969 dependency edges. Both produced the same ten inherited warning records, no
blocked result, and no CISA KEV match. The post-change fingerprint differs because `sha2`, already
present transitively, became a direct dependency for internal validator hashing. Four GitHub
enrichments with invalid CVSS metrics remain explicit partial-source diagnostics. Closing this
container exception does not waive or relabel those warnings as clean.

## Ongoing controls

- Every base, source archive, helper image, and delivered validator artifact is digest-pinned.
- The Gradle 7.4 wrapper distribution is pinned by its official SHA-256, and strict dependency
  verification checks the complete resolved Gradle/Maven graph against committed checksums before
  executing the upstream suite or assembling the validator.
- The runtime is UID/GID 10001, loopback-only by default, and fail-closed for wildcard binding
  without an exact peer allowlist.
- The binary and validator are read-only; only `/data` is writable by the service user.
- The production image has no general-purpose diagnostic utility. Smoke-only curl/shell work uses
  a separately pinned helper image.
- Any image, base, validator override, Cargo lockfile, or network-policy change requires a fresh
  build, SBOM, vulnerability scan, and smoke run. A non-zero or unavailable scan must not be
  represented as clean.

## Closure record

| Decision | Date | Notes |
|---|---|---|
| Closed without acceptance | 2026-08-14 | Vulnerable image replaced; exact remediated image scan has zero findings. |
