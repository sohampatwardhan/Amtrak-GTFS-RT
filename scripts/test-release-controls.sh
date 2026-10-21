#!/usr/bin/env bash

set -euo pipefail

# Deterministic contract tests for the release-control helpers. All Git and JSON
# fixtures are local so CI does not need network access or release credentials.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTEXT="$ROOT/scripts/release-context.sh"
POLICY="$ROOT/scripts/release-vulnerability-policy.sh"
ASSEMBLE="$ROOT/scripts/assemble-release-manifest.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
expect_fail() { "$@" >/dev/null 2>&1 && fail "command unexpectedly succeeded: $*"; return 0; }
assert_line() { grep -Fqx "$2" "$1" || fail "missing line '$2' in $1"; }

select_out="$TMP/select.out"
"$CONTEXT" select push v1.2.3 '' refs/tags/v1.2.3 main "$select_out"
assert_line "$select_out" 'tag=v1.2.3'
assert_line "$select_out" 'version=1.2.3'

: >"$select_out"
"$CONTEXT" select workflow_dispatch '' v2.0.0 refs/heads/main main "$select_out"
assert_line "$select_out" 'tag=v2.0.0'
expect_fail "$CONTEXT" select workflow_dispatch '' v2.0.0 refs/heads/release main "$select_out"
expect_fail "$CONTEXT" select workflow_dispatch '' v2.0.0 refs/heads/release release "$select_out"
expect_fail "$CONTEXT" select workflow_dispatch '' '' refs/heads/main main "$select_out"
expect_fail "$CONTEXT" select push latest '' refs/tags/latest main "$select_out"
expect_fail "$CONTEXT" select schedule v1.2.3 '' refs/heads/main main "$select_out"

repo="$TMP/repo"
git init -q "$repo"
git -C "$repo" config user.email test@example.invalid
git -C "$repo" config user.name 'Release Controls Test'
mkdir -p "$repo/scripts"
printf 'ok\n' >"$repo/payload"
# The fixture script must evaluate dirname in the temporary repository, not here.
# shellcheck disable=SC2016
printf '#!/usr/bin/env bash\nset -euo pipefail\ntest ! -e "$(dirname "$0")/../metadata-fails"\n' >"$repo/scripts/check-release-metadata.sh"
chmod +x "$repo/scripts/check-release-metadata.sh"
git -C "$repo" add .
git -C "$repo" commit -qm initial
git -C "$repo" tag v1.2.3
tag_sha="$(git -C "$repo" rev-parse 'v1.2.3^{commit}')"

verify_out="$TMP/verify.out"
"$CONTEXT" verify v1.2.3 "$repo" "$verify_out"
assert_line "$verify_out" "revision=$tag_sha"
test "${#tag_sha}" -eq 40 || fail 'fixture SHA is not full length'

printf 'new\n' >>"$repo/payload"
git -C "$repo" commit -qam second
expect_fail "$CONTEXT" verify v1.2.3 "$repo" "$verify_out"
expect_fail "$CONTEXT" verify v9.9.9 "$repo" "$verify_out"
git -C "$repo" checkout -q --detach v1.2.3
touch "$repo/metadata-fails"
expect_fail "$CONTEXT" verify v1.2.3 "$repo" "$verify_out"
rm "$repo/metadata-fails"

clean="$TMP/clean.json"
vulnerable="$TMP/vulnerable.json"
malformed="$TMP/malformed.json"
printf '{"matches":[]}\n' >"$clean"
printf '%s\n' '{"matches":[{"vulnerability":{"id":"CVE-2026-0001","severity":"High","fix":{"versions":[]}},"artifact":{"name":"libdemo","version":"1.0.0"}}]}' >"$vulnerable"
printf '%s\n' '{"matches":[{"vulnerability":{"id":"CVE-2026-0001","severity":3,"fix":{"versions":[""]}},"artifact":{"name":"","version":null}}]}' >"$malformed"

summary="$TMP/summary.txt"
"$POLICY" summarize "$clean" "$summary"
test "$(cat "$summary")" = $'ID\tSEVERITY\tARTIFACT\tINSTALLED\tFIXED' || fail 'clean summary changed'
"$POLICY" enforce "$clean"
"$POLICY" summarize "$vulnerable" "$summary"
assert_line "$summary" $'CVE-2026-0001\tHigh\tlibdemo\t1.0.0\tnone available'
expect_fail "$POLICY" enforce "$vulnerable"
expect_fail "$POLICY" summarize "$malformed" "$summary"
expect_fail "$POLICY" enforce "$TMP/missing.json"

unfixed_high="$TMP/unfixed-high.json"
fixed_medium="$TMP/fixed-medium.json"
fixed_high="$TMP/fixed-high.json"
fixed_critical="$TMP/fixed-critical.json"
printf '%s\n' '{"matches":[{"vulnerability":{"id":"CVE-2026-0002","severity":"Medium","fix":{"versions":["2.0.0"]}},"artifact":{"name":"libdemo","version":"1.0.0"}}]}' >"$fixed_medium"
printf '%s\n' '{"matches":[{"vulnerability":{"id":"CVE-2026-0004","severity":"High","fix":{"versions":["1.2.3"]}},"artifact":{"name":"libdemo","version":"1.0.0"}}]}' >"$fixed_high"
printf '%s\n' '{"matches":[{"vulnerability":{"id":"CVE-2026-0003","severity":"Critical","fix":{"versions":["2.0.0"]}},"artifact":{"name":"libdemo","version":"1.0.0"}}]}' >"$fixed_critical"
cp "$vulnerable" "$unfixed_high"
"$POLICY" enforce-fixable "$clean"
"$POLICY" enforce-fixable "$unfixed_high"
"$POLICY" enforce-fixable "$fixed_medium"
expect_fail "$POLICY" enforce-fixable "$fixed_high"
expect_fail "$POLICY" enforce-fixable "$fixed_critical"
expect_fail "$POLICY" enforce-fixable "$malformed"
expect_fail "$POLICY" enforce-fixable "$TMP/missing.json"

valid_sbom="$TMP/valid.spdx.json"
empty_sbom="$TMP/empty.spdx.json"
bad_sbom="$TMP/bad.spdx.json"
printf '%s\n' '{"spdxVersion":"SPDX-2.3","packages":[{"name":"demo"}]}' >"$valid_sbom"
printf '%s\n' '{"spdxVersion":"SPDX-2.3","packages":[]}' >"$empty_sbom"
printf '%s\n' '[]' >"$bad_sbom"
"$POLICY" validate-sbom "$valid_sbom"
expect_fail "$POLICY" validate-sbom "$empty_sbom"
expect_fail "$POLICY" validate-sbom "$bad_sbom"

digest="sha256:$(printf 'a%.0s' {1..64})"
digest_file="$TMP/digest.txt"
"$POLICY" write-digest "$digest" "$digest_file"
assert_line "$digest_file" "$digest"
test "$(wc -l <"$digest_file" | tr -d ' ')" = 1 || fail 'digest record must have one line'
expect_fail "$POLICY" write-digest "${digest%A}A" "$digest_file"
expect_fail "$POLICY" write-digest sha256:abc "$digest_file"

# assemble-release-manifest.sh: retry a silent inspect failure, create only when
# the version tag is absent, and accept an existing tag whose digests match.
export RELEASE_MANIFEST_INSPECT_SLEEP=0
AMD64="sha256:$(printf '1%.0s' {1..64})"
ARM64="sha256:$(printf '2%.0s' {1..64})"
INDEX="sha256:$(printf '3%.0s' {1..64})"
OTHER="sha256:$(printf '4%.0s' {1..64})"
printf '%s\n' "$AMD64" >"$TMP/amd64"
printf '%s\n' "$ARM64" >"$TMP/arm64"
mkdir -p "$TMP/bin" "$TMP/docker-state"
export DOCKER_LOG="$TMP/docker.log"
export DOCKER_STATE="$TMP/docker-state"
export PATH="$TMP/bin:$PATH"
cat >"$TMP/bin/docker" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$DOCKER_LOG"
if [ "$1 $2 $3" = "buildx imagetools create" ]; then
  printf 'create\n' >>"$DOCKER_STATE/events"
  exit 0
fi
if [ "$1 $2 $3" != "buildx imagetools inspect" ]; then
  printf 'unexpected docker command: %s\n' "$*" >&2
  exit 99
fi
ref="$4"
raw=0
if [ "${5:-}" = --raw ]; then
  raw=1
fi
n=0
if [ -f "$DOCKER_STATE/inspects" ]; then
  n="$(wc -l <"$DOCKER_STATE/inspects" | tr -d ' ')"
fi
printf '%s %s\n' "$ref" "$raw" >>"$DOCKER_STATE/inspects"
mode="$(cat "$DOCKER_STATE/mode")"
case "$mode" in
  retry255)
    [ "$n" -ge 2 ] || exit 255
    ;;
  always255) exit 255 ;;
  absent-then-ok)
    if [ "$n" -lt 1 ]; then
      printf 'manifest unknown\n' >&2
      exit 1
    fi
    ;;
  absent-then-255)
    if [ "$n" -lt 1 ]; then
      printf 'manifest unknown\n' >&2
      exit 1
    fi
    [ "$n" -ge 3 ] || exit 255
    ;;
  denied-then-ok)
    if [ "$n" -lt 2 ]; then
      printf 'requested access to the resource is denied\n' >&2
      exit 1
    fi
    ;;
  mismatch | latest-drift) ;;
  *)
    printf 'bad mode %s\n' "$mode" >&2
    exit 99
    ;;
esac
if [ "$raw" -eq 1 ]; then
  if [ "$mode" = mismatch ]; then
    cat "$DOCKER_STATE/bad.json"
  else
    cat "$DOCKER_STATE/good.json"
  fi
  exit 0
fi
digest="$(cat "$DOCKER_STATE/index")"
if [ "$mode" = latest-drift ] && [[ "$ref" == *:latest ]]; then
  digest="$(cat "$DOCKER_STATE/other")"
fi
printf 'Digest: %s\n' "$digest"
EOF
chmod +x "$TMP/bin/docker"
printf '{"manifests":[{"digest":"%s","platform":{"os":"linux","architecture":"amd64"}},{"digest":"%s","platform":{"os":"linux","architecture":"arm64"}}]}\n' \
  "$AMD64" "$ARM64" >"$DOCKER_STATE/good.json"
printf '{"manifests":[{"digest":"%s","platform":{"os":"linux","architecture":"amd64"}},{"digest":"%s","platform":{"os":"linux","architecture":"arm64"}}]}\n' \
  "$OTHER" "$ARM64" >"$DOCKER_STATE/bad.json"
printf '%s\n' "$INDEX" >"$DOCKER_STATE/index"
printf '%s\n' "$OTHER" >"$DOCKER_STATE/other"

reset_docker() {
  : >"$DOCKER_LOG"
  : >"$DOCKER_STATE/events"
  rm -f "$DOCKER_STATE/inspects"
  printf '%s\n' "$1" >"$DOCKER_STATE/mode"
}

create_count() {
  if [ -s "$DOCKER_STATE/events" ]; then
    grep -c '^create$' "$DOCKER_STATE/events" || true
  else
    printf '0\n'
  fi
}

run_assemble() {
  : >"$TMP/github-output"
  : >"$TMP/assemble.out"
  : >"$TMP/assemble.err"
  GITHUB_OUTPUT="$TMP/github-output" "$ASSEMBLE" "ghcr.io/example/app" 1.2.3 "$TMP/amd64" "$TMP/arm64" digest \
    >"$TMP/assemble.out" 2>"$TMP/assemble.err"
}

reset_docker retry255
run_assemble
assert_line "$TMP/github-output" "digest=$INDEX"
test "$(create_count)" = 0 || fail 'retry of a matching tag must not create'
grep -q 'with no message' "$TMP/assemble.err" || fail 'silent inspect failure was not reported'

reset_docker absent-then-ok
run_assemble
assert_line "$TMP/github-output" "digest=$INDEX"
test "$(create_count)" = 1 || fail 'absent tag was not created once'

reset_docker absent-then-255
run_assemble
assert_line "$TMP/github-output" "digest=$INDEX"
test "$(create_count)" = 1 || fail 'post-create inspect retries must not create again'

reset_docker denied-then-ok
run_assemble
assert_line "$TMP/github-output" "digest=$INDEX"
test "$(create_count)" = 0 || fail 'denied must be retried, not treated as absent'

reset_docker mismatch
set +e
run_assemble
mismatch_status=$?
set -e
test "$mismatch_status" -ne 0 || fail 'mismatch must fail'
grep -q 'refusing to overwrite it' "$TMP/assemble.err" || fail 'mismatch must refuse'
test "$(create_count)" = 0 || fail 'mismatch must not create'

reset_docker latest-drift
set +e
run_assemble
drift_status=$?
set -e
test "$drift_status" -ne 0 || fail 'latest drift must fail'
grep -q ':latest resolved to' "$TMP/assemble.err" || fail 'drifting latest tag must fail'
test "$(create_count)" = 0 || fail 'latest drift must not create'

reset_docker always255
set +e
run_assemble
always_status=$?
set -e
test "$always_status" -eq 255 || fail "persistent silent inspect should exit 255, got $always_status"
grep -q 'with no message' "$TMP/assemble.err" || fail 'persistent silent inspect should say it had no message'
test "$(create_count)" = 0 || fail 'unreadable tag must not be created'

printf 'release-control tests passed\n'
