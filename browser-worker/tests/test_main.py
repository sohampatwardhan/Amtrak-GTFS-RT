"""Entrypoint wiring: one listener for the enabled sources, nothing when both are off."""

import threading
import urllib.request

from worker.__main__ import build_components, main
from worker.server import HEALTH_PATH, NOTICES_PATH


def test_build_components_wires_both_sources(tmp_path):
    components = build_components({"NOTICES_SNAPSHOT_DIR": str(tmp_path), "LISTEN_PORT": "0"})
    assert components is not None
    server = components.server
    try:
        assert components.config.sources() == ["tracks", "notices"]
        assert components.snapshots.tracks is not None
        notices = components.snapshots.notices
        assert notices.read() is None  # nothing fetched yet
        notices.update("<html>na-service-alert wired</html>")
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        _, port = server.server_address
        base = f"http://127.0.0.1:{port}"
        with urllib.request.urlopen(f"{base}{HEALTH_PATH}", timeout=5) as resp:
            assert resp.status == 200
        with urllib.request.urlopen(f"{base}{NOTICES_PATH}", timeout=5) as resp:
            assert resp.status == 200 and b"na-service-alert" in resp.read()
    finally:
        server.shutdown()
        server.server_close()


def test_a_disabled_source_gets_no_store(tmp_path):
    components = build_components(
        {"AMTRAK_TRACKS_AMTRAK": "off", "NOTICES_SNAPSHOT_DIR": str(tmp_path), "LISTEN_PORT": "0"}
    )
    try:
        assert components.snapshots.tracks is None
        assert components.snapshots.notices is not None
    finally:
        components.server.server_close()


def test_nothing_runs_when_both_sources_are_off(tmp_path):
    env = {"AMTRAK_TRACKS": "off", "AMTRAK_ADVISORIES": "off", "LISTEN_PORT": "0"}
    assert build_components(env) is None
    assert main(env) is None  # returns without binding, launching, or polling
