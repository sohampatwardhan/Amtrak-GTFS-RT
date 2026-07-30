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
#
set -euo pipefail

GTFS_VALIDATOR_VERSION="8.0.1"
# Pinned so the gate is reproducible; this project publishes no releases.
RT_VALIDATOR_COMMIT="7041fa3fcaf674bf730e17325c179d329cdff6f2"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CACHE_DIR="${VALIDATOR_CACHE_DIR:-.validator-cache}"
FEED_DIR="${FEED_DIR:-out}"
REPORT_DIR="${REPORT_DIR:-validation-reports}"
BASELINE="${BASELINE:-validation/baseline.json}"

GTFS_VALIDATOR_JAR="$CACHE_DIR/gtfs-validator-${GTFS_VALIDATOR_VERSION}.jar"
RT_VALIDATOR_SRC="$CACHE_DIR/rt-validator-src"
RT_VALIDATOR_JAR="$CACHE_DIR/gtfs-realtime-validator-${RT_VALIDATOR_COMMIT:0:7}.jar"

log()  { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
fail() { printf '\033[31mFAIL: %s\033[0m\n' "$*" >&2; exit 1; }

command -v jq >/dev/null || fail "jq is required"

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

mkdir -p "$CACHE_DIR" "$REPORT_DIR"

# ---------------------------------------------------------------------------
# 1. Feeds
# ---------------------------------------------------------------------------
if [ ! -f "$FEED_DIR/static.zip" ] || [ ! -f "$FEED_DIR/trip-updates.pb" ]; then
  log "Generating feeds into $FEED_DIR (live Amtrak fetch)"
  cargo build --release
  mkdir -p "$FEED_DIR"
  AMTRAK_OUTPUT_DIR="$FEED_DIR" AMTRAK_POLL_SECS=30 AMTRAK_BIND_ADDR=127.0.0.1:0 \
    ./target/release/amtrak-gtfs-rt-service > "$FEED_DIR/service.log" 2>&1 &
  SERVICE_PID=$!
  # shellcheck disable=SC2064
  trap "kill $SERVICE_PID 2>/dev/null || true" EXIT
  for _ in $(seq 1 90); do
    [ -f "$FEED_DIR/trip-updates.pb" ] && [ -f "$FEED_DIR/vehicle-positions.pb" ] \
      && [ -f "$FEED_DIR/alerts.pb" ] && [ -f "$FEED_DIR/static.zip" ] && break
    kill -0 "$SERVICE_PID" 2>/dev/null || break
    sleep 1
  done
  kill "$SERVICE_PID" 2>/dev/null || true
  wait "$SERVICE_PID" 2>/dev/null || true
  trap - EXIT
  if [ ! -f "$FEED_DIR/trip-updates.pb" ]; then
    cat "$FEED_DIR/service.log" >&2 || true
    fail "could not generate feeds (Amtrak upstream may be unreachable)"
  fi
fi
log "Feeds under validation"
ls -lh "$FEED_DIR"/*.pb "$FEED_DIR"/static.zip

# ---------------------------------------------------------------------------
# 2. Validator tooling
# ---------------------------------------------------------------------------
if [ ! -f "$GTFS_VALIDATOR_JAR" ]; then
  log "Downloading gtfs-validator $GTFS_VALIDATOR_VERSION"
  curl -sSfL -o "$GTFS_VALIDATOR_JAR" \
    "https://github.com/MobilityData/gtfs-validator/releases/download/v${GTFS_VALIDATOR_VERSION}/gtfs-validator-${GTFS_VALIDATOR_VERSION}-cli.jar"
fi

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
# 3. Static GTFS
# ---------------------------------------------------------------------------
log "Validating static GTFS"
rm -rf "$REPORT_DIR/static"
run_java -jar "$GTFS_VALIDATOR_JAR" -i "$FEED_DIR/static.zip" -o "$REPORT_DIR/static" >/dev/null 2>&1 \
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
  cp "$FEED_DIR/$feed.pb" "$work/"
  run_java -jar "$RT_VALIDATOR_JAR" -gtfs "$FEED_DIR/static.zip" -gtfsRealtimePath "$work" >/dev/null 2>&1 \
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

allowed_static=$(jq -r '.static.allowed_error_codes | keys[]?' "$BASELINE")
allowed_rt=$(jq -r '.realtime.allowed_error_codes | keys[]?' "$BASELINE")

new_static=$(comm -23 <(echo "$static_errors" | grep -v '^$' | sort -u) <(echo "$allowed_static" | grep -v '^$' | sort -u) || true)
new_rt=$(comm -23 <(echo "$rt_codes" | grep -v '^$' | sort -u) <(echo "$allowed_rt" | grep -v '^$' | sort -u) || true)

status=0
if [ -n "$new_static" ]; then
  echo
  printf '\033[31mNew static ERROR code(s) not in baseline:\033[0m\n'
  echo "$new_static" | sed 's/^/  /'
  status=1
fi
if [ -n "$new_rt" ]; then
  echo
  printf '\033[31mNew realtime ERROR code(s) not in baseline:\033[0m\n'
  echo "$new_rt" | sed 's/^/  /'
  status=1
fi

# Surface baseline entries that no longer occur, so the ratchet can be tightened.
stale_rt=$(comm -13 <(echo "$rt_codes" | grep -v '^$' | sort -u) <(echo "$allowed_rt" | grep -v '^$' | sort -u) || true)
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
