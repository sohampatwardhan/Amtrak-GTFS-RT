# Amtrak Status Tracks Hardening Plan

**Goal:** Publish Amtrak's own track numbers (from the train-status API, read by the `track-fetcher` sidecar) without letting a blocked sidecar, a bad label, or a busy hour degrade the feed or a Raspberry Pi.

**Architecture:** Unchanged from PR #22. A resident-browser sidecar polls `www.amtrak.com/dotcom/travel-service/statuses/{train}` for trains near a stop and serves `/tracks.json`. The scratch Rust service reads it in the existing track refresher, merges it with NJ Transit and Hartford, keeps a seen track until departure, and adds newly seen Amtrak labels to the augmented static feed. This plan adds the safeguards from the PR #22 review.

## Constraints

- The Rust image stays scratch and browser-free. Tracks stay default-off.
- No feed generation waits on the sidecar, on disk I/O for observations, or on a static rebuild.
- A track is stamped only when its platform stop is in the active static snapshot (already true).
- Every failure mode fails toward "publish fewer tracks", never toward "publish a stale or wrong track".

## Decisions

### 1. Precedence respects freshness

- Amtrak ranks first only while fresh: the document behind the row was fetched by the sidecar within `AMTRAK_TRACKS_MAX_AGE_SECS`.
- A retained Amtrak row from an older document still publishes until departure plus grace, but ranks below NJ Transit and Hartford, so a live board reading of a track change wins.
- Freshness is measured from the sidecar's fetch time: each bundled document carries `fetchedAt` (clamped to the service's clock), so a document the sidecar keeps re-serving does not look new.
- An upsert with an older `fetchedAt` than the stored document is ignored.
- `source_precedence` sits above the `AssignmentStore` docs, which it had displaced.

### 2. The sidecar is honest about failure

- A cycle marks success only when it had nothing to fetch or stored at least one document. If every status fetch fails, `/tracks.json` turns `503` after `MAX_STALE_SECS`.
- `/healthz` reports `503` when no cycle has finished within `LIVENESS_SECS` (900), and the image's `HEALTHCHECK` uses it.
- Unusable answers are counted per cycle with their reason (`rejected={'http 403': 3}`).
- The service logs the cause of a failed sidecar read (connection error or HTTP status) and leaves body validation to the parser.

### 3. Browser lifecycle is bounded

- A browser failure (launch, crash, or a fetch exceeding its timeout) ends the cycle's fetches.
- Relaunch backs off from 60 s, doubling to `RELAUNCH_BACKOFF_MAX_SECS` (900), reset by the next successful fetch.
- The in-page `fetch` aborts after `FETCH_TIMEOUT_SECS` (20); `evaluate` is also bounded by `asyncio.wait_for`.
- The browser is recycled after `BROWSER_MAX_AGE_SECS` (6 h).
- The page load waits for `domcontentloaded`, so first-party scripts set their cookies before the first status call.
- `BLOCK_THIRD_PARTY` (default on) aborts every request whose host is not `amtrak.com` or a subdomain, on top of the image, font, media and stylesheet block.

### 4. Fewer, better-targeted requests

- A train is hot only when its next future in-window event is within `TRACK_HOT_WINDOW_SECS` and at least one upcoming in-window stop has no track in its last document. Hot trains are taken next-stop-first.
- Trains whose in-window events are all past, or whose upcoming stops already have tracks, rotate under the cooldown.
- A failed attempt starts the cooldown, so a failing train is not retried every cycle.
- `last_fetched` is pruned to trains still due; the per-train track cache expires with its document.
- Arizona stations (no DST) use `America/Phoenix`.

### 5. Observed platforms are bounded, separate and safe

- `is_track_label` returns to the original grammar (`4`, `14`, `2A`, `A`) plus an explicit named-track list (`MAIN`). The all-uppercase shortcut and its deny-list are gone, so NJ Transit and Hartford parsing is unchanged.
- `SharedPlatforms` keeps configured and observed labels apart. `observed-platforms.txt` stores the observed delta only, so removing a configured track takes effect and deleting the file resets observations.
- Only stations in the active static snapshot are observed, refreshed from each staged snapshot. Observations are capped at 1000 labels.
- The file is written atomically (temp file, fsync, rename) once per refresh, outside the table lock, on a blocking thread. A failed write is logged and retried on the next refresh.
- A file that fails to parse is renamed to `*.invalid`, logged, and ignored.
- Static augmentation tries the full table, then the configured table alone, then upstream bytes. One bad observation can no longer remove every platform stop.

### 6. Static rebuilds are rate-limited

- A new observation wakes static refresh after a 30 s debounce, and never sooner than 10 minutes after the previous platform-triggered rebuild.
- Deferred: re-augmenting cached upstream bytes instead of re-downloading. The rate limit bounds the cost for now.

### 7. Packaging and CI

- Compose: `read_only`, tmpfs for `/tmp` and `/home/fetcher`, `cap_drop: [ALL]`, `no-new-privileges`, `init`, `mem_limit: 768m`, `pids_limit: 256`, no published port.
- Dockerfile: `HEALTHCHECK` on `/healthz`. `LISTEN_HOST` is configurable (default `0.0.0.0` inside the container).
- CI builds the `track-fetcher` image natively on `ubuntu-24.04` and `ubuntu-24.04-arm`, which runs the Dockerfile's `ldd` check on both architectures, and validates the `tracks` Compose profile.

### Not changed

- Chromium's own sandbox stays off (Playwright's default); enabling it needs user namespaces the hardened container does not grant. The container is the boundary, and third-party hosts are blocked.
- Two service dates of the same train number at the same stop still share a board key. It needs a multi-day train at the same station twice within the retention window.
- Wall-clock times inside a fall-back hour are read as the first occurrence.

## Tasks

- [x] Rust: freshness-aware precedence, `fetchedAt` per document, doc-comment placement, refresher error causes (`store.rs`, `amtrak.rs`, `refresher.rs`)
- [x] Rust: label grammar, observed delta, known stations, cap, atomic persistence, configured-only fallback, rebuild rate limit (`mod.rs`, `static_augment.rs`, `static_gtfs.rs`, `main.rs`)
- [x] Sidecar: honest success, liveness, browser backoff/timeouts/recycle, third-party blocking, targeted selection, pruning, Arizona (`fetcher/*`)
- [x] Packaging: Compose hardening, healthcheck, `LISTEN_HOST`, CI image build
- [x] Tests for each decision; README, sidecar README and CHANGELOG updated

## Verification

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
- `python -m pytest tests -q` in `track-fetcher/`
- `docker compose -f docker-compose.yml --profile tracks config`
- On device (not in CI): one hour on a Pi 5 with `--profile tracks`, checking the sidecar's RSS stays under `mem_limit`, `rejected=` stays near zero with third-party blocking on, and NYP/PVD tracks appear in trip updates.
