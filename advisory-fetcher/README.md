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

- **Poll loop** (`fetcher/poller.py`): every `POLL_INTERVAL_SECS`, launch one browser, load the
  page, wait for the `na-service-alert__*` markup, capture the HTML, then close the browser. Nothing
  browser-related stays resident between polls. Image/font/media/CSS requests are blocked during the
  fetch (script/XHR allowed so Akamai's sensor still runs).
- **Snapshot store** (`fetcher/store.py`): the latest HTML is written atomically; unchanged content
  isn't rewritten.
- **HTTP server** (`fetcher/server.py`): serves the snapshot as `200 text/html` while fresh, `503`
  when missing or older than `MAX_STALE_SECS` (the service then fails open), and `200` on `/healthz`.

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
| `BROWSER_MODE` | `headless-shell` | `headless-shell` (efficient) or `xvfb-headful` (fallback) |
| `BLOCK_SUBRESOURCES` | `true` | Skip image/font/media/CSS during the fetch |

## Run

```bash
docker compose up -d --build
```

The service turns advisories on with `AMTRAK_ADVISORIES=on` and
`AMTRAK_ADVISORIES_URL=http://advisory-fetcher:8080/service-alerts-and-notices`.

> **Prerequisite:** advisory *consumption* lives in the service-advisories work
> ([PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8)), not yet on `main`. The
> fetcher runs and serves snapshots regardless, but end-to-end alerts require that service build.

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
