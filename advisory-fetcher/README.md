# advisory-fetcher

A standalone sidecar that makes Amtrak's **Service Alerts & Notices** available to the
`amtrak-gtfs-rt` feed-producer service, which otherwise can't fetch them: `www.amtrak.com` is behind
**Akamai Bot Manager**, which resets a plain HTTP request that lacks a JavaScript-earned `_abck`
cookie. The fetcher runs a real headless browser (Playwright + `chromium-headless-shell`) to earn
that cookie, loads the notices page, and serves the resulting HTML over HTTP. The service GETs that
endpoint (its `AMTRAK_ADVISORIES_URL`) and turns it into scoped GTFS-RT alerts.

The browser — and its CVE surface — lives **only** in this image; the Rust service keeps its
scratch/musl, no-vulnerabilities posture.

## How it works

- **Poll loop** (`fetcher/poller.py`): every `POLL_INTERVAL_SECS` (default 15 minutes), launch one
  browser and render the notices page until the HTML size is stable and the `na-service-alert__*`
  markup is present, then close that page. Detail pages already snapshotted within
  `DETAIL_TTL_SECS` (default 24 hours) are left cached; the others are rendered in the same session
  (so the Akamai cookie still applies) with the same stable-size stop. A detail that fails is
  skipped; the list is still stored. The browser is closed at the end of the cycle, and killed if
  `browser.close()` is still waiting on page timers after `BROWSER_CLOSE_SECS`. Image/font/media/CSS
  requests are blocked during the fetch (script/XHR allowed so Akamai's sensor still runs).
- **Snapshot store** (`fetcher/store.py`): the latest list HTML is written atomically; unchanged
  content isn't rewritten. Detail pages are stored under `details/` and only for safe
  `/alert/<slug>.html` paths.
- **HTTP server** (`fetcher/server.py`): serves the list snapshot at `SERVE_PATH` and each detail
  snapshot at its `/alert/<slug>.html` path, `200 text/html` while fresh and `503` when the list
  (or that detail) is missing or older than `MAX_STALE_SECS`. A detail that was never stored is
  `404`. The service then keeps a title-only description for that alert. `/healthz` is `200`.

## Configuration (environment)

| Variable | Default | Meaning |
|---|---|---|
| `AMTRAK_SOURCE_URL` | `https://www.amtrak.com/service-alerts-and-notices` | Page to fetch |
| `POLL_INTERVAL_SECS` | `900` | Seconds between page loads |
| `NAV_TIMEOUT_SECS` | `45` | Per-cycle navigation/selector timeout |
| `MAX_STALE_SECS` | `3600` | Serve `503` once the snapshot is older than this |
| `LISTEN_PORT` | `8080` | Snapshot HTTP port |
| `SNAPSHOT_DIR` | `/snapshot` | Where the snapshot file is written |
| `SERVE_PATH` | `/service-alerts-and-notices` | HTTP path the service GETs |
| `GATE_SELECTOR` | `[class*='na-service-alert']` | Selector proving the markup rendered |
| `BROWSER_MODE` | `headless-shell` | `headless-shell` (efficient), or `xvfb-headful` (fallback: full Chromium under an Xvfb display the fetcher starts itself). `xvfb-headful` needs an image built with `--build-arg BROWSER_BUNDLE=full`; the default image stops at startup and says so |
| `BLOCK_SUBRESOURCES` | `true` | Skip image/font/media/CSS during the fetch |
| `DETAIL_TTL_SECS` | `86400` | Do not re-render a detail page younger than this |
| `DETAIL_MAX_STALE_SECS` | `86400` | Serve a stored detail as fresh for this long. Must be ≥ `DETAIL_TTL_SECS` |
| `RENDER_STABLE_MS` | `400` | HTML size must stay unchanged this long, with the gate selector present, before the renderer stops |
| `RENDER_POLL_MS` | `200` | How often to sample page HTML while waiting for a stable size |
| `BROWSER_CLOSE_SECS` | `2` | Graceful browser shutdown budget; the renderer is killed after this |

## Run

The primary stack is the repository-root [`docker-compose.yml`](../docker-compose.yml). It runs
this image with the scratch service image and the track sidecar, and turns advisories on:

```text
AMTRAK_ADVISORIES=on
AMTRAK_ADVISORIES_URL=http://advisory-fetcher:8080/service-alerts-and-notices
```

Releases from `v0.7.0` publish this image for `linux/amd64` and `linux/arm64` (Raspberry Pi) as
`ghcr.io/sohampatwardhan/amtrak-gtfs-rt-advisory-fetcher`, tagged with the version, the minor
version, and `latest`. The root Compose file names it at the release version. From the repository
root:

```bash
docker compose pull && docker compose up -d       # released images
AMTRAK_IMAGE_TAG=local docker compose up -d --build  # build from this checkout
docker pull ghcr.io/sohampatwardhan/amtrak-gtfs-rt-advisory-fetcher:0.7.1  # this image alone
```

[`docker-compose.yml`](docker-compose.yml) in this directory includes that root file, so the same
stack starts from here. `docker buildx bake` at the root builds this image and the service without starting them.
The scratch service can still be built and run alone, with advisories left off; see the root README.

Advisory *consumption* from
[PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8) and this fetcher from
[PR #9](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/9) are both merged to `main`.
Advisories remain default-off and still require an operator to run the sidecar and configure the
service URL; source integration is not evidence of a deployment.

## Disable / rollback

Advisories are **default-off** in the service. To turn the feature off, unset `AMTRAK_ADVISORIES`
(or stop the fetcher): the service reverts to serving no advisories with **zero code change** and
keeps generating its other feeds (it is fail-open — a missing/`503` fetcher never breaks it).

## Tests

```bash
python3 -m pytest        # offline unit tests (no browser needed)
```

The real browser path and the Akamai bypass are verified on the device (spike + on-device e2e); see
[spike/README.md](spike/README.md) and `spike/results.md`.
