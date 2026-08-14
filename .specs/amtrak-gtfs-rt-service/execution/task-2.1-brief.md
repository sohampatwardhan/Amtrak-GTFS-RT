# Task 2.1 Brief: Immutable Generation Persistence

## Contract

Implement task 2.1 from [04_tasks.md](../04_tasks.md): publish every coherent feed set through one durable filesystem commit, expose only immutable complete generations, and recover the last good generation after process or publication failure.

## Requirement Criteria

R1.1, R3.5, and R5.4–R5.8.

## Owned File

- [src/writer.rs](../../../src/writer.rs)

## Interfaces

Consume task 1.2 `ValidatedGeneration`, `GenerationId`, `FeedSetManifest`, and the output directory. Produce `PublishedGeneration`, `GenerationStore::{open,current,get,commit}`, and `GenerationPublisher::publish` with immutable `Arc<[u8]>` artifacts and one in-memory visibility swap after durable publication.

## Verification

- `cargo test writer -- --nocapture`
- Repeat failure-injection and concurrent-reader tests.
- Prove recovery ignores temporary/partial state and preserves last-good under outage.
- Review public durability, retention, visibility, and failure-atomicity documentation.
