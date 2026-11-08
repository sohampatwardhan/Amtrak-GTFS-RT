"""Routes for the latest notices snapshot. :mod:`worker.server` serves them.

The service GETs this endpoint (its `AMTRAK_ADVISORIES_URL`) on its own TTL and parses the body.
Freshness is encoded as the HTTP status the service already understands (Requirements 2.1, 2.3,
3.1): a fresh snapshot is `200 text/html`; a missing or stale one is `503`, which the service
treats as a fetch failure and falls open (serves last-good or no advisories). Routing does no
browser work (Requirement 8.1).
"""

from __future__ import annotations

from urllib.parse import urlsplit

from .store import SnapshotStore, safe_detail_name

HEALTH_PATH = "/healthz"


def route(
    store: SnapshotStore,
    serve_path: str,
    max_stale_secs: int,
    path: str,
    now: float,
    detail_max_stale_secs: int | None = None,
) -> tuple[int, str, bytes]:
    """Decide the response for a GET, as ``(status, content_type, body)``.

    Pure and side-effect free so it can be unit-tested without a socket:

    * ``serve_path`` → ``200 text/html`` with the snapshot while ``now - last_success`` is within
      ``max_stale_secs``; ``503`` when no snapshot exists yet or it is stale (freshness as status,
      R2.3/R3.1).
    * ``/alert/<slug>.html`` → the matching detail snapshot while it is within
      ``detail_max_stale_secs`` (the list limit when that argument is omitted); ``404`` when
      that detail was never stored (the service then keeps a title-only description).
    * ``/healthz`` → ``200`` liveness.
    * anything else → ``404``.
    """
    if path == HEALTH_PATH:
        return 200, "text/plain", b"ok"
    clean = urlsplit(path).path
    if clean == serve_path:
        snapshot = store.read()
        if snapshot is None:
            return 503, "text/plain", b"no snapshot yet"
        data, last_success = snapshot
        if now - last_success > max_stale_secs:
            return 503, "text/plain", b"snapshot stale"
        return 200, "text/html", data
    if safe_detail_name(clean) is not None:
        snapshot = store.read_detail(clean)
        if snapshot is None:
            return 404, "text/plain", b"detail not snapshotted"
        data, last_success = snapshot
        detail_limit = max_stale_secs if detail_max_stale_secs is None else detail_max_stale_secs
        if now - last_success > detail_limit:
            return 503, "text/plain", b"snapshot stale"
        return 200, "text/html", data
    return 404, "text/plain", b"not found"

