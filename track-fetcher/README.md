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
- Trains whose closest stop is inside `TRACK_HOT_WINDOW_SECS` (default 12 minutes) are fetched
  every cycle. The rest of the window is rotated, and a train fetched within
  `TRACK_COOLDOWN_SECS` (default 90 seconds) waits. Each cycle fetches at most
  `MAX_FETCHES_PER_CYCLE` trains (default 24).
- One resident `chromium-headless-shell` performs the status fetches. Image, font, stylesheet,
  and media requests are blocked. The browser is relaunched if a fetch fails.
- `/tracks.json` is the latest documents (`200`) or `503` when no poll has finished or the last
  one is older than `MAX_STALE_SECS`. The service treats `503` as a missed refresh and keeps
  tracks it already stored until each train's departure.

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
| `MAX_STALE_SECS` | `300` | Serve `503` once the last completed poll is older than this |
| `LISTEN_PORT` | `8081` | Snapshot HTTP port |
| `SERVE_PATH` | `/tracks.json` | Path the Rust service GETs |
| `BROWSER_MODE` | `headless-shell` | `headless-shell`, or `xvfb-headful` where full Chromium exists |
| `BLOCK_SUBRESOURCES` | `true` | Abort image/font/media/CSS during the page load |

## Run

The root Compose file starts this image only with the `tracks` profile. Point the service at it
explicitly; an empty `AMTRAK_TRACKS_AMTRAK_URL` leaves the source off:

```bash
AMTRAK_TRACKS=on \
AMTRAK_TRACKS_AMTRAK_URL=http://track-fetcher:8081/tracks.json \
  docker compose --profile tracks up -d --build
```

Offline tests (no browser, no network):

```bash
python -m pip install --requirement requirements.txt
python -m pytest tests -q
```
