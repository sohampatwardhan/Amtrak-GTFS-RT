# Task 3.1 Brief: Recoverable Static and Realtime Refresh

## Contract

Implement task 3.1 from [04_tasks.md](../04_tasks.md): fetch each static snapshot once, validate and parse those exact bytes, stage static changes without switching them independently, and promote a refresh only through task 2.1's durable coherent-generation commit.

## Requirement Criteria

R1.2, R3.4, R3.6, R4.1, R5.1–R5.3, R5.9, and R7.5–R7.8/R7.11.

## Owned Files

- [src/static_gtfs.rs](../../../src/static_gtfs.rs)
- [src/orchestrator.rs](../../../src/orchestrator.rs)

## Interfaces

Consume the pinned task-1.1 validator, task-1.2 builder/validator, task-2.1 publisher/store, source chain, intervals, and static URL. Produce exact-byte static fetch/staging, the MobilityData validator adapter, pending-static promotion through a committed generation, recoverable polling, and credential-free structured outcomes.

## Verification

- `cargo test static_gtfs -- --nocapture`
- `cargo test orchestrator -- --nocapture`
- Prove one fetch supplies retained/parser/validator bytes and versionless snapshots remain distinct.
- Prove standards errors/tool failures and every refresh-stage failure preserve last-good, pending promotion, and retry behavior.
- Review public static-staging and telemetry contracts.
