# Task 5.1 Release Checkpoint Report

```yaml
status: checkpoint_complete_release_blocked
task_id: "5.1"
capability_tier: frontier
resolved_model: controller
reasoning_level: extra_high
changed_files: ["scripts/validate-feeds.sh", "validation/baseline.json", "validation-reports/release-evidence.json", "validation-reports/release-decision.json", ".security/dependency-audit/release.json", ".security/dependency-audit/release.md"]
criteria:
  - id: "R1.2-R1.5,R8.1-R8.10"
    result: pass
    evidence: ["One manifest selected generation 1786670322000000000-0; its exact static and three realtime artifacts passed the content-pinned MobilityData static validator, separate realtime validation, independent Java decoding, and the accountable exception ratchet."]
  - id: "task 5.1 dependency delivery contract"
    result: block
    evidence: ["The fresh release audit completed a 375-package inventory but returned UNAVAILABLE/2 because CISA KEV exceeded the tool evidence limit; four GitHub CVSS records were partial and ten findings include critical RUSTSEC-2022-0011 through upstream amtrak-gtfs-rt."]
  - id: "controlled rollout"
    result: block
    evidence: ["No deployment was authorized or performed. Controlled-consumer migration is not started. Rollback retains the prior binary and persisted last-good immutable generation."]
verification:
  commands: ["cargo fmt --check", "cargo clippy --all-targets --all-features -- -D warnings", "cargo test --all-targets --all-features", "RUSTDOCFLAGS='-D warnings' cargo doc --no-deps", "bash -n scripts/validate-feeds.sh", "scripts/validate-feeds.sh --offline-fixtures --as-of 2026-08-13", "scripts/validate-feeds.sh", "dependency-security-audit release mode", "tampered cached gtfs-validator regression", "git diff --check"]
  exits: [0, 0, 0, 0, 0, 0, 0, 2, 1, 0]
findings:
  - "The API found and coherently published live Amtrak data: 116 trip updates, 116 vehicle positions, and 33 alerts in the validated feed set."
  - "The release gate surfaced only approved realtime E029 in this live sample and zero static errors; stale exception codes remain visible for later evidence across varying operating conditions."
  - "The validator gate now checks the official 8.0.1 SHA-256 for downloaded and cached bytes before startup use and immediately before independent validation."
  - "Both independent reviewers agree that rollout must remain blocked; their content-pin and explicit-decision findings were repaired."
decision_record: "validation-reports/release-decision.json"
context_requests: []
```
