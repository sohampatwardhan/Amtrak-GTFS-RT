# Advisory-fetcher spike

The **gating** measurement for the advisory-fetcher (Requirements 7.1, 7.2, and the memory side of
4.1). Before any of the fetcher is built, this spike must prove — on the real Raspberry Pi 5, from
its own residential IP — that a real browser can **repeatably** retrieve Amtrak's Akamai-protected
Service Alerts page (the `na-service-alert__*` markup) and that it **fits the 1 GB** container
budget. If it can't, the project falls back to a managed API or shelves the feature (R7.3), and the
service image is never touched.

## What it measures

For each cycle, `spike.py` records whether the gate markup appeared, then reports:

- **success rate** over the run,
- **longest consecutive-success streak** — the repeatability proof (R7.2 requires ≥ 3), and
- **peak RSS** from cgroup v2 (`/sys/fs/cgroup/memory.peak`) — the memory measurement under the
  1 GB cap (R4.1 / R7.1). Outside a memory-capped container the peak file is absent and prints as
  `unavailable`.

## Modes

- `--mode headless-shell` (default, the efficient path): headless Chromium, no `channel`, which
  Playwright resolves to `chromium-headless-shell`. **Try this first.**
- `--mode xvfb-headful` (fallback): full Chromium (`channel="chromium"`) with basic stealth; run it
  under `xvfb-run` so it has an X server. Only if headless-shell is blocked.
- `--mode fixture --fixture <file.html>`: offline self-test of the reporting logic — no browser,
  no network. Used to verify the harness itself.

## Offline self-test (no browser needed)

```bash
python3 spike.py --mode fixture --fixture fixture_sample.html --cycles 3
```

Expect `success_rate=100% max_consecutive=3`.

## Running on the Pi as a throwaway container

Runs inside a `--rm` container from the official Playwright Python image, capped at 1 GB, with the
spike bind-mounted read-only — **nothing is left on the Pi**. Remove the pulled image afterward.

```bash
# copy spike.py to the Pi first, e.g. scp spike.py soham@141-hillside-1b.tailcb3419.ts.net:/tmp/spike/
IMAGE=mcr.microsoft.com/playwright/python:v1.61.0-noble

# headless-shell first
docker run --rm --memory=1g --ipc=host \
  -v /tmp/spike/spike.py:/spike/spike.py:ro \
  "$IMAGE" python /spike/spike.py --mode headless-shell --cycles 10

# only if headless-shell is blocked: full Chromium under Xvfb
docker run --rm --memory=1g --ipc=host \
  -v /tmp/spike/spike.py:/spike/spike.py:ro \
  "$IMAGE" xvfb-run -a python /spike/spike.py --mode xvfb-headful --cycles 10

# leave the Pi as found
docker rmi "$IMAGE"
```

`--memory=1g` makes cgroup v2 expose `memory.peak` inside the container, so the reported peak RSS
is the real figure under the production cap. Record the winning mode and the numbers in
[`results.md`](results.md) (task 1.2), which drives the Checkpoint 2 go/no-go.
