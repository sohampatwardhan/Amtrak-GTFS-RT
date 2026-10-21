#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap. Safe to run twice.
# Installs the host toolchain, Python test dependencies, the locked Cargo
# graph, and the official MobilityData GTFS validator JAR used by `cargo run`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export DEBIAN_FRONTEND=noninteractive

packages=(
  ca-certificates
  curl
  git
  jq
  libssl-dev
  pkg-config
  protobuf-compiler
  python-is-python3
  python3-pip
)
missing=()
for package in "${packages[@]}"; do
  if ! dpkg -s "$package" >/dev/null 2>&1; then
    missing+=("$package")
  fi
done
if [ "${#missing[@]}" -gt 0 ]; then
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends "${missing[@]}"
fi

# The default image ships Rust 1.83. The catenary amtrak-gtfs-rt crate uses
# edition 2024, which needs 1.85+. CI uses stable; the release image pins 1.96.
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain none --profile minimal --no-modify-path
  sudo ln -sfn "$HOME/.cargo/bin/rustup" /usr/local/bin/rustup
  sudo ln -sfn "$HOME/.cargo/bin/cargo" /usr/local/bin/cargo
  sudo ln -sfn "$HOME/.cargo/bin/rustc" /usr/local/bin/rustc
  sudo ln -sfn "$HOME/.cargo/bin/rustdoc" /usr/local/bin/rustdoc
fi

rustup toolchain install stable --profile minimal --component rustfmt,clippy --no-self-update
rustup default stable

if [ -x "$HOME/.cargo/bin/rustfmt" ]; then
  sudo ln -sfn "$HOME/.cargo/bin/rustfmt" /usr/local/bin/rustfmt
  sudo ln -sfn "$HOME/.cargo/bin/cargo-fmt" /usr/local/bin/cargo-fmt
  sudo ln -sfn "$HOME/.cargo/bin/cargo-clippy" /usr/local/bin/cargo-clippy
fi

hash -r
rustc --version
protoc --version

# Java 17+ is required to run the service (the validator probe). Unit tests do
# not start the JVM. The default image already has a newer JDK.
java_major="$(java -version 2>&1 | sed -n 's/.* version "\([0-9][0-9]*\).*/\1/p' | head -n 1 || true)"
if [ -z "${java_major}" ] || [ "${java_major}" -lt 17 ]; then
  sudo apt-get update
  sudo apt-get install -y --no-install-recommends openjdk-17-jre-headless
fi

# System site-packages are not writable by the agent user. sudo keeps pytest
# importable as `python -m pytest` without relying on ~/.local or a shell profile.
sudo python3 -m pip install --disable-pip-version-check --break-system-packages \
  --requirement advisory-fetcher/requirements.txt \
  --requirement track-fetcher/requirements.txt

cargo fetch --locked

# Official host artifact. Startup rejects any other SHA-256. The container
# image uses a different, repository-hardened JAR and must not be substituted.
validator_sha="19293ddd9b6f954f216d4f12054bd8a3232921751c4484339e339764a91000e2"
validator_jar="$ROOT/tools/gtfs-validator-v8.0.1-cli.jar"
validator_url="https://github.com/MobilityData/gtfs-validator/releases/download/v8.0.1/gtfs-validator-8.0.1-cli.jar"
mkdir -p "$ROOT/tools"
if [ ! -f "$validator_jar" ] \
  || ! echo "${validator_sha}  ${validator_jar}" | sha256sum --check --status --strict; then
  tmp="$(mktemp)"
  curl --fail --location --silent --show-error --output "$tmp" "$validator_url"
  echo "${validator_sha}  ${tmp}" | sha256sum --check --strict
  install -m 0644 "$tmp" "$validator_jar"
  rm -f "$tmp"
fi
