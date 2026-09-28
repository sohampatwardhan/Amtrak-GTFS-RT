# Builds the two images this repository ships, as separate targets.
#
#   docker buildx bake                 # scratch service + advisory-fetcher
#   docker buildx bake service         # scratch service only (advisories stay off)
#   docker buildx bake advisory-fetcher
#
# The service target is the `runtime` stage of the root Dockerfile. The fetcher target
# is advisory-fetcher/Dockerfile. Nothing in the service target copies the fetcher image.

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
}
