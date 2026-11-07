#!/usr/bin/env bash
# Bring up the three-role Compose stack on a clean volume and prove it publishes.
#
# VehiclePositions advances /v1/feed-set.json only after TripUpdates and Alerts both
# contribute, so one downloaded manifest proves all three roles start, share /data,
# and agree on a generation. Fetches live Amtrak data. Logs survive failure.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

project="${COMPOSE_PROJECT:-amtrak-stack-smoke}"
base_url="${BASE_URL:-http://127.0.0.1:8090}"
deadline_secs="${DEADLINE_SECS:-600}"
report_dir="${REPORT_DIR:-validation-reports/compose-stack}"
export AMTRAK_IMAGE_TAG="${AMTRAK_IMAGE_TAG:-stack-smoke}"
compose=(docker compose -p "$project" -f docker-compose.yml)

mkdir -p "$report_dir"
cleanup() {
  "${compose[@]}" ps -a >"$report_dir/ps.txt" 2>&1 || true
  "${compose[@]}" logs --no-color >"$report_dir/compose.log" 2>&1 || true
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

"${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
if [ "${SMOKE_SKIP_BUILD:-0}" = 1 ]; then
  "${compose[@]}" up --detach --no-build
else
  "${compose[@]}" up --detach --build
fi

manifest="$report_dir/feed-set.json"
start=$(date +%s)
while :; do
  for service in vehicle-positions trip-updates alerts; do
    state=$("${compose[@]}" ps -a --format '{{.State}}' "$service" 2>/dev/null || true)
    if [ "$state" = exited ] || [ "$state" = dead ]; then
      printf '%s exited before the first generation\n' "$service" >&2
      "${compose[@]}" logs --no-color --tail 60 "$service" >&2 || true
      exit 1
    fi
  done
  status=$(curl -sS -o "$manifest" -w '%{http_code}' "$base_url/v1/feed-set.json" || true)
  [ "$status" = 200 ] && break
  if [ $(($(date +%s) - start)) -ge "$deadline_secs" ]; then
    printf 'no generation within %ss (last status %s)\n' "$deadline_secs" "$status" >&2
    exit 1
  fi
  sleep 10
done
printf 'first generation after %ss\n' "$(($(date +%s) - start))"

# Every artifact the manifest names must download and be non-empty.
python3 -I - "$manifest" "$base_url" "$report_dir" <<'PY'
import json, sys, urllib.request
from pathlib import Path

manifest, base, out = json.loads(Path(sys.argv[1]).read_text()), sys.argv[2], Path(sys.argv[3])
expected = {"static_zip", "official_static_zip", "platform_addon_zip",
            "trip_updates", "vehicle_positions", "alerts"}
urls = manifest["urls"]
missing = expected - urls.keys()
if missing:
    sys.exit(f"manifest lacks {sorted(missing)}")
for name in sorted(expected):
    url = urls[name] if urls[name].startswith("http") else base + urls[name]
    with urllib.request.urlopen(url, timeout=60) as response:
        body = response.read()
    if response.status != 200 or not body:
        sys.exit(f"{name}: status {response.status}, {len(body)} bytes")
    (out / name).write_bytes(body)
    print(f"{name}: {len(body)} bytes")
counts = manifest["entity_counts"]
print(f"generation {manifest['generation_id']}: static {manifest['static_version']}, "
      f"trip_updates={counts['trip_updates']} vehicle_positions={counts['vehicle_positions']} "
      f"alerts={counts['alerts']}")
if counts["vehicle_positions"] == 0:
    sys.exit("generation published with no vehicle positions")
PY
