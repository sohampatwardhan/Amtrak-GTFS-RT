#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?usage: scripts/extract-release-notes.sh VERSION OUTPUT}"
OUTPUT="${2:?usage: scripts/extract-release-notes.sh VERSION OUTPUT}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Match the version's changelog heading with any ISO release date, rather than a hardcoded one, so
# every release resolves its own notes.
HEADING_RE="^## \[${VERSION//./\\.}\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$"
TEMP="$(mktemp)"
trap 'rm -f "$TEMP"' EXIT

heading_count="$(grep -Ec "$HEADING_RE" "$ROOT/CHANGELOG.md" || true)"
if [ "$heading_count" -ne 1 ]; then
  printf 'Expected exactly one changelog heading matching %q; found %s\n' \
    "$HEADING_RE" "$heading_count" >&2
  exit 1
fi
HEADING="$(grep -E "$HEADING_RE" "$ROOT/CHANGELOG.md")"

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
