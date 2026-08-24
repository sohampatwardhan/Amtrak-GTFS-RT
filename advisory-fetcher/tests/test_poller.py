"""Tests for the poll loop and subresource-blocking (task 3.4, Requirements 1.1-1.3/4.1-4.2/8.1/8.3).

These run offline: the Playwright browser layer is injected as a fake `launcher`, so no browser is
needed. The real browser path (`_default_launcher`) is exercised on-device in task 5.1.
"""

import asyncio

import pytest

from fetcher.config import Config
from fetcher.poller import poll_once, run_forever, should_block
from fetcher.store import SnapshotStore

CFG = Config.from_env({"POLL_INTERVAL_SECS": "1", "SNAPSHOT_DIR": "/unused"})


@pytest.mark.parametrize("rtype", ["image", "font", "media", "stylesheet"])
def test_blocked_resource_types(rtype):
    assert should_block(rtype) is True


@pytest.mark.parametrize("rtype", ["document", "script", "xhr", "fetch"])
def test_allowed_resource_types(rtype):
    assert should_block(rtype) is False


def test_poll_once_returns_html_when_gated():
    async def fake(cfg):
        return True, "<html>na-service-alert</html>"

    assert asyncio.run(poll_once(CFG, launcher=fake)) == "<html>na-service-alert</html>"


def test_poll_once_returns_none_when_blocked():
    async def fake(cfg):
        return False, ""

    assert asyncio.run(poll_once(CFG, launcher=fake)) is None


def _run(store, cfg, launcher, iterations):
    async def noop_sleep(_):
        return None

    asyncio.run(run_forever(store, cfg, launcher=launcher, sleep=noop_sleep, iterations=iterations))


def test_run_forever_stores_successful_fetch(tmp_path):
    store = SnapshotStore(tmp_path)
    cfg = Config.from_env({"SNAPSHOT_DIR": str(tmp_path), "POLL_INTERVAL_SECS": "1"})

    async def fake(cfg):
        return True, "<html>na-service-alert ALX</html>"

    _run(store, cfg, fake, iterations=1)
    snapshot = store.read()
    assert snapshot is not None and b"na-service-alert" in snapshot[0]


def test_run_forever_is_fail_open_on_error(tmp_path):
    store = SnapshotStore(tmp_path)
    cfg = Config.from_env({"SNAPSHOT_DIR": str(tmp_path), "POLL_INTERVAL_SECS": "1"})

    async def boom(cfg):
        raise RuntimeError("browser exploded")

    _run(store, cfg, boom, iterations=2)  # must not raise
    assert store.read() is None  # no snapshot written, no crash


def test_run_forever_keeps_snapshot_when_blocked(tmp_path):
    store = SnapshotStore(tmp_path)
    store.update("<html>previous na-service-alert</html>")
    cfg = Config.from_env({"SNAPSHOT_DIR": str(tmp_path), "POLL_INTERVAL_SECS": "1"})

    async def blocked(cfg):
        return False, ""

    _run(store, cfg, blocked, iterations=1)
    data, _ = store.read()
    assert data == b"<html>previous na-service-alert</html>"  # untouched
