#!/usr/bin/env bash

set -euo pipefail

# Offline smoke test for a browser-sidecar image (track-fetcher or advisory-fetcher). It needs no
# network: Chromium renders a local page, and the server is checked on loopback inside a container
# started with --network none. Amtrak is never contacted, so a blocked runner cannot fail a release.
#
#   scripts/test-fetcher-image.sh IMAGE KIND [VERSION REVISION ARCH]
#
# KIND is track-fetcher or advisory-fetcher. With VERSION, REVISION and ARCH the release contract
# is checked too: OCI labels (source, licenses, version, revision) and the image architecture.
USAGE='usage: scripts/test-fetcher-image.sh IMAGE KIND [VERSION REVISION ARCH]'
IMAGE="${1:?$USAGE}"
KIND="${2:?$USAGE}"
VERSION="${3:-}"
REVISION="${4:-}"
ARCH="${5:-}"
CONTAINER="fetcher-smoke-$$-$(date +%s)"

case "$KIND" in
  track-fetcher) PORT=8081; SERVE_PATH=/tracks.json ;;
  advisory-fetcher) PORT=8080; SERVE_PATH=/service-alerts-and-notices ;;
  *) printf '%s\n' "$USAGE" >&2; exit 2 ;;
esac

if [ -n "$VERSION" ] && { [ -z "$REVISION" ] || [ -z "$ARCH" ]; }; then
  printf '%s\n' "$USAGE" >&2
  exit 2
fi

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

label() {
  docker image inspect "$IMAGE" -f "{{index .Config.Labels \"org.opencontainers.image.$1\"}}"
}

# The same confinement as the root compose file's track-fetcher service.
HARDENING=(
  --read-only --tmpfs /tmp --tmpfs /home/fetcher
  --cap-drop ALL --security-opt no-new-privileges:true --init
  --memory 768m --pids-limit 256 --shm-size 64m
)

test "$(docker image inspect "$IMAGE" -f '{{.Config.User}}')" = fetcher
test "$(label source)" = "https://github.com/sohampatwardhan/Amtrak-GTFS-RT"
test "$(label licenses)" = "AGPL-3.0-only"
test "$(label title)" = "Amtrak GTFS-RT $KIND"
if [ -n "$VERSION" ]; then
  test "$(label version)" = "$VERSION"
  test "$(label revision)" = "$REVISION"
  test "$(docker image inspect "$IMAGE" -f '{{.Architecture}}')" = "$ARCH"
fi

# Chromium (headless shell) launches under the confinement and renders a page.
docker run --rm --network none "${HARDENING[@]}" --entrypoint python "$IMAGE" -c '
import asyncio
from playwright.async_api import async_playwright

async def main():
    async with async_playwright() as p:
        browser = await p.chromium.launch(args=["--disable-dev-shm-usage"])
        page = await browser.new_page()
        await page.set_content("<p id=x>smoke</p>")
        assert await page.text_content("#x") == "smoke"
        await browser.close()

asyncio.run(main())
'

# The long-running process serves liveness, and its endpoint is 503 before any poll succeeds.
docker run --detach --name "$CONTAINER" --network none "${HARDENING[@]}" \
  -e POLL_INTERVAL_SECS=3600 "$IMAGE" >/dev/null

probe() {
  docker exec "$CONTAINER" python -c '
import sys, urllib.error, urllib.request
try:
    print(urllib.request.urlopen("http://127.0.0.1:" + sys.argv[1] + sys.argv[2], timeout=5).status)
except urllib.error.HTTPError as error:
    print(error.code)
' "$PORT" "$1" 2>/dev/null
}

healthy=""
for _ in $(seq 1 60); do
  if [ "$(docker inspect "$CONTAINER" -f '{{.State.Running}}')" != true ]; then
    docker logs "$CONTAINER" >&2 || true
    printf '%s exited during startup\n' "$KIND" >&2
    exit 1
  fi
  if [ "$(probe /healthz || true)" = 200 ]; then
    healthy=yes
    break
  fi
  sleep 1
done
if [ -z "$healthy" ]; then
  docker logs "$CONTAINER" >&2 || true
  printf '%s did not answer /healthz within 60s\n' "$KIND" >&2
  exit 1
fi
test "$(probe "$SERVE_PATH")" = 503

# SIGTERM stops it well within Docker's grace period, including mid-poll.
start="$(date +%s)"
docker stop --time 20 "$CONTAINER" >/dev/null
elapsed=$(( $(date +%s) - start ))
if [ "$elapsed" -ge 15 ]; then
  printf '%s did not exit within 15s of SIGTERM (%ss)\n' "$KIND" "$elapsed" >&2
  exit 1
fi
test "$(docker inspect "$CONTAINER" -f '{{.State.OOMKilled}}')" = false

printf 'Fetcher image smoke test passed: %s (%s)\n' "$IMAGE" "$KIND"
