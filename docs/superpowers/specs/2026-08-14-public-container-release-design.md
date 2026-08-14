# Public Container Release Design

**Date:** 2026-08-14

**Target release:** `v0.2.0`

**Project license:** `AGPL-3.0-only`

## Objective

Make Amtrak GTFS-RT publicly discoverable and installable as a versioned container without
weakening the repository's existing fail-closed build, runtime, dependency, or evidence gates.
The release consists of a public GitHub repository, a public multi-platform image in GitHub
Container Registry (GHCR), and a GitHub Release that identifies the immutable image digest and
retains release evidence.

## Scope

The release will:

- prepare version `0.2.0` in `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md`;
- update `README.md` with public GHCR pull, run, upgrade, rollback, and digest-pinning examples;
- publish `linux/amd64` and `linux/arm64` under
  `ghcr.io/sohampatwardhan/amtrak-gtfs-rt`;
- publish semantic tags `0.2.0`, `0.2`, and `latest`, with `v0.2.0` remaining the Git tag;
- attach SBOM, vulnerability, license, and immutable-image metadata to the GitHub Release;
- add OCI source, revision, version, license, description, and documentation metadata;
- preserve the existing non-root scratch runtime, peer authorization, `/data` persistence,
  internal healthcheck, and pinned validator build;
- make the repository and package public only after local pre-publication checks pass; and
- verify anonymous installation from GHCR after publication.

This release does not deploy or expose a running service, publish to Docker Hub, add Kubernetes
manifests, or change the service's network authorization model.

## Release Architecture

### Repository preparation

Work occurs on a release branch based on the merged `main` commit. The preparation change bumps
the crate to `0.2.0`, closes the current changelog section as `0.2.0 — 2026-08-14`, creates a new
empty Unreleased section, adds the release workflow, and updates operator documentation. It is
merged through a pull request after normal CI passes.

Before repository visibility changes, a local history audit checks tracked files and all reachable
Git history for credentials, private keys, tokens, accidental private material, and oversized
artifacts. Findings block publication until removed or explicitly reviewed. Existing user-owned
working-tree changes are isolated from release work.

### Build and platform validation

The release workflow is triggered only by an exact stable tag matching `vMAJOR.MINOR.PATCH`. It
uses least-privilege job permissions and current, commit-pinned GitHub Actions. A matrix builds
`linux/amd64` and `linux/arm64` independently with Buildx/QEMU and pushes each image by canonical
digest rather than assigning public release tags immediately.

For each platform digest, the workflow:

1. pulls the exact pushed digest;
2. runs the existing bounded container smoke/recovery harness;
3. produces an SPDX SBOM containing a non-empty inventory;
4. runs Grype against that exact digest and requires zero matches;
5. checks the expected non-root user, healthcheck, architecture, license/source labels, and
   embedded project license; and
6. uploads digest, SBOM, vulnerability JSON/table, and license evidence as workflow artifacts.

No version or `latest` tag is created unless every platform succeeds.

### Manifest and release publication

After both platform jobs pass, a publication job combines the two canonical digests into one OCI
manifest list and applies `0.2.0`, `0.2`, and `latest`. The job records the resulting manifest-list
digest and generates provenance tied to that digest. The immutable deployment identity is:

```text
ghcr.io/sohampatwardhan/amtrak-gtfs-rt@sha256:…
```

The workflow then creates the `v0.2.0` GitHub Release from the matching changelog section and
attaches:

- `LICENSE`;
- a text file containing image name, semantic tags, per-platform digests, manifest digest, and
  pull commands;
- per-platform SPDX SBOMs;
- per-platform Grype JSON and human-readable reports; and
- a third-party license inventory derived from the resolved Rust and delivered image inventories.

The first GHCR publication may initially be private because repository visibility does not control
package visibility. After the package exists, its visibility is explicitly set to public and an
unauthenticated `docker pull` by semantic tag and digest is required to pass. The package is linked
to the source repository through `GITHUB_TOKEN` publication and OCI source metadata.

## Licensing

The project remains `AGPL-3.0-only`; no relicensing occurs. `Cargo.toml`, the repository `LICENSE`,
the GitHub Release, and OCI `org.opencontainers.image.licenses` metadata all use the same SPDX
identifier. The exact project license is copied into the final image at
`/licenses/AGPL-3.0-only.txt`, and `org.opencontainers.image.source` points recipients to the exact
public source repository and release revision.

Third-party components retain their own licenses. A generated third-party notice bundle containing
the resolved Rust dependency license texts is committed, freshness-checked against `Cargo.lock`,
copied into `/licenses`, and attached to the release. The minimized Java runtime's legal directory
and the validator JAR's embedded notices remain in their delivered artifacts. The release records
declared licenses separately instead of implying that every bundled component is authored under
AGPL. A missing, unknown, or incompatible delivered-component license blocks publication for
review.

## Installation Contract

The recommended Linux installation uses host networking so the existing loopback-only default
remains effective:

```bash
docker volume create amtrak-data
docker run -d --name amtrak-gtfs-rt --restart unless-stopped \
  --network host \
  -v amtrak-data:/data \
  ghcr.io/sohampatwardhan/amtrak-gtfs-rt:0.2.0
```

Production documentation also gives the immutable digest form. Bridge networking remains
supported only with an exact observed peer allowlist, as already documented. Upgrades recreate the
container over the retained `amtrak-data` volume; rollback selects the previous image digest
without migrating or rewriting stored generations.

## Failure Handling and Idempotency

- A failed platform build, smoke test, license check, SBOM, or vulnerability scan prevents manifest
  and GitHub Release creation.
- An unavailable scanner or incomplete inventory is a failure, never a clean result.
- A tag whose version differs from `Cargo.toml` or `CHANGELOG.md` is rejected before building.
- Rerunning the same tag may reuse immutable platform digests and update missing evidence. If a
  semantic tag already exists, the workflow compares its digest and fails rather than moving it to
  different bytes.
- If publication partially succeeds, untagged canonical digests may remain private in GHCR; no
  public semantic tag or GitHub Release is created until all gates pass.
- `latest` is updated only for a successful stable release and is never the recommended production
  pin.
- Repository or package visibility is not changed if the pre-publication history/license audit
  fails.

## Verification and Acceptance

The release is complete when:

- normal Rust formatting, Clippy, tests, documentation, and workflow syntax checks pass;
- the spec checker and a fresh release-mode dependency audit complete with all warnings explicitly
  reviewed and no blocking or unavailable result;
- both platform builds pass the full container harness and exact-digest zero-match scan;
- each SBOM has a non-empty inventory and expected architecture;
- the repository reports public visibility and GitHub recognizes its AGPL-3.0 license;
- the GHCR package reports public visibility and is linked to the repository;
- anonymous pulls by `0.2.0` and immutable digest succeed;
- the pulled image reports the expected two-platform manifest, OCI labels, embedded license,
  non-root user, healthcheck, and startup behavior; and
- the GitHub `v0.2.0` release is published at the same Git commit and lists the exact GHCR digest.

## Rollback

If a release defect is found, mark the GitHub Release as affected, stop recommending `latest`, and
direct operators to the last known-good immutable digest while preserving `/data`. Do not delete or
rewrite the affected tag or digest; release a corrected patch version so audit history remains
intact.
