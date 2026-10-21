# Advisory-fetcher image (linux/amd64 and linux/arm64, including Raspberry Pi).
#
# Slim Debian Python plus Playwright's chromium-headless-shell only. `playwright install --with-deps`
# also pulls xvfb and the large font packs (Noto emoji, Unifont, CJK) and is what made this image
# about 1.4 GB. Those packages are not required for headless-shell HTML capture, so the runtime
# installs the Chromium shared libraries Playwright lists for Debian 12 plus a small Latin font set.
# A real browser is still required: Amtrak's Akamai edge rejects non-browser TLS clients.
FROM python:3.12-slim-bookworm

# Pin Playwright to an exact release. Its bundled chromium-headless-shell is the build the
# on-device spike verified against Amtrak's Akamai gate.
ARG PLAYWRIGHT_VERSION=1.61.0
# `headless-shell` (default) installs only chromium-headless-shell. `full` also installs full
# Chromium and Xvfb for BROWSER_MODE=xvfb-headful, the fallback if Amtrak blocks the headless shell.
# It makes the image larger, so build it only when that mode is needed:
#   docker build --build-arg BROWSER_BUNDLE=full .
ARG BROWSER_BUNDLE=headless-shell

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PIP_NO_CACHE_DIR=1 \
    PYTHONUNBUFFERED=1

# Debian 12 names from Playwright 1.61 nativeDeps.ts (chromium), plus fontconfig so the shell can
# shape text. `--no-install-recommends` keeps recommended GUI stacks out.
RUN apt-get update \
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

# `--only-shell` installs chromium-headless-shell and skips full Chromium; the `full` bundle installs
# both and checks full Chromium's libraries too. Dependencies were installed above, so
# `--with-deps` is intentionally not used.
RUN pip install --no-cache-dir "playwright==${PLAYWRIGHT_VERSION}" \
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

# Snapshot directory, writable by the non-root user (default SNAPSHOT_DIR=/snapshot).
RUN mkdir -p /snapshot && chown 10001:10001 /snapshot
VOLUME ["/snapshot"]

WORKDIR /app
COPY fetcher/ /app/fetcher/

# Set by the release workflow; local builds keep the defaults.
ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown

LABEL org.opencontainers.image.title="Amtrak GTFS-RT advisory-fetcher" \
      org.opencontainers.image.description="Amtrak service-alerts sidecar for Amtrak GTFS-RT" \
      org.opencontainers.image.source="https://github.com/sohampatwardhan/Amtrak-GTFS-RT" \
      org.opencontainers.image.documentation="https://github.com/sohampatwardhan/Amtrak-GTFS-RT/tree/main/advisory-fetcher" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.version="$OCI_VERSION" \
      org.opencontainers.image.revision="$OCI_REVISION"

USER fetcher
EXPOSE 8080

# The fetcher serves the snapshot on :8080 and runs the poll loop; SIGTERM shuts it down cleanly.
ENTRYPOINT ["python", "-m", "fetcher"]
