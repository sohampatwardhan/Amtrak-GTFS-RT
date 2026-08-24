#!/usr/bin/env bash

set -euo pipefail

# Deterministic contract tests for the release-control helpers. All Git and JSON
# fixtures are local so CI does not need network access or release credentials.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONTEXT="$ROOT/scripts/release-context.sh"
POLICY="$ROOT/scripts/release-vulnerability-policy.sh"
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

printf 'release-control tests passed\n'
