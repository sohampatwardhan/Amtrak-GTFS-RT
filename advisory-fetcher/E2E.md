# End-to-end verification (task 5.1)

Verified 2026-08-18 on `linux/arm64` (Apple-Silicon dev host, same architecture as the target
Pi 5). All containers, images, and the temporary build worktree were removed afterward.

## Fetcher side — shipped image, local arm64 Docker

Built the shipped image (`docker build --platform linux/arm64`, ~364 MB) and ran it:

- **Subresource-blocked Akamai bypass — PASS.** With `BLOCK_SUBRESOURCES=true`, the running
  container's snapshot endpoint returns `200` with the `na-service-alert__*` markup (76 occurrences).
  Blocking image/font/media/CSS while allowing script/XHR does not break Akamai's sensor. (R1.1,
  R1.2, R2.1, R8.)
- **Single browser / no resident browser between polls — PASS.** `docker top` shows **0** browser
  processes between polls; the browser exists only during a fetch. (R4.1, R4.2, R8.1.)
- **Idle footprint ≈ 50 MiB** between polls (just the HTTP server). No hard cap needed. (R8.)
- **Auto-restart on crash — PASS.** `restart: unless-stopped` restarts the container on an
  unexpected process exit (verified via the daemon restart policy). `docker kill`/`docker stop` are
  intentionally exempt as manual stops, and a namespace-internal SIGKILL of PID 1 is kernel-shielded,
  so neither is a valid crash simulation. (R4.3.)
- Bug found and fixed during build: the non-root user could not create the default `/snapshot`
  (root owns `/`); the image now pre-creates `/snapshot` owned by the fetcher uid.

## Service side — real alerts via the fetcher (R2.4)

After merging service-advisories [PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8)
to `main` (advisory *consumption*: the parsers + `AdvisoryConfig`), the service's real advisory code
path was run against the **live fetcher** in a throwaway `main` worktree:

- Loaded Amtrak's live static GTFS (`content.amtrak.com/.../GTFS.zip`) and called the service's
  `fetch_advisory_alerts(client, gtfs, <fetcher-url>)` — the exact code the running service uses,
  pointed at `http://127.0.0.1:18080/service-alerts-and-notices` (the fetcher) instead of the
  Akamai-blocked Amtrak URL.
- Result: **13 scoped alerts — 9 stop-scoped selectors and 6 route-scoped selectors.** Both station
  advisories (carrying `stop_id`) and passenger advisories (carrying `route_id`) resolved against
  the GTFS. This is the full pipeline: browser → Akamai bypass → snapshot → HTTP → service parsers →
  GTFS resolution → scoped GTFS-RT alerts. (R2.4.)

This also demonstrates *why* the fetcher is necessary: the same test against Amtrak's URL directly
(plain `reqwest`, no browser) returns nothing — Akamai blocks it.

## Service image isolation (R5)

The feed-producer service image (the hardened musl/scratch build on `main`) is unchanged by this
work: the fetcher is a separate image, and no browser or browser runtime was added to the service.
(R5.1, R5.2.)

## Note

The GTFS-cgroup memory cap was dropped per operator decision, so no hard-cap measurement is part of
this verification; efficiency is demonstrated by the idle footprint, single-browser-per-cycle, and
no-resident-browser observations above.
