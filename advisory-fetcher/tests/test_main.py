"""Tests for the entrypoint wiring (task 3.5, Requirements 3.1/8.1)."""

import threading
import urllib.request

from fetcher.__main__ import build_components
from fetcher.server import HEALTH_PATH


def test_build_components_wires_config_store_server(tmp_path):
    cfg, store, server = build_components(
        {"SNAPSHOT_DIR": str(tmp_path), "LISTEN_PORT": "0", "POLL_INTERVAL_SECS": "1"}
    )
    try:
        assert cfg.snapshot_dir == str(tmp_path)
        assert store.read() is None  # nothing fetched yet
        # A fixture "fetch" populates the endpoint the server serves.
        store.update("<html>na-service-alert wired</html>")
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        _, port = server.server_address
        base = f"http://127.0.0.1:{port}"
        with urllib.request.urlopen(f"{base}{HEALTH_PATH}", timeout=5) as resp:
            assert resp.status == 200
        with urllib.request.urlopen(f"{base}{cfg.serve_path}", timeout=5) as resp:
            assert resp.status == 200 and b"na-service-alert" in resp.read()
    finally:
        server.shutdown()
        server.server_close()
