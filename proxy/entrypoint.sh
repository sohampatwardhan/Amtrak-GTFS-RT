#!/bin/sh
# Writes a Caddyfile from the environment and execs Caddy.
# AMTRAK_PROXY_PRINT_CONFIG=1 prints the file and exits, for the compose config test.
set -eu

RATE_EVENTS="${AMTRAK_PROXY_RATE_EVENTS:-120}"
RATE_WINDOW="${AMTRAK_PROXY_RATE_WINDOW:-1m}"
TLS_MODE="${AMTRAK_PROXY_TLS:-off}"
CONFIG="${AMTRAK_PROXY_CONFIG:-/tmp/Caddyfile}"

case "$TLS_MODE" in
  off | "")
    SITE=":8080"
    GLOBAL="auto_https off"
    TLS_DIRECTIVE=""
    ;;
  internal)
    SITE=":8443"
    GLOBAL="auto_https off"
    TLS_DIRECTIVE="	tls internal"
    ;;
  *)
    if [ -z "${AMTRAK_PROXY_DOMAIN:-}" ]; then
      echo "AMTRAK_PROXY_DOMAIN is required when AMTRAK_PROXY_TLS is an ACME contact email" >&2
      exit 1
    fi
    SITE="${AMTRAK_PROXY_DOMAIN}"
    GLOBAL="email ${TLS_MODE}"
    TLS_DIRECTIVE=""
    ;;
esac

cat >"$CONFIG" <<EOF
{
	admin off
	${GLOBAL}
	log {
		output stdout
		format console
	}
}

${SITE} {
	encode gzip
	log {
		output stdout
		format console
	}
	# Caddy weakens a strong ETag when it gzips. The origin compares the quoted
	# sha256 exactly, so a later If-None-Match of W/"..." is forwarded as "...".
	map {http.request.header.If-None-Match} {forward_inm} {
		~^W/(.+)\$ \$1
		default {http.request.header.If-None-Match}
	}
	rate_limit {
		zone per_ip {
			key {remote_host}
			events ${RATE_EVENTS}
			window ${RATE_WINDOW}
		}
	}
${TLS_DIRECTIVE}
	reverse_proxy vehicle-positions:8080 {
		flush_interval -1
		header_up If-None-Match {forward_inm}
		header_up If-Modified-Since {http.request.header.If-Modified-Since}
	}
}
EOF

if [ "${AMTRAK_PROXY_PRINT_CONFIG:-}" = "1" ]; then
  cat "$CONFIG"
  exit 0
fi

exec caddy run --config "$CONFIG" --adapter caddyfile
