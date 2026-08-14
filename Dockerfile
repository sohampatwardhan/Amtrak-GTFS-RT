# syntax=docker/dockerfile:1
#
# Multi-stage image for the Amtrak GTFS-RT service.
#
# WHY multi-stage: the build toolchain (Rust, Cargo, protoc, C compiler, the whole
# repository checkout) must never reach the runtime image. Only two artifacts cross the
# boundary into the final stage: the locked release binary and a digest-verified validator
# JAR. Everything else — compilers, package indexes, source, build caches — stays behind.
#
# WHY digest pins: every base `FROM` is pinned to an immutable index digest in addition to a
# human-readable tag. The tag documents intent; the digest is what actually resolves, so a
# retag or a poisoned mirror cannot silently change the bytes we build and ship. See
# .specs/containerized-service/03_design.md ("Current Technology Evidence") for provenance.
#
# Build:  docker build --tag amtrak-gtfs-rt:local .
# Requires BuildKit (default on modern Docker) for the bind/cache mounts and secrets-free
# reproducible caching below.

# ---------------------------------------------------------------------------------------------
# Stage 1 — Rust release builder
#
# WHY these packages only: pkg-config + libssl-dev satisfy the native OpenSSL link path present
# in Cargo.lock; protobuf-compiler (protoc) is required to compile the GTFS-RT .proto bindings;
# git lets Cargo resolve any git dependencies; build-essential provides the C toolchain; and
# ca-certificates lets Cargo and git fetch over HTTPS. None of these are installed in the final
# image.
# ---------------------------------------------------------------------------------------------
FROM rust:1.96-slim-bookworm@sha256:e18a79fc84dfcfc3ab5ba72290398a644c135c97eaa881447fddc354ee4701a3 AS builder

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get install --yes --no-install-recommends \
        build-essential \
        ca-certificates \
        git \
        libssl-dev \
        pkg-config \
        protobuf-compiler

WORKDIR /build

# WHY read-only bind mounts instead of COPY: only the three declared source inputs
# (Cargo.toml, Cargo.lock, src/) are exposed to the compiler, so no unrelated tracked or
# untracked host file can influence the build. WHY the cp inside the same RUN: the Cargo
# registry/git/target directories are BuildKit cache mounts that vanish when the layer
# finishes, so the freshly built binary must be copied to a real path (/out) before then.
# `--locked` forbids any Cargo.lock drift; a stale or edited lockfile fails the build.
RUN --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=src,target=src \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release && \
    mkdir -p /out && \
    cp /build/target/release/amtrak-gtfs-rt-service /out/amtrak-gtfs-rt-service

# ---------------------------------------------------------------------------------------------
# Stage 2 — Validator acquisition gate
#
# WHY a separate stage on the runtime base: the MobilityData GTFS validator is a mutable
# external download, so it is fetched and verified in isolation and only the accepted bytes are
# copied forward. WHY the SHA-256 is hardcoded and the URL is an ARG: the URL may be overridden
# for a mirror or for negative testing, but the expected digest is the identity of the artifact
# and is intentionally NOT configurable — any mismatch fails the build here, before the byte can
# reach the runtime image. This mirrors the runtime startup probe, which re-verifies the JAR.
# ---------------------------------------------------------------------------------------------
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS validator

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && \
    apt-get install --yes --no-install-recommends \
        ca-certificates \
        curl \
        libdigest-sha-perl

# The URL is overridable (ARG) only for a mirror or for negative testing. The expected digest is
# deliberately a literal in the RUN below, NOT an ARG: it is the artifact's identity, so no
# --build-arg can weaken or replace it. Overriding the URL with different bytes therefore fails
# the digest check and aborts the build.
ARG VALIDATOR_URL=https://github.com/MobilityData/gtfs-validator/releases/download/v8.0.1/gtfs-validator-8.0.1-cli.jar

RUN mkdir -p /out && \
    curl --fail --location --silent --show-error --output /tmp/gtfs-validator.jar "${VALIDATOR_URL}" && \
    echo "19293ddd9b6f954f216d4f12054bd8a3232921751c4484339e339764a91000e2  /tmp/gtfs-validator.jar" | shasum -a 256 -c - && \
    cp /tmp/gtfs-validator.jar /out/gtfs-validator-v8.0.1-cli.jar

# ---------------------------------------------------------------------------------------------
# Stage 3 — Debian slim runtime image
#
# WHY Debian slim (glibc) over Alpine (musl): the resolved dependency graph uses a native
# OpenSSL/TLS path and the service shells out to a Java validator; a glibc runtime minimizes
# native-compatibility risk. Alpine is retained in the design as a future *measured*
# optimization, not a second implementation path.
#
# WHY these packages only: libssl3 for the native TLS path, ca-certificates so outbound HTTPS to
# Amtrak/GitHub trusts real roots, openjdk-17-jre-headless to run the validator, curl for the
# healthcheck probe, and libdigest-sha-perl so the running service can re-verify the JAR digest.
# Package indexes are removed so they cannot age into the shipped layer.
# ---------------------------------------------------------------------------------------------
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241 AS runtime

RUN apt-get update && \
    apt-get install --yes --no-install-recommends \
        ca-certificates \
        curl \
        libdigest-sha-perl \
        libssl3 \
        openjdk-17-jre-headless && \
    rm -rf /var/lib/apt/lists/*

# WHY a fixed unprivileged UID/GID 10001: the service never needs root, and a stable numeric
# identity lets an operator pre-own a bind-mounted /data on the host. The binary and JAR are
# copied root-owned (the default), so this user can execute but never modify them.
RUN groupadd --system --gid 10001 amtrak && \
    useradd --system --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin amtrak

COPY --from=builder /out/amtrak-gtfs-rt-service /usr/local/bin/amtrak-gtfs-rt-service
COPY --from=validator /out/gtfs-validator-v8.0.1-cli.jar /opt/amtrak/gtfs-validator-v8.0.1-cli.jar

# WHY /data is created and chowned to the service user: a fresh named volume inherits this
# path's ownership, so the unprivileged process can persist generations without any runtime
# chown. GenerationStore::open (src/writer.rs) remains the sole authority for path safety and
# last-good recovery; the container layer never touches generation bytes directly.
RUN mkdir -p /data && chown 10001:10001 /data

# WHY these defaults: /data is the persistence boundary; the JAR path matches the copy above;
# and the bind address is loopback-only. A loopback default is the safe posture — it works
# directly with host networking but is intentionally unreachable through a plain bridge port
# map. Bridged operation must explicitly set AMTRAK_BIND_ADDR=0.0.0.0:8080 AND an exact
# AMTRAK_ALLOWED_PEER_IPS allowlist; the binary fails closed at startup otherwise. No wildcard
# allowlist is baked into the image.
ENV AMTRAK_OUTPUT_DIR=/data \
    AMTRAK_GTFS_VALIDATOR_JAR=/opt/amtrak/gtfs-validator-v8.0.1-cli.jar \
    AMTRAK_BIND_ADDR=127.0.0.1:8080

# Documents the persistence boundary and the served port; neither grants access on its own.
VOLUME ["/data"]
EXPOSE 8080/tcp

USER 10001:10001

# WHY /livez and not /readyz: the Docker healthcheck reports process *liveness* — is PID 1 up and
# serving HTTP at all. /livez is public by design and never gated by peer policy, so it works over
# loopback inside the container. Feed *readiness* (/readyz) is a separate, peer-gated signal that
# an operator checks explicitly; conflating them would make Docker restart a healthy process that
# is merely waiting for its first good generation. Grace/interval/timeout/retries match the design.
HEALTHCHECK --interval=30s --timeout=5s --start-period=120s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8080/livez || exit 1

# WHY exec form and no wrapper: the Rust binary stays PID 1, so signal handling, exit codes, and
# process supervision behave exactly as the host-run binary. No entrypoint shim is introduced.
ENTRYPOINT ["/usr/local/bin/amtrak-gtfs-rt-service"]
