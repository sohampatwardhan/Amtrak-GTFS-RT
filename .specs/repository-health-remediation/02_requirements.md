# Requirements: Repository Health Remediation

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Introduction

These requirements cover repository delivery health rather than runtime feed behavior. A **release candidate** is one immutable Git tag and the exact two-platform container image associated with its checked-out revision. **Diagnostic evidence** comprises the software bill of materials (SBOM), machine-readable vulnerability report, human-readable vulnerability report, and the exact image digest inspected. **Publication** means creating or confirming public version tags, attestations, and the GitHub release; it does not mean deploying a running service.

### Requirement 1: Advisory-fetcher continuous integration

**User Story:** As a maintainer, I want every shipped component tested on each proposed change, so that Python regressions cannot merge behind a Rust-only green check.

#### Acceptance Criteria

1. **R1.1** WHEN a pull request targets `main`, THE CI_Pipeline SHALL run the advisory-fetcher offline unit-test suite.
2. **R1.2** WHEN a change is pushed to `main`, THE CI_Pipeline SHALL run the advisory-fetcher offline unit-test suite.
3. **R1.3** THE CI_Pipeline SHALL install exact versions of the Python interpreter, test runner, and Playwright package used by the advisory-fetcher test job.
4. **R1.4** THE CI_Pipeline SHALL complete the advisory-fetcher offline unit-test suite without downloading a browser binary.
5. **R1.5** IF an advisory-fetcher unit test fails, THEN THE CI_Pipeline SHALL report the Python test job as failed.

### Requirement 2: Canonical release selection

**User Story:** As a release operator, I want tag-push and manual recovery runs to resolve one canonical immutable release identity, so that every gate evaluates the intended source revision.

#### Acceptance Criteria

1. **R2.1** WHEN a semantic-version tag push starts a release, THE Release_Selector SHALL select the pushed tag as the canonical release tag.
2. **R2.2** WHEN an authorized operator manually starts a release recovery with a semantic-version tag, THE Release_Selector SHALL select the requested tag as the canonical release tag.
3. **R2.3** WHEN a canonical release tag is selected, THE Release_Selector SHALL resolve the checked-out commit of that tag as the canonical release revision.
4. **R2.4** IF a manual release tag is absent from the repository, THEN THE Release_Selector SHALL stop the release before image validation.
5. **R2.5** IF a selected release tag is not an exact `vMAJOR.MINOR.PATCH` tag, THEN THE Release_Selector SHALL stop the release before image validation.
6. **R2.6** THE Release_Selector SHALL expose the same canonical tag and revision to every platform-validation and publication gate in the run.

### Requirement 3: Immutable artifact verification

**User Story:** As a release operator, I want existing container artifacts reused only when they match the canonical release, so that recovery cannot mutate or bless unrelated images.

#### Acceptance Criteria

1. **R3.1** WHEN an existing version-tagged container manifest is found, THE Artifact_Verifier SHALL require exactly one `linux/amd64` image and one `linux/arm64` image.
2. **R3.2** WHEN an existing platform image is selected, THE Artifact_Verifier SHALL verify that its recorded source revision equals the canonical release revision.
3. **R3.3** WHEN an existing platform image is selected, THE Artifact_Verifier SHALL verify that its recorded version equals the canonical release version.
4. **R3.4** IF an existing version tag has a missing, duplicate, or unexpected platform, THEN THE Artifact_Verifier SHALL stop the release without changing the tag.
5. **R3.5** IF existing image metadata disagrees with the canonical release identity, THEN THE Artifact_Verifier SHALL stop the release without changing the image.
6. **R3.6** IF no existing platform image is available, THEN THE Release_Workflow SHALL create a new platform image from the canonical release revision before validation.

### Requirement 4: Actionable release evidence

**User Story:** As a release reviewer, I want complete evidence from failed and successful scans, so that I can identify the exact reason for a release decision without reproducing the scan.

#### Acceptance Criteria

1. **R4.1** WHEN a platform image reaches security validation, THE Evidence_Recorder SHALL generate an SBOM for the exact platform digest.
2. **R4.2** WHEN a platform image reaches security validation, THE Evidence_Recorder SHALL generate a machine-readable vulnerability report for the exact platform digest.
3. **R4.3** WHEN a platform image reaches security validation, THE Evidence_Recorder SHALL generate a human-readable vulnerability report for the exact platform digest.
4. **R4.4** WHEN vulnerability findings exist, THE Evidence_Recorder SHALL report each finding's identifier, severity, affected artifact, installed version, and available fixed version.
5. **R4.5** WHEN diagnostic evidence files exist, THE Evidence_Recorder SHALL retain them for review before the vulnerability decision can stop the job.
6. **R4.6** IF any required diagnostic evidence is missing or malformed, THEN THE Evidence_Recorder SHALL report the platform validation as failed.

### Requirement 5: Fail-closed vulnerability policy

**User Story:** As a security reviewer, I want release publication gated by explicit evidence, so that missing or vulnerable images are never described as clean.

#### Acceptance Criteria

1. **R5.1** WHEN a well-formed vulnerability report contains zero matches, THE Vulnerability_Gate SHALL allow that platform to continue to attestation.
2. **R5.2** IF a vulnerability report contains one or more matches, THEN THE Vulnerability_Gate SHALL reject that platform.
3. **R5.3** IF vulnerability scanning does not complete successfully, THEN THE Vulnerability_Gate SHALL reject that platform.
4. **R5.4** IF either required platform is rejected, THEN THE Release_Publisher SHALL not publish or update release tags.
5. **R5.5** IF either required platform is rejected, THEN THE Release_Publisher SHALL not create a GitHub release.

### Requirement 6: Controlled recovery and publication

**User Story:** As a release operator, I want failed immutable releases recoverable through the normal gates, so that corrected workflow logic can finish a release without moving its tag.

#### Acceptance Criteria

1. **R6.1** WHEN a manual recovery is requested, THE Release_Workflow SHALL apply the same platform smoke, scan, evidence, attestation, manifest, and publication gates used by a tag push.
2. **R6.2** IF a manual recovery is requested from a branch or pull-request event without an explicit tag input, THEN THE Release_Workflow SHALL not publish a release.
3. **R6.3** THE Release_Workflow SHALL not move or delete the canonical Git tag.
4. **R6.4** THE Release_Workflow SHALL grant write permissions only to jobs that upload images, attestations, manifests, or GitHub release assets.
5. **R6.5** WHEN both platform validations pass, THE Release_Publisher SHALL verify or assemble one two-platform manifest before creating the GitHub release.
6. **R6.6** WHEN a manual recovery completes successfully, THE Release_Publisher SHALL record the canonical release tag, revision, manifest digest, and platform digests in the release evidence.
7. **R6.7** IF a manual recovery's workflow control ref is not `refs/heads/main`, THEN THE Release_Workflow SHALL stop the release before image validation.

### Requirement 7: Accurate repository state and handoff

**User Story:** As a maintainer, I want documentation and delivery records to distinguish source, release, and deployment state, so that operators do not act on stale lifecycle claims.

#### Acceptance Criteria

1. **R7.1** THE Documentation_Set SHALL identify PR #8 and PR #9 as merged into `main` without any contradictory current-state claim.
2. **R7.2** THE Documentation_Set SHALL identify v0.2.0 as the latest completed GitHub release until a later GitHub release exists.
3. **R7.3** THE Documentation_Set SHALL identify v0.3.0 source as integrated on `main` while its GitHub release remains incomplete.
4. **R7.4** THE Documentation_Set SHALL distinguish public container publication from deployment of a running service.
5. **R7.5** THE Documentation_Set SHALL retain deployment, anonymous public exposure, proxy trust, and orchestration as out of scope.
6. **R7.6** THE Delivery_Record SHALL identify the exact commands and evidence required for an operator to recover v0.3.0 after the remediation is merged.
7. **R7.7** THE Delivery_Record SHALL identify PR #7 as a separate unresolved feature stream that this remediation does not modify.

## Assumptions and Risk Classification

- **Security and authorization:** applicable. Manual recovery uses repository-controlled workflow authorization, and all publication remains gated by exact release identity and zero-match vulnerability evidence.
- **Supply chain:** applicable. Existing images may be reused only after immutable digest, platform, version, and revision checks; missing evidence is a failure.
- **Observability:** applicable. Failed scans must leave sufficient retained evidence to explain the decision.
- **Migration:** not applicable. No runtime data format, volume layout, or API contract changes.
- **Performance and accessibility:** not applicable to the repository-control surface beyond normal bounded CI execution.
- **Rollout and rollback:** applicable. Merging the remediation changes no running deployment and triggers no release recovery; workflow and documentation changes are independently revertible.

## Approval

Status: **Re-approved on 2026-08-23 through the explicit direction to apply the audit fixes and execute.**
