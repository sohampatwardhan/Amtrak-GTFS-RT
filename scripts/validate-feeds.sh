#!/usr/bin/env bash
#
# Feed validation gate.
#
# Runs MobilityData's GTFS and GTFS-Realtime validators against the feeds this
# service produces, then compares the reported ERROR codes against
# validation/baseline.json. Exits non-zero when a validator reports an ERROR
# code that is not in the baseline — i.e. a genuine regression.
#
# Occurrence counts are deliberately not gated: they track how many trains are
# running and vary legitimately between runs.
#
# Requires: Java 17+ and Maven, or Docker (used automatically as a fallback).
#
# Usage:
#   scripts/validate-feeds.sh              # generate feeds, then validate
#   FEED_DIR=out scripts/validate-feeds.sh # validate feeds already in ./out
#   scripts/validate-feeds.sh --offline-fixtures --as-of 2026-08-13
#
set -euo pipefail

GTFS_VALIDATOR_VERSION="8.0.1"
GTFS_VALIDATOR_SHA256="19293ddd9b6f954f216d4f12054bd8a3232921751c4484339e339764a91000e2"
# Pinned so the gate is reproducible; this project publishes no releases.
RT_VALIDATOR_COMMIT="7041fa3fcaf674bf730e17325c179d329cdff6f2"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CACHE_DIR="${VALIDATOR_CACHE_DIR:-.validator-cache}"
FEED_DIR="${FEED_DIR:-out}"
REPORT_DIR="${REPORT_DIR:-validation-reports}"
SERVICE_URL="${SERVICE_URL:-}"
VALIDATION_BIND_ADDR="${VALIDATION_BIND_ADDR:-127.0.0.1:18080}"
BASELINE="${BASELINE:-validation/baseline.json}"
AS_OF="${VALIDATION_AS_OF:-$(date -u +%F)}"
MODE="live"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --offline-fixtures) MODE="offline-fixtures"; shift ;;
    --as-of)
      [ "$#" -ge 2 ] || { echo "--as-of requires YYYY-MM-DD" >&2; exit 2; }
      AS_OF="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

GTFS_VALIDATOR_JAR="$CACHE_DIR/gtfs-validator-${GTFS_VALIDATOR_VERSION}.jar"
RT_VALIDATOR_SRC="$CACHE_DIR/rt-validator-src"
RT_VALIDATOR_JAR="$CACHE_DIR/gtfs-realtime-validator-${RT_VALIDATOR_COMMIT:0:7}.jar"

log()  { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAIL: %s\033[0m\n' "$*" >&2; exit 1; }

command -v jq >/dev/null || fail "jq is required"

# ---------------------------------------------------------------------------
# Pure offline ratchet
#
# The baseline is data, not a bypass: every realtime exception is owned,
# explains its upstream cause, and expires after review_on. Static GTFS remains
# zero-tolerance. This path is deliberately defined and dispatched before any
# Java probe, feed generation, download, or live Amtrak request.
# ---------------------------------------------------------------------------
ratchet() {
  local baseline_file="$1" observed_file="$2" as_of="$3" result
  if ! result=$(jq -e --arg as_of "$as_of" --slurpfile observed "$observed_file" '
    def nonempty: type == "string" and length > 0;
    def valid_date:
      . as $date
      | type == "string"
      and test("^[0-9]{4}-[0-9]{2}-[0-9]{2}$")
      and (try (($date + "T00:00:00Z" | fromdateiso8601 | strftime("%Y-%m-%d")) == $date) catch false);
    def valid_record:
      type == "object"
      and (.code | nonempty)
      and (.upstream_cause | nonempty)
      and (.owner | nonempty)
      and (.review_on | valid_date);
    def unique_codes: map(.code) | length == (unique | length);

    if ($as_of | valid_date) | not then error("as-of date must be valid YYYY-MM-DD")
    elif (.static.allowed_errors | type) != "array" then error("static.allowed_errors must be an array")
    elif (.realtime.allowed_errors | type) != "array" then error("realtime.allowed_errors must be an array")
    elif (.static.allowed_errors | length) != 0 then error("static ERROR exceptions are forbidden")
    elif (.realtime.allowed_errors | all(valid_record)) | not then error("malformed realtime exception record")
    elif (.realtime.allowed_errors | unique_codes) | not then error("duplicate realtime exception code")
    elif any(.realtime.allowed_errors[]; .review_on < $as_of) then error("expired realtime exception record")
    elif ($observed | length) != 1
      or (($observed[0].static_error_codes | type) != "array")
      or (($observed[0].realtime_error_codes | type) != "array")
      or (($observed[0].static_error_codes + $observed[0].realtime_error_codes | all(nonempty)) | not)
      then error("malformed observed-error fixture")
    else
      (.realtime.allowed_errors | map(.code) | unique) as $allowed
      | ($observed[0].static_error_codes | unique) as $static
      | ($observed[0].realtime_error_codes | unique) as $realtime
      | {
          new_static: $static,
          new_realtime: ($realtime - $allowed),
          stale_realtime: ($allowed - $realtime)
        }
    end
  ' "$baseline_file" 2>&1); then
    printf 'Invalid validator baseline/fixture: %s\n' "$result" >&2
    return 2
  fi

  printf '%s\n' "$result"
  if [ "$(jq '.new_static | length' <<<"$result")" -ne 0 ] \
    || [ "$(jq '.new_realtime | length' <<<"$result")" -ne 0 ]; then
    return 1
  fi
}

offline_fixtures() {
  local fixture_dir observed fixture_baseline fixture_output rc failures=0
  fixture_dir=$(mktemp -d "${TMPDIR:-/tmp}/amtrak-ratchet.XXXXXX")
  trap 'rm -rf "$fixture_dir"' RETURN

  for fixture in accepted missing-field expired new-error static-error resolved-error; do
    fixture_baseline="$fixture_dir/$fixture-baseline.json"
    observed="$fixture_dir/$fixture-observed.json"
    cp "$BASELINE" "$fixture_baseline"
    case "$fixture" in
      accepted)
        jq -n '{static_error_codes:[], realtime_error_codes:["E003"]}' >"$observed"
        expected=0 ;;
      missing-field)
        jq 'del(.realtime.allowed_errors[0].owner)' "$BASELINE" >"$fixture_baseline"
        jq -n '{static_error_codes:[], realtime_error_codes:["E003"]}' >"$observed"
        expected=2 ;;
      expired)
        jq '.realtime.allowed_errors[0].review_on="2020-01-01"' "$BASELINE" >"$fixture_baseline"
        jq -n '{static_error_codes:[], realtime_error_codes:["E003"]}' >"$observed"
        expected=2 ;;
      new-error)
        jq -n '{static_error_codes:[], realtime_error_codes:["E999"]}' >"$observed"
        expected=1 ;;
      static-error)
        jq -n '{static_error_codes:["missing_required_field"], realtime_error_codes:[]}' >"$observed"
        expected=1 ;;
      resolved-error)
        jq -n '{static_error_codes:[], realtime_error_codes:[]}' >"$observed"
        expected=0 ;;
    esac
    if fixture_output=$(ratchet "$fixture_baseline" "$observed" "$AS_OF"); then rc=0; else rc=$?; fi
    if [ "$fixture" = "resolved-error" ] \
      && ! jq -e '.stale_realtime | index("E003") != null' <<<"$fixture_output" >/dev/null; then
      printf 'offline fixture resolved-error: did not surface stale E003\n' >&2
      failures=1
      continue
    fi
    if [ "$rc" -ne "$expected" ]; then
      printf 'offline fixture %s: expected %s, got %s\n' "$fixture" "$expected" "$rc" >&2
      failures=1
    else
      printf 'offline fixture %s: pass\n' "$fixture"
    fi
  done
  return "$failures"
}

if [ "$MODE" = "offline-fixtures" ]; then
  [ -f "$BASELINE" ] || fail "baseline file not found: $BASELINE"
  offline_fixtures
  exit $?
fi

# Fail on baseline schema/expiry before probing Java or touching live inputs.
[ -f "$BASELINE" ] || fail "baseline file not found: $BASELINE"
preflight_observed=$(mktemp "${TMPDIR:-/tmp}/amtrak-ratchet-preflight.XXXXXX")
jq -n '{static_error_codes:[], realtime_error_codes:[]}' >"$preflight_observed"
if ! ratchet "$BASELINE" "$preflight_observed" "$AS_OF" >/dev/null; then
  rm -f "$preflight_observed"
  fail "validator baseline is malformed or has expired exceptions"
fi
rm -f "$preflight_observed"

# ---------------------------------------------------------------------------
# Toolchain: prefer a local Java 17+/Maven, fall back to Docker.
# Both validators need Java 17 (the RT validator's README claiming 11 is stale).
# ---------------------------------------------------------------------------
java_major() {
  command -v java >/dev/null || { echo 0; return; }
  java -version 2>&1 | head -1 | sed -E 's/.*version "([0-9]+).*/\1/' || echo 0
}

USE_DOCKER=0
if [ "$(java_major)" -ge 17 ] && command -v mvn >/dev/null; then
  USE_DOCKER=0
elif docker info >/dev/null 2>&1; then
  USE_DOCKER=1
  echo "Java 17+/Maven not found locally; using Docker."
else
  fail "need Java 17 + Maven, or a running Docker daemon"
fi

# Paths passed to these helpers must be relative to the repo root so they
# resolve identically on the host and inside the container.
run_java() {
  if [ "$USE_DOCKER" -eq 1 ]; then
    docker run --rm -v "$ROOT:/w" -w /w eclipse-temurin:17-jre java "$@"
  else
    java "$@"
  fi
}

run_mvn_package() {
  if [ "$USE_DOCKER" -eq 1 ]; then
    docker run --rm -v "$ROOT:/w" -w "/w/$RT_VALIDATOR_SRC" \
      maven:3.9-eclipse-temurin-17 \
      mvn -q -B package -DskipTests -Dmaven.repo.local="/w/$CACHE_DIR/.m2"
  else
    (cd "$RT_VALIDATOR_SRC" && mvn -q -B package -DskipTests)
  fi
}

verify_gtfs_validator() {
  local actual
  command -v shasum >/dev/null || fail "shasum is required to verify gtfs-validator"
  actual=$(shasum -a 256 "$GTFS_VALIDATOR_JAR" | awk '{print $1}') \
    || fail "could not hash gtfs-validator"
  [ "$actual" = "$GTFS_VALIDATOR_SHA256" ] \
    || fail "gtfs-validator ${GTFS_VALIDATOR_VERSION} SHA-256 mismatch"
}

mkdir -p "$CACHE_DIR" "$REPORT_DIR"

# ---------------------------------------------------------------------------
# 1. Validator tooling
# ---------------------------------------------------------------------------
if [ ! -f "$GTFS_VALIDATOR_JAR" ]; then
  log "Downloading gtfs-validator $GTFS_VALIDATOR_VERSION"
  curl -sSfL -o "$GTFS_VALIDATOR_JAR" \
    "https://github.com/MobilityData/gtfs-validator/releases/download/v${GTFS_VALIDATOR_VERSION}/gtfs-validator-${GTFS_VALIDATOR_VERSION}-cli.jar"
fi
verify_gtfs_validator

if [ ! -f "$RT_VALIDATOR_JAR" ]; then
  log "Building gtfs-realtime-validator @ ${RT_VALIDATOR_COMMIT:0:7} (no releases published upstream)"
  if [ ! -d "$RT_VALIDATOR_SRC/.git" ]; then
    rm -rf "$RT_VALIDATOR_SRC"
    git clone --quiet https://github.com/MobilityData/gtfs-realtime-validator.git "$RT_VALIDATOR_SRC"
  fi
  git -C "$RT_VALIDATOR_SRC" fetch --quiet origin
  git -C "$RT_VALIDATOR_SRC" checkout --quiet "$RT_VALIDATOR_COMMIT"
  run_mvn_package
  built=$(find "$RT_VALIDATOR_SRC" -name '*withAllDependencies.jar' -path '*target*' | head -1)
  [ -n "$built" ] || fail "RT validator build produced no jar"
  cp "$built" "$RT_VALIDATOR_JAR"
fi

# ---------------------------------------------------------------------------
# 2. One manifest-pinned feed set
#
# The release gate never discovers mutable filenames in the output directory.
# It fetches one manifest, proves all four URLs name that exact generation, then
# downloads exactly those artifacts for every subsequent validator/decoder.
# SERVICE_URL may point at an already-running controlled candidate; otherwise
# the script starts the candidate locally and waits for its first generation.
# ---------------------------------------------------------------------------
INPUT_DIR="$REPORT_DIR/feed-set-artifacts"
MANIFEST="$REPORT_DIR/feed-set.json"
rm -rf "$INPUT_DIR"
mkdir -p "$INPUT_DIR" "$FEED_DIR"
rm -f "$MANIFEST"

SERVICE_PID=""
if [ -z "$SERVICE_URL" ]; then
  log "Starting release candidate for manifest-first discovery"
  cargo build --release
  SERVICE_URL="http://$VALIDATION_BIND_ADDR"
  if [ "$USE_DOCKER" -eq 1 ]; then
    JAVA_SHIM_DIR="$CACHE_DIR/docker-java-bin"
    mkdir -p "$JAVA_SHIM_DIR"
    cp scripts/validation-docker-java "$JAVA_SHIM_DIR/java"
    chmod 700 "$JAVA_SHIM_DIR/java"
    PATH="$ROOT/$JAVA_SHIM_DIR:$PATH"
    export PATH VALIDATION_REPO_ROOT="$ROOT"
  fi
  AMTRAK_OUTPUT_DIR="$FEED_DIR" \
    AMTRAK_POLL_SECS=30 \
    AMTRAK_BIND_ADDR="$VALIDATION_BIND_ADDR" \
    AMTRAK_GTFS_VALIDATOR_JAR="$GTFS_VALIDATOR_JAR" \
    ./target/release/amtrak-gtfs-rt-service > "$REPORT_DIR/service.log" 2>&1 &
  SERVICE_PID=$!
  # shellcheck disable=SC2064
  trap "kill $SERVICE_PID 2>/dev/null || true" EXIT
fi

for _ in $(seq 1 180); do
  if curl -fsS "$SERVICE_URL/v1/feed-set.json" -o "$MANIFEST"; then break; fi
  if [ -n "$SERVICE_PID" ] && ! kill -0 "$SERVICE_PID" 2>/dev/null; then break; fi
  sleep 1
done
if [ ! -s "$MANIFEST" ]; then
  [ -z "$SERVICE_PID" ] || cat "$REPORT_DIR/service.log" >&2 || true
  fail "could not fetch a feed-set manifest (candidate or Amtrak upstream unavailable)"
fi

generation_id=$(jq -er '.generation_id | select(type == "string" and test("^[0-9]+-[0-9]+$"))' "$MANIFEST") \
  || fail "feed-set manifest has an invalid generation_id"
for artifact in static_zip trip_updates vehicle_positions alerts; do
  url=$(jq -er --arg artifact "$artifact" '.urls[$artifact] | select(type == "string")' "$MANIFEST") \
    || fail "feed-set manifest is missing $artifact URL"
  expected_prefix="/v1/generations/$generation_id/"
  case "$url" in
    "$expected_prefix"*) ;;
    *) fail "feed-set URL does not name manifest generation: $artifact" ;;
  esac
  case "$artifact:$url" in
    "static_zip:${expected_prefix}static.zip") output="static.zip" ;;
    "trip_updates:${expected_prefix}trip-updates.pb") output="trip-updates.pb" ;;
    "vehicle_positions:${expected_prefix}vehicle-positions.pb") output="vehicle-positions.pb" ;;
    "alerts:${expected_prefix}alerts.pb") output="alerts.pb" ;;
    *) fail "feed-set URL has an unexpected artifact name: $artifact" ;;
  esac
  curl -fsS "$SERVICE_URL$url" -o "$INPUT_DIR/$output" \
    || fail "could not fetch manifest-pinned artifact: $artifact"
done

if [ -n "$SERVICE_PID" ]; then
  kill "$SERVICE_PID" 2>/dev/null || true
  wait "$SERVICE_PID" 2>/dev/null || true
  SERVICE_PID=""
  trap - EXIT
fi
log "Manifest-pinned generation under validation: $generation_id"
ls -lh "$MANIFEST" "$INPUT_DIR"/*.pb "$INPUT_DIR/static.zip"

# ---------------------------------------------------------------------------
# 3. Static GTFS
# ---------------------------------------------------------------------------
log "Validating static GTFS"
rm -rf "$REPORT_DIR/static"
verify_gtfs_validator
run_java -jar "$GTFS_VALIDATOR_JAR" -i "$INPUT_DIR/static.zip" -o "$REPORT_DIR/static" >/dev/null 2>&1 \
  || fail "gtfs-validator did not run to completion"
[ -f "$REPORT_DIR/static/report.json" ] || fail "gtfs-validator produced no report.json"

static_errors=$(jq -r '[.notices[] | select(.severity=="ERROR") | .code] | unique | join("\n")' \
  "$REPORT_DIR/static/report.json")
echo "Static notices by severity:"
jq -r '.notices | group_by(.severity)[] | "  \(.[0].severity): \(map(.totalNotices)|add) across \(length) code(s)"' \
  "$REPORT_DIR/static/report.json"

# ---------------------------------------------------------------------------
# 4. GTFS-Realtime
#
# Each feed type is validated in its own directory: the batch processor treats
# every .pb file in a directory as a successive iteration of ONE feed, so mixing
# feed types would produce spurious cross-iteration errors.
# ---------------------------------------------------------------------------
log "Validating GTFS-Realtime feeds"
rt_codes=""
for feed in trip-updates vehicle-positions alerts; do
  work="$REPORT_DIR/rt/$feed"
  rm -rf "$work"; mkdir -p "$work"
  cp "$INPUT_DIR/$feed.pb" "$work/"
  run_java -jar "$RT_VALIDATOR_JAR" -gtfs "$INPUT_DIR/static.zip" -gtfsRealtimePath "$work" >/dev/null 2>&1 \
    || fail "gtfs-realtime-validator did not run to completion on $feed"
  results="$work/$feed.pb.results.json"
  [ -f "$results" ] || fail "no results.json produced for $feed"

  echo "  $feed:"
  jq -r '.[] | "    \(.errorMessage.validationRule.severity) \(.errorMessage.validationRule.errorId) x\(.occurrenceList|length) \(.errorMessage.validationRule.title)"' \
    "$results" | sort
  codes=$(jq -r '.[] | select(.errorMessage.validationRule.severity=="ERROR") | .errorMessage.validationRule.errorId' "$results")
  rt_codes="$rt_codes$codes"$'\n'
done

# ---------------------------------------------------------------------------
# 5. Ratchet against the baseline
# ---------------------------------------------------------------------------
log "Comparing against $BASELINE"
[ -f "$BASELINE" ] || fail "baseline file not found: $BASELINE"

observed_errors="$REPORT_DIR/observed-errors.json"
jq -n \
  --arg static "$static_errors" \
  --arg realtime "$rt_codes" \
  '{
    static_error_codes: ($static | split("\n") | map(select(length > 0)) | unique),
    realtime_error_codes: ($realtime | split("\n") | map(select(length > 0)) | unique)
  }' >"$observed_errors"

jq -n \
  --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg generation_id "$generation_id" \
  --arg gtfs_validator "$GTFS_VALIDATOR_VERSION" \
  --arg rt_validator "$RT_VALIDATOR_COMMIT" \
  '{
    generated_at: $generated_at,
    generation_id: $generation_id,
    gtfs_validator: $gtfs_validator,
    gtfs_realtime_validator_commit: $rt_validator,
    manifest: "feed-set.json",
    artifacts: ["feed-set-artifacts/static.zip", "feed-set-artifacts/trip-updates.pb", "feed-set-artifacts/vehicle-positions.pb", "feed-set-artifacts/alerts.pb"],
    independent_decode: {
      implementation: "MobilityData GTFS-Realtime validator Java protobuf runtime",
      feeds: ["trip-updates", "vehicle-positions", "alerts"],
      result_files: ["rt/trip-updates/trip-updates.pb.results.json", "rt/vehicle-positions/vehicle-positions.pb.results.json", "rt/alerts/alerts.pb.results.json"]
    }
  }' >"$REPORT_DIR/release-evidence.json"

if ratchet_result=$(ratchet "$BASELINE" "$observed_errors" "$AS_OF"); then
  status=0
else
  status=$?
fi
[ "$status" -ne 2 ] || fail "validator baseline or observed errors are malformed/expired"

new_static=$(jq -r '.new_static[]' <<<"$ratchet_result")
new_rt=$(jq -r '.new_realtime[]' <<<"$ratchet_result")
stale_rt=$(jq -r '.stale_realtime[]' <<<"$ratchet_result")

if [ -n "$new_static" ]; then
  echo
  printf '\033[31mStatic ERROR code(s) are forbidden:\033[0m\n'
  echo "$new_static" | sed 's/^/  /'
fi
if [ -n "$new_rt" ]; then
  echo
  printf '\033[31mNew realtime ERROR code(s) not in baseline:\033[0m\n'
  echo "$new_rt" | sed 's/^/  /'
fi

# Surface baseline entries that no longer occur, so the ratchet can be tightened.
if [ -n "$stale_rt" ]; then
  echo
  echo "Baseline codes no longer reported (consider removing from $BASELINE):"
  echo "$stale_rt" | sed 's/^/  /'
fi

echo
if [ "$status" -eq 0 ]; then
  printf '\033[32mPASS: no new validator ERROR codes.\033[0m Reports in %s/\n' "$REPORT_DIR"
else
  printf '\033[31mFAIL: validation regressed.\033[0m Reports in %s/\n' "$REPORT_DIR"
fi
exit "$status"
