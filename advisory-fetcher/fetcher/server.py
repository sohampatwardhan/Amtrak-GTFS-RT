"""HTTP server that exposes the latest advisories snapshot to the feed-producer service.

The service GETs this endpoint (its `AMTRAK_ADVISORIES_URL`) on its own TTL and parses the body.
Freshness is encoded as the HTTP status the service already understands (Requirements 2.1, 2.3,
3.1): a fresh snapshot is `200 text/html`; a missing or stale one is `503`, which the service
treats as a fetch failure and falls open (serves last-good or no advisories). The server does no
browser work and blocks on `accept()` when idle (Requirement 8.1).
"""

from __future__ import annotations

import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .store import SnapshotStore

HEALTH_PATH = "/healthz"


def route(
    store: SnapshotStore,
    serve_path: str,
    max_stale_secs: int,
    path: str,
    now: float,
) -> tuple[int, str, bytes]:
    """Decide the response for a GET, as ``(status, content_type, body)``.

    Pure and side-effect free so it can be unit-tested without a socket:

    * ``serve_path`` → ``200 text/html`` with the snapshot while ``now - last_success`` is within
      ``max_stale_secs``; ``503`` when no snapshot exists yet or it is stale (freshness as status,
      R2.3/R3.1).
    * ``/healthz`` → ``200`` liveness.
    * anything else → ``404``.
    """
    if path == HEALTH_PATH:
        return 200, "text/plain", b"ok"
    if path == serve_path:
        snapshot = store.read()
        if snapshot is None:
            return 503, "text/plain", b"no snapshot yet"
        data, last_success = snapshot
        if now - last_success > max_stale_secs:
            return 503, "text/plain", b"snapshot stale"
        return 200, "text/html", data
    return 404, "text/plain", b"not found"


def make_handler(store: SnapshotStore, serve_path: str, max_stale_secs: int):
    """Build a `BaseHTTPRequestHandler` subclass bound to this store and configuration."""

    class SnapshotHandler(BaseHTTPRequestHandler):
        """Serves GETs via :func:`route`; all other methods are unsupported."""

        def do_GET(self) -> None:  # noqa: N802 — http.server's required method name
            status, content_type, body = route(
                store, serve_path, max_stale_secs, self.path, time.time()
            )
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args: object) -> None:  # keep the access log quiet
            return

    return SnapshotHandler


def make_server(store: SnapshotStore, serve_path: str, max_stale_secs: int, port: int) -> ThreadingHTTPServer:
    """Create a threaded HTTP server serving the snapshot on ``port`` (bound, not yet serving)."""
    return ThreadingHTTPServer(("0.0.0.0", port), make_handler(store, serve_path, max_stale_secs))
