#!/usr/bin/env bash

set -euo pipefail

VERSION="${1:?usage: scripts/check-release-metadata.sh VERSION [TAG]}"
TAG="${2:-v${VERSION}}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

actual="$(
  cargo metadata --manifest-path "$ROOT/Cargo.toml" --locked --offline --no-deps \
    --format-version 1 \
    | jq -r '.packages[] | select(.name == "amtrak-gtfs-rt-service") | .version'
)"

[ "$actual" = "$VERSION" ]
[ "$TAG" = "v${VERSION}" ]
grep -Fqx "## [${VERSION}] - 2026-08-14" "$ROOT/CHANGELOG.md"
grep -Fqx 'license = "AGPL-3.0-only"' "$ROOT/Cargo.toml"
test -s "$ROOT/THIRD_PARTY_LICENSES.html"

if command -v cargo-about >/dev/null 2>&1; then
  generated="$(mktemp)"
  trap 'rm -f "$generated"' EXIT
  "$ROOT/scripts/generate-third-party-licenses.sh" "$generated"
  cmp -s "$ROOT/THIRD_PARTY_LICENSES.html" "$generated"
fi
