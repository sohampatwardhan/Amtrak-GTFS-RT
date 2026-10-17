# Track-fetcher image (linux/amd64 and linux/arm64, including Raspberry Pi).
#
# Same browser shape as advisory-fetcher: slim Debian Python plus Playwright's
# chromium-headless-shell. One process stays up between polls. The maps train feed is decrypted
# in Python; only www.amtrak.com status calls go through the browser.
FROM python:3.12-slim-bookworm

ARG PLAYWRIGHT_VERSION=1.61.0
ARG CRYPTOGRAPHY_VERSION=46.0.3

ENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright \
    PIP_NO_CACHE_DIR=1 \
    PYTHONUNBUFFERED=1

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
    && rm -rf /var/lib/apt/lists/*

RUN pip install --no-cache-dir \
        "playwright==${PLAYWRIGHT_VERSION}" \
        "cryptography==${CRYPTOGRAPHY_VERSION}" \
    && python -m playwright install --only-shell chromium \
    && shell="$(find /ms-playwright -type f \( -name chrome-headless-shell -o -name headless_shell \) -print -quit)" \
    && test -n "${shell}" \
    && ldd "${shell}" \
    && ! ldd "${shell}" | grep 'not found' \
    && useradd --create-home --uid 10001 fetcher \
    && chmod -R a+rX /ms-playwright

WORKDIR /app
COPY fetcher/ /app/fetcher/

USER fetcher
EXPOSE 8081

ENTRYPOINT ["python", "-m", "fetcher"]
