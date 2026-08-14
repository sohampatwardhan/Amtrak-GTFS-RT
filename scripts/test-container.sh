#!/usr/bin/env bash
#
# test-container.sh — non-interactive smoke harness for the amtrak-gtfs-rt container image.
#
# WHAT it proves (one bounded run against a real Amtrak fetch):
#   * the image builds a working service that reaches Docker "healthy", serves a coherent
#     feed generation, and exposes four independently decodable artifacts;
#   * direct-peer authorization admits only allow-listed socket peers and ignores forwarding
#     headers, a wildcard bind without a policy refuses to start, and a non-writable /data
#     refuses to start;
#   * a retained named volume recovers the last-good generation, byte-for-byte, when the
#     container is recreated with no upstream connectivity.
#
# WHY fail-closed and uniquely named: the harness runs unattended (locally and in CI). It uses
# set -euo pipefail, a unique per-run name prefix, an explicit private bridge subnet, and a trap
# that only ever removes the container/network/volume it created — it never touches unrelated
# Docker objects. The named volume is deliberately kept alive through the whole run (the restart
# recovery step depends on it) and removed only in final cleanup.
#
# WHY peers are separate containers with static bridge IPs: on a user-defined bridge, containers
# reach each other directly with their real source IP (no NAT), so an allow-listed peer and a
# denied peer are the deterministic, engine-agnostic way to exercise the socket-peer policy.
# Host-published traffic is SNAT-translated differently across engines (Docker Desktop vs. the
# Linux bridge), so the observed host peer IP is discovered and reported, never assumed.
#
# Usage:  scripts/test-container.sh [IMAGE]        (default image: amtrak-gtfs-rt:local)
# This harness never runs `docker push` and never deploys; it only builds evidence locally.

set -euo pipefail

IMAGE="${1:-amtrak-gtfs-rt:local}"

# Bounded deadline (seconds) for the live fetch + static validation + first generation. The
# packaged Java validator plus a live Amtrak download dominate this; keep it generous so a slow
# but healthy run is not reported as a failure, while a truly stuck run still terminates.
HEALTH_DEADLINE="${HEALTH_DEADLINE:-480}"

# Unique, collision-resistant identifiers for every Docker object this run creates.
RUN_ID="amtrak-smoke-$$-$(date +%s)"
NET="${RUN_ID}-net"
VOL="${RUN_ID}-vol"
EMPTY_VOL="${RUN_ID}-emptyvol"   # throwaway empty volume for the no-generation guard
SVC="${RUN_ID}-svc"
ALLOW_PEER="${RUN_ID}-allow"
DENY_PEER="${RUN_ID}-deny"
GUARD="${RUN_ID}-guard"          # short-lived fail-closed guard container
SEED="${RUN_ID}-seed"            # short-lived root helper that writes into the volume

# Bound (seconds) for a fail-closed guard container: a correct image exits almost immediately,
# so anything still running past this did NOT fail closed.
GUARD_DEADLINE="${GUARD_DEADLINE:-30}"

# A private subnet unlikely to collide with existing Docker networks. The gateway and the two
# peer IPs are fixed so the allowlist and the assertions are deterministic.
SUBNET="172.31.243.0/24"
GATEWAY="172.31.243.1"
ALLOW_IP="172.31.243.10"
DENY_IP="172.31.243.20"
HOST_PORT="${HOST_PORT:-18080}"   # published only on host loopback

WORKDIR="$(mktemp -d)"
REPORT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/validation-reports/container"

fail_count=0
step() { printf '\n=== %s ===\n' "$1"; }
pass() { printf 'PASS  %s\n' "$1"; }
bad()  { printf 'FAIL  %s\n' "$1"; fail_count=$((fail_count + 1)); }
die()  { printf 'ERROR %s\n' "$1" >&2; exit 1; }

# Remove only this run's objects. Runs on every exit; the volume is removed last because the
# restart-recovery step needs it to survive the service container's destruction.
cleanup() {
  docker rm -f "$SVC" "$ALLOW_PEER" "$DENY_PEER" "$GUARD" "$SEED" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  docker volume rm "$VOL" "$EMPTY_VOL" >/dev/null 2>&1 || true
  rm -rf "$WORKDIR" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# Run a guard container in the background and wait a bounded time for it to exit on its own. A
# correctly fail-closed image refuses the unsafe config and exits within a second or two; a
# regression that wrongly starts serving would otherwise block the whole (unattended) harness
# forever, so this never blocks longer than GUARD_DEADLINE. Returns 0 if the container exited by
# itself (failed closed), 1 if it had to be killed (did NOT fail closed).
run_guarded() { # docker-run-args... (must not include -d/--name; $GUARD is used)
  docker rm -f "$GUARD" >/dev/null 2>&1 || true
  docker run -d --name "$GUARD" "$@" >/dev/null 2>&1 || return 0  # immediate refusal == exited
  local waited=0
  while [ "$waited" -lt "$GUARD_DEADLINE" ]; do
    case "$(docker inspect -f '{{.State.Status}}' "$GUARD" 2>/dev/null || echo gone)" in
      running) ;;
      *) docker rm -f "$GUARD" >/dev/null 2>&1 || true; return 0 ;;
    esac
    sleep 1; waited=$((waited + 1))
  done
  docker rm -f "$GUARD" >/dev/null 2>&1 || true
  return 1
}

# curl from a throwaway peer container at a fixed bridge IP. Prints the HTTP status on the last
# line so callers can assert it; the image already ships curl, so no extra image is pulled.
peer_status() { # ip name url [extra curl args...]
  local ip="$1" name="$2" url="$3"; shift 3
  docker run --rm --name "$name" --network "$NET" --ip "$ip" --entrypoint curl "$IMAGE" \
    -s -o /dev/null -w '%{http_code}' "$@" "$url"
}
peer_fetch() { # ip name url outfile
  local ip="$1" name="$2" url="$3" out="$4"
  docker run --rm --name "$name" --network "$NET" --ip "$ip" --entrypoint curl "$IMAGE" \
    -fsS "$url" >"$out"
}

# Map an artifact file name to the jq path of its URL in the manifest. Used instead of an
# associative array so the harness runs on bash 3.2 (the default on macOS) as well as bash 4+.
manifest_key() {
  case "$1" in
    static.zip)           echo '.urls.static_zip' ;;
    trip-updates.pb)      echo '.urls.trip_updates' ;;
    vehicle-positions.pb) echo '.urls.vehicle_positions' ;;
    alerts.pb)            echo '.urls.alerts' ;;
    *) return 1 ;;
  esac
}
ARTIFACTS="static.zip trip-updates.pb vehicle-positions.pb alerts.pb"

command -v docker >/dev/null || die "docker is required"
docker image inspect "$IMAGE" >/dev/null 2>&1 || die "image not found: $IMAGE (build it first)"
mkdir -p "$REPORT_DIR"

# -----------------------------------------------------------------------------------------------
step "provision isolated bridge network and named volume"
docker network create --driver bridge --subnet "$SUBNET" --gateway "$GATEWAY" "$NET" >/dev/null
docker volume create "$VOL" >/dev/null
pass "network $NET ($SUBNET, gw $GATEWAY) and volume $VOL created"

# -----------------------------------------------------------------------------------------------
# Fail-closed configuration guards. Each must exit *before* opening a listener; run_guarded bounds
# the wait so a regression that wrongly starts serving is reported, never left to hang the harness.
step "wildcard bind without a peer policy must refuse to start"
if run_guarded --network "$NET" -e AMTRAK_BIND_ADDR=0.0.0.0:8080 "$IMAGE"; then
  pass "startup rejected 0.0.0.0 bind without AMTRAK_ALLOWED_PEER_IPS"
else
  bad "container did not fail closed on 0.0.0.0 bind without an allowlist (still running at deadline)"
fi

step "non-writable /data must refuse to start"
# A read-only mount of the (empty) named volume: the generation store cannot create its layout,
# so startup must fail before serving. The :ro mount performs no writes, so $VOL stays pristine.
if run_guarded --network "$NET" -v "$VOL":/data:ro "$IMAGE"; then
  pass "startup rejected a non-writable /data mount"
else
  bad "container did not fail closed on a non-writable /data (still running at deadline)"
fi

step "no recoverable generation and no upstream must fail closed (not serve an empty feed set)"
# A fresh empty volume with no connectivity cannot recover a last-good generation or bootstrap a
# static feed, so the service must exit rather than open a listener with no feed set. This is the
# container-observable complement to R6.6; the admitted-503-when-no-generation router invariant
# itself is verified deterministically by the unchanged Rust test
# `serve::tests::readiness_obeys_no_generation_and_exact_freshness_boundaries` (same binary here).
docker volume create "$EMPTY_VOL" >/dev/null
if run_guarded --network none -v "$EMPTY_VOL":/data "$IMAGE"; then
  pass "startup failed closed with no recoverable generation and no upstream"
else
  bad "container kept running with no generation and no upstream (should fail closed)"
fi

# -----------------------------------------------------------------------------------------------
step "start the service (wildcard bind, exact allowlist, host-loopback publication)"
# Allowlist admits the bridge gateway (host-published path) and the allow-listed peer container.
# The port is published only on 127.0.0.1 so the feed boundary is never exposed on all host
# interfaces.
docker run -d --name "$SVC" --network "$NET" \
  -p "127.0.0.1:${HOST_PORT}:8080" \
  -e AMTRAK_BIND_ADDR=0.0.0.0:8080 \
  -e "AMTRAK_ALLOWED_PEER_IPS=${GATEWAY},${ALLOW_IP}" \
  -v "$VOL":/data \
  "$IMAGE" >/dev/null
SVC_IP="$(docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$SVC")"
[ -n "$SVC_IP" ] || die "could not determine service container IP"
pass "service $SVC started at $SVC_IP:8080"

step "wait for Docker health, readiness, and manifest (deadline ${HEALTH_DEADLINE}s)"
started="$(date +%s)"
health_ts=""
while :; do
  status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "$SVC" 2>/dev/null || echo gone)"
  state="$(docker inspect -f '{{.State.Status}}' "$SVC" 2>/dev/null || echo gone)"
  if [ "$state" != "running" ]; then
    docker logs --tail 40 "$SVC" || true
    die "service exited before becoming healthy (state=$state)"
  fi
  if [ "$status" = "healthy" ] && [ -z "$health_ts" ]; then
    health_ts="$(date +%s)"
  fi
  # Readiness and the manifest are gated by peer policy, so probe them from the allow-listed peer.
  ready_code="$(peer_status "$ALLOW_IP" "${ALLOW_PEER}-r" "http://${SVC_IP}:8080/readyz" || echo 000)"
  if [ -n "$health_ts" ] && [ "$ready_code" = "200" ]; then
    break
  fi
  now="$(date +%s)"
  if [ $((now - started)) -ge "$HEALTH_DEADLINE" ]; then
    docker logs --tail 60 "$SVC" || true
    die "deadline exceeded before healthy+ready (health=$status ready=$ready_code)"
  fi
  sleep 3
done
TIME_TO_HEALTH=$((health_ts - started))
pass "healthy in ${TIME_TO_HEALTH}s; /readyz=200 via allow-listed peer"

# -----------------------------------------------------------------------------------------------
step "fetch manifest and all four artifacts via the allow-listed peer"
peer_fetch "$ALLOW_IP" "${ALLOW_PEER}-m" "http://${SVC_IP}:8080/v1/feed-set.json" "$WORKDIR/manifest.json"
GEN_ID="$(jq -r '.generation_id' "$WORKDIR/manifest.json")"
[ -n "$GEN_ID" ] && [ "$GEN_ID" != "null" ] || die "manifest has no generation_id"
pass "manifest generation_id=$GEN_ID"

for name in $ARTIFACTS; do
  url_path="$(jq -r "$(manifest_key "$name")" "$WORKDIR/manifest.json")"
  [ -n "$url_path" ] && [ "$url_path" != "null" ] || die "manifest missing url for $name"
  peer_fetch "$ALLOW_IP" "${ALLOW_PEER}-a" "http://${SVC_IP}:8080${url_path}" "$WORKDIR/$name"
  pass "fetched $name ($(wc -c <"$WORKDIR/$name" | tr -d ' ') bytes) from $url_path"
done

step "independently inspect the static ZIP and three protobuf messages"
unzip -tqq "$WORKDIR/static.zip" || die "static.zip failed integrity check"
# Exact-name membership via zipinfo (-Z1 prints one entry name per line); avoids the column
# formatting of `unzip -l`.
unzip -Z1 "$WORKDIR/static.zip" >"$WORKDIR/static-entries.txt"
for required in agency.txt stops.txt routes.txt trips.txt stop_times.txt; do
  grep -Fxq "$required" "$WORKDIR/static-entries.txt" || die "static.zip missing $required"
done
pass "static.zip integrity ok and contains core GTFS files ($(wc -l <"$WORKDIR/static-entries.txt" | tr -d ' ') entries)"

# Dependency-free GTFS-Realtime check: parse the wire format directly and require a FeedHeader
# (field 1) carrying gtfs_realtime_version (its field 1). This decodes the bytes independently of
# the service's own encoder, so it is a genuine cross-check rather than a round-trip.
for pb in trip-updates.pb vehicle-positions.pb alerts.pb; do
  python3 - "$WORKDIR/$pb" <<'PY' || die "$pb is not a valid GTFS-Realtime FeedMessage"
import sys
data = open(sys.argv[1], "rb").read()
assert data, "empty protobuf"
def varint(b, i):
    shift = val = 0
    while True:
        x = b[i]; i += 1
        val |= (x & 0x7F) << shift
        if not x & 0x80:
            return val, i
        shift += 7
i = 0
tag, i = varint(data, i)
assert (tag >> 3) == 1 and (tag & 7) == 2, "first field is not FeedHeader (len-delimited field 1)"
hlen, i = varint(data, i)
header = data[i:i + hlen]
j = 0
htag, j = varint(header, j)
assert (htag >> 3) == 1 and (htag & 7) == 2, "FeedHeader missing gtfs_realtime_version"
vlen, j = varint(header, j)
version = header[j:j + vlen].decode("utf-8")
print(f"{sys.argv[1].split('/')[-1]}: gtfs_realtime_version={version}")
PY
  pass "$pb decodes as a GTFS-Realtime FeedMessage"
done

# Retain the last-good artifact digests (one file per artifact) for the restart-recovery
# comparison. The original manifest at $WORKDIR/manifest.json holds the last-good generation id.
for name in $ARTIFACTS; do
  shasum -a 256 "$WORKDIR/$name" | awk '{print $1}' >"$WORKDIR/$name.goodsha"
done

# -----------------------------------------------------------------------------------------------
step "denied peer and spoofed forwarding headers must both get 403"
code="$(peer_status "$DENY_IP" "${DENY_PEER}-1" "http://${SVC_IP}:8080/v1/feed-set.json")"
[ "$code" = "403" ] && pass "denied peer $DENY_IP -> 403" || bad "denied peer got $code (want 403)"

# Same denied peer, now spoofing every forwarding header with an allow-listed identity. Because
# authorization uses only the socket peer, these must not change the result.
code="$(peer_status "$DENY_IP" "${DENY_PEER}-2" "http://${SVC_IP}:8080/v1/feed-set.json" \
  -H "X-Forwarded-For: ${ALLOW_IP}" -H "Forwarded: for=${GATEWAY}" -H "X-Real-IP: ${ALLOW_IP}")"
[ "$code" = "403" ] && pass "spoofed forwarding headers still -> 403" || bad "spoofed headers got $code (want 403)"

step "host-loopback publication: /livez reachable, observed host peer recorded"
host_livez="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${HOST_PORT}/livez" || echo 000)"
[ "$host_livez" = "200" ] && pass "host 127.0.0.1:${HOST_PORT}/livez -> 200 (public liveness)" \
  || bad "host /livez got $host_livez (want 200)"
# The manifest over the published port depends on the engine's SNAT source IP. Try it; if denied,
# recover the observed peer from the audit log so the operator can set the correct allowlist.
host_manifest="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${HOST_PORT}/v1/feed-set.json" || echo 000)"
if [ "$host_manifest" = "200" ]; then
  pass "host-published manifest -> 200 (observed peer is in the allowlist)"
else
  observed="$(docker logs "$SVC" 2>&1 | grep -o 'peer=[0-9.]*' | tail -1 || true)"
  printf 'NOTE  host-published manifest -> %s; observed %s (engine SNAT differs; set allowlist accordingly)\n' \
    "$host_manifest" "${observed:-peer=unknown}"
fi

# -----------------------------------------------------------------------------------------------
step "restart recovery over the same volume, with an incomplete newest candidate, upstream down"
docker rm -f "$SVC" >/dev/null
# Plant an incomplete/corrupt newest generation candidate directly in the volume: a directory whose
# id sorts after every real generation but which has no manifest.txt and only garbage bytes. Per
# R4.4 the service must still expose only the older *valid* generation, never this candidate. A
# root helper writes it (the volume dirs are owned by uid 10001) and hands ownership back.
docker run --rm --name "$SEED" --user 0 -v "$VOL":/data --entrypoint sh "$IMAGE" -c '
  cand=/data/generations/9999999999999999999-0
  mkdir -p "$cand"
  echo "corrupt-incomplete-candidate" > "$cand/static.zip"
  chown -R 10001:10001 "$cand"
' >/dev/null
pass "planted incomplete newest generation candidate 9999999999999999999-0"

# --network none removes all connectivity, so no refresh can succeed; the service must recover the
# retained last-good generation from the volume. Default env => loopback bind + empty allowlist,
# so a container-loopback request (via docker exec) is admitted for the manifest read.
docker run -d --name "$SVC" --network none -v "$VOL":/data "$IMAGE" >/dev/null
started="$(date +%s)"
while :; do
  status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "$SVC" 2>/dev/null || echo gone)"
  state="$(docker inspect -f '{{.State.Status}}' "$SVC" 2>/dev/null || echo gone)"
  [ "$state" = "running" ] || { docker logs --tail 40 "$SVC" || true; die "offline recovery container exited (state=$state)"; }
  [ "$status" = "healthy" ] && break
  now="$(date +%s)"
  [ $((now - started)) -ge "$HEALTH_DEADLINE" ] && { docker logs --tail 60 "$SVC" || true; die "offline recovery did not become healthy"; }
  sleep 3
done
pass "offline container healthy from retained volume in $(( $(date +%s) - started ))s"

docker exec "$SVC" curl -fsS "http://127.0.0.1:8080/v1/feed-set.json" >"$WORKDIR/recovered-manifest.json"
REC_ID="$(jq -r '.generation_id' "$WORKDIR/recovered-manifest.json")"
if [ "$REC_ID" = "$GEN_ID" ]; then
  pass "recovered generation_id matches ($REC_ID)"
  pass "incomplete newest candidate 9999999999999999999-0 was NOT exposed (R4.4)"
else
  bad "recovered generation_id=$REC_ID != original $GEN_ID (incomplete candidate may have been exposed)"
fi

for name in $ARTIFACTS; do
  url_path="$(jq -r "$(manifest_key "$name")" "$WORKDIR/manifest.json")"
  docker exec "$SVC" curl -fsS "http://127.0.0.1:8080${url_path}" >"$WORKDIR/rec-$name"
  rec_sha="$(shasum -a 256 "$WORKDIR/rec-$name" | awk '{print $1}')"
  good_sha="$(cat "$WORKDIR/$name.goodsha")"
  [ "$rec_sha" = "$good_sha" ] && pass "recovered $name bytes identical" \
    || bad "recovered $name differs (got $rec_sha, want $good_sha)"
done

# -----------------------------------------------------------------------------------------------
step "image size, SBOM, and CVE evidence for the exact image"
IMG_SIZE_MB="$(docker image inspect "$IMAGE" --format '{{.Size}}' | awk '{printf "%.0f", $1/1048576}')"
pass "image size ${IMG_SIZE_MB} MB (uncompressed); time-to-health ${TIME_TO_HEALTH}s"

# SBOM: Docker Scout analyzes the image locally and does not need auth.
if docker scout version >/dev/null 2>&1 &&
   docker scout sbom --format spdx --output "$REPORT_DIR/sbom.spdx.json" "$IMAGE" >/dev/null 2>&1; then
  pass "SBOM written to validation-reports/container/sbom.spdx.json"
else
  printf 'NOTE  SBOM evidence unavailable (docker scout sbom did not complete); not clean\n'
fi

# CVE evidence. Prefer Docker Scout (the design-named tool), but its `cves` command queries the
# Scout service and needs `docker login`. Fall back to grype, which scans the local image against
# its own database with no registry auth. Either way, never call unavailable evidence "clean".
cve_done=""
scout_err="$(docker scout cves --format markdown --output "$REPORT_DIR/cves.md" "$IMAGE" 2>&1 >/dev/null || true)"
if [ -s "$REPORT_DIR/cves.md" ]; then
  pass "CVE report (docker scout) written to validation-reports/container/cves.md (review it; do not assume clean)"
  cve_done=1
elif command -v grype >/dev/null 2>&1; then
  # grype: auth-free local scan. Save both a human table and JSON, and surface the counts so the
  # result is reviewed, not assumed clean.
  if grype "$IMAGE" -o table >"$REPORT_DIR/cves-grype.txt" 2>/dev/null &&
     grype "$IMAGE" -o json  >"$REPORT_DIR/cves-grype.json" 2>/dev/null; then
    counts="$(grype "$IMAGE" -o json 2>/dev/null | jq -r '[.matches[].vulnerability.severity] | group_by(.) | map("\(length) \(.[0])") | join(", ")' 2>/dev/null || true)"
    pass "CVE report (grype) written to validation-reports/container/cves-grype.{txt,json}; findings: ${counts:-see report} (review it; do not assume clean)"
    cve_done=1
  fi
fi
if [ -z "$cve_done" ]; then
  if printf '%s' "$scout_err" | grep -qi 'log in'; then
    printf 'NOTE  CVE evidence UNAVAILABLE: docker scout cves needs `docker login` and grype is not installed. Not clean; run `docker login` or install grype, then re-run.\n'
  else
    printf 'NOTE  CVE evidence UNAVAILABLE: no scanner produced a report (%s). Not clean.\n' "$(printf '%s' "$scout_err" | head -1)"
  fi
fi

step "confirm no publication or deployment occurred"
pass "harness performed no docker push and no deployment"

# -----------------------------------------------------------------------------------------------
printf '\n'
if [ "$fail_count" -eq 0 ]; then
  echo "CONTAINER SMOKE HARNESS PASSED"
else
  echo "CONTAINER SMOKE HARNESS FAILED ($fail_count assertion failures)"
fi
exit "$fail_count"
