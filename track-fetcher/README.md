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
- `/healthz` is `503` once no poll has finished within `LIVENESS_SECS`, and after three polls
  in a row fail. The image's `HEALTHCHECK` uses it, so a wedged poll loop, or a loop that keeps
  running while every poll fails, is restarted. One failed poll stays healthy.

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
| `LIVENESS_SECS` | `900` | `/healthz` fails once no poll has finished for this long. It also fails after three polls in a row fail |
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
AMTRAK_IMAGE_TAG=local FETCHER_BROWSER_BUNDLE=full FETCHER_BROWSER_MODE=xvfb-headful \
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
off; `AMTRAK_TRACKS=off` with `--scale track-fetcher=0` runs the stack without tracks or Chromium.

Releases from `v0.7.0` publish this image for `linux/amd64` and `linux/arm64` (Raspberry Pi) as
`ghcr.io/sohampatwardhan/amtrak-gtfs-rt-track-fetcher`, tagged with the version, the minor version,
and `latest`. The root Compose file names it at the release version, so the stack can be pulled
rather than built:

```bash
docker compose pull && docker compose up -d       # released images
AMTRAK_IMAGE_TAG=local docker compose up -d --build  # build from this checkout
docker pull ghcr.io/sohampatwardhan/amtrak-gtfs-rt-track-fetcher:0.7.1  # this image alone
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
(`python -m pip install playwright==1.64.1`, then `python -m playwright install --only-shell
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

## Station coverage

`python -m fetcher.coverage` answers two questions: what track and gate Amtrak has posted for a train, and which stations publish that information and how far ahead. It is a one-shot (or a time-boxed watch). It does not poll or serve `/tracks.json`.

The profile it writes is the `amtrak-station-lead-time/v1` file `AMTRAK_TRACKS_LEAD_TIME_FILE` loads. An `observed` station marked `wider` uses that station's suggested lookahead and hot window. `sparse`, `no_track`, `none`, `covered`, `unknown`, and missing stations keep `TRACK_WINDOW_BEFORE_SECS` (30 minutes) and `TRACK_HOT_WINDOW_SECS` (12 minutes). Kingston posting a track overnight is the `wider` case. Providence posting one about ten minutes before the stop stays on the global window.

### What it requests

Same calls as the train-status page, through one headless Chromium session on `https://www.amtrak.com/tickets/train-status.html`, then `fetch` from that page. Image, font, stylesheet, media, and every host outside `amtrak.com` are blocked. There is no second client, no cookie copy, and no retry.

| Call | Role |
|---|---|
| `GET https://maps.amtrak.com/services/MapDataService/trains/getTrainsData` | Active train numbers and each train's origin departure time. Plain HTTP. Decrypted with the sidecar's `decrypt.py`. Not used when `tracks` is given an explicit `--date`. |
| `GET https://www.amtrak.com/dotcom/travel-service/statuses/{train}?service-date=` | The only source of the rows. `service-date` is the origin departure date, including for stops after midnight. |
| `GET https://www.amtrak.com/dotcom/travel-service/statuses/stations?origin-code=&destination-code=&departure-date=` | One corridor-board probe per survey (default NYP to WAS). Recorded only as whether the body still contains track and gate numbers. Not copied into the stop rows. `tracks` does not call it. `--no-board` skips it. |
| `GET /dotcom/travel-service/statuses/stops/{station}?service-numbers=&departure-date=` | Not called. The page fires it and a check on 2026-11-01 found no track or gate fields. The report says so. |

A track is `departure.track.number` when that number is assigned, otherwise `arrival.track.number`. Blank and `TBD` are not numbers. Gates are `gate.number` on that side. Lead time is the event time minus `track.asOf`, in seconds. The event time is `statusInfo.dateTime` when Amtrak sent one, otherwise `schedule.dateTime`. The station table's **Ahead** columns use the earlier of the two sides. The **Track** column uses the side the sidecar would publish.

Requests run in a fixed order: origin date, then train number. The delay between www.amtrak.com requests is fixed (default 2 seconds, minimum 1, no jitter). Survey's default cap is 300 requests including the one board probe; `tracks` defaults to 40. The hard maximum is 500. A run that hits the cap sets `truncated` and lists the trains left for `--offset`. A 403, 401, 429, or a dropped connection stops the run, writes whatever it already collected, and exits 3 with the reason. It does not fall through to `curl`.

### Commands

From `track-fetcher/`, with Playwright's headless shell installed the same way as `fetcher.dump`:

```bash
python -m pip install --requirement requirements.txt
python -m pip install playwright==1.64.1
python -m playwright install --only-shell chromium

# Train 66, origin date 2026-10-31, only Kingston's stop.
python -m fetcher.coverage tracks 66 --date 2026-10-31 --station KIN --out train66-kin.json

# Omit --date to use each train's origin date from the maps feed.
python -m fetcher.coverage tracks 66 --station KIN --out train66-kin.json

# Every active rail train, plus the NYP-WAS board probe.
python -m fetcher.coverage survey --out survey.json

# Repeat for an hour, one survey every 15 minutes, then merge.
python -m fetcher.coverage survey --interval 900 --duration 3600 --out-dir snapshots
python -m fetcher.coverage merge snapshots --out coverage.json
```

`--station` may be repeated. On `survey`, and on `tracks` without `--date`, it limits which trains are fetched to those whose maps-feed stops include that code. On `tracks --date` the named trains are still fetched and the filter only drops rows, because the feed is not consulted.

Each `--out` path (and `snapshots/merged.json` after a watch) is accompanied by:

| File | Contents |
|---|---|
| `survey.json` | Stops, per-station stats, the profile, and the request log (`fetched_at`, HTTP status, error) |
| `survey.csv` | One row per stop, same columns every time |
| `survey.stations.csv` | The station table |
| `survey.profile.json` | The `amtrak-station-lead-time/v1` profile. Set `AMTRAK_TRACKS_LEAD_TIME_FILE` to this path |

Stdout is the station table (and, for `tracks`, the per-stop tracks above it). Counts and errors go to stderr. Exit 0 means at least one status document or a successful board probe. Exit 1 means the run finished and Amtrak had nothing to give. Exit 3 means the browser or Akamai stopped it.

Do not commit survey output. It is public train data, and it goes stale within hours.

A full fleet at the default 2 second delay is on the order of ten minutes. Check that volume against Amtrak's terms of use before leaving a watch or a cron job running.

### Cron

A directory of hourly snapshots, merged each morning. `merge` on a directory reads `*.json` and skips `*.profile.json` and any file whose `kind` is `merged`, so the previous `coverage.json` in that directory is not folded in again.

```cron
15 6-22 * * * cd /path/to/track-fetcher && python -m fetcher.coverage survey --out /var/lib/amtrak-coverage/survey-$(date -u +\%Y\%m\%dT\%H\%M\%SZ).json
30 7 * * * cd /path/to/track-fetcher && python -m fetcher.coverage merge /var/lib/amtrak-coverage --out /var/lib/amtrak-coverage/coverage.json
```

`--interval` is the in-process version of that first line, bounded by `--duration` (at most 24 hours) and `--max-cycles` (default 24). A cycle that has started runs to its own cap; the next one does not start after the deadline.

### How trip updates load the profile

`survey.profile.json` has `schema` `amtrak-station-lead-time/v1`. For each station it carries `stops_seen`, `pct_with_track`, `lead_min_secs`, `lead_median_secs`, `lead_max_secs`, the same three numbers for the sidecar's chosen side (`chosen_lead_*`), `suggested_window_before_secs`, `suggested_hot_window_secs`, `compared_to_default` (`wider`, `covered`, or `unknown`), and `confidence` (`none`, `no_track`, `sparse` when fewer than three measurable leads, or `observed`).

`suggested_window_before_secs` is the longest measured lead, plus 10 percent (at least a minute), rounded up to a minute, and clamped between 10 minutes and 12 hours. It is null when that lead is zero or negative: the stamp is not before the event, which is common for a train that has already arrived, and `compared_to_default` is then `unknown`. It is `wider` when the suggestion is longer than the global 1800 seconds and `covered` when the tracks already show up inside that global window. `suggested_hot_window_secs` widens past 720 only when the median chosen lead is 15 minutes or less.

`AMTRAK_TRACKS_LEAD_TIME_FILE` points at this file. The Rust loader and `fetcher/lead_time.py` apply it the same way:

1. Adopt a station only when `confidence` is `observed` and `compared_to_default` is `wider`. That station's lookahead becomes `suggested_window_before_secs`, and its hot window becomes `suggested_hot_window_secs` when that value is a positive integer.
2. Leave `sparse` stations on the global windows even when their suggestion is `wider`. Three measurable leads are required before a station changes the poll.
3. Leave `covered`, `unknown`, `no_track`, `none`, and missing stations on `TRACK_WINDOW_BEFORE_SECS` and `TRACK_HOT_WINDOW_SECS`. A `covered` station is not narrowed: Providence posting about ten minutes ahead stays inside the 30-minute window.
4. Leave `TRACK_WINDOW_AFTER_SECS` global.
5. Prefer `merge` of several snapshots over one survey. Lead time comes from Amtrak's `asOf`, so a single fetch is enough when the track is already posted. It is not enough for a station that posts only in the last minutes, because a too-early fetch has no `asOf` to measure.

`store.tracked_codes` is intentionally not shared with this parser. The hot set still treats any non-empty number, including `TBD`, as a track already seen. This report does not.

### Seeing the same JSON in Chrome

1. Open `https://www.amtrak.com/tickets/train-status.html`.
2. DevTools → Network → Fetch/XHR. Filter by `statuses`.
3. Search the train and the date it left its origin. An overnight train still running on November 1 uses October 31 if that is the day it departed.
4. Open `statuses/{number}?service-date=YYYY-MM-DD`. In the JSON, each `data[0].stops[]` entry has `arrival` and `departure`. The track is `track.number` and `track.asOf`. The gate is `gate.number` and `gate.asOf`.
5. The page may also request `statuses/stations?origin-code=&destination-code=&departure-date=` and `statuses/stops/{STATION}`. The per-train document is the one this tool records stop by stop. The stations response is only checked for whether `track` and `gate` numbers are present.
