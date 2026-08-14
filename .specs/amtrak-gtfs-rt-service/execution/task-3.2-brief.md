# Task 3.2 Brief: Controlled Immutable Feed Delivery

## Contract

Implement task 3.2 from [04_tasks.md](../04_tasks.md): serve liveness publicly and protect readiness, the feed-set manifest, and immutable generation artifacts using direct transport-peer identity. Resolve all artifacts through `GenerationStore`, never route-derived filesystem paths.

## Requirement Criteria

R4.2–R4.6, R5.6–R5.8, and R6.1–R6.5.

## Owned Files

- [src/serve.rs](../../../src/serve.rs)
- [README.md](../../../README.md)

## Interfaces

Consume `AccessPolicy`, `GenerationStore`, `PublishedGeneration`, `ConnectInfo<SocketAddr>`, and an injected clock. Produce `AppState`, `router(AppState)`, `/livez`, `/readyz`, `/v1/feed-set.json`, and closed immutable generation routes with exact status/content-type behavior.

## Current Technology Evidence

Context7 `/tokio-rs/axum/axum_v0_8_4` confirms typed `State`, `ConnectInfo<SocketAddr>`, `into_make_service_with_connect_info::<SocketAddr>()`, `MockConnectInfo` for tests, and direct `Router` service testing. Installed Axum is 0.8.9 and matches this 0.8 API family.

## Verification

- `cargo test serve::tests -- --nocapture`
- Exercise allowed, denied, missing, and spoofed peers; every readiness boundary; unavailable and immutable lookup behavior; strict identifiers/artifact names; removed routes; manifest coherence; and content types.
- Review Rust API documentation and consumer migration instructions.
