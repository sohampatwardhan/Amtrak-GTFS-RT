#!/usr/bin/env bash
# Exercise the checked-out source against Amtrak; keep logs even on failure.
# Rust tests bound each live operation; CI additionally bounds compilation.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

report_dir="${REPORT_DIR:-validation-reports/live-amtrak}"
mkdir -p "$report_dir"
tests=(
  sources::amtrak::tests::live_fetch_returns_batch
  orchestrator::tests::live_candidate_builds_and_passes_semantic_validation
)

# Cargo succeeds when a filter matches zero tests. Verify exact registration
# first, so a renamed, deleted, or no-longer-ignored test cannot turn CI green.
cargo test --locked --lib -- --ignored --list \
  2>&1 | tee "$report_dir/test-list.log"
for test_name in "${tests[@]}"; do
  if ! grep -Fxq "$test_name: test" "$report_dir/test-list.log"; then
    printf 'Missing expected ignored test: %s\n' "$test_name" >&2
    exit 1
  fi
done

status=0
for test_name in "${tests[@]}"; do
  if ! cargo test --locked --lib "$test_name" \
    -- --ignored --exact --nocapture 2>&1 | tee "$report_dir/${test_name##*::}.log"; then
    status=1
  fi
done
exit "$status"
