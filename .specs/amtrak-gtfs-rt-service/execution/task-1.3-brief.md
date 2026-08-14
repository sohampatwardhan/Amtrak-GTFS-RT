# Task 1.3 Brief: Accountable Validator Exceptions

## Contract

Implement task 1.3 from [04_tasks.md](../04_tasks.md): replace bare realtime validator allowances with owned, review-dated records and provide a pure offline ratchet that rejects malformed, expired, or newly observed errors before Java, downloads, or live Amtrak discovery.

## Requirement Criteria

R1.2, R1.8, R3.6, R5.1, R7.5, R7.6, R7.7, and R7.10.

## Owned Files

- [validation/baseline.json](../../../validation/baseline.json)
- [scripts/validate-feeds.sh](../../../scripts/validate-feeds.sh)

## Interfaces

Consume validator reports plus exception records containing `code`, `upstream_cause`, `owner`, and `review_on`. Produce a fail-closed offline parser and deterministic static/realtime error-code ratchet using an injected as-of date.

## Verification

- Run the offline fixture mode for accepted, malformed, expired, newly observed, and resolved findings.
- `bash -n scripts/validate-feeds.sh`
- Verify the offline path performs no Java, network, or live-feed work.
- Review inline contract documentation.
