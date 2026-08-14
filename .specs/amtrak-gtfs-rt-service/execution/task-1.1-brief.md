# Task 1.1 Brief: Safe Configuration and Peer Access

## Contract

Implement the first approved Stage 1 task in [04_tasks.md](../04_tasks.md): safe environment configuration, loopback-first binding, exact direct-peer authorization, a fixed 300-second freshness limit, and pinned MobilityData GTFS validator provisioning.

## Requirement Criteria

R6.2, R6.3, R6.4, R7.1, R7.2, R7.3, R7.4, R7.9, and R7.11. R6.5 remains assigned to task 3.2, where denial audit events have HTTP request context.

## Owned Files

- [src/config.rs](../../../src/config.rs)
- [src/main.rs](../../../src/main.rs), limited to enforcing validation before startup composition
- [src/static_gtfs.rs](../../../src/static_gtfs.rs), limited to the expanded `Config` test fixture

## Interfaces

Produce `ValidatedConfig`, `AccessPolicy`, `AccessDecision`, `Config::validate(self) -> Result<ValidatedConfig, ConfigError>`, and `authorize(&AccessPolicy, IpAddr) -> AccessDecision`. Forwarding headers are outside this direct-transport-peer boundary.

## Verification

- `cargo test config -- --nocapture`
- Affected `static_gtfs` regression tests
- Rustdoc with warnings denied
- Changed-file and requirement-scope inspection
