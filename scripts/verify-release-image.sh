#!/usr/bin/env bash

set -euo pipefail

IMAGE="${1:?usage: scripts/verify-release-image.sh IMAGE VERSION REVISION ARCH}"
VERSION="${2:?usage: scripts/verify-release-image.sh IMAGE VERSION REVISION ARCH}"
REVISION="${3:?usage: scripts/verify-release-image.sh IMAGE VERSION REVISION ARCH}"
ARCH="${4:?usage: scripts/verify-release-image.sh IMAGE VERSION REVISION ARCH}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_ID="release-image-verify-$$-$(date +%s)"
CONTAINER="${RUN_ID}-container"
WORKDIR="$(mktemp -d)"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

test "$(docker image inspect "$IMAGE" -f '{{.Config.User}}')" = "10001:10001"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.licenses"}}')" = "AGPL-3.0-only"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.source"}}')" = "https://github.com/sohampatwardhan/Amtrak-GTFS-RT"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.version"}}')" = "$VERSION"
test "$(docker image inspect "$IMAGE" -f '{{index .Config.Labels "org.opencontainers.image.revision"}}')" = "$REVISION"
test "$(docker image inspect "$IMAGE" -f '{{.Architecture}}')" = "$ARCH"
test "$(docker image inspect "$IMAGE" -f '{{json .Config.Healthcheck.Test}}')" = '["CMD","/usr/local/bin/amtrak-gtfs-rt-service","--healthcheck"]'

docker create --name "$CONTAINER" "$IMAGE" >/dev/null
docker cp "$CONTAINER:/licenses/AGPL-3.0-only.txt" "$WORKDIR/AGPL-3.0-only.txt"
docker cp "$CONTAINER:/licenses/THIRD_PARTY_LICENSES.html" "$WORKDIR/THIRD_PARTY_LICENSES.html"

cmp -s "$ROOT/LICENSE" "$WORKDIR/AGPL-3.0-only.txt"
cmp -s "$ROOT/THIRD_PARTY_LICENSES.html" "$WORKDIR/THIRD_PARTY_LICENSES.html"
test -s "$WORKDIR/THIRD_PARTY_LICENSES.html"

printf 'Release image contract verified: %s (%s, %s, %s)\n' \
  "$IMAGE" "$VERSION" "$REVISION" "$ARCH"
