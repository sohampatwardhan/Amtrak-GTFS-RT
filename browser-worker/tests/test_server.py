"""One listener serves both sources; a source that is off answers 404."""

import json
import threading
import time
import urllib.error
import urllib.request

from worker.config import WorkerConfig
from worker.notices.store import SnapshotStore
from worker.server import HEALTH_PATH, NOTICES_PATH, TRACKS_PATH, Snapshots, make_server, route
from worker.tracks.store import TrackStore

DETAIL = "/alert/boston-south-station-restroom-improvement-project.html"


def _snapshots(tmp_path, **flags):
    cfg = WorkerConfig.from_env({"LISTEN_PORT": "0", **flags})
    return Snapshots(
        config=cfg,
        tracks=TrackStore(started_at=1_000) if cfg.tracks is not None else None,
        notices=SnapshotStore(tmp_path) if cfg.notices is not None else None,
    )


def test_both_sources_are_routed(tmp_path):
    snapshots = _snapshots(tmp_path)
    snapshots.tracks.mark_success(1_000)
    snapshots.notices.update("<html>na-service-alert</html>")
    snapshots.notices.update_detail(DETAIL, "<p>Atlantic Avenue</p>")
    _, listed = snapshots.notices.read()

    status, content_type, body = route(snapshots, TRACKS_PATH, 1_010)
    assert (status, content_type) == (200, "application/json")
    assert json.loads(body)["documents"] == []
    status, content_type, body = route(snapshots, NOTICES_PATH, listed + 1)
    assert (status, content_type, body) == (200, "text/html", b"<html>na-service-alert</html>")
    status, _, body = route(snapshots, DETAIL, listed + 1)
    assert (status, body) == (200, b"<p>Atlantic Avenue</p>")
    assert route(snapshots, "/other", 1_010)[0] == 404


def test_a_source_that_is_off_answers_404(tmp_path):
    tracks_only = _snapshots(tmp_path, AMTRAK_ADVISORIES="off")
    assert route(tracks_only, NOTICES_PATH, 1_000) == (404, "text/plain", b"notices are off")
    assert route(tracks_only, DETAIL, 1_000)[0] == 404
    notices_only = _snapshots(tmp_path, AMTRAK_TRACKS_AMTRAK="off")
    assert route(notices_only, TRACKS_PATH, 1_000) == (404, "text/plain", b"tracks are off")


def test_health_follows_the_track_loop_and_is_ok_without_it(tmp_path):
    snapshots = _snapshots(tmp_path)
    assert route(snapshots, HEALTH_PATH, 1_100)[0] == 200
    stalled = 1_000 + snapshots.config.tracks.liveness_secs + 1
    assert route(snapshots, HEALTH_PATH, stalled) == (503, "text/plain", b"poll loop stalled")
    notices_only = _snapshots(tmp_path, AMTRAK_TRACKS="off")
    assert route(notices_only, HEALTH_PATH, stalled) == (200, "text/plain", b"ok")


def test_live_server_serves_both_paths(tmp_path):
    snapshots = _snapshots(tmp_path)
    snapshots.tracks.mark_cycle(time.time())  # the track loop is alive
    snapshots.notices.update("<html>live na-service-alert</html>")
    server = make_server(snapshots)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        assert host == "127.0.0.1"
        base = f"http://127.0.0.1:{port}"
        with urllib.request.urlopen(f"{base}{HEALTH_PATH}", timeout=5) as resp:
            assert resp.status == 200
        with urllib.request.urlopen(f"{base}{NOTICES_PATH}", timeout=5) as resp:
            assert resp.status == 200 and b"na-service-alert" in resp.read()
        try:
            urllib.request.urlopen(f"{base}{TRACKS_PATH}", timeout=5)
        except urllib.error.HTTPError as error:
            assert error.code == 503  # no track poll has succeeded yet
        else:
            raise AssertionError("tracks served before any poll")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
