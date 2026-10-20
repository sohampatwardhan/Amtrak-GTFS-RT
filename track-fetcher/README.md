# track-fetcher

A sidecar that reads Amtrak's own track numbers for the GTFS-RT service.

`www.amtrak.com` resets plain HTTP clients (Akamai). The Amtrak app loads

`GET /dotcom/travel-service/statuses/{train}?service-date={YYYY-MM-DD}`

and shows `stops[].departure.track.number`, or `stops[].arrival.track.number` when the departure
track is empty. This process keeps one headless browser, calls that URL for trains that are near
a stop, and serves the JSON at `/tracks.json`. The maps train feed
(`maps.amtrak.com/.../getTrainsData`) is decrypted in process and is what decides which trains
are near a stop; it does not itself contain track numbers.

The browser stays in this image. The Rust service stays scratch and browser-free.

## How it works

- Decrypt the maps feed, then keep trains that have a rail stop between
  `TRACK_WINDOW_AFTER_SECS` in the past and `TRACK_WINDOW_BEFORE_SECS` in the future
  (default 10 minutes back and 30 minutes ahead). An estimated time replaces the scheduled time.
  Bus stops are ignored.
- A train is hot when its next stop is inside `TRACK_HOT_WINDOW_SECS` (default 12 minutes) and
  one of its upcoming stops in the window has no track in its last document. Hot trains are
  fetched every cycle, soonest first. Every other due train, including one that has just left a
  stop, is rotated, and a train attempted within `TRACK_COOLDOWN_SECS` (default 90 seconds)
  waits; a failed attempt counts. Each cycle fetches at most `MAX_FETCHES_PER_CYCLE` trains
  (default 24).
- Arizona stations use `America/Phoenix` (no daylight saving time), although the train feed
  labels them Mountain.
- One resident `chromium-headless-shell` performs the status fetches. Image, font, stylesheet,
  and media requests are blocked, and so is every host outside `amtrak.com` while
  `BLOCK_THIRD_PARTY` is on. Each fetch aborts after `FETCH_TIMEOUT_SECS`. The browser is
  recycled every `BROWSER_MAX_AGE_SECS`.
- A browser failure (crash, launch failure, or a hung fetch) ends that cycle's fetches. The next
  launch waits 1 minute, doubling up to `RELAUNCH_BACKOFF_MAX_SECS`, and the wait resets after a
  successful fetch.
- Each cycle logs `due=`, `picked=`, `stored=`, and `rejected=` with the reason per train (for
  example `{'http 403': 3}`), so an Akamai block is visible in the logs.
- `/tracks.json` is the latest documents (`200`), each with `fetchedAt`, the time it was read
  from Amtrak. It is `503` when no poll has succeeded or the last success is older than
  `MAX_STALE_SECS`. A poll succeeds only when it had nothing to fetch or stored at least one
  document, so if Amtrak stops answering the endpoint goes `503` instead of serving old documents
  as current. The service treats `503` as a missed refresh and keeps tracks it already stored
  until each train's departure, ranked below a live NJ Transit or Hartford reading.
- `/healthz` is `503` once no poll has finished within `LIVENESS_SECS`, and the image's
  `HEALTHCHECK` uses it, so a wedged poll loop is restarted.

On 2026-10-17 the app showed track 1 at Providence for train 167 about ten minutes before
arrival. That same document had the track on `arrival.track` (`asOf` 16:57 for a 17:06 schedule).
Other stations publish a track hours ahead; it is already on the document when the train enters
the window. After departure the departure track is often cleared, which is why the service keeps
a track it has seen until the train leaves.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `TRAINS_URL` | maps `getTrainsData` | Encrypted train list used to choose who is near a stop |
| `AMTRAK_PAGE_URL` | `https://www.amtrak.com/tickets/train-status.html` | Page the browser opens so later `fetch` calls are same-origin |
| `POLL_INTERVAL_SECS` | `60` | Seconds between cycles |
| `TRACK_WINDOW_BEFORE_SECS` | `1800` | How far ahead a stop's ETA may be |
| `TRACK_WINDOW_AFTER_SECS` | `600` | How recently a stop may have passed and still be fetched |
| `TRACK_HOT_WINDOW_SECS` | `720` | Closest stop within this many seconds is fetched every cycle |
| `TRACK_COOLDOWN_SECS` | `90` | Minimum gap before the same outer-window train is fetched again |
| `MAX_FETCHES_PER_CYCLE` | `24` | Cap on status requests per cycle |
| `DOCUMENT_TTL_SECS` | `7200` | How long a fetched document stays in `/tracks.json` |
| `NAV_TIMEOUT_SECS` | `45` | Browser navigation and maps-feed timeout |
| `FETCH_TIMEOUT_SECS` | `20` | Abort one status request after this many seconds |
| `BROWSER_MAX_AGE_SECS` | `21600` | Recycle the resident browser after this long |
| `RELAUNCH_BACKOFF_MAX_SECS` | `900` | Longest wait before relaunching a failed browser |
| `MAX_STALE_SECS` | `300` | Serve `503` once the last successful poll is older than this |
| `LIVENESS_SECS` | `900` | `/healthz` fails once no poll has finished for this long |
| `LISTEN_HOST` | `0.0.0.0` | Snapshot HTTP address. The healthcheck probes loopback, so keep `0.0.0.0` or `127.0.0.1` |
| `LISTEN_PORT` | `8081` | Snapshot HTTP port |
| `SERVE_PATH` | `/tracks.json` | Path the Rust service GETs |
| `BROWSER_MODE` | `headless-shell` | `headless-shell` (efficient), or `xvfb-headful` (fallback: full Chromium under an Xvfb display the fetcher starts itself). `xvfb-headful` needs an image built with `--build-arg BROWSER_BUNDLE=full`; the default image stops at startup and says so |
| `BLOCK_SUBRESOURCES` | `true` | Abort image/font/media/CSS during the page load |
| `BLOCK_THIRD_PARTY` | `true` | Abort every request to a host outside `amtrak.com`. Turn off only if on-device checks show the status call needs a third-party host |

## If Amtrak blocks the headless shell

Akamai lets a real browser through because it runs Amtrak's scripts and earns the bot-manager
cookies; plain HTTP clients are reset. `chromium-headless-shell` is the lightest browser that does
this, but it is also the easiest to recognize as headless. If the per-cycle log shows `rejected=`
filling with `http 403`, first set `BLOCK_THIRD_PARTY=false` in case the status call needs a host
outside `amtrak.com`. If 403s continue, rebuild with full Chromium and Xvfb and run headful:

```bash
FETCHER_BROWSER_BUNDLE=full FETCHER_BROWSER_MODE=xvfb-headful \
  docker compose up -d --build
```

The full image is larger and headful Chromium uses more memory; check it stays under the
768 MB `mem_limit` on a Pi.

## Isolation

Chromium runs Amtrak's own page scripts without Chromium's sandbox (Playwright's default in a
container), so the container is the boundary. In the root Compose file the service is read-only
with tmpfs for `/tmp` and the fetcher's home, drops every capability, sets `no-new-privileges`,
runs under `init`, and is limited to 768 MB and 256 processes. Do not publish its port: only
`amtrak-gtfs-rt` needs to reach it, on the internal network.

The sidecar sends up to `MAX_FETCHES_PER_CYCLE` status requests to `www.amtrak.com` each cycle,
fewer once trains near a stop already show their tracks. Check that this volume suits Amtrak's
terms of use before running it.

## Run

The root Compose file starts this image with the stack and points the service at it, so tracks and
gates from Amtrak's status are on by default. An empty `AMTRAK_TRACKS_AMTRAK_URL` leaves this source
off; `AMTRAK_TRACKS=off` with `--scale track-fetcher=0` runs the stack without tracks or Chromium:

```bash
docker compose up -d --build
```

## One-shot track dump

`python -m fetcher.dump` prints every track Amtrak has posted for the trains you name, then exits.
It opens the train-status page once in headless Chromium, the same way the sidecar does, then
fetches one status document per train and service date from inside that page. It waits
`--delay` seconds between requests (default 1.5), blocks the same page weight and third-party
hosts, and refuses more than `--max-requests` train-days (default 40). A service date is the day
the train left its origin, so pass yesterday's date too for overnight trains still running.

```bash
python -m fetcher.dump 66 67 49 2150 --date 2026-10-19 --date 2026-10-20
```

```text
Train  Service date  Station  Side       Track  Gate  Scheduled                  As of                      Station name
66     2026-10-19    WAS      departure  24     H     2026-10-19T22:00:00-04:00  2026-10-19T21:38:18-04:00  Washington, DC
66     2026-10-19    BWI      arrival    1            2026-10-19T22:33:00-04:00  2026-10-19T10:33:01-04:00  BWI Marshall Airport, MD
67     2026-10-19    NYP      arrival    10           2026-10-19T23:05:00-04:00  2026-10-19T23:32:09-04:00  New York, NY
49     2026-10-19    BUF      arrival    MAIN         2026-10-20T00:29:00-04:00  2026-10-19T12:29:06-04:00  Buffalo, NY
```

`--json` prints one object per track instead. Locally it needs Playwright and its headless shell
(`python -m pip install playwright==1.61.0`, then `python -m playwright install --only-shell
chromium`). The image already has both:

```bash
docker run --rm --read-only --tmpfs /tmp --shm-size 64m \
  --entrypoint python track-fetcher:local -m fetcher.dump 66 2150
```

Offline tests (no browser, no network):

```bash
python -m pip install --requirement requirements.txt
python -m pytest tests -q
```
