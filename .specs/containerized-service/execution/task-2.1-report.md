# Task 2.1 Report — Add the smoke harness and operator runbook

**Files:** [`scripts/test-container.sh`](../../../scripts/test-container.sh),
[`README.md`](../../../README.md) (new "Container" section)
**Mode:** controller (executed directly, not delegated)
**Requirements:** 4.1–4.4, 5.2–5.6, 6.1–6.6, 7.1–7.6

## What was built

**`scripts/test-container.sh`** — a fail-closed (`set -euo pipefail`), non-interactive smoke
harness, bash 3.2-compatible (macOS default) as well as bash 4+. It provisions a uniquely named
private bridge (`172.31.243.0/24`, explicit gateway), an isolated named volume, and uniquely named
containers, and an `EXIT` trap removes only those objects — the volume last, so it survives the
restart-recovery step. Authorization is exercised with statically-IP'd peer containers on the
user-defined bridge (source IP is preserved, so allow/deny is deterministic across engines).

The harness proves, in one bounded run against a live Amtrak fetch:
- wildcard bind without an allowlist refuses to start; a non-writable `/data` refuses to start;
- the service reaches Docker `healthy`, `/readyz` returns `200` (via an allow-listed peer), and the
  manifest identifies one coherent generation;
- all four manifest-selected artifacts are fetched and **independently** decoded — the static ZIP
  by integrity + core-file membership, and each protobuf by a dependency-free wire-format parser
  that confirms a `FeedHeader` with `gtfs_realtime_version` (a genuine cross-check, not a
  round-trip through the service's own encoder);
- a denied peer gets `403`, and the same peer spoofing `X-Forwarded-For`/`Forwarded`/`X-Real-IP`
  with allow-listed identities still gets `403`;
- host-loopback publication works for public `/livez`, and the observed host peer is reported;
- recreating the container over the retained volume with **no connectivity** (`--network none`)
  recovers the same `generation_id` with byte-identical artifacts;
- image size and time-to-health are recorded; a Docker Scout SBOM is exported and the CVE report
  is attempted, with unavailable CVE evidence reported as **not clean** rather than assumed clean.

**`README.md` "Container" section** — build command; the image contract; host-vs-container env
differences (table); a safe host-networking run; a dedicated-bridge run with the required
`AMTRAK_BIND_ADDR=0.0.0.0:8080` + exact `AMTRAK_ALLOWED_PEER_IPS`; health/readiness/manifest/
artifact commands; the `/livez`-vs-`/readyz` rationale; volume retention and rollback; a pointer
to the smoke test; and an explicit out-of-scope note (anonymous exposure, proxy/forwarded-identity
trust, registry publication, orchestration). The existing Deployment block is unchanged.

## Verification evidence

- `bash -n scripts/test-container.sh` → clean; `scripts/test-container.sh amtrak-gtfs-rt:local`
  → **CONTAINER SMOKE HARNESS PASSED** (exit 0). Healthy in 6–7s; image **156 MB**; static.zip
  18.6 MB / 8 GTFS entries; three protobufs decode as GTFS-Realtime 2.0; offline recovery
  reproduced generation `…863000000000-0` byte-for-byte.
- `cargo fmt --check` → clean; `cargo clippy --all-targets --all-features -- -D warnings` → clean;
  `cargo test --all-targets --all-features` → 54 passed, 0 failed, 2 ignored;
  `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` → clean.
- Offline feed gate `scripts/validate-feeds.sh --offline-fixtures` → exit 0 (all 6 fixtures pass).
- Live feed gate `scripts/validate-feeds.sh` → **PASS: no new validator ERROR codes**
  (static: INFO/WARNING only; RT within baseline). Reports in `validation-reports/`.
- Docker Scout: SBOM written to `validation-reports/container/sbom.spdx.json`. **CVE evidence
  UNAVAILABLE** — `docker scout cves` requires `docker login`; recorded as *not clean*, not as
  "no CVEs". This is consistent with the design decision that unavailable image evidence cannot
  ship; the existing release block stands.
- `git diff --check` clean. No `docker push` and no deployment were performed.

## Review resolution (repair round 1)

Independent review raised two confidence-85 findings; both were fixed and re-verified:

1. **Fail-closed guards could hang.** The two config-guard `docker run` calls were foreground with
   no bound; a regression that wrongly started serving would hang the unattended harness. Added a
   `run_guarded` helper that runs each guard detached and waits at most `GUARD_DEADLINE` (30s),
   reporting a still-running container as a failure instead of blocking.
2. **R4.4 and R6.6 were not exercised.** Added a deterministic R4.4 test: a root helper plants an
   incomplete newest generation candidate (`9999999999999999999-0`, no `manifest.txt`) in the
   volume, and the offline restart must still expose only the older valid generation — verified
   (`incomplete newest candidate was NOT exposed`). For R6.6, the transient "listener up, no
   generation committed" window is not deterministically forceable without a fault hook (an empty
   volume with no upstream fails closed at startup, which the harness now also asserts); the
   admitted-`503`-when-no-generation router invariant is verified deterministically by the
   unchanged Rust test `serve::tests::readiness_obeys_no_generation_and_exact_freshness_boundaries`,
   which runs identically in the packaged binary.

Re-run after the fixes: **CONTAINER SMOKE HARNESS PASSED** (exit 0), no leftover Docker objects.

## CVE evidence (caveat follow-up, 2026-08-14)

The harness now prefers Docker Scout for CVE evidence and falls back to
[grype](https://github.com/anchore/grype) when Scout is unauthenticated (grype scans the local
image with no registry login). grype `0.117.0` was installed and produced
`validation-reports/container/cves-grype.{txt,json}` for `amtrak-gtfs-rt:local`.

The report is **not clean**: 401 matches (33 Critical, 74 High, 146 Medium, 10 Low, 122 Negligible,
16 Unknown). The Critical/High findings are almost entirely inherited, not introduced by this
packaging:

- **97 OS-package findings** from the Debian base and, predominantly, the packages
  `openjdk-17-jre-headless` pulls in — `perl`/`libperl`, `curl`/`libcurl4`, `libglib2.0-0`,
  `libssh2-1`, `libexpat1`, `libcups2`, `libc6`. Many are Debian `won't fix` / negligible EPSS.
- **10 Java-archive findings** inside the bundled MobilityData validator JAR
  (`commons-compress 1.20`, `commons-beanutils 1.9.2`).

So the "CVE evidence unavailable" caveat is **resolved** (evidence now exists and was reviewed),
but it shows the image is not vulnerability-free. Follow-up triage classified all 33 Critical
matches as Debian `not-fixed` or `won't-fix`. The only 10 fixable Critical/High matches are High
findings in Java libraries embedded in MobilityData GTFS Validator v8.0.1, confirmed on 2026-08-14
as the latest upstream release. A smaller/distroless runtime would not remove those vendor-JAR
findings. It could reduce OS-package surface, but would require redesigning the service's Java,
`shasum`, glibc, and health-probe dependencies and rerunning the complete compatibility suite. The
resulting residual risk is documented in a
[proposed risk-acceptance record](../../../.security/risk-acceptance/containerized-service.md).

## Notes / shipping posture

Local container verification is complete and CVE evidence now exists (grype). Available
remediation was investigated; the remaining exception is documented but **not accepted**. Shipping
still requires explicit risk-owner approval and remains subject to the pre-existing base-service
dependency-audit block, inherited from the `catenary/amtrak-gtfs-rt` crate chain and unaffected by
containerization. Registry publication and deployment stay out of scope and require separate user
authorization.
