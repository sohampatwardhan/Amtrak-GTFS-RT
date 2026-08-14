# syntax=docker/dockerfile:1
#
# Hardened multi-stage image for the Amtrak GTFS-RT service.
#
# The final image is assembled from scratch. Build tools, package databases, shells, download
# clients, and the source tree stay in disposable stages. The only executable application
# artifacts copied forward are the locked Rust service and a byte-reproducible validator built
# from the pinned MobilityData v8.0.1 source with reviewed dependency fixes.

ARG OCI_VERSION=dev
ARG OCI_REVISION=unknown

# -------------------------------------------------------------------------------------------------
# Stage 1 — locked Rust release builder
# -------------------------------------------------------------------------------------------------
FROM rust:1.96-alpine@sha256:a41f7740f8b45d45795624eec13a8b42263cc700f19f7e4e86e04d3dda08a479 AS builder

RUN --mount=type=cache,target=/var/cache/apk,sharing=locked \
    apk add \
        build-base \
        ca-certificates \
        git \
        openssl-dev \
        openssl-libs-static \
        pkgconf \
        protobuf-dev

WORKDIR /build

# Build against musl and statically link the inherited native-tls/OpenSSL path. The only native
# runtime dependency is then musl itself, shared with the Java runtime below.
RUN --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=bind,source=src,target=src \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=amtrak-target-musl-static-openssl,target=/build/target \
    OPENSSL_STATIC=1 \
    OPENSSL_LIB_DIR="$(pkg-config --variable=libdir openssl)" \
    OPENSSL_INCLUDE_DIR="$(pkg-config --variable=includedir openssl)" \
    cargo build --locked --release && \
    mkdir -p /out && \
    cp /build/target/release/amtrak-gtfs-rt-service /out/amtrak-gtfs-rt-service && \
    ! ldd /out/amtrak-gtfs-rt-service | grep -E 'lib(ssl|crypto|gcc_s|stdc\+\+)\.so'

# -------------------------------------------------------------------------------------------------
# Stage 2 — validator source, test, reproducibility, and digest gate
# -------------------------------------------------------------------------------------------------
FROM eclipse-temurin:17-jdk@sha256:f9b295135b39ed8c650c713c6116600dd4c39ac5f3883f566d96fdec917ce3b2 AS validator

RUN apt-get update && \
    apt-get install --yes --no-install-recommends git && \
    rm -rf /var/lib/apt/lists/*

# The commit is the v8.0.1 release. The archive and the rebuilt CLI JAR are both immutable gates;
# overriding the URL cannot change accepted bytes.
ARG VALIDATOR_SOURCE_URL=https://github.com/MobilityData/gtfs-validator/archive/d74d7177f9f7c6bc7adc69508bb939362f2cf770.tar.gz

COPY container/validator-dependency-overrides.gradle /opt/amtrak/validator-dependency-overrides.gradle
COPY container/validator-verification-metadata.xml /opt/amtrak/validator-verification-metadata.xml

WORKDIR /build
RUN --mount=type=cache,target=/root/.gradle \
    curl --fail --location --silent --show-error \
      --output /tmp/validator-source.tar.gz "${VALIDATOR_SOURCE_URL}" && \
    echo "651872b1a7abbde5b999d7261f875532eebaee22a9d7ce4946b8f764cdf7b8a3  /tmp/validator-source.tar.gz" \
      | sha256sum --check --strict && \
    mkdir -p /build/validator && \
    tar --extract --gzip --file /tmp/validator-source.tar.gz \
      --directory /build/validator --strip-components=1 && \
    cd /build/validator && \
    printf '\ndistributionSha256Sum=8cc27038d5dbd815759851ba53e70cf62e481b87494cc97cfd97982ada5ba634\n' \
      >> gradle/wrapper/gradle-wrapper.properties && \
    cp /opt/amtrak/validator-verification-metadata.xml gradle/verification-metadata.xml && \
    git init --quiet && \
    git config user.name container-build && \
    git config user.email container-build@invalid && \
    git add --all && \
    git commit --quiet --message 'pinned MobilityData v8.0.1 source' && \
    git tag v8.0.1 && \
    ./gradlew --no-daemon \
      --dependency-verification=strict \
      --init-script /opt/amtrak/validator-dependency-overrides.gradle \
      test :cli:shadowJar && \
    echo "24ca7e890ca15bfbb36fa889fcb16200f7276995b7e6ec75551a8b7175e818d7  cli/build/libs/gtfs-validator-8.0.1-cli.jar" \
      | sha256sum --check --strict && \
    mkdir -p /out && \
    cp cli/build/libs/gtfs-validator-8.0.1-cli.jar \
      /out/gtfs-validator-v8.0.1-amtrak-hardened.1-cli.jar

# -------------------------------------------------------------------------------------------------
# Stage 3 — construct the package-manager-free runtime filesystem
# -------------------------------------------------------------------------------------------------
FROM amazoncorretto:17-alpine-jdk@sha256:e1138bf0cca62e04692de650ffe8923f35c39fcb554458c7acd98efc2d135144 AS runtime-files

USER 0

# Generate a Java 17 runtime containing only the four modules used by the validator, then copy its
# musl/zlib closure and minimal identity/configuration files. BusyBox, apk, libssl, and libcrypto
# are intentionally excluded from the final image.
RUN set -eu; \
    /usr/lib/jvm/java-17-amazon-corretto/bin/jlink \
      --module-path /usr/lib/jvm/java-17-amazon-corretto/jmods \
      --add-modules java.base,java.desktop,java.logging,java.xml \
      --no-man-pages \
      --no-header-files \
      --compress=2 \
      --output /runtime/opt/java; \
    mkdir -p \
      /runtime/opt/amtrak \
      /runtime/usr/lib \
      /runtime/lib \
      /runtime/etc/ssl/certs \
      /runtime/data \
      /runtime/tmp; \
    chmod 0555 /runtime/opt/amtrak; \
    cp -a /lib/ld-musl-*.so.1 /runtime/lib/; \
    cp -a /usr/lib/libz.so.1* /runtime/usr/lib/; \
    cp /etc/ssl/certs/ca-certificates.crt /runtime/etc/ssl/certs/ca-certificates.crt; \
    printf 'amtrak:x:10001:10001:Amtrak service:/data:/sbin/nologin\n' > /runtime/etc/passwd; \
    printf 'amtrak:x:10001:\n' > /runtime/etc/group; \
    chown 10001:10001 /runtime/data; \
    chmod 1777 /runtime/tmp

# -------------------------------------------------------------------------------------------------
# Stage 4 — final scratch image
# -------------------------------------------------------------------------------------------------
FROM scratch AS runtime

ARG OCI_VERSION
ARG OCI_REVISION

LABEL org.opencontainers.image.title="Amtrak GTFS-RT" \
      org.opencontainers.image.description="Validated static and realtime GTFS feeds for Amtrak" \
      org.opencontainers.image.source="https://github.com/sohampatwardhan/Amtrak-GTFS-RT" \
      org.opencontainers.image.documentation="https://github.com/sohampatwardhan/Amtrak-GTFS-RT#container" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.version="$OCI_VERSION" \
      org.opencontainers.image.revision="$OCI_REVISION"

COPY --from=runtime-files /runtime/ /
COPY --from=builder --chmod=0555 /out/amtrak-gtfs-rt-service /usr/local/bin/amtrak-gtfs-rt-service
COPY --from=validator --chmod=0444 \
    /out/gtfs-validator-v8.0.1-amtrak-hardened.1-cli.jar \
    /opt/amtrak/gtfs-validator-v8.0.1-amtrak-hardened.1-cli.jar
COPY --chmod=0444 container/licenses/AGPL-3.0-only.txt /licenses/AGPL-3.0-only.txt
COPY --chmod=0444 THIRD_PARTY_LICENSES.html /licenses/THIRD_PARTY_LICENSES.html

ENV JAVA_HOME=/opt/java \
    PATH=/opt/java/bin \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    AMTRAK_OUTPUT_DIR=/data \
    AMTRAK_GTFS_VALIDATOR_JAR=/opt/amtrak/gtfs-validator-v8.0.1-amtrak-hardened.1-cli.jar \
    AMTRAK_BIND_ADDR=127.0.0.1:8080

VOLUME ["/data"]
EXPOSE 8080/tcp

USER 10001:10001

# The service performs its own bounded loopback HTTP probe, so the production image does not need
# curl, a shell, or another general-purpose utility.
HEALTHCHECK --interval=30s --timeout=5s --start-period=120s --retries=3 \
    CMD ["/usr/local/bin/amtrak-gtfs-rt-service", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/amtrak-gtfs-rt-service"]
