# Discovery: Containerized Service

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Problem and Outcome

The Rust API currently runs directly on a host with a Rust-built binary, Java 17 or newer, the pinned MobilityData GTFS validator JAR, `shasum`, and a writable generation directory. Operators need a repeatable container image that packages those runtime dependencies and starts the same service without weakening validation, persistence, or access controls.

The outcome is a production-oriented Dockerfile and minimal build context that can build the service reproducibly, run it as a non-root user, persist immutable generations through a mounted directory, and support a local health check. The existing API and feed behavior remain unchanged.

## Users and Current Workaround

The primary user is the internal service operator. Today the operator must install Rust build prerequisites for compilation and separately provision Java, `shasum`, and the exact validator JAR described in the [project README](../../README.md). The local validation workflow can fall back to Docker, but the API itself does not yet ship as a container image.

## Scope and Non-Goals

In scope:

- a multi-stage Dockerfile that builds the locked Rust application and copies only runtime necessities into the final image;
- the pinned MobilityData GTFS validator 8.0.1 JAR, verified by its approved SHA-256 during the image build;
- Java 17 or newer, `shasum`, TLS root certificates, and a non-root runtime identity;
- a writable `/data` mount for `AMTRAK_OUTPUT_DIR`;
- container defaults and documentation that preserve the service's direct-peer authorization contract;
- a [.dockerignore](../../.dockerignore) that excludes build output, local generations, caches, Git metadata, and secrets; and
- build and runtime smoke verification through `/livez`, `/readyz`, and `/v1/feed-set.json`.

Out of scope:

- publishing an image to a registry;
- Docker Compose, Kubernetes, or another orchestrator;
- changing the direct-peer authorization model to trust proxy headers;
- exposing the service anonymously on the public internet;
- changing feed generation, polling, validation, retention, or HTTP response semantics; and
- resolving the existing release dependency-audit deployment block.

## Constraints and Success Measures

- The build requires the current Rust toolchain and `protoc`; the runtime does not require Cargo or the protobuf compiler.
- Startup rejects a missing or altered validator JAR and requires the `shasum` executable and Java 17 or newer.
- The final process must not run as root and must be able to atomically create and sync generation directories under `/data`.
- The service authorizes the direct socket peer and ignores forwarded identity headers. Docker bridge addresses vary, so the image must not hard-code a permissive peer allowlist. Operators must explicitly provide `AMTRAK_ALLOWED_PEER_IPS` when binding to `0.0.0.0:8080`.
- A stopped and recreated container using the same `/data` volume must recover its retained last-good generation.
- Success means the image builds from a clean context, starts with an explicit safe network policy, becomes live, publishes a fresh coherent generation, passes readiness and manifest smoke checks, and recovers that generation after restart.

## Approaches Considered

| Approach | Benefits | Costs / risks | Reversibility | Decision |
|---|---|---|---|---|
| Multi-stage Debian image with the validator embedded and digest-verified | Matches the existing glibc binary and Java subprocess contract; one deployable artifact; straightforward non-root filesystem setup; no runtime download | Runtime image includes a JRE and is larger than distroless; base image updates still require rebuilding and review | High; image stages and base pins can be changed independently later | **Recommended** |
| Alpine builder and Alpine Java runtime with the validator embedded | Smaller base layers and package set; musl is well suited to a tightly controlled stack; Docker publishes an official Alpine-oriented Rust multi-stage pattern | The application graph includes a `native-tls`/OpenSSL path, so the build and runtime must carry compatible musl-native libraries; Java, the validator JAR, and certificates reduce the percentage size saving; requires explicit amd64 and arm64 smoke evidence | High if builder and runtime move together | Viable optimization after measuring the complete image, not rejected |
| Minimal or distroless binary image with validator handled by a sidecar | Smaller API image and potentially lower application-image attack surface | The current process directly invokes `java`, `shasum`, and a local JAR, so this requires an application and deployment-contract redesign rather than only a Dockerfile | Medium; introduces another service boundary and operational dependency | Reject for this increment |
| Generic runtime image with the validator JAR mounted by the operator | Avoids downloading the JAR during image build and allows operator-controlled provisioning | Easy to mount the wrong bytes or omit the file; more setup; image is not self-contained; startup failures shift to deployment time | High | Defer as an optional advanced override, not the default |

Docker's current official guidance (`/docker/docs`, checked 2026-08-14) supports multi-stage Rust builds, Cargo cache mounts, a minimal runtime stage, and an unprivileged runtime user. It describes glibc-based images as the broad-compatibility choice for Java and other runtimes with native dependencies, and musl-based Alpine images as appropriate when footprint is prioritized for a tightly controlled stack. Those practices inform the recommendation without making Alpine ineligible.

## Chosen Direction

Use a BuildKit-enabled multi-stage Dockerfile. A Rust builder stage installs `protoc` and Git, compiles the locked release binary with Cargo cache mounts, and exports only the executable. A separate download stage retrieves the official validator 8.0.1 CLI JAR and verifies the exact repository-pinned SHA-256. A slim Debian runtime stage installs Java 17, `shasum`, and CA certificates, creates a fixed unprivileged service user, copies the binary and validated JAR with controlled ownership, and runs with `/data` as the persistent output directory.

The image documents port 8080 but does not silently broaden authorization. A usable bridged invocation supplies both `AMTRAK_BIND_ADDR=0.0.0.0:8080` and an exact `AMTRAK_ALLOWED_PEER_IPS` value appropriate to its network. The Docker run example will show that requirement explicitly.

## Architecture and Flow Outline

Build time compiles the service and verifies the external validator artifact. Runtime contains only the service binary, required validator runtime, trusted certificates, and writable persisted data. The Docker network remains outside the application's trust decision: the application continues to authorize the transport peer it actually sees.

## Failure and Verification Strategy

The image build fails on Cargo lockfile drift, compilation failure, validator download failure, or digest mismatch. Container startup fails on an invalid bind/allowlist combination, missing runtime tools, unreadable validator, or unwritable data directory. No failure may cause the image to fall back to an unvalidated static feed.

Verification will build the image without host build artifacts, inspect its configured non-root user, run it with an isolated named volume and explicit peer policy, wait for `/livez` and `/readyz`, fetch the manifest and all four pinned artifacts, stop and recreate the container with the same volume, and confirm last-good recovery. Existing Rust tests and formatting checks remain part of the implementation gate.

## Open Decisions

- Exact immutable digests for the Rust builder and Debian runtime base images belong in design because they depend on the selected build platform and update policy.
- Debian slim is the approved default base family. Alpine remains a future optimization only after a complete image build proves its actual size and native-library behavior.
- Multi-architecture publication is not required, but the Dockerfile should avoid architecture-specific downloads where practical.

## Approval

Status: **Approved on 2026-08-14**
