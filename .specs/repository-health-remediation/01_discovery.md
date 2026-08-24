# Discovery: Repository Health Remediation

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Problem and Outcome

The repository's Rust service is healthy, but its delivery controls no longer cover the whole product and do not provide enough evidence when a release gate fails. The normal [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) builds and tests only Rust, even though [`advisory-fetcher/`](../../advisory-fetcher/) is now a shipped Python component. The v0.3.0 release workflow passed both platform smoke tests but stopped at an opaque zero-vulnerability assertion before uploading the reports that explained the failure. Because tag-triggered workflows use the workflow stored at the immutable tag, repairing `main` alone cannot safely rerun that release. Several human-readable state documents also describe already-merged work as pending or retain superseded release blocks.

The desired outcome is one reviewable remediation branch that makes every maintained component part of CI, preserves the strict release security policy while making failures actionable, provides a safe manual path for retrying an immutable failed release from a corrected workflow, and reconciles project documentation with authoritative GitHub and test evidence.

## Users and Current Workaround

- Maintainers currently run the advisory-fetcher's offline tests manually and must trust retained execution notes because pull-request CI does not run them.
- Release operators currently inspect long workflow logs and reproduce scans locally. A failed immutable tag cannot use a corrected tag-local workflow without moving the tag or creating an ad hoc recovery mechanism.
- Reviewers and future maintainers currently reconcile conflicting claims across [`README.md`](../../README.md), [`advisory-fetcher/README.md`](../../advisory-fetcher/README.md), and historical spec state by consulting merged pull requests and Actions runs.

## Scope and Non-Goals

In scope:

- Run deterministic offline Python tests for the advisory-fetcher in pull-request and `main` CI alongside the existing Rust job.
- Keep the release policy fail-closed on any Grype match while always retaining a human-readable summary, JSON report, table report, and SBOM from the exact platform digest.
- Allow an explicitly requested immutable tag to be validated by the corrected workflow on `main`, using the checked-out tag SHA rather than the workflow-run SHA and reusing only verified existing digests.
- Correct root, advisory-fetcher, and spec-state documentation where repository evidence now contradicts the text.
- Add verification for the workflow's release-input selection and vulnerability-report policy where it can run deterministically without registry publication.

Out of scope:

- Moving, deleting, or replacing the existing v0.3.0 tag.
- Weakening the zero-match vulnerability policy, suppressing findings, or publishing when evidence is unavailable.
- Automatically dispatching a release or changing external registry/release state as part of this pull request.
- Deploying the service, exposing it anonymously, trusting forwarded identity, or adding orchestration.
- Rebasing, merging, or otherwise absorbing the separate station-and-train-status work in PR #7.
- Changing Rust service behavior, advisory parsing behavior, or runtime dependency resolution.

## Constraints and Success Measures

Constraints:

- Preserve the dirty existing checkout; work occurs on `codex/repository-health-remediation` from current `origin/main` in an isolated worktree.
- Keep Git tags and already-published digests immutable. Manual recovery must prove that the requested tag, checkout revision, image metadata, and platform digests agree.
- Keep browser binaries out of normal CI; the offline Python tests mock browser behavior and need the Playwright package, not a Chromium download.
- Keep evidence available even when the policy gate fails. Diagnostic upload steps must use failure-safe conditions without allowing later publication steps to run after a failed gate.
- Do not introduce or update runtime dependencies. Test tooling must be exact-version pinned in CI.

Success is demonstrated when:

- Rust and advisory-fetcher test jobs both run on pull requests and `main`, with the Python job covering [`advisory-fetcher/tests`](../../advisory-fetcher/tests).
- Synthetic clean and vulnerable Grype reports prove that the policy helper accepts zero matches, rejects one or more matches, and prints actionable finding identities.
- Workflow validation proves both tag-push and manual-tag selection use the immutable tag checkout and its actual revision throughout build and verification inputs.
- Scan/SBOM evidence upload is ordered before the fail-closed policy decision, while attest/publish jobs remain dependent on a passing policy gate.
- All corrected documentation agrees that PRs #8 and #9 are merged, v0.2.0 is the latest completed GitHub release, v0.3.0 source is on `main`, and deployment remains a separate blocked/not-authorized action.

## Approaches Considered

| Approach | Benefits | Costs / risks | Reversibility | Decision |
|---|---|---|---|---|
| Comprehensive CI, release recovery, diagnostics, and documentation remediation | Addresses each observed repository-owned weakness; preserves immutable artifacts and the strict scan policy; produces reviewable evidence | More workflow logic and verification than a documentation-only change; manual recovery remains an explicit post-merge operator action | High: workflow and documentation changes can be reverted without changing runtime data | **Selected** |
| CI and documentation only | Small and easy to review | Leaves v0.3.0 without a supported retry path and leaves future release failures opaque | High | Rejected as incomplete |
| Abandon v0.3.0 and prepare v0.3.1 | Uses the existing tag-push path | Leaves a confusing incomplete tag/image state and avoids fixing weak diagnostics; creates a new version without product changes | Medium | Rejected |

## Chosen Direction

The remediation will add a distinct Python CI job, extract deterministic vulnerability-report evaluation into a locally testable script, restructure release steps so evidence is uploaded before the policy decision, and extend the release workflow with an explicit manual tag input. The workflow will derive a canonical release tag and checked-out revision once and pass those values to every platform and publication check. Existing digest reuse remains conditional on metadata and manifest verification; a manual retry is not permission to mutate an incompatible artifact.

Documentation updates will state three separate lifecycle facts instead of collapsing them: source integration on `main`, completed public container release, and deployment/consumer rollout. Historical execution evidence will remain intact, while current gate summaries and contradicted change-control prose will be corrected.

## Architecture and Flow Outline

The change has three bounded surfaces:

1. **Continuous integration:** the existing Rust job remains unchanged in purpose, and a sibling Python job installs exact test-tool versions and runs the advisory-fetcher test suite offline.
2. **Release control:** tag selection and revision resolution feed the existing per-platform build/smoke/scan jobs. Those jobs create and upload diagnostic evidence before a separate policy step can stop attestation and publication. Manual dispatch uses the same downstream jobs and gates as a tag push.
3. **Project state:** root and component guides describe supported operation, while feature state ledgers distinguish completed implementation/release from intentionally absent deployment.

These surfaces share no runtime code path and can be reviewed independently within one repository-health change.

## Failure and Verification Strategy

- A Python dependency installation or unit-test failure fails only the Python CI job but blocks the pull request through required checks.
- A scanner, SBOM, report-shape, or vulnerability-policy failure stops attestation and publication. Evidence upload still runs when files exist, and missing evidence is reported rather than described as clean.
- An invalid manual tag, non-semantic tag, revision mismatch, missing platform, incompatible existing digest, or image-metadata mismatch fails before publication.
- Local verification covers Rust tests, Python tests, shell syntax, policy-script fixtures, workflow structure, spec validation, and documentation consistency searches. GitHub Actions remains authoritative for hosted-runner, registry, attestation, and release behavior.

## Open Decisions

- Dispatching recovery for v0.3.0 is intentionally deferred until this pull request is reviewed and merged. The remediation will document the exact operator command and required evidence but will not execute it from the feature branch.
- PR #7 requires its own rebase and product review; this remediation neither closes nor modifies it.

## Approval

Status: **Approved on 2026-08-23**. The user requested that the comprehensive weakness remediation be documented and delivered through the spec-driven workflow.
