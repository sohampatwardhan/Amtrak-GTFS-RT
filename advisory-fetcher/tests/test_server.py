"""Tests for the snapshot HTTP server (task 3.3, Requirements 2.1/2.3/3.1/8.1)."""

import threading
import time
import urllib.request

from fetcher.server import HEALTH_PATH, make_server, route
from fetcher.store import SnapshotStore

SERVE_PATH = "/service-alerts-and-notices"
MAX_STALE = 3600


def test_healthz_ok(tmp_path):
    store = SnapshotStore(tmp_path)
    status, ctype, body = route(store, SERVE_PATH, MAX_STALE, HEALTH_PATH, now=1000.0)
    assert status == 200 and body == b"ok" and ctype == "text/plain"


def test_503_when_no_snapshot(tmp_path):
    store = SnapshotStore(tmp_path)
    status, _, _ = route(store, SERVE_PATH, MAX_STALE, SERVE_PATH, now=1000.0)
    assert status == 503


def test_200_when_fresh(tmp_path):
    store = SnapshotStore(tmp_path)
    store.update("<html>fresh na-service-alert</html>")
    _, last_success = store.read()
    status, ctype, body = route(store, SERVE_PATH, MAX_STALE, SERVE_PATH, now=last_success + 10)
    assert status == 200 and ctype == "text/html"
    assert b"na-service-alert" in body


def test_503_when_stale(tmp_path):
    store = SnapshotStore(tmp_path)
    store.update("<html>old</html>")
    _, last_success = store.read()
    status, _, _ = route(store, SERVE_PATH, MAX_STALE, SERVE_PATH, now=last_success + MAX_STALE + 1)
    assert status == 503


def test_404_for_unknown_path(tmp_path):
    store = SnapshotStore(tmp_path)
    status, _, _ = route(store, SERVE_PATH, MAX_STALE, "/nope", now=1000.0)
    assert status == 404


def test_live_server_serves_snapshot_and_health(tmp_path):
    """End-to-end over a real socket: healthz is 200 and a stored snapshot is served."""
    store = SnapshotStore(tmp_path)
    store.update("<html>live na-service-alert</html>")
    server = make_server(store, SERVE_PATH, MAX_STALE, port=0)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        base = f"http://127.0.0.1:{port}"
        with urllib.request.urlopen(f"{base}{HEALTH_PATH}", timeout=5) as resp:
            assert resp.status == 200
        with urllib.request.urlopen(f"{base}{SERVE_PATH}", timeout=5) as resp:
            assert resp.status == 200
            assert b"na-service-alert" in resp.read()
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        time.sleep(0)
