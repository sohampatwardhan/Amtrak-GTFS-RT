#!/usr/bin/env bash

set -euo pipefail

# Select and verify immutable release identity. Manual recovery is deliberately
# main-only: repository metadata cannot widen the write-capable control boundary.
usage() {
  printf '%s\n' \
    'usage: release-context.sh select EVENT_NAME REF_NAME INPUT_TAG CONTROL_REF DEFAULT_BRANCH OUTPUT_FILE' \
    '       release-context.sh verify TAG SOURCE_DIR OUTPUT_FILE' >&2
  exit 2
}

require_tag() {
  [[ "$1" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || {
    printf 'invalid release tag: %s\n' "$1" >&2
    return 1
  }
}

command="${1:-}"
case "$command" in
  select)
    [ "$#" -eq 7 ] || usage
    event_name="$2"
    ref_name="$3"
    input_tag="$4"
    control_ref="$5"
    default_branch="$6"
    output_file="$7"

    case "$event_name" in
      push)
        tag="$ref_name"
        ;;
      workflow_dispatch)
        [ "$default_branch" = main ] || {
          printf 'manual release controls require default branch main\n' >&2
          exit 1
        }
        [ "$control_ref" = refs/heads/main ] || {
          printf 'manual release controls require refs/heads/main\n' >&2
          exit 1
        }
        tag="$input_tag"
        ;;
      *)
        printf 'unsupported release event: %s\n' "$event_name" >&2
        exit 1
        ;;
    esac
    require_tag "$tag"
    printf 'tag=%s\nversion=%s\n' "$tag" "${tag#v}" >>"$output_file"
    ;;
  verify)
    [ "$#" -eq 4 ] || usage
    tag="$2"
    source_dir="$3"
    output_file="$4"
    require_tag "$tag"
    [ -d "$source_dir/.git" ] || {
      printf 'release source is not a Git checkout: %s\n' "$source_dir" >&2
      exit 1
    }
    tag_revision="$(git -C "$source_dir" rev-parse --verify "$tag^{commit}")"
    source_revision="$(git -C "$source_dir" rev-parse --verify HEAD)"
    [ "$source_revision" = "$tag_revision" ] || {
      printf 'release source HEAD %s does not equal %s at %s\n' "$source_revision" "$tag" "$tag_revision" >&2
      exit 1
    }
    "$source_dir/scripts/check-release-metadata.sh" "${tag#v}" "$tag"
    printf 'revision=%s\n' "$source_revision" >>"$output_file"
    ;;
  *)
    usage
    ;;
esac
