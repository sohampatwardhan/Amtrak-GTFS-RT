# Discovery: Advisory Fetcher (Playwright sidecar)

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Problem and Outcome

The service-advisories work ([PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8))
built and unit-tested the full pipeline to turn Amtrak's Service Alerts & Notices into scoped
GTFS-RT alerts — but it ships **default-off** because the feed-producer service **cannot fetch the
source**: `www.amtrak.com` is behind **Akamai Bot Manager**, which resets a plain `reqwest`/`curl`
HTTP/2 stream after the request (it needs a JavaScript-earned `_abck` sensor cookie). This was
reproduced from a clean US IP, so it is IP-independent; the advisory content is server-rendered into
that blocked document, and there is no separate JSON API.

The desired outcome is a **standalone advisory-fetcher component** that uses a **real browser
(Playwright/Chromium)** to earn the Akamai sensor cookie, load the notices page, and write a
**snapshot** (the advisories HTML, or pre-parsed JSON) to a location the service reads via
`AdvisoryConfig.url` (a `file://` path on a shared volume, or the fetcher's localhost endpoint). The
browser — and its CVE surface, resource cost, and the Akamai arms race — stays **isolated in the
fetcher's own image**, so the feed-producer service keeps its scratch/musl, "no-vulnerabilities"
posture unchanged.

## Users and Current Workaround

The user is a **service operator** who wants station and passenger advisories live in the feed.
Today they can't: the service-side feature is inert (default-off) with no reachable source, and a
plain server-side fetch is Akamai-blocked. The only thing that currently obtains the page is a real
browser (how the advisories were even observed).

## Scope and Non-Goals

In scope:

- A **spike** (first and gating) that proves whether a headless — or Xvfb-headful — Playwright
  Chromium, from a server IP, defeats Akamai and returns the advisory markup, and how durably.
- If the spike passes: an advisory-fetcher component that periodically loads the page in a real
  browser and writes a **snapshot** in the format the service consumes.
- The **snapshot contract** between fetcher and service (location + format + freshness/staleness
  semantics), consistent with the service's existing `AdvisoryConfig` source.
- Its own container image and a documented way to run it alongside the service (compose/sidecar).

Out of scope:

- The service-side parsing/mapping/decorator (done and tested in PR #8; unchanged here).
- Bundling a browser into the feed-producer service image (explicitly rejected — it would undo the
  container CVE remediation).
- Consumer-side alert matching (a separate service-advisories follow-up).

## Constraints and Success Measures

Constraints:

- The feed-producer service image and its scratch/musl posture **must not change**; the browser
  lives only in the fetcher image.
- The fetcher is **best-effort**: if it can't produce a fresh snapshot, the service (already
  fail-open) simply serves no/last advisories — never a failed generation.
- Respectful polling (advisories change slowly; cache/poll on the order of the service's static
  refresh, not per-second).
- **Must run on a Raspberry Pi (constrained RAM, ARM64) without OOM.** Chromium is memory-heavy, so
  the fetcher must be **memory-bounded**: a single short-lived browser per poll (launch → fetch →
  close, not a resident browser), `--disable-dev-shm-usage`, disabled cache/GPU, and a hard
  container memory limit; its peak RSS must fit a Pi-class budget with margin. If Chromium cannot be
  made to fit, that is a spike failure → Approach B (a managed API uses **zero local browser RAM**).

Success measures:

- **Spike:** from a server IP, the fetcher retrieves the notices document containing the
  `na-service-alert__*` markup (the same DOM the PR #8 parsers already handle), repeatably, **with
  peak memory measured under a Raspberry-Pi-class cap** (and on ARM64) — proving both the Akamai
  bypass and that it fits the deployment target without OOM.
- **End-to-end:** with the fetcher running, the service (advisories enabled) emits a known station
  advisory (e.g. `ALX`) and a known route advisory (e.g. Hartford Line / Valley Flyer) as scoped
  alerts.

## Approaches Considered

| Approach | Benefits | Costs / risks | Decision |
|---|---|---|---|
| **A. Playwright fetcher sidecar** — real browser earns the Akamai cookie, writes a snapshot the service reads. | Only option that both defeats Akamai and preserves the scratch/musl service image; reuses the finished PR #8 parsing/decorator; browser CVEs/cost isolated. | New component + image + deploy; headless is the most-detected mode (may need Xvfb-headful + stealth); ongoing Akamai arms-race maintenance. | **Chosen** (user-selected) — but **gated on the spike** |
| **B. Managed scraping/bot-bypass API** — a third-party service fetches the page. | No browser to run/maintain; provider handles Akamai. | External paid dependency, API key, data leaves to a third party; still brittle. | Fallback if the spike fails |
| **C. Bundle a browser in the service image** | One container. | Undoes the container CVE remediation (glibc+Chromium, ~1 GB, reintroduced CVE surface). | Rejected |
| **D. Abandon advisories** | Zero cost. | Loses the station/passenger advisory feature. | Rejected while the spike is worth trying |

Library/capability note: Playwright drives real Chromium (executes Akamai's JS sensor); this is the
standard mechanism for Akamai-protected pages. The exact viability from a *server* IP in *headless*
mode is precisely what the spike must establish — not assumed here.

## Chosen Direction

Build the **Playwright fetcher as a standalone sidecar**, **spike-first**: prove the Akamai bypass
from a server IP before building the component around it. On success, the fetcher writes a snapshot
the (already-built, fail-open) service consumes via `AdvisoryConfig`; on spike failure, fall back to
a managed scraping API (Approach B) or shelve the feature — without touching the service image
either way.

## Architecture and Flow Outline

```
┌─ advisory-fetcher (own image: Playwright + Chromium; headless or Xvfb) ─┐
│  real browser → earns Akamai _abck sensor cookie → loads notices page    │
│  → writes snapshot (advisories HTML or pre-parsed JSON) on a schedule     │
└───────────────┬───────────────────────────────────────────────────────────┘
                ▼  shared volume (file://) or localhost endpoint
┌─ amtrak-gtfs-rt-service (scratch/musl, unchanged; PR #8 code) ───────────┐
│  AdvisoryConfig.url → snapshot → existing parsers/decorator → scoped alerts│
└─────────────────────────────────────────────────────────────────────────────┘
```

## Failure and Verification Strategy

- **Spike gate:** if headless Chromium is detected/blocked, try Xvfb-headful + stealth; if still
  blocked from a server IP, the chosen approach fails → Approach B or shelve. The spike must report
  repeatability, not a single lucky fetch.
- **Best-effort runtime:** a fetcher failure leaves the last snapshot (or none); the service stays
  fail-open. No generation ever fails because of the fetcher.
- **Isolation:** browser and its dependencies exist only in the fetcher image; verify the service
  image is byte-for-byte unchanged.
- **Memory safety on a Pi:** the fetcher runs under a hard container memory limit; if Chromium hits
  it, the fetcher is OOM-killed and restarts in isolation — because it is a separate container, that
  never affects the fail-open service, which just keeps serving the last/no advisories. The spike
  must show steady-state peak RSS comfortably under the Pi budget, not merely a one-off success.

## Open Decisions

1. **Spike outcome** — headless vs Xvfb-headful; whether a server IP passes at all (drives
   everything downstream).
2. **Memory bounding on a Pi** — the launch-per-poll + flags budget, the container memory cap, and
   whether to use `chromium-headless-shell` (lighter) or a beefier non-Pi host for the fetcher if
   Chromium can't fit the Pi.
3. **Fetcher language/runtime** — Playwright Node vs Python (both first-class); pick in design.
4. **Snapshot format** — raw HTML (service parses, reusing PR #8 code) vs pre-parsed JSON (fetcher
   parses). Raw HTML maximizes reuse of the tested parsers.
5. **Transport** — shared volume `file://` vs a localhost HTTP endpoint the service GETs.
6. **Deployment** — docker-compose sidecar vs separate scheduled job.

## Approval

Status: **Approved on 2026-08-18**
