#!/usr/bin/env bash

# Assemble or verify one multi-arch GHCR tag set.
#
# A tag created moments earlier can fail `docker buildx imagetools inspect` with
# exit 255 and no stderr while GHCR is still making the new manifest readable.
# Retry that read. If the version tag is already present and its platform
# digests match, verify it instead of refusing: a rerun of a publish that
# already tagged this image must not treat its own tags as a conflict.
# A tag whose digests differ is still refused, and a tag that stays unreadable
# is not overwritten.

set -euo pipefail

IMAGE="${1:?usage: assemble-release-manifest.sh IMAGE VERSION AMD64_FILE ARM64_FILE OUTPUT_KEY}"
VERSION="${2:?}"
AMD64_FILE="${3:?}"
ARM64_FILE="${4:?}"
OUTPUT_KEY="${5:?}"

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
[[ "$OUTPUT_KEY" =~ ^[A-Za-z0-9_]+$ ]]
AMD64_DIGEST="$(cat "$AMD64_FILE")"
ARM64_DIGEST="$(cat "$ARM64_FILE")"
[[ "$AMD64_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]
[[ "$ARM64_DIGEST" =~ ^sha256:[0-9a-f]{64}$ ]]
[ "$AMD64_DIGEST" != "$ARM64_DIGEST" ]

MINOR="${VERSION%.*}"
ATTEMPTS="${RELEASE_MANIFEST_INSPECT_ATTEMPTS:-6}"

manifest_matches() {
  jq -e --arg amd64 "$AMD64_DIGEST" --arg arm64 "$ARM64_DIGEST" '
    (.manifests | type == "array" and length == 2) and
    ([.manifests[] | .platform.os + "/" + .platform.architecture] | sort
      == ["linux/amd64", "linux/arm64"]) and
    ([.manifests[].digest] | sort == ([$amd64, $arm64] | sort))
  ' "$1" >/dev/null
}

# retry_inspect REF DEST MODE RAW
# MODE=allow-absent returns 86 immediately on manifest unknown / not found.
# MODE=until-present retries that too. RAW=raw passes --raw; RAW=text does not.
retry_inspect() {
  local ref="$1" dest="$2" mode="$3" raw="$4"
  local attempt=1 delay=2 err status args
  err="$(mktemp)"
  args=(buildx imagetools inspect "$ref")
  if [ "$raw" = raw ]; then
    args+=(--raw)
  fi
  while true; do
    : >"$err"
    # `cmd || status=$?` keeps the real exit code under set -e. A failed `if`
    # with no else would report 0 and hide the 255 this helper exists to retry.
    status=0
    docker "${args[@]}" >"$dest" 2>"$err" || status=$?
    if [ "$status" -eq 0 ] && [ ! -s "$dest" ]; then
      status=1
      printf 'empty manifest\n' >"$err"
    fi
    if [ "$status" -eq 0 ]; then
      rm -f "$err"
      return 0
    fi
    printf 'imagetools inspect %s failed (exit %s, attempt %s/%s)' \
      "$ref" "$status" "$attempt" "$ATTEMPTS" >&2
    if [ -s "$err" ]; then
      printf ':\n' >&2
      cat "$err" >&2
    else
      printf ' with no message\n' >&2
    fi
    if [ "$mode" = allow-absent ] && grep -Eqi 'manifest unknown|not found' "$err"; then
      rm -f "$err"
      return 86
    fi
    if [ "$attempt" -ge "$ATTEMPTS" ]; then
      rm -f "$err"
      return "$status"
    fi
    if [ "${RELEASE_MANIFEST_INSPECT_SLEEP:-1}" != 0 ]; then
      sleep "$delay"
    fi
    if [ "$delay" -lt 8 ]; then
      delay=$((delay * 2))
    fi
    attempt=$((attempt + 1))
  done
}

read_index_digest() {
  local ref="$1" dest digest status
  dest="$(mktemp)"
  set +e
  retry_inspect "$ref" "$dest" until-present text
  status=$?
  set -e
  if [ "$status" -ne 0 ]; then
    rm -f "$dest"
    return "$status"
  fi
  digest="$(awk '$1 == "Digest:" { print $2; exit }' "$dest")"
  rm -f "$dest"
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]
  printf '%s\n' "$digest"
}

raw="$(mktemp)"
trap 'rm -f "$raw"' EXIT

set +e
retry_inspect "$IMAGE:$VERSION" "$raw" allow-absent raw
status=$?
set -e
if [ "$status" -eq 86 ]; then
  docker buildx imagetools create \
    --tag "$IMAGE:$VERSION" \
    --tag "$IMAGE:$MINOR" \
    --tag "$IMAGE:latest" \
    "$IMAGE@$AMD64_DIGEST" \
    "$IMAGE@$ARM64_DIGEST"
  set +e
  retry_inspect "$IMAGE:$VERSION" "$raw" until-present raw
  status=$?
  set -e
fi
if [ "$status" -ne 0 ]; then
  exit "$status"
fi

if ! manifest_matches "$raw"; then
  printf '%s:%s: immutable version tag appeared after context resolution; refusing to overwrite it.\n' \
    "$IMAGE" "$VERSION" >&2
  exit 1
fi

manifest_digest="$(read_index_digest "$IMAGE:$VERSION")" || exit $?
for tag in "$MINOR" latest; do
  resolved="$(read_index_digest "$IMAGE:$tag")" || exit $?
  if [ "$resolved" != "$manifest_digest" ]; then
    printf '%s:%s resolved to %s, expected %s\n' \
      "$IMAGE" "$tag" "$resolved" "$manifest_digest" >&2
    exit 1
  fi
done

if [ -z "${GITHUB_OUTPUT:-}" ]; then
  printf 'GITHUB_OUTPUT is required\n' >&2
  exit 1
fi
printf '%s=%s\n' "$OUTPUT_KEY" "$manifest_digest" >>"$GITHUB_OUTPUT"
