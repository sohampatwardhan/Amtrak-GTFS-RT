#!/usr/bin/env bash
# The proxy is a Compose profile, not a required container. This checks the rendered
# project and the Caddyfile the entrypoint writes. It does not start the proxy.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if ! command -v docker >/dev/null; then
  echo "docker is required" >&2
  exit 1
fi

default_config="$(docker compose -f docker-compose.yml config)"
if printf '%s\n' "$default_config" | grep -Eq '^  proxy:'; then
  echo "proxy is present without --profile proxy" >&2
  exit 1
fi
printf '%s\n' "$default_config" | grep -Fq 'host_ip: 127.0.0.1' \
  && printf '%s\n' "$default_config" | grep -Fq 'published: "8090"' \
  || { echo "vehicle-positions no longer publishes 127.0.0.1:8090" >&2; exit 1; }

proxy_config="$(docker compose -f docker-compose.yml --profile proxy config)"
printf '%s\n' "$proxy_config" | grep -Eq '^  proxy:' \
  || { echo "proxy profile did not render the proxy service" >&2; exit 1; }
printf '%s\n' "$proxy_config" | grep -Fq '172.31.240.10' \
  || { echo "proxy has no fixed allowlisted address" >&2; exit 1; }
printf '%s\n' "$proxy_config" | grep -Fq 'published: "8091"' \
  || { echo "proxy HTTP port is not 8091" >&2; exit 1; }
grep -Fq 'caddy:2.11.7-alpine@sha256:e70b164da81ef698b3ad19af799ea6086ce6d9e48ed4079eb9fe26f6fbf14ee5' \
  proxy/Dockerfile \
  || { echo "proxy base image is not digest-pinned" >&2; exit 1; }
grep -Fq 'caddy:2.11.7-builder-alpine@sha256:95a5e7e2692b5eb9545aadc5b90a39f6f509bee59c9508a493f83ee902eaef63' \
  proxy/Dockerfile \
  || { echo "proxy builder image is not digest-pinned" >&2; exit 1; }

render() {
  AMTRAK_PROXY_PRINT_CONFIG=1 AMTRAK_PROXY_CONFIG="$(mktemp)" "$@" sh proxy/entrypoint.sh
}

off="$(render)"
printf '%s\n' "$off" | grep -Fq 'encode gzip' || { echo "gzip missing" >&2; exit 1; }
printf '%s\n' "$off" | grep -Fq 'output stdout' || { echo "access log missing" >&2; exit 1; }
printf '%s\n' "$off" | grep -Fq 'key {remote_host}' || { echo "per-IP rate limit missing" >&2; exit 1; }
printf '%s\n' "$off" | grep -Fq 'events 120' || { echo "default rate missing" >&2; exit 1; }
printf '%s\n' "$off" | grep -Fq 'header_up If-None-Match {forward_inm}' \
  || { echo "ETag forwarding missing" >&2; exit 1; }
printf '%s\n' "$off" | grep -Fq 'auto_https off' || { echo "TLS is on by default" >&2; exit 1; }
printf '%s\n' "$off" | grep -Fq 'tls internal' && { echo "internal TLS leaked into the default config" >&2; exit 1; }

internal="$(AMTRAK_PROXY_TLS=internal render)"
printf '%s\n' "$internal" | grep -Fq 'tls internal' || { echo "internal TLS missing" >&2; exit 1; }

if AMTRAK_PROXY_TLS=ops@example.com render >/dev/null 2>/tmp/proxy-compose-tls.err; then
  echo "ACME mode ran without a domain" >&2
  exit 1
fi
grep -Fq 'AMTRAK_PROXY_DOMAIN is required' /tmp/proxy-compose-tls.err \
  || { echo "ACME mode did not require a domain" >&2; exit 1; }

acme="$(AMTRAK_PROXY_TLS=ops@example.com AMTRAK_PROXY_DOMAIN=feeds.example.com render)"
printf '%s\n' "$acme" | grep -Fq 'email ops@example.com' || { echo "ACME email missing" >&2; exit 1; }
printf '%s\n' "$acme" | grep -Fq 'feeds.example.com' || { echo "ACME site missing" >&2; exit 1; }

echo "proxy compose profile and Caddyfile ok"
