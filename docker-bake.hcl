# Builds the images this repository ships, as separate targets.
#
#   docker buildx bake                 # scratch service + advisory-fetcher
#   docker buildx bake service         # scratch service only (advisories stay off)
#   docker buildx bake advisory-fetcher
#   docker buildx bake track-fetcher   # optional; not in the default group
#
# The service target is the `runtime` stage of the root Dockerfile. The fetcher target
# is advisory-fetcher/Dockerfile. Nothing in the service target copies a fetcher image.
# track-fetcher is built only when named, so the default bake and the scratch release
# stay the same.
#
# FETCHER_BROWSER_BUNDLE=full adds full Chromium and Xvfb to both fetcher images, for
# BROWSER_MODE=xvfb-headful:  FETCHER_BROWSER_BUNDLE=full docker buildx bake

variable "FETCHER_BROWSER_BUNDLE" {
  default = "headless-shell"
}

group "default" {
  targets = ["service", "advisory-fetcher"]
}

target "service" {
  context    = "."
  dockerfile = "Dockerfile"
  target     = "runtime"
  tags       = ["amtrak-gtfs-rt:local"]
}

target "advisory-fetcher" {
  context    = "advisory-fetcher"
  dockerfile = "Dockerfile"
  tags       = ["advisory-fetcher:local"]
  args       = { BROWSER_BUNDLE = FETCHER_BROWSER_BUNDLE }
}

target "track-fetcher" {
  context    = "track-fetcher"
  dockerfile = "Dockerfile"
  tags       = ["track-fetcher:local"]
  args       = { BROWSER_BUNDLE = FETCHER_BROWSER_BUNDLE }
}
