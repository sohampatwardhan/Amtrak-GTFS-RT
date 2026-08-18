# Requirements: Advisory Fetcher (Playwright sidecar)

<!-- spec-nav:start -->
**Spec navigation:** [State](00_state.md) · [Discovery](01_discovery.md) · [Requirements](02_requirements.md) · [Design](03_design.md) · [Tasks](04_tasks.md) · [Execution](05_execution.md)
<!-- spec-nav:end -->

## Introduction

The feed-producer service already contains the full, unit-tested pipeline for turning Amtrak's
Service Alerts & Notices into scoped GTFS-RT alerts ([PR #8](https://github.com/sohampatwardhan/Amtrak-GTFS-RT/pull/8)),
but it ships **default-off** because it cannot fetch the source page: `www.amtrak.com` is behind
Akamai Bot Manager, which resets a plain HTTP/2 request that lacks a JavaScript-earned `_abck`
cookie. This spec covers a **standalone advisory-fetcher** component that uses a real browser to
obtain the page and writes a **snapshot** the service consumes through its existing
`AdvisoryConfig` source, while keeping the browser (and its CVE surface and memory cost) isolated in
the fetcher's own container image.

The work is **spike-first**: whether a server-IP browser actually defeats Akamai, and whether
Chromium fits a Raspberry-Pi memory budget, is unproven and gates the rest of the feature.

### Domain terms

- **Advisory Fetcher** — the new standalone component that runs a browser and writes snapshots.
- **Feed-producer service** — the existing `amtrak-gtfs-rt-service`; unchanged by this spec.
- **Snapshot** — the advisories document the fetcher produces (advisory HTML, or pre-parsed JSON)
  that the service reads via `AdvisoryConfig.url`.
- **`na-service-alert__*` markup** — the advisory DOM classes the PR #8 parsers already consume.
- **Spike** — the gating first phase that proves or disproves the Akamai bypass and the Pi memory
  fit before the component is built.

### Assumptions

- The service's `AdvisoryConfig` snapshot-consumption contract (URL/path source, TTL staleness,
  fail-open) exists and is unchanged; this spec produces the snapshot that contract reads.
- The advisory content is only available as server-rendered HTML in the Akamai-protected page;
  there is no separate JSON API.
- The target deployment device is a **Raspberry Pi 5 (ARM64, 8 GB RAM)**. **No hard container
  memory cap is required** (operator decision): memory stays modest through launch-per-poll and a
  single browser at a time, and the Pi's 8 GB is ample for a per-poll headless Chromium.
- The target device is reachable for on-device execution and confirmed: Pi 5, 4 cores,
  ~7.9 GB RAM, Debian 13 (trixie), **Docker 29.7.2** installed, no pre-existing browser/Node
  runtime. Its egress is a residential IP, and a plain `curl` to the notices page still fails with
  an Akamai HTTP/2 stream reset there — confirming on real hardware that a real browser (not a plain
  fetch) is required even from a residential IP. (The kernel's memory cgroup controller is disabled,
  so a hard `--memory` cap could not be enforced anyway; since no hard cap is required, this does
  not affect the design.)

## Requirements

### Requirement 1: Obtain advisories through a real browser

**User Story:** As a service operator, I want the fetcher to obtain Amtrak's Service Alerts &
Notices page using a real browser, so that Akamai-protected advisory content becomes available to
the feed.

1. **R1.1** WHEN the fetcher begins a poll cycle, THE Advisory_Fetcher SHALL load the Amtrak Service Alerts & Notices page in a real browser that executes the page's JavaScript.
2. **R1.2** WHEN a page load completes successfully, THE Advisory_Fetcher SHALL produce output containing the `na-service-alert__*` advisory markup that the service's parsers consume.
3. **R1.3** IF the browser does not obtain the advisory markup within a bounded per-cycle timeout, THEN THE Advisory_Fetcher SHALL abort the cycle without overwriting a previously valid snapshot.

### Requirement 2: Write a snapshot the service already reads

**User Story:** As a service operator, I want the fetcher to write the advisories to the location
and format the service already consumes, so that no feed-producer service code changes.

1. **R2.1** WHEN a poll cycle obtains the advisory markup, THE Advisory_Fetcher SHALL write it to the snapshot source the service reads via `AdvisoryConfig.url`.
2. **R2.2** WHEN the fetcher writes a snapshot, THE Advisory_Fetcher SHALL make the write atomic so a reader observes either the complete previous snapshot or the complete new one.
3. **R2.3** WHEN the fetcher writes a snapshot, THE Advisory_Fetcher SHALL update the snapshot's freshness marker so the service can detect staleness through its existing TTL.
4. **R2.4** WHEN the feed-producer service reads a fetcher-produced snapshot, THE Feed_Producer_Service SHALL emit the scoped station and route alerts the existing parsers derive from that markup.

### Requirement 3: Best-effort, fail-open operation

**User Story:** As a service operator, I want a fetcher failure to never break the feed, so that
advisories are a best-effort enhancement rather than a dependency.

1. **R3.1** IF the fetcher cannot produce a fresh snapshot, THEN THE Feed_Producer_Service SHALL continue serving feeds using the last snapshot or none, without a failed generation.
2. **R3.2** IF the fetcher process crashes or is killed, THEN THE Feed_Producer_Service SHALL continue serving feeds unaffected.

### Requirement 4: Memory efficiency and resilience

**User Story:** As a service operator, I want the fetcher to use little memory and recover from
crashes on its own, so that it runs unattended on a shared device without needing a hard cap.

1. **R4.1** WHEN a poll cycle runs, THE Advisory_Fetcher SHALL launch a single browser instance at a time so its peak memory stays bounded to one browser rather than growing across cycles.
2. **R4.2** WHEN a poll cycle completes, THE Advisory_Fetcher SHALL close the browser so that no browser process remains resident between polls.
3. **R4.3** IF the fetcher process crashes or is terminated, THEN THE Advisory_Fetcher SHALL restart and resume polling without manual intervention.

### Requirement 5: Keep the browser out of the service image

**User Story:** As a maintainer, I want the browser confined to the fetcher image, so that the
feed-producer service keeps its scratch/musl, no-vulnerabilities container posture.

1. **R5.1** THE Feed_Producer_Service image SHALL remain free of any browser or browser runtime dependency.
2. **R5.2** THE Advisory_Fetcher SHALL package the browser and its dependencies only within its own container image.

### Requirement 6: Respectful polling cadence

**User Story:** As a service operator, I want the fetcher to poll gently, so that it neither
hammers Amtrak nor draws bot-defense attention.

1. **R6.1** WHERE a poll interval is configured, THE Advisory_Fetcher SHALL wait at least that interval between successive page loads.
2. **R6.2** THE Advisory_Fetcher SHALL default its poll interval to the order of the service's static-data refresh rather than a sub-minute rate.

### Requirement 7: Spike gates the build

**User Story:** As a service operator, I want the spike to decide the approach before the component
is built, so that we do not invest in a sidecar that cannot work or cannot fit the device.

1. **R7.1** WHEN the spike runs, THE Advisory_Fetcher_Spike SHALL report whether a server-IP browser obtains the `na-service-alert__*` markup and the success rate over repeated cycles on ARM64.
2. **R7.2** WHEN the spike obtains the markup, THE Advisory_Fetcher_Spike SHALL demonstrate at least three consecutive successful retrievals to establish repeatability rather than a single success.
3. **R7.3** IF the spike cannot obtain the markup repeatably from a server IP, THEN THE project SHALL fall back to the managed-API approach or shelve the feature, without building the sidecar or changing the service image.

### Requirement 8: Resource efficiency

**User Story:** As a service operator, I want the fetcher to stay lightweight, so that it costs
almost nothing to run continuously on the Pi alongside other workloads.

1. **R8.1** WHILE idle between poll cycles, THE Advisory_Fetcher SHALL run no browser process and perform no work beyond waiting for the next scheduled poll.
2. **R8.2** WHEN a freshly fetched advisory document is identical to the current snapshot's content, THE Advisory_Fetcher SHALL refresh the snapshot's freshness marker without rewriting the snapshot payload.
3. **R8.3** WHEN a poll cycle completes or aborts, THE Advisory_Fetcher SHALL release all browser and temporary-file resources it acquired for that cycle.

## Approval

Status: **Approved on 2026-08-18**
