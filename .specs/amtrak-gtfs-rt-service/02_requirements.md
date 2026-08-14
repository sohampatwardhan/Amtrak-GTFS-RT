# Requirements: Amtrak GTFS-RT Service

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Introduction

These requirements define the observable contract for an internally operated service that converts Amtrak schedule and train-status data into a coherent GTFS static feed and GTFS-Realtime TripUpdates, VehiclePositions, and Alerts. An **authorized consumer** is a caller admitted by the operator's internal access policy. A **feed generation** is one mutually consistent static/realtime publication state. An **approved validator exception** is a documented validator error code attributed to upstream data, with an owner and review date.

> [!IMPORTANT]
> Approval gate: approve these requirements before work begins on [03_design.md](03_design.md).

## Requirements

### Requirement 1: Standards-compatible feed products

**User Story:** As an internal transit-data consumer, I want conventional GTFS and GTFS-Realtime feed products, so that I can use Amtrak status without an Amtrak-specific integration.

#### Acceptance Criteria

1. **R1.1** WHEN valid static and realtime source data are available, THE Feed_Service SHALL publish one coherent feed generation containing static GTFS, TripUpdates, VehiclePositions, and Alerts.
2. **R1.2** THE Static_Feed SHALL be parseable as GTFS by the selected standards validator.
3. **R1.3** THE Trip_Updates_Feed SHALL be decodable as a GTFS-Realtime FeedMessage.
4. **R1.4** THE Vehicle_Positions_Feed SHALL be decodable as a GTFS-Realtime FeedMessage.
5. **R1.5** THE Alerts_Feed SHALL be decodable as a GTFS-Realtime FeedMessage.
6. **R1.6** THE Trip_Updates_Feed SHALL contain only entities carrying trip-update payloads.
7. **R1.7** THE Vehicle_Positions_Feed SHALL contain only entities carrying vehicle-position payloads.
8. **R1.8** THE Alerts_Feed SHALL contain only entities carrying alert payloads.

### Requirement 2: Train status semantics

**User Story:** As an internal transit-data consumer, I want live Amtrak observations represented with GTFS-Realtime semantics, so that trip status and train location remain useful to downstream transit applications.

#### Acceptance Criteria

1. **R2.1** WHEN a live train is matched to a scheduled trip, THE Trip_Updates_Feed SHALL identify that scheduled trip.
2. **R2.2** WHEN a matched train has a source-provided stop-time prediction, THE Trip_Updates_Feed SHALL publish the corresponding stop-time update.
3. **R2.3** WHEN a matched train has a valid geographic observation, THE Vehicle_Positions_Feed SHALL publish that observation's coordinates.
4. **R2.4** WHEN a vehicle position is published for a matched train, THE Vehicle_Positions_Feed SHALL identify the matching scheduled trip.
5. **R2.5** WHEN an active source alert applies to an Amtrak route, trip, or stop, THE Alerts_Feed SHALL publish an informed entity for that target.
6. **R2.6** WHEN an active source alert is published, THE Alerts_Feed SHALL include non-empty human-readable alert text.
7. **R2.7** IF a live train cannot be matched to the active static feed, THEN THE Feed_Service SHALL omit any realtime entity that would claim a nonexistent scheduled trip.

### Requirement 3: Static and realtime consistency

**User Story:** As an internal transit-data consumer, I want realtime identifiers bound to the served static schedule, so that every published reference has a consistent interpretation.

#### Acceptance Criteria

1. **R3.1** THE Feed_Service SHALL ensure every published realtime trip identifier resolves in the served static feed.
2. **R3.2** THE Feed_Service SHALL ensure every published realtime stop identifier resolves in the served static feed.
3. **R3.3** THE Feed_Service SHALL identify the served static feed version in each realtime feed header.
4. **R3.4** WHEN a replacement static feed is accepted, THE Feed_Service SHALL withhold it until a realtime generation has been produced against that replacement.
5. **R3.5** WHEN a replacement static/realtime generation becomes current, THE Feed_Service SHALL make the generation observable as one consistent publication state.
6. **R3.6** IF a static refresh fails validation or loading, THEN THE Feed_Service SHALL retain the preceding coherent feed generation.

### Requirement 4: Freshness and health semantics

**User Story:** As an operator, I want readiness to reflect realtime feed age, so that internal consumers are not told stale train status is healthy.

#### Acceptance Criteria

1. **R4.1** WHEN a realtime refresh succeeds, THE Feed_Service SHALL record that refresh as the latest successful realtime generation.
2. **R4.2** WHILE the latest successful realtime generation is less than 300 seconds old, THE Readiness_Status SHALL report ready.
3. **R4.3** IF the latest successful realtime generation reaches 300 seconds of age, THEN THE Readiness_Status SHALL report not ready.
4. **R4.4** WHEN a successful realtime refresh follows a not-ready freshness state, THE Readiness_Status SHALL report ready.
5. **R4.5** THE Readiness_Status SHALL expose the age of the latest successful realtime generation.
6. **R4.6** THE Liveness_Status SHALL report whether the service process can answer health checks independently of feed readiness.
7. **R4.7** THE Realtime_Feed_Headers SHALL expose generation timestamps that allow consumers to detect last-good data age.

### Requirement 5: Last-good and failure behavior

**User Story:** As an operator, I want failed refreshes isolated from current output, so that transient upstream or processing failures do not corrupt the feed set.

#### Acceptance Criteria

1. **R5.1** IF a realtime source request fails, THEN THE Feed_Service SHALL retain the current feed generation unchanged.
2. **R5.2** IF a realtime source returns an empty candidate generation, THEN THE Feed_Service SHALL retain the current feed generation unchanged.
3. **R5.3** IF conversion or validation of a candidate generation fails, THEN THE Feed_Service SHALL retain the current feed generation unchanged.
4. **R5.4** IF publication of a candidate generation fails, THEN THE Feed_Service SHALL retain the current feed generation unchanged.
5. **R5.5** THE Feed_Service SHALL prevent consumers from observing a partially written feed artifact.
6. **R5.6** THE Feed_Service SHALL prevent consumers from observing realtime feed types from different refresh generations.
7. **R5.7** IF no successful generation has ever been published, THEN THE Feed_Endpoints SHALL report that feed content is unavailable.
8. **R5.8** IF no successful generation has ever been published, THEN THE Readiness_Status SHALL report not ready.
9. **R5.9** WHEN a recoverable refresh failure occurs, THE Feed_Service SHALL attempt later scheduled refreshes without operator intervention.

### Requirement 6: Internal access boundary

**User Story:** As an operator, I want feed access limited to controlled consumers, so that the service does not become an unplanned public API with uncontrolled demand.

#### Acceptance Criteria

1. **R6.1** WHEN an authorized internal consumer requests an available feed, THE Access_Boundary SHALL return that feed.
2. **R6.2** IF a caller is not authorized by the configured internal access policy, THEN THE Access_Boundary SHALL deny access to feed content.
3. **R6.3** IF the service starts without explicit network-exposure configuration, THEN THE Access_Boundary SHALL accept feed requests only from the local host.
4. **R6.4** IF a non-local listening interface is configured without a corresponding access policy, THEN THE Feed_Service SHALL refuse to become ready.
5. **R6.5** WHEN access is denied, THE Access_Boundary SHALL emit an audit event without recording caller credentials.

### Requirement 7: Operator configuration and observability

**User Story:** As an operator, I want configurable inputs and actionable runtime evidence, so that I can operate the service safely without changing its source code.

#### Acceptance Criteria

1. **R7.1** THE Operator_Configuration SHALL allow the static-data source to be selected without a source-code change.
2. **R7.2** THE Operator_Configuration SHALL allow the realtime polling interval to be selected without a source-code change.
3. **R7.3** THE Operator_Configuration SHALL allow the publication location to be selected without a source-code change.
4. **R7.4** THE Operator_Configuration SHALL allow the listening interface to be selected without a source-code change.
5. **R7.5** WHEN a refresh attempt completes, THE Operational_Telemetry SHALL record its success or failure outcome.
6. **R7.6** WHEN a refresh succeeds, THE Operational_Telemetry SHALL record the selected source.
7. **R7.7** WHEN a refresh succeeds, THE Operational_Telemetry SHALL record each published feed's entity count.
8. **R7.8** WHEN a refresh fails, THE Operational_Telemetry SHALL identify the failed processing stage.
9. **R7.9** IF operator configuration is invalid, THEN THE Feed_Service SHALL reject startup with an actionable configuration error.
10. **R7.10** IF a required long-running service activity terminates unexpectedly, THEN THE Feed_Service SHALL terminate with a failure status.
11. **R7.11** THE Operational_Telemetry SHALL exclude configured credentials and access secrets.

### Requirement 8: Verification and release gate

**User Story:** As a maintainer, I want objective release evidence, so that an internal deployment does not introduce malformed or newly regressed feeds.

#### Acceptance Criteria

1. **R8.1** WHEN a release candidate is evaluated, THE Release_Gate SHALL validate the candidate static feed with the selected GTFS validator.
2. **R8.2** WHEN a release candidate is evaluated, THE Release_Gate SHALL validate each realtime feed type with the selected GTFS-Realtime validator.
3. **R8.3** IF the static feed has any ERROR finding, THEN THE Release_Gate SHALL reject the release candidate.
4. **R8.4** IF a realtime feed has an ERROR finding without an approved validator exception, THEN THE Release_Gate SHALL reject the release candidate.
5. **R8.5** THE Validator_Exception_Record SHALL identify the accepted error code.
6. **R8.6** THE Validator_Exception_Record SHALL identify the upstream cause.
7. **R8.7** THE Validator_Exception_Record SHALL identify an owner.
8. **R8.8** THE Validator_Exception_Record SHALL identify a review date.
9. **R8.9** WHEN a release candidate is evaluated, THE Release_Gate SHALL verify that an independent GTFS-Realtime consumer can decode each realtime feed.
10. **R8.10** IF any required deterministic verification fails, THEN THE Release_Gate SHALL reject the release candidate.

## Risk Classification

- **Security and authorization:** applicable because feeds are intentionally internal and uncontrolled public access is out of scope; Requirements 6.1–6.5 define the observable boundary.
- **Privacy:** limited applicability because the feeds describe public train operations rather than individual passengers; credentials and access secrets remain protected by Requirements 6.5 and 7.11.
- **Performance and capacity:** bounded to controlled internal demand; public-scale capacity is explicitly outside this increment.
- **Observability:** applicable to freshness, source selection, failure localization, and safe recovery under Requirements 4, 5, and 7.
- **Migration and rollback:** applicable because static/realtime generations must switch coherently and retain a last-good rollback state under Requirements 3 and 5.
- **Accessibility:** no human-facing user interface is included; machine-readable interoperability is covered by Requirements 1, 2, and 8.

## Approval

Status: **Approved on 2026-08-11**
