# Task 4.1 Brief: Complete Service Composition and Supervision

## Contract

Implement task 4.1 from [04_tasks.md](../04_tasks.md): replace every compatibility path in `main` with validated configuration, recovered generation state, standards-gated static lifecycle, generation polling, peer-aware immutable HTTP delivery, and fail-fast sibling-cancelling supervision.

## Requirement Criteria

R1.1, R1.2, R3.5, R4.1, R5.9, R7.5, R7.8, R7.10, and R7.11.

## Owned Files

- [src/main.rs](../../../src/main.rs)
- [src/writer.rs](../../../src/writer.rs)
- [src/static_gtfs.rs](../../../src/static_gtfs.rs)
- [src/orchestrator.rs](../../../src/orchestrator.rs)
- [src/serve.rs](../../../src/serve.rs)
- [README.md](../../../README.md)

The additional files are a mechanical scope repair: task 4.1 explicitly removes
the compatibility paths staged in tasks 3.1 and 3.2, so their definitions and
temporary deployment warning must be deleted in their owning files.

## Interfaces

Consume task 3.1 refresh futures, task 3.2 `router(AppState)`, and recovered durable state. Produce one composition root, transport-peer propagation, `supervise(...) -> Result<(), ServiceError>`, and end-to-end manifest-to-artifact behavior.

## Verification

- `cargo test --all-targets --all-features`
- Target each required future's success/failure and sibling cancellation.
- Recover a retained generation while refresh is unavailable.
- Build, commit, discover, and serve one mocked generation end to end.
- Review composition and supervision documentation.
