# Optional reverse proxy. Not part of the default Compose project.
#
# Caddy's standard build has no per-IP rate limit, so this image adds
# github.com/mholt/caddy-ratelimit. Both upstream images are digest-pinned. The release
# gate builds this Dockerfile and scans the result.
FROM caddy:2.11.7-builder-alpine@sha256:95a5e7e2692b5eb9545aadc5b90a39f6f509bee59c9508a493f83ee902eaef63 AS builder

WORKDIR /src
# Caddy 2.11.7 still vendors older golang.org/x/net and go-jose.
# Fixable advisories (GO-2026-6612, GO-2026-6860, GHSA-mgcc-cww5-fp2q): replace
# them with the fixed releases while building the same Caddy tag.
RUN xcaddy build v2.11.7 \
      --with github.com/mholt/caddy-ratelimit@5625512f24f6f59d6f64fb3aafe5eecff0b286db \
      --replace golang.org/x/net=golang.org/x/net@v0.62.0 \
      --replace github.com/go-jose/go-jose/v4=github.com/go-jose/go-jose/v4@v4.1.6 \
    && ./caddy list-modules | grep -q '^http.handlers.rate_limit$'

FROM caddy:2.11.7-alpine@sha256:e70b164da81ef698b3ad19af799ea6086ce6d9e48ed4079eb9fe26f6fbf14ee5

USER root
# Alpine packages in the pinned tag can lag security updates. The release gate fails a
# fixable Critical or High, so upgrade before the scan. Upgrading alpine-baselayout can
# replace /etc/passwd, so the non-root user is created after that upgrade. The process
# listens above 1024 and does not need to bind privileged ports.
RUN apk upgrade --no-cache \
    && rm -rf /var/cache/apk \
    && addgroup -S -g 65532 caddy \
    && adduser -S -D -H -u 65532 -h /var/lib/caddy -s /sbin/nologin -G caddy caddy
COPY --from=builder /src/caddy /usr/bin/caddy
COPY entrypoint.sh /usr/local/bin/amtrak-proxy-entrypoint
RUN chmod 0555 /usr/local/bin/amtrak-proxy-entrypoint /usr/bin/caddy
USER caddy

EXPOSE 8080 8443
ENTRYPOINT ["/usr/local/bin/amtrak-proxy-entrypoint"]
