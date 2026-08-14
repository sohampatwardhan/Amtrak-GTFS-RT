# Public Container Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development` (recommended) or `executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish Amtrak GTFS-RT v0.2.0 as a public, AGPL-licensed, multi-platform GHCR image and GitHub Release with exact-digest validation and anonymous installation evidence.

**Architecture:** Prepare and review release metadata on a branch, then use a tag-triggered GitHub Actions matrix to build and push canonical `linux/amd64` and `linux/arm64` digests on native runners. Each digest is pulled, smoke-tested, inventoried, and scanned before a publication job creates semantic manifest tags, provenance, and the GitHub Release; repository/package visibility changes and anonymous pulls are verified separately.

**Tech Stack:** Rust/Cargo, Docker Buildx, GHCR, GitHub Actions, Syft 1.51.0, Grype 0.117.0, cargo-about 0.9.1, dependency-security-audit, Gitleaks 8.30.1, Bash, jq, GitHub CLI.

## Global Constraints

- Release version is `0.2.0`; Git tag is `v0.2.0`.
- Image is `ghcr.io/sohampatwardhan/amtrak-gtfs-rt` with `linux/amd64` and `linux/arm64` manifests.
- Public tags are `0.2.0`, `0.2`, and `latest`; production documentation recommends the immutable manifest digest.
- Project license remains exactly `AGPL-3.0-only`; third-party licenses remain separately attributed.
- The final image remains scratch-based, UID/GID `10001`, loopback-only by default, and persistent at `/data`.
- No manifest tag or GitHub Release is created until both exact platform digests pass smoke, inventory, license, and zero-match Grype gates.
- Missing scanners, incomplete SBOMs, missing licenses, dependency-audit unavailability, or history-audit findings fail closed.
- Existing user-owned changes in `/Users/soham/GitRepos/Amtrak-GTFS-RT` remain untouched; implementation uses `/tmp/amtrak-release-v020`.
- Repository and package visibility changes happen only after the pre-publication audit and merged release-preparation PR.

---

### Task 1: Version, changelog, and deterministic third-party licensing

- [x] **Task status:** Complete and reviewed

**Files:**
- Create: `about.toml`
- Create: `about.hbs`
- Create: `THIRD_PARTY_LICENSES.html`
- Create: `scripts/generate-third-party-licenses.sh`
- Create: `scripts/check-release-metadata.sh`
- Modify: `Cargo.toml:3`
- Modify: `Cargo.lock:85`
- Modify: `CHANGELOG.md:7-55`
- Test: `scripts/check-release-metadata.sh`

**Interfaces:**
- Consumes: `Cargo.lock`, root package metadata, repository `LICENSE`, and target release `0.2.0`.
- Produces: `scripts/check-release-metadata.sh VERSION [TAG]`, committed `THIRD_PARTY_LICENSES.html`, and a changelog section extractable as `## [0.2.0] - 2026-08-14`.

- [x] **Step 1: Add the failing metadata checker**

Create `scripts/check-release-metadata.sh` with strict Bash mode. It must accept version as argument 1 and optional tag as argument 2, derive the root package version using `cargo metadata --locked --offline --no-deps`, and fail unless all of these are true:

```bash
#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:?usage: scripts/check-release-metadata.sh VERSION [TAG]}"
TAG="${2:-v${VERSION}}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

actual="$(cargo metadata --manifest-path "$ROOT/Cargo.toml" --locked --offline --no-deps \
  --format-version 1 | jq -r '.packages[] | select(.name == "amtrak-gtfs-rt-service") | .version')"
[ "$actual" = "$VERSION" ]
[ "$TAG" = "v${VERSION}" ]
grep -Fqx "## [${VERSION}] - 2026-08-14" "$ROOT/CHANGELOG.md"
grep -Fqx 'license = "AGPL-3.0-only"' "$ROOT/Cargo.toml"
test -s "$ROOT/THIRD_PARTY_LICENSES.html"
```

- [x] **Step 2: Run the checker and confirm the pre-release tree fails**

Run: `bash scripts/check-release-metadata.sh 0.2.0 v0.2.0`

Expected: non-zero because Cargo and changelog still report `0.1.0`/Unreleased and the third-party notice bundle does not exist.

- [x] **Step 3: Bump the crate and close the changelog release**

Change the root package version in `Cargo.toml` and the `amtrak-gtfs-rt-service` package entry in `Cargo.lock` to `0.2.0`. Keep a new empty `## [Unreleased]` section, rename the populated section to `## [0.2.0] - 2026-08-14`, add release bullets for public GHCR distribution and licensing, and set links exactly as:

```markdown
[Unreleased]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.2.0
[0.1.0]: https://github.com/sohampatwardhan/Amtrak-GTFS-RT/releases/tag/v0.1.0
```

- [x] **Step 4: Configure cargo-about and generate the notice bundle**

Create `about.toml` with both Linux musl targets, ignored dev dependencies, and this accepted license set:

```toml
accepted = [
  "0BSD", "AGPL-3.0", "AGPL-3.0-only", "Apache-2.0", "BSD-3-Clause",
  "BSL-1.0", "CC0-1.0", "CDLA-Permissive-2.0", "ISC",
  "LGPL-2.1-or-later", "MIT", "MIT-0", "MPL-2.0", "Unicode-3.0",
  "Unlicense", "Zlib",
]
targets = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"]
ignore-dev-dependencies = true
workarounds = ["chrono", "prost", "ring", "rustls", "rustix"]
```

Create `about.hbs` as a standalone HTML document that renders `overview`, each license's SPDX ID and complete text, and every `used_by` crate name/version. Install and run:

```bash
cargo install --locked cargo-about --version 0.9.1 --features cli
scripts/generate-third-party-licenses.sh THIRD_PARTY_LICENSES.html
test -s THIRD_PARTY_LICENSES.html
```

If cargo-about rejects a legacy non-SPDX expression, add only a checksum-backed crate clarification described by the tool output; do not globally accept an unidentified license.

- [x] **Step 5: Add freshness validation and pass the metadata checker**

Extend `scripts/check-release-metadata.sh` to generate the notice into `mktemp` output and `cmp` it with the committed file when `cargo-about` is installed. Run:

```bash
bash -n scripts/check-release-metadata.sh
bash scripts/check-release-metadata.sh 0.2.0 v0.2.0
git diff --check
```

Expected: all commands exit 0.

- [x] **Step 6: Commit Task 1**

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md about.toml about.hbs \
  THIRD_PARTY_LICENSES.html scripts/generate-third-party-licenses.sh \
  scripts/check-release-metadata.sh
git commit -m "chore: prepare v0.2.0 release metadata"
```

---

### Task 2: Licensed OCI image and operator documentation

- [x] **Task status:** Complete and reviewed

**Files:**
- Create: `container/licenses/AGPL-3.0-only.txt`
- Create: `scripts/verify-release-image.sh`
- Modify: `.dockerignore`
- Modify: `Dockerfile`
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Test: `scripts/verify-release-image.sh`

**Interfaces:**
- Consumes: `LICENSE`, `THIRD_PARTY_LICENSES.html`, OCI build arguments `OCI_VERSION` and `OCI_REVISION`, and an image reference.
- Produces: `/licenses/AGPL-3.0-only.txt`, `/licenses/THIRD_PARTY_LICENSES.html`, OCI labels, and `scripts/verify-release-image.sh IMAGE VERSION REVISION ARCH`.

- [x] **Step 1: Add the failing image metadata verifier**

Create `scripts/verify-release-image.sh` with strict Bash mode. It must inspect the image and require:

```bash
test "$(docker image inspect "$IMAGE" -f '{{.Config.User}}')" = "10001:10001"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.licenses"}}')" = "AGPL-3.0-only"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.source"}}')" = "https://github.com/sohampatwardhan/Amtrak-GTFS-RT"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.version"}}')" = "$VERSION"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.revision"}}')" = "$REVISION"
test "$(docker image inspect "$IMAGE" -f '{{.Architecture}}')" = "$ARCH"
```

It must create a stopped container, `docker cp` both `/licenses` files to a temporary directory, compare the AGPL text byte-for-byte with repository `LICENSE`, require a non-empty third-party HTML file, and clean up only that temporary container/directory via a trap.

- [x] **Step 2: Verify the current image fails the new contract**

Run: `bash scripts/verify-release-image.sh amtrak-gtfs-rt:local 0.2.0 "$(git rev-parse HEAD)" arm64`

Expected: non-zero because the existing image has no release labels or `/licenses` bundle.

- [x] **Step 3: Add license files and OCI metadata to the Dockerfile**

Copy `LICENSE` byte-for-byte to `container/licenses/AGPL-3.0-only.txt`. Ensure `.dockerignore` admits `LICENSE`, `THIRD_PARTY_LICENSES.html`, and `container/licenses/**`. Add before the final stage:

```dockerfile
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown
```

In the final stage redeclare the arguments, copy both license files, and add:

```dockerfile
ARG OCI_VERSION
ARG OCI_REVISION
LABEL org.opencontainers.image.title="Amtrak GTFS-RT" \
      org.opencontainers.image.description="Validated static and realtime GTFS feeds for Amtrak" \
      org.opencontainers.image.source="https://github.com/sohampatwardhan/Amtrak-GTFS-RT" \
      org.opencontainers.image.documentation="https://github.com/sohampatwardhan/Amtrak-GTFS-RT#container" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.version="$OCI_VERSION" \
      org.opencontainers.image.revision="$OCI_REVISION"
COPY --chmod=0444 container/licenses/AGPL-3.0-only.txt /licenses/AGPL-3.0-only.txt
COPY --chmod=0444 THIRD_PARTY_LICENSES.html /licenses/THIRD_PARTY_LICENSES.html
```

- [x] **Step 4: Document public installation, immutable deployment, upgrade, and rollback**

Update `README.md` to use `ghcr.io/sohampatwardhan/amtrak-gtfs-rt:0.2.0` in the recommended Linux host-network command, retain local-build instructions in a separate development subsection, document anonymous pull, `latest` as convenience-only, digest pinning, multi-platform support, named-volume retention, and the AGPL/source/third-party notice locations. Replace the statement that registry publication is out of scope with the exact release workflow contract.

- [x] **Step 5: Build and verify the licensed local image**

```bash
docker build \
  --build-arg OCI_VERSION=0.2.0 \
  --build-arg OCI_REVISION="$(git rev-parse HEAD)" \
  -t amtrak-gtfs-rt:release-test .
bash scripts/verify-release-image.sh amtrak-gtfs-rt:release-test 0.2.0 \
  "$(git rev-parse HEAD)" "$(docker image inspect amtrak-gtfs-rt:release-test -f '{{.Architecture}}')"
scripts/test-container.sh amtrak-gtfs-rt:release-test
```

Expected: image verifier and complete smoke/recovery harness pass.

- [x] **Step 6: Commit Task 2**

```bash
git add .dockerignore Dockerfile README.md CHANGELOG.md container/licenses \
  scripts/verify-release-image.sh
git commit -m "feat: add licensed public image contract"
```

---

### Task 3: Fail-closed scanner installation and release workflow

- [ ] **Task status:** Complete and reviewed

**Files:**
- Create: `scripts/install-release-scanners.sh`
- Create: `scripts/extract-release-notes.sh`
- Create: `.github/workflows/release.yml`
- Modify: `README.md`
- Test: workflow YAML, shell syntax, scanner checksums, and release metadata scripts

**Interfaces:**
- Consumes: exact tag `v0.2.0`, `scripts/test-container.sh IMAGE`, `scripts/verify-release-image.sh IMAGE VERSION REVISION ARCH`, GHCR credentials via `GITHUB_TOKEN`, and native matrix runners.
- Produces: canonical per-platform GHCR digests, per-platform evidence artifacts, semantic manifest tags, provenance, and GitHub Release assets.

- [ ] **Step 1: Add the scanner installer with immutable release-asset checksums**

Create `scripts/install-release-scanners.sh DESTDIR` supporting `x86_64`/`amd64` and `aarch64`/`arm64`. Download the matching official archives and verify these exact SHA-256 values before extracting only `syft` and `grype`:

```text
grype 0.117.0 linux_amd64 38525dab1e06f162ebaa02f94d82d1f807076b011a44180cf2777edf1a7b9c26
grype 0.117.0 linux_arm64 935f628bdf9331ffdd946931ea5fdb50045d3970ba52670cbeb44a88f127291b
syft  1.51.0  linux_amd64 2a2e837a2c8d59ec9af5472ee22d3b04ee463c4e44476ecf993fd1e5ab6ebc7f
syft  1.51.0  linux_arm64 6c0466811541ea03add5213a60a1562f0851e4c0b0ecfdee1a694a9455285900
```

The script must fail for unsupported architectures, checksum mismatch, failed download, missing executable, or version mismatch.

- [ ] **Step 2: Add deterministic changelog note extraction**

Create `scripts/extract-release-notes.sh VERSION OUTPUT`. Use `awk` to copy content strictly after `## [VERSION] - 2026-08-14` and before the next `## [` heading. Require non-empty output and fail if the heading occurs other than once.

- [ ] **Step 3: Test support scripts locally**

```bash
bash -n scripts/install-release-scanners.sh scripts/extract-release-notes.sh
scan_dir="$(mktemp -d)"
scripts/install-release-scanners.sh "$scan_dir"
"$scan_dir/syft" version
"$scan_dir/grype" version
notes="$(mktemp)"
scripts/extract-release-notes.sh 0.2.0 "$notes"
test -s "$notes"
```

Expected: all commands exit 0 and report Syft 1.51.0 / Grype 0.117.0.

- [ ] **Step 4: Create the tag-gated matrix workflow**

Create `.github/workflows/release.yml` triggered only by `push.tags: ['v*.*.*']`. Add concurrency `release-${{ github.ref }}` without cancel-in-progress. Start with `contents: read`; grant only job-specific `packages: write`, `id-token: write`, `attestations: write`, or `contents: write` where required. A failed run is retried with GitHub's rerun mechanism against the same immutable tag.

Use this native matrix:

```yaml
matrix:
  include:
    - platform: linux/amd64
      arch: amd64
      runner: ubuntu-24.04
      slug: linux-amd64
    - platform: linux/arm64
      arch: arm64
      runner: ubuntu-24.04-arm
      slug: linux-arm64
```

Pin actions to these immutable commits, retaining version comments:

```text
actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1       # v7.0.1
actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
actions/attest-build-provenance@4d101475d8b20a2381f78447822ac1eab6504dd8 # v4.2.2
docker/login-action@dbcb813823bdd20940b903addbd779551569679f     # v4.6.0
docker/setup-qemu-action@96fe6ef7f33517b61c61be40b68a1882f3264fb8 # v4.2.0
docker/setup-buildx-action@bb05f3f5519dd87d3ba754cc423b652a5edd6d2c # v4.2.0
docker/build-push-action@53b7df96c91f9c12dcc8a07bcb9ccacbed38856a # v7.3.0
docker/metadata-action@dc802804100637a589fabce1cb79ff13a1411302   # v6.2.0
```

The validation job must check tag/Cargo/changelog consistency, log in to GHCR, build one platform using `docker/build-push-action` with `tags: $IMAGE`, `outputs: type=image,name=$IMAGE,push-by-digest=true,name-canonical=true,push=true`, `sbom: true`, `provenance: mode=max`, and OCI build args. Pull `$IMAGE@$DIGEST`, tag it locally, install scanners, run the complete harness, require both report files, assert `.matches | length == 0`, require at least one SPDX package, run the image metadata verifier, and upload evidence plus a digest file named by platform slug.

- [ ] **Step 5: Add manifest, provenance, and GitHub Release publication**

Make a publication job depend on every matrix job. Download digest/evidence artifacts, log in, and create tags only from the two recorded digests:

```bash
docker buildx imagetools create \
  --tag "$IMAGE:0.2.0" \
  --tag "$IMAGE:0.2" \
  --tag "$IMAGE:latest" \
  "$IMAGE@$(cat digests/linux-amd64)" \
  "$IMAGE@$(cat digests/linux-arm64)"
```

Inspect the manifest JSON and require exactly `linux/amd64` and `linux/arm64`. Use `actions/attest-build-provenance` with `subject-name: $IMAGE`, the resolved manifest digest, and `push-to-registry: true`. Extract changelog notes, generate `image-release.txt` containing tags, manifest digest, both platform digests, and pull commands, then run:

```bash
gh release create "$TAG" --verify-tag --title "$TAG" --notes-file release-notes.md \
  LICENSE THIRD_PARTY_LICENSES.html image-release.txt release-evidence/**/*
```

If the release already exists, compare its target commit and attached `image-release.txt`; fail on any mismatch rather than replacing immutable evidence.

- [ ] **Step 6: Validate workflow structure and release guards**

Run:

```bash
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml")'
actionlint .github/workflows/release.yml
bash scripts/check-release-metadata.sh 0.2.0 v0.2.0
if rg -n 'uses: .*@v[0-9]' .github/workflows/release.yml; then exit 1; fi
git diff --check
```

Expected: YAML/actionlint/metadata/diff checks pass and the major-tag scan finds nothing.

- [ ] **Step 7: Commit Task 3**

```bash
git add .github/workflows/release.yml README.md scripts/install-release-scanners.sh \
  scripts/extract-release-notes.sh
git commit -m "ci: publish verified multi-platform releases"
```

---

### Task 4: Pre-publication history, dependency, license, and local quality gates

- [ ] **Task status:** Complete and reviewed

**Files:**
- Create: `.security/dependency-audit/release-v0.2.0.json`
- Create: `.security/dependency-audit/release-v0.2.0.md`
- Create when warnings require acceptance: `.security/risk-acceptance/v0.2.0-cargo-dependencies.md`
- Modify: `.security/dependency-audit/.gitignore`
- Modify when generated sidecars change: `.specs/containerized-service/sidecars/*`
- Test: full local gates and repository-history audit

**Interfaces:**
- Consumes: the final release-preparation commit, complete Git history, Cargo lockfile, and exact local release image.
- Produces: a redacted Gitleaks result, oversized-object review, license notice verification, full release-mode dependency audit, and merge-ready PR evidence.

- [ ] **Step 1: Scan all reachable history with pinned Gitleaks**

Download `gitleaks_8.30.1_darwin_arm64.tar.gz`, verify SHA-256
`b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5`, extract to a temporary directory, and run:

```bash
gitleaks git --redact --log-opts="--all" --report-format json \
  --report-path /tmp/amtrak-gitleaks-redacted.json
jq -e 'length == 0' /tmp/amtrak-gitleaks-redacted.json
```

Never print unredacted findings. Any finding stops the visibility change for targeted review.

- [ ] **Step 2: Audit history for oversized or private material**

Generate a sorted object-size inventory with `git rev-list --objects --all`,
`git cat-file --batch-check='%(objecttype) %(objectname) %(objectsize) %(rest)'`, and require manual review of every blob at least 10 MiB. Search commit paths for `.env`, private-key extensions, database dumps, credentials, archives, and validation outputs; inspect matches without printing content. Record zero unresolved findings in the PR body.

- [ ] **Step 3: Run local code, spec, container, and license gates**

```bash
cargo fmt --all -- --check
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
cargo test --locked --offline --all-targets --all-features
cargo doc --locked --offline --no-deps --all-features
bash -n scripts/*.sh
bash scripts/check-release-metadata.sh 0.2.0 v0.2.0
python3 /Users/soham/.agents/skills/spec-driven/scripts/spec-check.py \
  .specs/containerized-service --ready --emit-json .specs/containerized-service/sidecars
scripts/test-container.sh amtrak-gtfs-rt:release-test
git diff --check
```

- [ ] **Step 4: Run the fresh release dependency audit**

Run against the exact release-preparation revision:

```bash
python3 /Users/soham/.agents/skills/dependency-security-audit/scripts/dependency_security_audit.py \
  --root /tmp/amtrak-release-v020 --mode release --revision "$(git rev-parse HEAD)" \
  --output /tmp/amtrak-release-audit --format human
```

Require complete inventory and required sources. Copy `latest.json`/`latest.md` to the two tracked release evidence paths and update `.gitignore`. If the ten inherited no-fix warnings remain, document mitigations, exact warning identities, no KEV match, container exclusion/scan evidence, and the user's explicit acceptance before tagging; a blocked or unavailable result stops release.

- [ ] **Step 5: Commit release audit evidence**

```bash
git add .security/dependency-audit .security/risk-acceptance .specs/containerized-service/sidecars
git commit -m "docs: record v0.2.0 release audit"
```

---

### Task 5: Review, merge, and make the audited repository public

- [ ] **Task status:** Complete and reviewed

**Files:**
- No additional source files unless review repairs are required.
- External state: branch, pull request, repository visibility, branch protection.

**Interfaces:**
- Consumes: audited release branch and user authorization to make the repository public.
- Produces: merged `main`, public repository visibility, and an exact release commit ready for tagging.

- [ ] **Step 1: Push the branch and open the release-preparation PR**

Push `release-v0.2.0`, create a PR against `main`, and include version, licensing, exact local image evidence, history-audit result, dependency warnings/acceptance status, and the fact that publication happens only after merge.

- [ ] **Step 2: Wait for required PR checks and review the diff**

Require all GitHub checks to pass, verify the PR remains mergeable, inspect the complete base-to-head diff, and repair any P0/P1 or release-contract defect in additional commits.

- [ ] **Step 3: Merge without rewriting audited commits**

Merge the PR using a normal merge commit. Fetch `main`, record the merge SHA, and rerun the release metadata checker against the merged checkout.

- [ ] **Step 4: Reconfirm no secrets appeared and change repository visibility**

Run the redacted Gitleaks all-history scan once more on merged `main`. Then execute:

```bash
gh repo edit sohampatwardhan/Amtrak-GTFS-RT --visibility public --accept-visibility-change-consequences
gh repo view sohampatwardhan/Amtrak-GTFS-RT --json visibility --jq .visibility
```

Expected: `PUBLIC`. Verify Actions remain enabled and `main` has pull-request/CI branch protection appropriate for a public repository.

---

### Task 6: Final audit, tag, publish, expose the package, and verify anonymous installation

- [ ] **Task status:** Complete and reviewed

**Files:**
- External state: annotated Git tag `v0.2.0`, GHCR package, GitHub Release, workflow run.
- Local evidence: `/tmp/amtrak-v0.2.0-anonymous-verification`.

**Interfaces:**
- Consumes: public merged release commit and the tag-triggered release workflow.
- Produces: an exact-merge release audit, public multi-platform GHCR manifest, public GitHub Release, provenance/SBOM/CVE/license assets, and anonymous pull/startup proof.

- [ ] **Step 1: Audit the exact merged release commit**

Run a fresh `release` dependency audit with `--revision` set to the merged `main` SHA, save it under `/tmp/amtrak-v0.2.0-final-audit`, and require complete inventory, required-source availability, zero blocking findings, and the same reviewed warning fingerprint accepted during Task 4. A new finding, KEV match, block, unavailable source, or unaccepted warning stops tagging. Preserve `latest.json` and `latest.md` for upload as `release-v0.2.0-final-audit.{json,md}`.

- [ ] **Step 2: Create and push the annotated release tag**

At the exact merged `main` commit:

```bash
git tag -a v0.2.0 -m "Amtrak GTFS-RT v0.2.0"
git push origin v0.2.0
```

Verify `git rev-list -n1 v0.2.0` equals the intended release commit.

- [ ] **Step 3: Monitor the release workflow to completion**

Locate the tag-triggered `Release` workflow, watch it, and require both native platform validation jobs and publication job to succeed. Inspect logs for exact digest scan assertions, non-empty SBOM counts, metadata verification, and manifest creation.

- [ ] **Step 4: Make the linked GHCR package public**

After first publication, inspect package linkage and visibility. Change the container package to public through GitHub's package settings/API using the authenticated owner account, then verify the package reports `public` and links to `sohampatwardhan/Amtrak-GTFS-RT`.

- [ ] **Step 5: Attach the exact-merge audit and verify the GitHub Release contract**

Upload `release-v0.2.0-final-audit.json` and `.md` without replacing existing assets. Require `v0.2.0` to be non-draft/non-prerelease, target the tagged commit, contain the changelog notes, and expose `LICENSE`, `THIRD_PARTY_LICENSES.html`, `image-release.txt`, both platform SBOMs, both platform Grype reports, and the exact-merge audit. Require its recorded manifest digest to equal GHCR inspection.

- [ ] **Step 6: Verify anonymous multi-platform pulls**

Use a clean temporary `DOCKER_CONFIG` with no credentials. Pull `:0.2.0` and the recorded digest anonymously, inspect the manifest, and require exactly `linux/amd64` and `linux/arm64`. Confirm `:0.2`, `:latest`, and `:0.2.0` resolve to the same manifest digest.

- [ ] **Step 7: Verify anonymous startup using the README command**

On the available native platform, use a uniquely named volume/container and the documented host-network command with the immutable digest. Wait for Docker health, request `/livez` and `/v1/feed-set.json` from `127.0.0.1:8080`, inspect UID/labels/licenses, then remove only the temporary container and retain/remove its uniquely named test volume after recording the result.

- [ ] **Step 8: Record final release state**

Capture the repository URL, release URL, package URL, tag commit, manifest digest, two platform digests, anonymous pull result, health result, and remaining accepted Cargo warnings in the final handoff. Do not claim deployment occurred.

---

## Final Acceptance

- [ ] Repository history has zero unresolved secret/private-material findings and repository visibility is public.
- [ ] `Cargo.toml`, `Cargo.lock`, README, and CHANGELOG consistently describe `v0.2.0`.
- [ ] Project AGPL text and generated third-party license notices exist in source, image, and release assets.
- [ ] Exact `linux/amd64` and `linux/arm64` digests passed smoke, non-empty SBOM, metadata, and zero-match Grype gates.
- [ ] GHCR tags `0.2.0`, `0.2`, and `latest` share one immutable two-platform manifest digest.
- [ ] Provenance and per-platform SBOM/CVE evidence are published and refer to the released digests.
- [ ] GitHub Release `v0.2.0` targets the intended commit and contains all required assets.
- [ ] GHCR package is public, linked to the repository, and anonymously pullable by tag and digest.
- [ ] The README installation command starts the anonymously pulled image successfully.
- [ ] No production service deployment was performed.
