# 2026-11 Cargo Dependency Risk Acceptance

| Field | Value |
|---|---|
| Status | **Pending owner approval** |
| Supersedes | [v0.2.0 Cargo dependency risk acceptance](v0.2.0-cargo-dependencies.md) (expires 2026-11-12) |
| Prepared | 2026-11-07 |
| Audit revision | `088104119f86c09b0073674791fd531115316ab3` |
| Inventory | 375 packages, 969 dependency edges; complete |
| Inventory fingerprint | `613f04a586eb3b2235046ba81d7d2c757a559c1e8613723c3341546ef34d01fc` |
| Audits | [main 2026-11-07](../dependency-audit/main-2026-11-07.md) (WARNINGS, 0 blocking); [release 2026-11-07](../dependency-audit/release-2026-11-07.md) (UNAVAILABLE until this record is approved) |
| Reachability evidence | [2026-11-cargo-reachability.txt](evidence/2026-11-cargo-reachability.txt), [.json](evidence/2026-11-cargo-reachability.json) |
| Scope | `main` and the next release cut from it; re-review on any dependency change |
| Expiry | 2027-02-12 |

## Decision requested

This record asks to accept the exact 16 warning records below through 2027-02-12. It is not a
clean audit. KEV result: none of these records is in the CISA Known Exploited Vulnerabilities
catalog (version 2026.11.06, 1,769 entries; also checked independently). None of them has an
authoritative fixed version that can be reached without upstream changes.

### What changed since v0.2.0

- All nine v0.2.0 warnings are still present. No fix was published for any of them, and
  `catenarytransit/amtrak-gtfs-rt` upstream (`b33dc280`, 2026-11-07, crate 0.10.0) still
  declares `rust-crypto = "0.2"`.
- The fresh audit at `4c328d1` found 16 more records. Commit `0881041` cleared nine of them,
  including the blocking high-severity `quinn-proto` RUSTSEC-2026-0338, with a lockfile-only
  update to each advisory's fixed version: h2 0.4.16, quinn-proto 0.11.18, rustls 0.23.46,
  rustls-webpki 0.103.16 (needs aws-lc-rs 1.19.0 / aws-lc-sys 0.47.0), tokio 1.53.3, and
  webpki-root-certs 1.0.10. `Cargo.toml` is unchanged. `cargo build --locked` and
  `cargo test --locked --all-targets --all-features` pass (188 passed, 6 live or manual tests
  ignored).
- The other seven new records are against `zip 6.0.0`. Their fixes need zip 9.x (a major
  version, and a `Cargo.toml` change). The newest `gtfs-structures` (0.52.0) still requires
  zip `^8.6`, so bumping our direct requirement alone would leave an affected zip in the graph.
  Five of the seven were blocking until the reachability evidence below was supplied.

### Remaining findings

| Exact package | Advisory | Severity | Fix status | Dependency path | Reachability (evidence anchor) |
|---|---|---:|---|---|---|
| `atomic-polyfill 1.0.3` | `RUSTSEC-2023-0089` | Unknown | None; unmaintained | lockfile only: `geo-types → rstar (old) → heapless 0.7.17 → atomic-polyfill` | Not built in any target or feature set (`locked-not-built`) |
| `derivative 2.2.0` | `RUSTSEC-2024-0388` | Unknown | None; unmaintained | `gtfs-structures → derivative` (proc-macro) | Compile-time only (`build-time-only`) |
| `fxhash 0.2.1` | `RUSTSEC-2025-0057` | Unknown | None; unmaintained | `amtrak-gtfs-rt` / service `→ scraper → selectors → fxhash` | **Reachable** from HTML parsers (`fxhash-reachable`) |
| `gcc 0.3.55` | `RUSTSEC-2025-0121` | Unknown | None; deprecated | `amtrak-gtfs-rt → rust-crypto → gcc` (build-dependency) | Build script only (`build-time-only`) |
| `rust-crypto 0.2.36` | `RUSTSEC-2016-0005` | Unknown | No released safe version | `amtrak-gtfs-rt → rust-crypto` | Never referenced; 0 symbols in release binary (`rust-crypto-unreferenced`) |
| `rust-crypto 0.2.36` | `RUSTSEC-2022-0011` / `GHSA-JP3W-3Q88-34CF` | Critical | None published | `amtrak-gtfs-rt → rust-crypto` | Same as above |
| `rustc-serialize 0.3.25` | `RUSTSEC-2022-0004` / `GHSA-2226-4V3C-CFF8` | Medium | None published | `amtrak-gtfs-rt → rust-crypto → rustc-serialize` | Same as above |
| `rustc-serialize 0.3.25` | `RUSTSEC-2025-0025` | Unknown | None; unmaintained | `amtrak-gtfs-rt → rust-crypto → rustc-serialize` | Same as above |
| `time 0.1.45` | `RUSTSEC-2020-0071` / `CVE-2020-26235` | Medium | Needs rust-crypto removed upstream | `amtrak-gtfs-rt → rust-crypto → time` | Same as above; service never calls `set_var` |
| `zip 6.0.0` | `GHSA-8HXF-QR8P-62X4` / `CVE-2026-115906` | Medium | 9.0.0; not reachable via gtfs-structures | service direct; `gtfs-structures → zip` | `aes-crypto` feature off (`enabled-zip-features`) |
| `zip 6.0.0` | `GHSA-9HJ8-73FH-9963` / `CVE-2026-115907` | High | 9.0.0; not reachable via gtfs-structures | as above | `legacy-zip` off; no `extract()` (`enabled-zip-features`) |
| `zip 6.0.0` | `GHSA-M665-V7PM-7GRJ` / `CVE-2026-115905` | Critical | 9.0.5; not reachable via gtfs-structures | as above | No `ZipStreamReader` (`zip-api-call-sites`); see residual |
| `zip 6.0.0` | `RUSTSEC-2026-0367` / `GHSA-VV5M-JXXH-4627` | High | 9.0.0; not reachable via gtfs-structures | as above | No `extract()` (`zip-api-call-sites`) |
| `zip 6.0.0` | `RUSTSEC-2026-0368` / `GHSA-4V9H-P7M9-RH26` | Unknown | 9.0.0; not reachable via gtfs-structures | as above | No `extract()` (`zip-api-call-sites`) |
| `zip 6.0.0` | `RUSTSEC-2026-0369` / `GHSA-4684-H2Q9-2VCP` | Critical | 9.0.0; not reachable via gtfs-structures | as above | No `ZipStreamReader::extract` (`zip-api-call-sites`) |
| `zip 6.0.0` | `RUSTSEC-2026-0429` / `GHSA-Q7V9-X93C-XQV3` | High | 9.0.0; not reachable via gtfs-structures | as above | No `extract()` (`zip-api-call-sites`) |

The reachability claims come from a manual review: `cargo tree`, source grep across this crate,
`gtfs-structures 0.46.1`, and `amtrak-gtfs-rt f3a83db0`, and `nm` on a host
(aarch64-apple-darwin) release build. The musl container binaries were not symbol-inspected.
The owner should review the claims before approving.

### Residual risk this record does not exclude

- `official-static.zip` is served byte for byte. If `content.amtrak.com` served a crafted
  archive, a consumer that stream-reads it could see different entry names than the service's
  central-directory view (GHSA-M665). The download uses HTTPS, and the exact bytes must pass
  the MobilityData validator before they are published.
- `ZipArchive` still parses upstream-controlled headers. None of the seven zip records is a
  `ZipArchive`-only read flaw. A future one would be.
- The rust-crypto exclusion holds only while the catenary crate never calls `crypto::`.

## Mitigations and controls (current three-container layout)

- One Rust binary runs in three Compose services selected by `AMTRAK_ROLE`. Only
  `vehicle-positions` publishes a port, `127.0.0.1:8090` on the host. `trip-updates` and
  `alerts` have no published port.
- The `vehicle-positions` image is the root `Dockerfile` `runtime` stage: `FROM scratch`, UID/GID
  `10001`, no shell or package manager. `trip-updates` and `alerts` are `python:3.12-slim-bookworm`
  images with Chromium that run as the unprivileged `fetcher` user. They have a larger surface
  than the scratch image and are not covered by the scratch-image claims.
- Controlled routes authorize the direct socket peer. Compose sets an explicit
  `AMTRAK_ALLOWED_PEER_IPS`, and a non-loopback bind without an allowlist refuses to start.
  `Forwarded` and `X-Forwarded-For` are ignored.
- The static ZIP comes only from `content.amtrak.com` over HTTPS. It is validated with the
  digest-pinned MobilityData validator before publication. A failure keeps the previous
  generation.
- The `release-gates` workflow builds the image, runs the container smoke test, and applies the
  pinned Grype zero-match policy. Grype does not see Cargo advisories, so a clean scan does not
  clear the findings above.

## Review triggers

Revoke and re-audit on any of these:

- any `Cargo.lock` or direct-dependency change, including a move of the `amtrak-gtfs-rt` git
  revision (re-check the rust-crypto exclusion);
- a `gtfs-structures` release that accepts zip ≥ 9.0.5;
- any new zip advisory that affects the `ZipArchive` read path;
- a new CISA KEV match;
- a newly published fix;
- evidence of reachability or exploitation;
- a failed release gate;
- expiry on 2027-02-12.

## Approval record

**Pending.** No approval has been given. The owner must approve this exact set in conversation.
Proposed approval sentence:

> I explicitly accept the documented sixteen-warning Cargo dependency risk in
> .security/risk-acceptance/2026-11-cargo-dependencies.md through 2027-02-12.

On approval, record the date and the sentence here, change Status to **Approved**, and put the
same approval text in each `approval` field of
[2026-11-cargo-dependencies.json](2026-11-cargo-dependencies.json). Those fields are empty now,
so `--acceptance` rejects the file (fail-closed). Then re-run the release-mode audit with
`--acceptance` and `--reachability`.
