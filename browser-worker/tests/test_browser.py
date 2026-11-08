"""One Chromium for both sources, driven against a fake browser (no Playwright, no network)."""

import asyncio

import pytest

from worker.browser import SharedBrowser
from worker.notices import poller as notices_poller
from worker.notices.config import Config as NoticesConfig
from worker.notices.store import SnapshotStore
from worker.tracks.config import Config as TracksConfig
from worker.tracks.poller import ResidentBrowser
from worker.tracks.select import DueTrain

NOTICES_HTML = "<html><div class='na-service-alert'>Northeast Regional</div></html>"


class FakePage:
    def __init__(self):
        self.closed = False
        self.visited: list[str] = []

    async def goto(self, url, **_kwargs):
        self.visited.append(url)

    async def content(self):
        return NOTICES_HTML

    async def query_selector(self, _selector):
        return object()

    async def evaluate(self, _script, _args):
        return {"status": 200, "body": '{"data": []}'}

    async def close(self):
        self.closed = True


class FakeContext:
    def __init__(self):
        self.closed = False
        self.pages: list[FakePage] = []

    async def route(self, _pattern, _handler):
        return None

    async def new_page(self):
        page = FakePage()
        self.pages.append(page)
        return page

    async def close(self):
        self.closed = True


class FakeBrowser:
    def __init__(self):
        self.connected = True
        self.closed = False
        self.contexts: list[FakeContext] = []

    def is_connected(self):
        return self.connected and not self.closed

    async def new_context(self, **_kwargs):
        context = FakeContext()
        self.contexts.append(context)
        return context

    async def close(self):
        self.closed = True


class FakePlaywright:
    def __init__(self):
        self.stopped = False

    async def stop(self):
        self.stopped = True


class Launcher:
    """A ``start`` for :class:`SharedBrowser` that records every Chromium it would launch."""

    def __init__(self, failures: int = 0):
        self.browsers: list[FakeBrowser] = []
        self.failures = failures

    async def __call__(self, _mode):
        if self.failures:
            self.failures -= 1
            raise RuntimeError("launch failed")
        browser = FakeBrowser()
        self.browsers.append(browser)
        return FakePlaywright(), browser


def _notices_cfg(tmp_path):
    return NoticesConfig.from_env(
        {
            "NOTICES_SNAPSHOT_DIR": str(tmp_path),
            "NOTICES_RENDER_STABLE_MS": "1",
            "NOTICES_RENDER_POLL_MS": "1",
        }
    )


def test_leases_share_one_browser_and_the_last_return_closes_it():
    launcher = Launcher()
    shared = SharedBrowser(start=launcher)

    async def scenario():
        first = await shared.acquire()
        second = await shared.acquire()
        assert first.browser is second.browser
        assert shared.launches == 1 and shared.holders == 2
        await shared.release(second)
        assert shared.running and not launcher.browsers[0].closed
        await shared.release(first)
        assert not shared.running and launcher.browsers[0].closed

    asyncio.run(scenario())


def test_a_notices_cycle_alone_launches_the_browser_and_closes_it(tmp_path):
    launcher = Launcher()
    shared = SharedBrowser(start=launcher)
    cfg = _notices_cfg(tmp_path)

    async def scenario():
        for cycle in (1, 2):
            fetched = await notices_poller._default_launcher(cfg, SnapshotStore(tmp_path), shared)
            assert fetched is not None and "na-service-alert" in fetched.list_html
            assert not shared.running, "no Chromium between notices polls"
            assert shared.launches == cycle
            browser = launcher.browsers[-1]
            assert browser.closed
            assert all(context.closed for context in browser.contexts)

    asyncio.run(scenario())


def test_tracks_keep_one_browser_resident_through_a_notices_cycle(tmp_path):
    launcher = Launcher()
    shared = SharedBrowser(start=launcher)
    tracks = ResidentBrowser(TracksConfig(), shared)
    train = DueTrain("167", "2026-10-17", 60.0)

    async def scenario():
        assert await tracks.fetch(TracksConfig(), train) == {"data": []}
        fetched = await notices_poller._default_launcher(
            _notices_cfg(tmp_path), SnapshotStore(tmp_path), shared
        )
        assert fetched is not None
        assert shared.launches == 1, "tracks and notices share one Chromium"
        browser = launcher.browsers[0]
        assert not browser.closed
        status_context, notices_context = browser.contexts
        assert notices_context.closed and not status_context.closed
        assert await tracks.fetch(TracksConfig(), train) == {"data": []}
        assert shared.launches == 1
        await tracks.close()
        assert status_context.closed and browser.closed and not shared.running

    asyncio.run(scenario())


def test_a_disconnected_browser_is_relaunched_and_its_leases_go_stale():
    launcher = Launcher()
    shared = SharedBrowser(start=launcher)

    async def scenario():
        old = await shared.acquire()
        launcher.browsers[0].connected = False
        new = await shared.acquire()
        assert shared.launches == 2 and new.browser is launcher.browsers[1]
        await shared.release(old)  # stale: must not return the new browser's lease
        assert shared.running and shared.holders == 1
        await shared.release(new)
        assert not shared.running

    asyncio.run(scenario())


def test_close_tears_the_browser_down_while_leases_are_out():
    launcher = Launcher()
    shared = SharedBrowser(start=launcher)

    async def scenario():
        held = await shared.acquire()
        await shared.close()
        assert not shared.running and launcher.browsers[0].closed
        await shared.release(held)
        assert shared.holders == 0

    asyncio.run(scenario())


def test_a_failed_launch_holds_nothing_and_the_next_acquire_retries():
    launcher = Launcher(failures=1)
    shared = SharedBrowser(start=launcher)

    async def scenario():
        with pytest.raises(RuntimeError, match="launch failed"):
            await shared.acquire()
        assert shared.holders == 0 and not shared.running
        lease = await shared.acquire()
        assert shared.launches == 1
        await shared.release(lease)

    asyncio.run(scenario())
