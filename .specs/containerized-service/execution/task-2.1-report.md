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

## Notes / shipping posture

Local container verification is complete. Shipping remains blocked pending an authenticated Docker
Scout CVE report (and the pre-existing dependency-audit block from the base service). Registry
publication and deployment stay out of scope and require separate user authorization.
