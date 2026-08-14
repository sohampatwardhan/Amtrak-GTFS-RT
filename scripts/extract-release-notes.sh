#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?usage: scripts/extract-release-notes.sh VERSION OUTPUT}"
OUTPUT="${2:?usage: scripts/extract-release-notes.sh VERSION OUTPUT}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HEADING="## [${VERSION}] - 2026-08-14"
TEMP="$(mktemp)"
trap 'rm -f "$TEMP"' EXIT

heading_count="$(grep -Fxc "$HEADING" "$ROOT/CHANGELOG.md" || true)"
if [ "$heading_count" -ne 1 ]; then
  printf 'Expected exactly one changelog heading %q; found %s\n' \
    "$HEADING" "$heading_count" >&2
  exit 1
fi

awk -v heading="$HEADING" '
  $0 == heading { in_release = 1; next }
  in_release && /^## \[/ { exit }
  in_release { print }
' "$ROOT/CHANGELOG.md" >"$TEMP"

if ! grep -q '[^[:space:]]' "$TEMP"; then
  printf 'Release notes for %s are empty\n' "$VERSION" >&2
  exit 1
fi

mv "$TEMP" "$OUTPUT"
trap - EXIT
