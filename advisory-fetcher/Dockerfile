# Advisory-fetcher image (ARM64 target: Raspberry Pi 5).
#
# Slim Python base + Playwright at an exact release, installing ONLY the lean
# chromium-headless-shell (the spike-validated mode) plus its OS deps — the full Chromium binary is
# deliberately skipped (`--only-shell`) to keep the image small. Runs as a non-root user. The
# browser lives only in THIS image; the Rust feed-producer service keeps its scratch/musl posture.
FROM python:3.12-slim

# Pin Playwright to an exact release for a reproducible build. Its bundled chromium-headless-shell
# build is what the on-device spike verified against Amtrak's Akamai gate.
ARG PLAYWRIGHT_VERSION=1.61.0

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PIP_NO_CACHE_DIR=1 \
    PYTHONUNBUFFERED=1

# Install Playwright, then the headless-shell browser + system libraries (needs root, so before
# switching users). `--only-shell` installs chromium-headless-shell and skips full Chromium.
RUN pip install --no-cache-dir "playwright==${PLAYWRIGHT_VERSION}" \
 && python -m playwright install --only-shell --with-deps chromium \
 && useradd --create-home --uid 10001 fetcher \
 && chmod -R a+rX /ms-playwright

# Snapshot directory, writable by the non-root user (default SNAPSHOT_DIR=/snapshot).
RUN mkdir -p /snapshot && chown 10001:10001 /snapshot
VOLUME ["/snapshot"]

WORKDIR /app
COPY fetcher/ /app/fetcher/

USER fetcher
EXPOSE 8080

# The fetcher serves the snapshot on :8080 and runs the poll loop; SIGTERM shuts it down cleanly.
ENTRYPOINT ["python", "-m", "fetcher"]
