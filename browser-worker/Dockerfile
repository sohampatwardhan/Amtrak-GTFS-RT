# Standalone browser-worker images (linux/amd64 and linux/arm64, including Raspberry Pi).
#
# Build from the repository root. One Dockerfile produces both of today's standalone packages
# from the single worker package, as two targets, until docs/one-container.md retires them (#74):
#
#   docker build -f browser-worker/Dockerfile --target track-fetcher .
#   docker build -f browser-worker/Dockerfile --target advisory-fetcher .
#
# `track-fetcher` runs only the track source on 0.0.0.0:8081. `advisory-fetcher` runs only the
# notices source on 0.0.0.0:8080, its old port, with snapshots in /snapshot. Both listen on all
# interfaces because, as standalone containers, another container reads them.
#
# Slim Debian Python plus Playwright's chromium-headless-shell only. `playwright install
# --with-deps` also pulls xvfb and the large font packs (Noto emoji, Unifont, CJK) and is what made
# the old image about 1.4 GB. Those packages are not required for headless-shell capture, so the
# runtime installs the Chromium shared libraries Playwright lists for Debian 12 plus a small Latin
# font set. A real browser is still required: Amtrak's Akamai edge rejects non-browser clients.
FROM python:3.12-slim-bookworm AS worker

# 1.64.1 bundles Node 24.22.1, which fixes the fixable High advisories in 1.64.0's Node 24.21.0
# (CVE-2026-105482, CVE-2026-105483, CVE-2026-106069, CVE-2026-111497).
ARG PLAYWRIGHT_VERSION=1.64.1
ARG CRYPTOGRAPHY_VERSION=50.0.2
# `headless-shell` (default) installs only chromium-headless-shell. `full` also installs full
# Chromium and Xvfb for BROWSER_MODE=xvfb-headful, the fallback if Amtrak blocks the headless shell.
# It makes the image larger, so build it only when that mode is needed.
ARG BROWSER_BUNDLE=headless-shell

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PIP_NO_CACHE_DIR=1 \
    PYTHONUNBUFFERED=1

# Debian 12 names from Playwright's chromium nativeDeps, plus fontconfig so the shell can shape
# text. Upgrade distro packages first: the release gate fails a fixable Critical or High, and the
# python slim tag can lag Debian security updates.
RUN apt-get update \
    && apt-get upgrade --yes --no-install-recommends \
    && apt-get install --yes --no-install-recommends \
        ca-certificates \
        fonts-liberation \
        libasound2 \
        libatk-bridge2.0-0 \
        libatk1.0-0 \
        libatspi2.0-0 \
        libcairo2 \
        libcups2 \
        libdbus-1-3 \
        libdrm2 \
        libfontconfig1 \
        libfreetype6 \
        libgbm1 \
        libglib2.0-0 \
        libnspr4 \
        libnss3 \
        libpango-1.0-0 \
        libx11-6 \
        libxcb1 \
        libxcomposite1 \
        libxdamage1 \
        libxext6 \
        libxfixes3 \
        libxkbcommon0 \
        libxrandr2 \
    && if [ "${BROWSER_BUNDLE}" = "full" ]; then \
        apt-get install --yes --no-install-recommends xvfb; \
    elif [ "${BROWSER_BUNDLE}" != "headless-shell" ]; then \
        echo "BROWSER_BUNDLE must be headless-shell or full" >&2; exit 1; \
    fi \
    && rm -rf /var/lib/apt/lists/*

# `--only-shell` installs chromium-headless-shell and skips full Chromium; the `full` bundle
# installs both and checks full Chromium's libraries too.
RUN pip install --no-cache-dir \
        "playwright==${PLAYWRIGHT_VERSION}" \
        "cryptography==${CRYPTOGRAPHY_VERSION}" \
    && if [ "${BROWSER_BUNDLE}" = "full" ]; then \
        python -m playwright install chromium \
        && full="$(find /ms-playwright -type f -name chrome -path '*chrome-linux*' -print -quit)" \
        && test -n "${full}" \
        && ! ldd "${full}" | grep 'not found' \
        && command -v Xvfb; \
    else \
        python -m playwright install --only-shell chromium; \
    fi \
    && shell="$(find /ms-playwright -type f \( -name chrome-headless-shell -o -name headless_shell \) -print -quit)" \
    && test -n "${shell}" \
    && ldd "${shell}" \
    && ! ldd "${shell}" | grep 'not found' \
    && useradd --create-home --uid 10001 fetcher \
    && chmod -R a+rX /ms-playwright

# The notices snapshot directory, writable by the non-root user. A volume, so it stays writable
# under a read-only root filesystem.
RUN mkdir -p /snapshot && chown 10001:10001 /snapshot
VOLUME ["/snapshot"]

ENV LISTEN_HOST=0.0.0.0 \
    NOTICES_SNAPSHOT_DIR=/snapshot

WORKDIR /app
COPY browser-worker/worker/ /app/worker/

# Set by the release workflow; local builds keep the defaults.
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown

LABEL org.opencontainers.image.source="https://github.com/sohampatwardhan/Amtrak-GTFS-RT" \
      org.opencontainers.image.documentation="https://github.com/sohampatwardhan/Amtrak-GTFS-RT/tree/main/browser-worker" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.version="$OCI_VERSION" \
      org.opencontainers.image.revision="$OCI_REVISION"

USER fetcher

# /healthz is the track loop's liveness when tracks run (503 once no poll has finished within
# LIVENESS_SECS, or after three polls in a row fail), and 200 otherwise. It probes loopback, so
# LISTEN_HOST must stay 0.0.0.0 or 127.0.0.1.
HEALTHCHECK --interval=60s --timeout=10s --start-period=120s --retries=3 \
    CMD ["python", "-c", "import os, urllib.request; urllib.request.urlopen('http://127.0.0.1:' + os.environ['LISTEN_PORT'] + '/healthz', timeout=5)"]

ENTRYPOINT ["python", "-m", "worker"]

# Each standalone image runs one source, on the port its consumers already use. These are ENV
# defaults, so `docker run -e` still overrides them.
FROM worker AS advisory-fetcher
ENV AMTRAK_TRACKS_AMTRAK=off \
    LISTEN_PORT=8080
LABEL org.opencontainers.image.title="Amtrak GTFS-RT advisory-fetcher" \
      org.opencontainers.image.description="Amtrak service-alerts browser worker for Amtrak GTFS-RT"
EXPOSE 8080

FROM worker AS track-fetcher
ENV AMTRAK_ADVISORIES=off \
    LISTEN_PORT=8081
LABEL org.opencontainers.image.title="Amtrak GTFS-RT track-fetcher" \
      org.opencontainers.image.description="Amtrak train-status browser worker (tracks and boarding gates) for Amtrak GTFS-RT"
EXPOSE 8081
