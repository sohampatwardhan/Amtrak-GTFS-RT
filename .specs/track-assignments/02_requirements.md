# Requirements: Real-Time Track Assignments

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Introduction

These requirements cover publishing live Amtrak platform assignments at stations shared with NJ
Transit and the CTrail Hartford Line. Terms used below:

- **Track assignment:** one train number, one Amtrak station stop, one track label, and the time the
  source reported it.
- **Covered station:** an Amtrak stop that has a configured list of tracks.
- **Platform stop:** a `stops.txt` row for one track at a covered station.
- **Augmented static feed:** Amtrak's static GTFS with the platform stops, parent stations, and parent
  links for covered stations added, and nothing else changed.
- **Tracks enabled:** the operator has turned the feature on. When it is off, the service behaves as
  it did before this feature.

Assumption: NJ Transit data is obtained with the operator's registered RailData credentials, which
issue tokens valid for 24 hours and allow 10 token requests per day.

### Requirement 1: Platform stops in the static feed

**User Story:** As a transit-app consumer, I want each covered station's tracks published as stops,
so that a platform assignment resolves to a stop defined in the static feed.

#### Acceptance Criteria

1. **R1.1** WHERE tracks are enabled, THE Static_Augmenter SHALL add one platform stop for each configured track of each covered station to the published static feed.
2. **R1.2** WHERE tracks are enabled, THE Static_Augmenter SHALL add one parent station for each covered station to the published static feed.
3. **R1.3** WHERE tracks are enabled, THE Static_Augmenter SHALL set the parent of each covered station's original Amtrak stop to that station's new parent station.
4. **R1.4** WHERE tracks are enabled, THE Static_Augmenter SHALL set the parent of each platform stop to the parent station of its covered station.
5. **R1.5** WHERE tracks are enabled, THE Static_Augmenter SHALL set each platform stop's `platform_code` to its track label.
6. **R1.6** WHERE tracks are enabled, THE Static_Augmenter SHALL keep every stop id, trip, stop time, and non-stop file of the upstream feed unchanged.
7. **R1.7** WHERE tracks are disabled, THE Static_Augmenter SHALL publish the upstream static feed bytes unchanged.
8. **R1.8** IF a covered station's stop id is absent from the upstream feed, THEN THE Static_Augmenter SHALL omit that station's parent station and platform stops.
9. **R1.9** IF augmentation of an upstream snapshot fails, THEN THE Static_Augmenter SHALL publish the upstream static feed bytes unchanged for that snapshot.

### Requirement 2: Static integrity and versioning

**User Story:** As an operator, I want the augmented feed validated and versioned predictably, so that
consumers reload static data only when it actually changes.

#### Acceptance Criteria

1. **R2.1** THE Static_Pipeline SHALL submit the exact bytes it will publish to the standards validator.
2. **R2.2** IF the standards validator reports an `ERROR` notice for the augmented static feed, THEN THE Static_Pipeline SHALL publish the upstream static feed bytes unchanged for that snapshot.
3. **R2.3** WHEN the same upstream static feed is augmented twice with the same track configuration, THE Static_Augmenter SHALL produce byte-identical output.
4. **R2.4** WHERE tracks are enabled, THE Static_Pipeline SHALL report a static version that differs from the upstream version when the track configuration differs.
5. **R2.5** WHEN neither the upstream static feed nor the track configuration changes, THE Static_Pipeline SHALL report the same static version.

### Requirement 3: Spec-conformant assignments in trip updates

**User Story:** As a transit-app consumer, I want trip updates to name the assigned platform stop, so
that my app can show the rider's track.

#### Acceptance Criteria

1. **R3.1** WHEN a trip update stop time matches a fresh track assignment for a configured track, THE Track_Stamper SHALL set its `assigned_stop_id` to that track's platform stop id.
2. **R3.2** WHEN the Track_Stamper sets an `assigned_stop_id`, THE Track_Stamper SHALL populate that stop time's `stop_sequence` from the static trip.
3. **R3.3** WHEN the Track_Stamper sets an `assigned_stop_id`, THE Track_Stamper SHALL clear that stop time's `stop_id`.
4. **R3.4** IF a scheduled station occurs more than once in the static trip, THEN THE Track_Stamper SHALL leave that stop time unassigned.
5. **R3.5** IF a stop time is `SKIPPED`, THEN THE Track_Stamper SHALL leave that stop time unassigned.
6. **R3.6** IF a reported track label is not configured for its station, THEN THE Track_Stamper SHALL leave the stop time unassigned.
7. **R3.7** IF a reported track label is not configured for its station, THEN THE Track_Stamper SHALL log the station and label without logging credentials.
8. **R3.8** THE Orchestrator SHALL reject an `assigned_stop_id` that is not a stop in the active static feed.
9. **R3.9** THE Orchestrator SHALL reject an `assigned_stop_id` whose parent station differs from the parent station of the scheduled stop.
10. **R3.10** WHERE tracks are disabled, THE Track_Stamper SHALL leave every trip update unchanged.
11. **R3.11** IF a stop time has no arrival or departure time between one hour before and twelve hours after the generation time, THEN THE Track_Stamper SHALL leave that stop time unassigned.

### Requirement 4: Authorized NJ Transit access

**User Story:** As an operator, I want NJ Transit data read only through my registered RailData
account, so that the service uses NJT's sanctioned access.

#### Acceptance Criteria

1. **R4.1** THE RailData_Client SHALL request NJ Transit schedules only with a token issued to the configured RailData credentials.
2. **R4.2** THE Service SHALL contain no DepartureVision session bootstrap and no key copied from NJ Transit's web client.
3. **R4.3** WHILE a stored RailData token is younger than its validity period, THE RailData_Client SHALL reuse it instead of requesting a new token.
4. **R4.4** WHEN the service restarts with a stored unexpired RailData token, THE RailData_Client SHALL reuse the stored token.
5. **R4.5** THE RailData_Client SHALL request at most 10 tokens in any rolling 24-hour period.
6. **R4.6** IF RailData rejects a token, THEN THE RailData_Client SHALL request one replacement token before the next schedule request.
7. **R4.7** IF RailData credentials are not configured, THEN THE Board_Refresher SHALL skip NJ Transit stations and log that they are unconfigured.
8. **R4.8** THE Service SHALL never write RailData credentials to logs, responses, or published files.

### Requirement 5: Fresh assignments only

**User Story:** As a rider, I want tracks that reflect current board data, so that I am never sent to
a platform from an earlier train or day.

#### Acceptance Criteria

1. **R5.1** THE Track_Stamper SHALL ignore a track assignment older than the configured maximum age.
2. **R5.2** IF a board refresh fails, THEN THE Board_Refresher SHALL keep each earlier assignment only until it exceeds the configured maximum age.
3. **R5.3** WHEN a board refresh succeeds for a station, THE Board_Refresher SHALL replace that station's earlier assignments with the new ones.

### Requirement 6: Feed timing independent of boards

**User Story:** As an operator, I want board fetching separated from feed generation, so that a slow
or failed board cannot delay the feed.

#### Acceptance Criteria

1. **R6.1** THE Board_Refresher SHALL fetch boards on its own schedule outside feed generation.
2. **R6.2** WHEN a generation runs, THE Track_Stamper SHALL use the latest available assignments without waiting for a board request.
3. **R6.3** IF every board request times out, THEN THE Orchestrator SHALL publish the generation within the time it takes when tracks are disabled plus one second.
4. **R6.4** THE Board_Refresher SHALL bound each board request by a configured timeout.

### Requirement 7: Board sources

**User Story:** As an operator, I want the NJ Transit stations and New Haven Union read by default,
so that tracks appear where these sources publish them.

#### Acceptance Criteria

1. **R7.1** THE Board_Refresher SHALL read Amtrak track assignments for New York Penn, Newark Penn, Newark Airport, Metropark, New Brunswick, Princeton Junction, and Trenton from RailData by default.
2. **R7.2** THE Board_Refresher SHALL read Amtrak track assignments for New Haven Union from the Hartford Line board by default.
3. **R7.3** WHERE the operator configures an additional station mapping and track list, THE Board_Refresher SHALL read that station without a code change.
4. **R7.4** IF a board reports a track label that is empty or not a platform token, THEN THE Board_Refresher SHALL discard that row.

### Requirement 8: Operator tools and validation

**User Story:** As a maintainer, I want the status tool and feed validation to understand platform
assignments, so that regressions are caught before release.

#### Acceptance Criteria

1. **R8.1** WHEN a displayed stop time has an `assigned_stop_id`, THE Status_Tool SHALL show the platform stop's `platform_code` as the track.
2. **R8.2** WHEN a displayed stop time omits `stop_id`, THE Status_Tool SHALL identify its station from `stop_sequence` and the static trip.
3. **R8.3** WHEN the scheduled feed validation runs, THE Feed_Validation SHALL validate a generation produced with tracks enabled using the static and GTFS-Realtime validators.
4. **R8.4** IF the tracks-enabled generation produces a validator `ERROR` code outside the recorded allowlist, THEN THE Feed_Validation SHALL fail.
