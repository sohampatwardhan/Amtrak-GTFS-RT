# Spike results — on-device (Raspberry Pi 5), 2026-08-18

Ran the spike ([spike.py](spike.py)) on the target Pi (`soham@141-hillside-1b`, Comcast residential
IP, aarch64) as a throwaway `docker run --rm` container from
`mcr.microsoft.com/playwright/python:v1.61.0-noble`, spike bind-mounted read-only. The image and
spike files were removed afterward — the Pi was left as found.

## Result 1 — Akamai bypass: PASS (decisive)

**Headless-shell mode retrieved the `na-service-alert__*` markup on 10/10 consecutive cycles
(100% success, longest streak 10).** The real browser defeats Akamai Bot Manager from the Pi's
residential IP, and it does so in the **efficient** `chromium-headless-shell` mode — the Xvfb-headful
fallback was not needed. This answers the load-bearing question (R7.1 bypass, R7.2 repeatability)
in the affirmative.

```
mode=headless-shell cycles=10 success_rate=100% max_consecutive=10
per-cycle (.=markup, x=blocked): ..........
```

## Result 2 — Memory fit under a 1 GB cap: NOT YET CONCLUSIVE (blocked on Pi config)

Two environment issues surfaced:

1. **Playwright is not pre-installed** in this image's default Python (`pip show playwright` empty),
   although the browser binaries are pre-staged at `/ms-playwright` (`chromium_headless_shell-1228`).
   Worked around for the spike with `pip install playwright==1.61.0` +
   `PLAYWRIGHT_BROWSERS_PATH=/ms-playwright`. The production Dockerfile (task 4.1) installs Playwright
   itself, so this does not affect the shipped image.
2. **The Pi's memory cgroup controller is disabled.** `/proc/cmdline` contains
   `cgroup_disable=memory` and `/sys/fs/cgroup/cgroup.controllers` lists only `cpuset cpu io pids`.
   Consequently Docker **discards** `--memory=1g` ("kernel does not support memory limit ...
   Limitation discarded") and `/sys/fs/cgroup/memory.peak` does not exist — so the authoritative
   peak-under-cap measurement (R4.1 / R7.1) cannot be taken as-is.

A best-effort proxy — summing `VmRSS` across all container processes while running 10 cycles —
peaked at **~1007 MiB**, right at the 1024 MiB target. **This proxy overcounts** Chromium's
many shared library pages (per-process RSS double-counts shared memory), so true unique memory is
materially lower; the figure is a loose upper bound, not the accounted memory a cgroup cap enforces.

### To conclude the memory verdict, one of:

- **Enable the memory cgroup on the Pi** (edit `/boot/firmware/cmdline.txt`: drop
  `cgroup_disable=memory`, add `cgroup_enable=memory cgroup_memory=1`; **reboot**), then re-run with
  `--memory=1g` and read `memory.peak` for the authoritative figure. The Pi's cmdline is customized
  (`numa=fake=8`, `system_heap.max_order=0`, …), so this is a deliberate operator change.
- **Or raise the cap.** The Pi has 8 GB; a 1.5–2 GB cap would leave ample margin and still be a
  small fraction of the device. (This would revise R4.1's 1 GB figure.)

## Go / No-Go

- **Bypass: GO** — headless-shell works repeatably and efficiently on the real device.
- **Memory: RESOLVED — no hard cap** (operator decision, 2026-08-18). The hard 1 GB cap was
  dropped, so the disabled memory cgroup is moot and no reboot is needed. Efficiency instead comes
  from launch-per-poll, a single browser at a time, and **subresource-blocking** (skip
  images/fonts/media/CSS during the fetch; Chromium retained for its proven bypass). Requirement 4
  was reframed from a hard cap to memory efficiency + crash resilience.

**Checkpoint 2: PASS** — the gating result (repeatable bypass) holds; build proceeds. On-device
task 5.1 will re-confirm the subresource-blocked bypass and record the observed footprint on the
shipped image (remote SSH, no reboot required).
