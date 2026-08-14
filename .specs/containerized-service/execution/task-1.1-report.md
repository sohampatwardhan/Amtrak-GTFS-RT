# Task 1.1 Report — Implement the multi-stage Docker image

**Files:** [`Dockerfile`](../../../Dockerfile), [`.dockerignore`](../../../.dockerignore)
**Mode:** controller (executed directly, not delegated)
**Requirements:** 1.1–1.5, 2.1–2.5, 3.1–3.6, 4.1, 5.1, 5.2, 6.1, 6.2

## What was built

A three-stage BuildKit Dockerfile:

1. **Builder** (`rust:1.96-slim-bookworm` pinned to index digest) installs only build tooling
   (`build-essential`, `ca-certificates`, `git`, `libssl-dev`, `pkg-config`,
   `protobuf-compiler`), bind-mounts only `Cargo.toml`, `Cargo.lock`, and `src/` read-only, uses
   Cargo registry/git/target cache mounts, and runs `cargo build --locked --release`. The binary
   is copied to `/out` inside the same layer before the target cache disappears.
2. **Validator** (Debian 12 slim, same runtime digest) downloads the MobilityData validator from
   an overridable `VALIDATOR_URL` ARG and verifies it against the **inlined, non-overridable**
   SHA-256 `19293ddd…1000e2`. Only accepted bytes are copied forward.
3. **Runtime** (Debian 12 slim pinned to index digest) installs only `ca-certificates`, `curl`,
   `libdigest-sha-perl`, `libssl3`, `openjdk-17-jre-headless`, removes apt indexes, creates
   UID/GID 10001, copies the root-owned binary and JAR, creates `/data` owned by `10001:10001`,
   sets the safe env defaults, exposes `8080/tcp`, declares `VOLUME ["/data"]`, runs as
   `10001:10001`, adds the `/livez` loopback healthcheck, and runs the Rust binary as PID 1.

`.dockerignore` excludes VCS metadata, host build output, generated feeds, validator caches,
specs, security reports, docs/markdown, env files, credentials, keys, certificates, and editor
artifacts (defense-in-depth on top of the read-only build mounts).

## Verification evidence

- `docker build --tag amtrak-gtfs-rt:local .` → exit 0. Final image **156 MB** uncompressed.
- Image config inspection: `User=10001:10001`; entrypoint
  `["/usr/local/bin/amtrak-gtfs-rt-service"]`; env carries `AMTRAK_OUTPUT_DIR=/data`,
  `AMTRAK_GTFS_VALIDATOR_JAR=/opt/amtrak/gtfs-validator-v8.0.1-cli.jar`,
  `AMTRAK_BIND_ADDR=127.0.0.1:8080`, and **no** baked peer allowlist; `VOLUME=/data`;
  `ExposedPorts=8080/tcp`; healthcheck runs `/livez` with 30s interval, 5s timeout, 120s start
  period, 3 retries.
- Runtime assertions (as the image user): `java -version` → OpenJDK 17.0.20; `shasum` present;
  TLS roots at `/etc/ssl/certs/ca-certificates.crt`; `id` → `uid=10001 gid=10001`; packaged
  validator SHA-256 equals the pinned digest.
- Absence assertions: `cargo`, `rustc`, `protoc` all absent; no `/build`, `/src`, or `/Cargo.toml`
  in the final filesystem; no secret-shaped files; binary and validator are read-only to the
  service user; `/data` is writable.
- **Negative test:** rebuilding with `--build-arg VALIDATOR_URL=<v7.1.0 jar>` fails the validator
  stage with a SHA-256 mismatch (`did not complete successfully: exit code: 1`); no image is
  produced. This proves the digest cannot be bypassed by overriding the URL.
- `git diff --check` clean on both files.

Requirements 1.1–6.2 as cited are satisfied. Docker Scout SBOM/CVE export is deferred to task 2.1,
which owns image-security evidence per the task plan.
