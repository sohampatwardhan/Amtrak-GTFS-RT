#!/usr/bin/env bash
# Apply the repository rulesets in .github/rulesets/ to GitHub, matched by name.
#
#   scripts/apply-rulesets.sh           create or update every ruleset (needs repo admin)
#   scripts/apply-rulesets.sh --check   exit 1 if a live ruleset differs from its file
#
# The JSON files are the source of truth. Edit them, open a PR, and run this after merge.
# Needs gh (authenticated) and jq.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
MODE="${1:-apply}"

# Keep only the keys the file sets, so server-side defaults do not count as drift.
# shellcheck disable=SC2016
PROJECT='def project($want):
  if ($want | type) == "object" and type == "object" then
    . as $obj | reduce ($want | keys[]) as $k ({}; .[$k] = ($obj[$k] | project($want[$k])))
  elif ($want | type) == "array" and type == "array" and ($want | length) > 0 then
    [range(0; length) as $i | .[$i] | project($want[$i] // $want[-1])]
  else . end;'

status=0
for file in "$ROOT"/.github/rulesets/*.json; do
  name="$(jq -r .name "$file")"
  id="$(gh api "repos/$REPO/rulesets" --paginate \
    --jq ".[] | select(.name == \"$name\") | .id")"

  case "$MODE" in
    --check)
      if [ -z "$id" ]; then
        echo "missing: $name"
        status=1
        continue
      fi
      live="$(gh api "repos/$REPO/rulesets/$id" \
        | jq -S --slurpfile want "$file" "$PROJECT project(\$want[0])")"
      if diff -u <(jq -S . "$file") <(printf '%s\n' "$live") >/dev/null; then
        echo "in sync: $name"
      else
        echo "drift: $name"
        diff -u <(jq -S . "$file") <(printf '%s\n' "$live") || true
        status=1
      fi
      ;;
    apply)
      if [ -z "$id" ]; then
        gh api --method POST "repos/$REPO/rulesets" --input "$file" --jq '"created: \(.name) (\(.id))"'
      else
        gh api --method PUT "repos/$REPO/rulesets/$id" --input "$file" --jq '"updated: \(.name) (\(.id))"'
      fi
      ;;
    *)
      echo "usage: scripts/apply-rulesets.sh [--check]" >&2
      exit 2
      ;;
  esac
done
exit "$status"
