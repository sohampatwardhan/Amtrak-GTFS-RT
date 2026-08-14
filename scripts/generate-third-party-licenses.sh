#!/usr/bin/env bash

set -euo pipefail

OUTPUT="${1:?usage: scripts/generate-third-party-licenses.sh OUTPUT}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
raw="$(mktemp)"
trap 'rm -f "$raw"' EXIT

cargo about generate \
  --manifest-path "$ROOT/Cargo.toml" \
  --config "$ROOT/about.toml" \
  --locked \
  --fail \
  "$ROOT/about.hbs" >"$raw"

LC_ALL=C sed -e 's/\r$//' -e 's/[[:blank:]]*$//' "$raw" \
  | awk '{ line[NR] = $0 } END { last = NR; while (last > 0 && line[last] == "") last--; for (i = 1; i <= last; i++) print line[i] }' \
  >"$OUTPUT"
