# Task 1.2 Brief: Coherent Candidate Generations

## Contract

Implement task 1.2 from [04_tasks.md](../04_tasks.md): build one static-bound candidate generation, separate GTFS-Realtime payload types, normalize common headers, omit invalid or unresolved references, and reject candidates whose encoded artifacts cannot prove the required invariants.

## Requirement Criteria

R1.3–R1.8, R2.1–R2.7, R3.1–R3.3, and R4.7.

## Owned Files

- [src/orchestrator.rs](../../../src/orchestrator.rs)
- [src/sources/mod.rs](../../../src/sources/mod.rs)

## Interfaces

Consume `RtSource`, `RtBatch`, parsed static GTFS, static ZIP bytes/version, and injected `SystemTime`. Produce `SelectedBatch`, `CandidateGeneration`, `ValidatedGeneration`, `GenerationId`, `FeedSetManifest`, `GenerationBuilder::build`, and `CandidateValidator::validate`.

## Verification

- `cargo test orchestrator sources -- --nocapture`
- Independent protobuf decode and exact entity-type partitioning assertions
- Static trip/stop/route closure and common header assertions
- Rustdoc review with warnings denied
