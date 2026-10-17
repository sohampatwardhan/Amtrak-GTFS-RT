"""HTTP server for the latest train-status snapshot.

``/tracks.json`` is ``200 application/json`` while the last completed poll is within
``max_stale_secs``, and ``503`` before the first poll or after the snapshot goes stale. The Rust
service treats ``503`` as a failed refresh and keeps tracks it already stored. ``/healthz`` is
liveness and does not look at the snapshot.
"""

from __future__ import annotations

import json
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

from .store import TrackStore

HEALTH_PATH = "/healthz"
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
) -> tuple[int, str, bytes]:
    """Decide a GET response as ``(status, content_type, body)``."""
    clean = urlsplit(path).path
    if clean == HEALTH_PATH:
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
        "fetched_at": datetime.fromtimestamp(last_success, timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
        "documents": documents,
    }
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    return 200, "application/json", body


def make_handler(store: TrackStore, serve_path: str, max_stale_secs: int, document_ttl_secs: int):
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
) -> ThreadingHTTPServer:
    """Create a threaded server on ``port``. It is bound and not yet serving."""
    return ThreadingHTTPServer(
        ("0.0.0.0", port),
        make_handler(store, serve_path, max_stale_secs, document_ttl_secs),
    )
