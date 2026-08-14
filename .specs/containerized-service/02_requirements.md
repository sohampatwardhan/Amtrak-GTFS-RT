# Requirements: Containerized Service

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Context

These requirements define the observable contract for building and running the existing Amtrak GTFS-RT API as a container. A **generation volume** is durable storage supplied by the operator for the API's immutable feed generations and current-generation marker. A **pinned validator** is the reproducible hardened MobilityData GTFS validator 8.0.1 CLI artifact built from the repository-pinned source and dependency override policy.

Containerization does not change the API routes, feed semantics, freshness threshold, direct-peer authorization policy, or release-deployment decision.

### Requirement 1: Reproducible image build

**User Story:** As a service operator, I want a repeatable container build, so that the same reviewed source and lockfile produce an executable service image without host build artifacts.

#### Acceptance Criteria

1. **R1.1** WHEN an operator builds from a clean repository checkout, THE Container_Build SHALL produce an image containing an executable release build of the service.
2. **R1.2** THE Container_Build SHALL resolve Rust dependencies according to the committed lockfile.
3. **R1.3** IF the committed lockfile cannot be honored, THEN THE Container_Build SHALL terminate with a non-zero status.
4. **R1.4** THE Container_Build SHALL complete without reading host build outputs, generated feeds, local validator caches, or version-control metadata.
5. **R1.5** IF service compilation fails, THEN THE Container_Build SHALL terminate without producing a runnable final image.

### Requirement 2: Pinned validation runtime

**User Story:** As a service operator, I want the image to include the approved static-feed validator runtime, so that startup and static refreshes retain the existing standards gate without manual provisioning.

#### Acceptance Criteria

1. **R2.1** THE Runtime_Image SHALL contain the pinned validator artifact whose SHA-256 equals `24ca7e890ca15bfbb36fa889fcb16200f7276995b7e6ec75551a8b7175e818d7`.
2. **R2.2** THE Runtime_Image SHALL provide a Java runtime whose reported major version is at least 17.
3. **R2.3** THE Service_Process SHALL verify the validator SHA-256 internally without requiring a shell or checksum utility in the Runtime_Image.
4. **R2.4** IF the validator source archive or rebuilt artifact does not match its pinned digest, THEN THE Container_Build SHALL terminate with a non-zero status.
5. **R2.5** WHEN the container starts with its packaged defaults, THE Service_Process SHALL validate the packaged validator before opening its HTTP listener.

### Requirement 3: Least-privilege runtime

**User Story:** As a security-conscious operator, I want the API to run without root privileges or build tooling, so that a service compromise has a smaller host and image impact.

#### Acceptance Criteria

1. **R3.1** WHEN the container starts normally, THE Service_Process SHALL run with a non-zero user identifier.
2. **R3.2** THE Runtime_Image SHALL exclude the Rust compiler.
3. **R3.3** THE Runtime_Image SHALL exclude Cargo.
4. **R3.4** THE Runtime_Image SHALL exclude the protobuf compiler.
5. **R3.5** THE Runtime_Image SHALL contain trusted certificate authorities needed for HTTPS access to configured Amtrak sources.
6. **R3.6** THE Runtime_Image SHALL exclude repository-local secrets and untracked files from the final image.
7. **R3.7** THE Runtime_Image SHALL exclude shells, package managers, download clients, and package databases.

### Requirement 4: Persistent generation storage

**User Story:** As a service operator, I want feed generations to survive container replacement, so that routine restarts preserve last-good service and rollback evidence.

#### Acceptance Criteria

1. **R4.1** WHEN a writable generation volume is mounted at the documented location, THE Service_Process SHALL publish its output generations into that volume.
2. **R4.2** WHEN a container is recreated with a generation volume containing a valid current generation, THE Service_Process SHALL recover that generation before requiring a successful upstream refresh.
3. **R4.3** IF the generation volume is not writable by the runtime identity, THEN THE Service_Process SHALL terminate with a non-zero status.
4. **R4.4** IF the generation volume contains an incomplete or corrupt newest candidate plus an older valid generation, THEN THE Service_Process SHALL expose only the valid generation.

### Requirement 5: Explicit network authorization

**User Story:** As a service operator, I want container networking to preserve direct-peer authorization, so that packaging does not silently make controlled feeds publicly accessible.

#### Acceptance Criteria

1. **R5.1** THE Runtime_Image SHALL NOT configure a wildcard peer allowlist.
2. **R5.2** IF the service binds to a non-loopback address without an explicit peer allowlist, THEN THE Service_Process SHALL terminate before serving requests.
3. **R5.3** WHEN an admitted direct peer requests a controlled endpoint, THE Service_Process SHALL apply the existing direct socket-peer authorization policy.
4. **R5.4** WHEN a request supplies forwarding identity headers, THE Service_Process SHALL ignore those headers when making its authorization decision.
5. **R5.5** IF a direct peer is not admitted, THEN THE Service_Process SHALL return `403` for readiness, manifest, and generation-artifact requests.
6. **R5.6** THE Container_Documentation SHALL state that bridged operation requires an explicit exact peer allowlist appropriate to the selected container network.

### Requirement 6: Container health and feed smoke verification

**User Story:** As a service operator, I want standard container and HTTP checks, so that I can distinguish a running process from a fresh usable feed generation.

#### Acceptance Criteria

1. **R6.1** WHILE the service process is running, THE Container_Healthcheck SHALL test the public `/livez` endpoint through the container's loopback interface.
2. **R6.2** IF the `/livez` check repeatedly fails for the configured healthcheck interval, THEN THE Container_Runtime SHALL report the container as unhealthy.
3. **R6.3** WHEN the service has committed a generation less than 300 seconds old, THE Service_Process SHALL return `200` from `/readyz` to an admitted peer.
4. **R6.4** WHEN the service has committed a current generation, THE Service_Process SHALL return a manifest whose four artifact URLs identify that same immutable generation.
5. **R6.5** WHEN an operator fetches all four manifest-selected artifacts, THE Service_Process SHALL return a static ZIP and three independently decodable GTFS-Realtime protobuf feeds.
6. **R6.6** IF no generation has been committed, THEN THE Service_Process SHALL return `503` from `/readyz` to an admitted peer.

### Requirement 7: Operator usability and rollback

**User Story:** As a service operator, I want documented build, run, verification, and rollback procedures, so that I can operate the container without reconstructing hidden assumptions.

#### Acceptance Criteria

1. **R7.1** THE Container_Documentation SHALL provide a command that builds the image from the repository root.
2. **R7.2** THE Container_Documentation SHALL provide a local run command with a persistent generation volume and explicit network authorization values.
3. **R7.3** THE Container_Documentation SHALL provide commands that verify container health, readiness, the feed-set manifest, and manifest-selected artifact retrieval.
4. **R7.4** THE Container_Documentation SHALL identify every required environment value that differs from the non-container defaults.
5. **R7.5** THE Container_Documentation SHALL state that deleting the container does not delete a separately managed generation volume.
6. **R7.6** WHEN an operator replaces a candidate container with the prior image while retaining the generation volume, THE Prior_Service_Image SHALL recover the last valid compatible generation.

## Risk Classification

- **Security and authorization:** high relevance because container port publication can accidentally broaden reachability; Requirements 3 and 5 fail closed and retain the direct-peer policy.
- **Data durability and rollback:** high relevance because immutable generations and the current marker must survive container replacement; Requirements 4 and 7 cover recovery.
- **Dependency integrity:** high relevance because the validator is executable supply-chain input; Requirements 1 and 2 pin dependency resolution and artifact bytes.
- **Observability:** medium relevance; Requirement 6 keeps liveness distinct from readiness and feed usability.
- **Performance:** no new response-time target is introduced. Image size and build-cache efficiency are design measurements, not user-visible correctness requirements.
- **Privacy and accessibility:** no additional personal data or human user interface is introduced.

## Assumptions and Non-Goals

- A scratch final stage assembled from the pinned Corretto Alpine/musl runtime closure is approved by measured compatibility and scan evidence.
- The operator supplies a Docker-capable host and controls the container network and persistent volume.
- Registry publication, image signing, Docker Compose, Kubernetes resources, reverse-proxy trust, public hosting, and deployment are outside this requirements contract.
- The source-integration decision for the API does not override its existing deployment block.

## Approval

Status: **Approved on 2026-08-14, including the security remediation revision**
