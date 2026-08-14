# Task 2.1 Review — Add the smoke harness and operator runbook

**Reviewer:** independent `feature-dev:code-reviewer` agent (dedicated review; high-risk task —
destructive cleanup + security boundary).
**Initial verdict:** issues found (2 confidence-85 defects); all other axes sound.
**Final verdict after repair round 1:** **PASS** — both defects fixed and re-verified.

## Findings and resolution

1. **Fail-closed guard tests could hang indefinitely** (`test-container.sh` config guards were
   foreground `docker run` with no bound). *Fixed:* added the `run_guarded` helper (detached run,
   bounded `GUARD_DEADLINE` wait, still-running-at-deadline → failure). Re-run confirms both guards
   report the refusal, never block.

2. **R4.4 and R6.6 not exercised by the harness.** *Fixed:*
   - **R4.4** — added a deterministic test that plants an incomplete newest generation candidate
     in the volume and asserts the offline restart exposes only the older valid generation
     (`incomplete newest candidate ... was NOT exposed`).
   - **R6.6** — added a container-level fail-closed assertion (empty volume + no upstream exits
     rather than serving an absent feed set). The transient "listener up, no committed generation"
     window that R6.6 describes is not deterministically forceable at the container boundary
     without a fault hook; that admitted-`503` router invariant is verified deterministically by
     the unchanged Rust test `serve::tests::readiness_obeys_no_generation_and_exact_freshness_boundaries`
     (identical binary in the image). Recorded in the task report.

## Axes the reviewer confirmed sound
- Destructive cleanup only ever targets uniquely `$RUN_ID`-suffixed objects and the script's own
  `mktemp` workdir — no globs, no unrelated Docker objects.
- The named volume is created once and removed only in the single `EXIT` trap, after recovery.
- Peer authorization is deterministic (static-IP peer containers on a user-defined bridge preserve
  source IP); the SNAT-sensitive host path is a soft `NOTE`, matching the design caveat.
- The protobuf decode is a genuine independent wire-format cross-check, not a round-trip.
- Docker Scout CVE-unavailable output is never described as "clean".
- README "Container" section covers R7.1–7.6 and R5.6 with no scope creep.
