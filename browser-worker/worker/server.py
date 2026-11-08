"""The worker's one HTTP listener, serving both sources' snapshots.

The routes are the ones the Rust service already reads:

* ``/tracks.json`` — Amtrak's train-status documents (:mod:`worker.tracks.server`);
* ``/service-alerts-and-notices`` and ``/alert/<slug>.html`` — the notices list and its detail
  pages (:mod:`worker.notices.server`);
* ``/healthz`` — liveness. With tracks on it is the track loop's liveness: ``503`` once that loop
  stalls or keeps failing. With tracks off it is ``200``.

A source that is off answers ``404`` on its paths. Handlers only read the in-memory or on-disk
snapshot: they never wait on the browser, so a slow Amtrak page cannot stall a request.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

from .config import WorkerConfig
from .notices import server as notices_server
from .notices.store import SnapshotStore, safe_detail_name
from .tracks import server as tracks_server
from .tracks.store import TrackStore

HEALTH_PATH = "/healthz"
TRACKS_PATH = "/tracks.json"
NOTICES_PATH = "/service-alerts-and-notices"


@dataclass(frozen=True)
class Snapshots:
    """What the listener serves: the worker config and each enabled source's store."""

    config: WorkerConfig
    tracks: TrackStore | None = None
    notices: SnapshotStore | None = None


def route(snapshots: Snapshots, path: str, now: float) -> tuple[int, str, bytes]:
    """Decide a GET response as ``(status, content_type, body)``. Pure, for tests."""
    clean = urlsplit(path).path
    tracks = snapshots.config.tracks
    notices = snapshots.config.notices
    if clean == HEALTH_PATH:
        if tracks is not None and snapshots.tracks is not None:
            return _tracks(snapshots, HEALTH_PATH, now)
        return 200, "text/plain", b"ok"
    if clean == TRACKS_PATH:
        if tracks is None or snapshots.tracks is None:
            return 404, "text/plain", b"tracks are off"
        return _tracks(snapshots, path, now)
    if clean == NOTICES_PATH or safe_detail_name(clean) is not None:
        if notices is None or snapshots.notices is None:
            return 404, "text/plain", b"notices are off"
        return notices_server.route(
            snapshots.notices,
            NOTICES_PATH,
            notices.max_stale_secs,
            path,
            now,
            notices.detail_max_stale_secs,
        )
    return 404, "text/plain", b"not found"


def _tracks(snapshots: Snapshots, path: str, now: float) -> tuple[int, str, bytes]:
    tracks = snapshots.config.tracks
    assert tracks is not None and snapshots.tracks is not None
    return tracks_server.route(
        snapshots.tracks,
        TRACKS_PATH,
        tracks.max_stale_secs,
        tracks.document_ttl_secs,
        path,
        now,
        tracks.liveness_secs,
    )


def make_handler(snapshots: Snapshots):
    """Build a request handler bound to ``snapshots``."""

    class WorkerHandler(BaseHTTPRequestHandler):
        """Serves GETs through :func:`route`; other methods are unsupported."""

        def do_GET(self) -> None:  # noqa: N802 — http.server's required method name
            status, content_type, body = route(snapshots, self.path, time.time())
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args: object) -> None:  # keep the access log quiet
            return

    return WorkerHandler


def make_server(snapshots: Snapshots) -> ThreadingHTTPServer:
    """A threaded server on the configured host and port. It is bound and not yet serving."""
    config = snapshots.config
    return ThreadingHTTPServer((config.listen_host, config.listen_port), make_handler(snapshots))
