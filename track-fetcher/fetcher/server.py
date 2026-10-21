"""HTTP server for the latest train-status snapshot.

``/tracks.json`` is ``200 application/json`` while the last completed poll is within
``max_stale_secs``, and ``503`` before the first poll or after the snapshot goes stale. A poll
counts only when it had nothing to fetch or stored a document, so a sidecar Amtrak is blocking
goes ``503`` instead of serving old documents as current. The Rust service treats ``503`` as a
failed refresh and keeps tracks it already stored.

``/healthz`` is liveness: ``503`` when no poll has finished within ``liveness_secs``, so a wedged
loop is restarted, and ``503`` after ``POLL_FAILURES_BEFORE_UNHEALTHY`` polls in a row fail, so a
loop that keeps running but never reads Amtrak is restarted too. One failure stays healthy, which
lets the process come up before the first successful poll.
"""

from __future__ import annotations

import json
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

from .store import TrackStore, iso_utc

HEALTH_PATH = "/healthz"
# Three failed polls is about three minutes at the default interval: long enough that the offline
# image smoke test (one poll, then a long sleep) still sees a healthy process, and short enough
# that a feed Amtrak is rejecting does not stay "healthy" for the whole liveness window.
POLL_FAILURES_BEFORE_UNHEALTHY = 3
SOURCE_TEMPLATE = (
    "https://www.amtrak.com/dotcom/travel-service/statuses/{train}?service-date={date}"
)


def route(
    store: TrackStore,
    serve_path: str,
    max_stale_secs: int,
    document_ttl_secs: int,
    path: str,
    now: float,
    liveness_secs: int = 900,
) -> tuple[int, str, bytes]:
    """Decide a GET response as ``(status, content_type, body)``."""
    clean = urlsplit(path).path
    if clean == HEALTH_PATH:
        if now - store.last_cycle() > liveness_secs:
            return 503, "text/plain", b"poll loop stalled"
        if store.consecutive_failures() >= POLL_FAILURES_BEFORE_UNHEALTHY:
            return 503, "text/plain", b"polls failing"
        return 200, "text/plain", b"ok"
    if clean != serve_path:
        return 404, "text/plain", b"not found"
    snapshot = store.snapshot(now, document_ttl_secs)
    if snapshot is None:
        return 503, "text/plain", b"no snapshot yet"
    last_success, documents = snapshot
    if now - last_success > max_stale_secs:
        return 503, "text/plain", b"snapshot stale"
    payload = {
        "source": SOURCE_TEMPLATE,
        "fetched_at": iso_utc(last_success),
        "documents": documents,
    }
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    return 200, "application/json", body


def make_handler(
    store: TrackStore,
    serve_path: str,
    max_stale_secs: int,
    document_ttl_secs: int,
    liveness_secs: int,
):
    """Build a request handler bound to this store."""

    class SnapshotHandler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802 — http.server's method name
            status, content_type, body = route(
                store,
                serve_path,
                max_stale_secs,
                document_ttl_secs,
                self.path,
                time.time(),
                liveness_secs,
            )
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args: object) -> None:
            return

    return SnapshotHandler


def make_server(
    store: TrackStore,
    serve_path: str,
    max_stale_secs: int,
    document_ttl_secs: int,
    port: int,
    *,
    host: str = "0.0.0.0",
    liveness_secs: int = 900,
) -> ThreadingHTTPServer:
    """Create a threaded server on ``host:port``. It is bound and not yet serving."""
    return ThreadingHTTPServer(
        (host, port),
        make_handler(store, serve_path, max_stale_secs, document_ttl_secs, liveness_secs),
    )
