#!/usr/bin/env bash

set -euo pipefail

DESTDIR="${1:?usage: scripts/install-release-scanners.sh DESTDIR}"
GRYPE_VERSION="0.117.0"
SYFT_VERSION="1.51.0"
SYSTEM="$(uname -s)"
MACHINE="$(uname -m)"

case "$SYSTEM:$MACHINE" in
  Linux:x86_64|Linux:amd64)
    PLATFORM="linux_amd64"
    GRYPE_SHA256="38525dab1e06f162ebaa02f94d82d1f807076b011a44180cf2777edf1a7b9c26"
    SYFT_SHA256="2a2e837a2c8d59ec9af5472ee22d3b04ee463c4e44476ecf993fd1e5ab6ebc7f"
    ;;
  Linux:aarch64|Linux:arm64)
    PLATFORM="linux_arm64"
    GRYPE_SHA256="935f628bdf9331ffdd946931ea5fdb50045d3970ba52670cbeb44a88f127291b"
    SYFT_SHA256="6c0466811541ea03add5213a60a1562f0851e4c0b0ecfdee1a694a9455285900"
    ;;
  Darwin:arm64)
    PLATFORM="darwin_arm64"
    GRYPE_SHA256="bfcefa3f3b1690d9c77d847841b32ebd6106ab0e0e32f810924707e704d53584"
    SYFT_SHA256="4f37f4c7fefce0a68e4cf71ba3f5f9829a99e65d89b29f7ee41b8c2c10ea8c59"
    ;;
  *)
    printf 'Unsupported scanner platform: %s/%s\n' "$SYSTEM" "$MACHINE" >&2
    exit 1
    ;;
esac

mkdir -p "$DESTDIR"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

install_tool() {
  local name="$1" version="$2" expected="$3"
  local archive="${name}_${version}_${PLATFORM}.tar.gz"
  local url="https://github.com/anchore/${name}/releases/download/v${version}/${archive}"

  curl --fail --location --silent --show-error --output "$WORKDIR/$archive" "$url"
  printf '%s  %s\n' "$expected" "$WORKDIR/$archive" | shasum -a 256 --check --strict
  tar -xzf "$WORKDIR/$archive" -C "$DESTDIR" "$name"
  chmod 0755 "$DESTDIR/$name"
  test -x "$DESTDIR/$name"
}

install_tool grype "$GRYPE_VERSION" "$GRYPE_SHA256"
install_tool syft "$SYFT_VERSION" "$SYFT_SHA256"

"$DESTDIR/grype" version | grep -F "Version:             $GRYPE_VERSION" >/dev/null
"$DESTDIR/syft" version | grep -F "Version:             $SYFT_VERSION" >/dev/null
