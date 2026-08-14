# Spec State: Amtrak GTFS-RT Service

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

| Gate | Status | Evidence |
|---|---|---|
| Discovery | approved | Approved 2026-08-11: harden the existing vertical slice as an internal-only feed service |
| Requirements | approved | Approved 2026-08-11: eight user stories and 63 EARS criteria |
| Design | approved | Approved 2026-08-11: immutable generation publication with manifest-pinned delivery |
| Tasks | approved | Approved 2026-08-13: eight tasks across five dependency stages |
| Audit | not_run | Optional spec audit was not requested; approved tasks are in execution |
| Execution | completed_blocked | All eight tasks reached a terminal checkpoint; live feed validation passed, but rollout is blocked by unavailable/non-shippable release dependency evidence |

## Change Control

- The existing Rust implementation is repository evidence, not an approved requirements baseline; discovery must decide what to preserve, harden, replace, or defer.
- Material changes to the product boundary or selected delivery approach require discovery re-approval before requirements begin.
- On 2026-08-11, discovery selected public-release hardening of the existing vertical slice; replacing the architecture or expanding immediately into a multi-source hosted platform remains out of scope.
- On 2026-08-11, the service boundary changed from a public endpoint to an internally operated feed service because uncontrolled external demand is out of scope; the revised discovery requires approval.
- On 2026-08-11, the revised internal-service discovery boundary and five-minute readiness threshold were approved.
- On 2026-08-11, the requirements contract was approved; design may proceed without changing the approved product boundary.
- On 2026-08-11, the technical design was approved; task planning may proceed without implementation.
- On 2026-08-13, generated JSON artifacts moved into [sidecars](sidecars/) under the spec-driven development v1.1.1 suite layout; human-authored numbered Markdown and diagram IR remain in place.
- On 2026-08-13, the task plan was approved and execution opened on branch `amtrak-gtfs-rt-service-spec`.
- On 2026-08-13, self-hardening clarified approved behavior without changing the product boundary: static replacements must pass the pinned runtime standards validator, freshness uses the generation timestamp, durable restart recovery and directory sync ordering are explicit, and direct-peer authorization fails closed.
- On 2026-08-14, implementation and live feed validation completed. Release decision is BLOCK: no deployment or consumer migration occurred, and rollback preserves the prior binary plus the persisted last-good immutable generation.
- On 2026-08-14, the user authorized publishing the feature branch and merging [PR #1](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/1) into `main`; this source-integration decision does not override the deployment block.
