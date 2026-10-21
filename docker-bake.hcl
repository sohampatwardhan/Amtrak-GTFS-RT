# Builds the images this repository ships, as separate targets.
#
#   docker buildx bake                 # scratch service + advisory-fetcher
#   docker buildx bake service         # scratch service only (advisories stay off)
#   docker buildx bake advisory-fetcher
#   docker buildx bake track-fetcher   # not in the default group
#   docker buildx bake default track-fetcher   # everything the root compose file runs
#
# The service target is the `runtime` stage of the root Dockerfile. The fetcher target
# is advisory-fetcher/Dockerfile. Nothing in the service target copies a fetcher image.
# track-fetcher is built only when named, so the default bake and the scratch release
# stay the same.
#
# Each target is tagged `<name>:local` and with the GHCR name the root compose file runs,
# at AMTRAK_IMAGE_TAG (the release version by default), so `docker compose up` uses what
# bake built. Tag local builds apart from the pulled release with
# `AMTRAK_IMAGE_TAG=local docker buildx bake`. Releases are built and pushed by
# .github/workflows/release.yml, not from here.
#
# FETCHER_BROWSER_BUNDLE=full adds full Chromium and Xvfb to both fetcher images, for
# BROWSER_MODE=xvfb-headful:  FETCHER_BROWSER_BUNDLE=full docker buildx bake

variable "FETCHER_BROWSER_BUNDLE" {
  default = "headless-shell"
}

# Keep in step with Cargo.toml and docker-compose.yml; scripts/check-release-metadata.sh checks it.
variable "AMTRAK_IMAGE_TAG" {
  default = "0.7.0"
}

variable "REGISTRY" {
  default = "ghcr.io/sohampatwardhan"
}

group "default" {
  targets = ["service", "advisory-fetcher"]
}

target "service" {
  context    = "."
  dockerfile = "Dockerfile"
  target     = "runtime"
  tags       = ["amtrak-gtfs-rt:local", "${REGISTRY}/amtrak-gtfs-rt:${AMTRAK_IMAGE_TAG}"]
}

target "advisory-fetcher" {
  context    = "advisory-fetcher"
  dockerfile = "Dockerfile"
  tags       = ["advisory-fetcher:local", "${REGISTRY}/amtrak-gtfs-rt-advisory-fetcher:${AMTRAK_IMAGE_TAG}"]
  args       = { BROWSER_BUNDLE = FETCHER_BROWSER_BUNDLE }
}

target "track-fetcher" {
  context    = "track-fetcher"
  dockerfile = "Dockerfile"
  tags       = ["track-fetcher:local", "${REGISTRY}/amtrak-gtfs-rt-track-fetcher:${AMTRAK_IMAGE_TAG}"]
  args       = { BROWSER_BUNDLE = FETCHER_BROWSER_BUNDLE }
}
