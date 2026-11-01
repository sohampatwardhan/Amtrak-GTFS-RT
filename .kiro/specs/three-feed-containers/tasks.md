# Implementation Plan

- [x] 1. Shared library and role entry
  - Move the service into `src/lib.rs` and select `AMTRAK_ROLE` in `run_service`.
  - _Requirements: 1.3_

- [x] 2. Fleet snapshot and publication gate
- [x] 2.1 Write and read the fleet snapshot and contributions
  - `cohort.rs` stores one poll and the sibling protobufs.
  - _Requirements: 1.1, 1.2_
- [x] 2.2 VehiclePositions, TripUpdates, and Alerts loops
  - Only VehiclePositions polls. The other roles decorate. The assembler publishes when both contributions exist.
  - _Requirements: 1.1, 1.2, 1.3_

- [x] 3. Cached HTTP and early hints
  - `ETag`, `Last-Modified`, `304`, and a spawned trip-interest hook.
  - _Requirements: 1.4, 2.1, 2.2, 2.3, 2.4_

- [x] 4. Lead-time profile and 15-minute schedule
- [x] 4.1 Parse `amtrak-station-lead-time/v1`
  - Adopt only `observed` plus `wider`. Fall back otherwise.
  - _Requirements: 3.5, 3.6, 3.7_
- [x] 4.2 Debounce, coalesce, cap, and the base interval
  - Rust `TrackScheduler` and the Python poller. Hot checks still run between base polls.
  - _Requirements: 3.1, 3.2, 3.3, 3.4_

- [x] 5. Compose, bake, rustdoc CI, and agent docs
  - Three images, `cargo doc -D warnings`, AGENTS.md matches the tree.
  - _Requirements: 1.1, 2.3_

## Task graph

Arrows mean depends-on. Critical path is the publication gate through cached HTTP. Profile parsing can proceed beside the snapshot once the library exists.

```mermaid
---
config:
  layout: elk
---
flowchart TB
  subgraph phase1 [Phase 1 Library]
    t1["1. Shared library and role entry"]
  end
  subgraph phase2 [Phase 2 Snapshot]
    subgraph parallel2 [Parallel after 1]
      t21["2.1 Fleet snapshot"]
      t41["4.1 Lead-time profile"]
    end
    t22["2.2 Role loops"]
  end
  subgraph phase3 [Phase 3 Request path]
    t3["3. Cached HTTP and early hints"]
    t42["4.2 Debounce and base interval"]
  end
  subgraph phase4 [Phase 4 Packaging]
    t5["5. Compose bake CI and docs"]
  end
  subgraph legend [Legend]
    legCrit["Critical path"]
    legPar["Parallelizable"]
    legBlock["Blocked"]
  end
  t1 --> t21
  t1 --> t41
  t21 --> t22
  t22 --> t3
  t41 --> t42
  t3 --> t42
  t42 --> t5
  classDef critical fill:#fde68a,stroke:#b45309,color:#1c1917
  classDef parallel fill:#dbeafe,stroke:#1d4ed8,color:#1e3a8a
  classDef blocked fill:#fecaca,stroke:#b91c1c,color:#7f1d1d
  class t1,t21,t22,t3,t42,t5 critical
  class t41 parallel
  class legCrit critical
  class legPar parallel
  class legBlock blocked
```

The critical path is the one that makes a generation visible on the existing URLs. Profile parsing does not block that path.
