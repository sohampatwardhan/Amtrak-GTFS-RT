"""One poll cycle with injected train data and status responses. No browser, no network."""

import asyncio
import threading
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import pytest

from fetcher.config import Config
from fetcher.poller import (
    TRAINS_ACCEPT,
    BrowserUnavailable,
    ResidentBrowser,
    StatusMiss,
    document_from_eval,
    load_trains,
    poll_once,
    run_forever,
    should_abort,
    should_block,
    status_url,
)
from fetcher.select import DueTrain
from fetcher.store import TrackStore

NOW = datetime(2026, 10, 17, 21, 0, tzinfo=timezone.utc)


def _geojson():
    return {
        "features": [
            {
                "properties": {
                    "TrainNum": "0167",
                    "OrigSchDep": "10/17/2026 4:25:00 PM",
                    "Station1": {
                        "code": "PVD",
                        "tz": "E",
                        "bus": False,
                        "scharr": "10/17/2026 17:06:00",
                    },
                }
            }
        ]
    }


def test_status_url_and_blocked_types():
    assert status_url("167", "2026-10-17") == (
        "https://www.amtrak.com/dotcom/travel-service/statuses/167?service-date=2026-10-17"
    )
    assert should_block("image") is True
    assert should_block("script") is False
    assert should_block("fetch") is False


def test_a_cycle_stores_the_document_and_a_miss_keeps_it():
    store = TrackStore()
    seen: list[DueTrain] = []

    async def trains(_cfg):
        return _geojson()

    async def status(_cfg, train: DueTrain):
        seen.append(train)
        return {"data": [{"travelService": {"number": train.train_number, "date": train.service_date}}]}

    cfg = Config(max_fetches_per_cycle=24, hot_window_secs=720, cooldown_secs=90)
    asyncio.run(
        poll_once(store, cfg, {}, now=NOW, trains_fetcher=trains, status_fetcher=status)
    )
    assert [train.train_number for train in seen] == ["167"]
    assert seen[0].service_date == "2026-10-17"
    snapshot = store.snapshot(NOW.timestamp(), 7200)
    assert snapshot is not None
    document = snapshot[1][0]
    assert document["data"][0]["travelService"]["number"] == "167"

    async def fail(_cfg, _train):
        return None

    later = NOW + timedelta(seconds=60)
    asyncio.run(
        poll_once(store, cfg, {}, now=later, trains_fetcher=trains, status_fetcher=fail)
    )
    kept = store.snapshot(later.timestamp(), 7200)
    assert kept is not None
    assert kept[1][0]["data"][0]["travelService"]["number"] == "167"


def test_a_failed_train_list_does_not_replace_the_snapshot():
    store = TrackStore()
    store.mark_success(1_000)

    async def boom(_cfg):
        raise RuntimeError("maps down")

    async def unused(_cfg, _train):
        raise AssertionError("status should not be called")

    cfg = Config()

    async def once():
        await run_forever(
            store,
            cfg,
            trains_fetcher=boom,
            status_fetcher=unused,
            sleep=_no_sleep,
            clock=lambda: NOW,
            iterations=1,
        )

    asyncio.run(once())
    assert store.snapshot(NOW.timestamp(), 7200)[0] == 1_000


async def _no_sleep(_seconds: float) -> None:
    return None


def _status_for(numbers):
    async def status(_cfg, train: DueTrain):
        numbers.append(train.train_number)
        return {"data": []}

    return status


def _two_trains():
    return {
        "features": [
            {
                "properties": {
                    "TrainNum": number,
                    "OrigSchDep": "10/17/2026 4:25:00 PM",
                    "Station1": {
                        "code": "PVD",
                        "tz": "E",
                        "bus": False,
                        "scharr": "10/17/2026 17:06:00",
                    },
                }
            }
            for number in ("167", "171")
        ]
    }


def test_a_cycle_where_every_fetch_fails_is_not_a_success():
    store = TrackStore()

    async def trains(_cfg):
        return _two_trains()

    async def rejected(_cfg, _train):
        raise StatusMiss("http 403")

    last_fetched: dict = {("9", "2026-10-16"): 1.0}
    asyncio.run(
        poll_once(store, Config(), last_fetched, now=NOW, trains_fetcher=trains, status_fetcher=rejected)
    )
    assert store.snapshot(NOW.timestamp(), 7200) is None
    # Failed attempts still start the cooldown, and trains no longer due are forgotten.
    assert set(last_fetched) == {("167", "2026-10-17"), ("171", "2026-10-17")}

    async def nothing_due(_cfg):
        return {"features": []}

    asyncio.run(
        poll_once(store, Config(), {}, now=NOW, trains_fetcher=nothing_due, status_fetcher=rejected)
    )
    assert store.snapshot(NOW.timestamp(), 7200) == (NOW.timestamp(), [])


def test_a_browser_failure_ends_the_cycle():
    store = TrackStore()
    calls: list[str] = []

    async def trains(_cfg):
        return _two_trains()

    async def down(_cfg, train: DueTrain):
        calls.append(train.train_number)
        raise BrowserUnavailable("TargetClosedError")

    asyncio.run(poll_once(store, Config(), {}, now=NOW, trains_fetcher=trains, status_fetcher=down))
    assert calls == ["167"]


def test_every_cycle_is_recorded_for_liveness_even_when_it_fails():
    store = TrackStore(started_at=0)

    async def boom(_cfg):
        raise RuntimeError("maps down")

    asyncio.run(
        run_forever(
            store,
            Config(),
            trains_fetcher=boom,
            status_fetcher=_status_for([]),
            sleep=_no_sleep,
            clock=lambda: NOW,
            iterations=1,
        )
    )
    assert store.last_cycle() == NOW.timestamp()
    assert store.consecutive_failures() == 1


def test_repeated_failed_polls_are_counted_and_a_success_clears_them():
    store = TrackStore(started_at=0)

    async def boom(_cfg):
        raise RuntimeError("maps down")

    async def trains(_cfg):
        return {"features": []}

    async def run(fetcher, iterations):
        await run_forever(
            store,
            Config(),
            trains_fetcher=fetcher,
            status_fetcher=_status_for([]),
            sleep=_no_sleep,
            clock=lambda: NOW,
            iterations=iterations,
        )

    asyncio.run(run(boom, 2))
    assert store.consecutive_failures() == 2
    asyncio.run(run(trains, 1))
    assert store.consecutive_failures() == 0


class _MapsFeed(BaseHTTPRequestHandler):
    """406 for ``Accept: application/json``; 200 ``text/plain`` JSON otherwise."""

    seen: list[str] = []

    def do_GET(self):  # noqa: N802 — http.server's method name
        accept = self.headers.get("Accept", "")
        type(self).seen.append(accept)
        if accept == "application/json":
            body = b"not acceptable"
            status = 406
        else:
            body = b'{"features":[]}'
            status = 200
        self.send_response(status)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args: object) -> None:
        return


def test_load_trains_sends_an_accept_header_the_maps_feed_allows():
    _MapsFeed.seen = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), _MapsFeed)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        url = f"http://127.0.0.1:{server.server_address[1]}/trains/getTrainsData"
        rejected = Request(url, headers={"Accept": "application/json"})
        with pytest.raises(HTTPError) as error:
            urlopen(rejected, timeout=5)
        assert error.value.code == 406
        assert load_trains(Config(trains_url=url)) == {"features": []}
        assert TRAINS_ACCEPT == "*/*"
        assert _MapsFeed.seen == ["application/json", "*/*"]
    finally:
        server.shutdown()
        server.server_close()


def test_routing_blocks_page_weight_and_third_parties():
    cfg = Config()
    assert should_abort(cfg, "image", "https://www.amtrak.com/logo.png")
    assert not should_abort(cfg, "script", "https://www.amtrak.com/app.js")
    assert not should_abort(cfg, "fetch", "https://assets.amtrak.com/x")
    assert should_abort(cfg, "script", "https://tags.example.net/t.js")
    assert should_abort(cfg, "script", "https://notamtrak.com/t.js")
    open_cfg = Config(block_third_party=False, block_subresources=False)
    assert not should_abort(open_cfg, "script", "https://tags.example.net/t.js")
    assert not should_abort(open_cfg, "image", "https://www.amtrak.com/logo.png")


def test_unusable_answers_name_their_reason():
    assert document_from_eval({"status": 200, "body": '{"data": []}'}) == {"data": []}
    for raw, reason in [
        (None, "no response"),
        ({"status": 403, "body": "denied"}, "http 403"),
        ({"status": 200, "body": "<html>"}, "not json"),
        ({"status": 200, "body": "{}"}, "no data"),
    ]:
        with pytest.raises(StatusMiss, match=reason):
            document_from_eval(raw)


class _FakePage:
    def __init__(self, result):
        self.result = result

    async def evaluate(self, _script, _args):
        if isinstance(self.result, Exception):
            raise self.result
        return self.result


def test_browser_relaunch_backs_off_and_resets_after_a_success():
    clock = [1_000.0]
    cfg = Config(relaunch_backoff_max_secs=150, browser_max_age_secs=3_600)
    browser = ResidentBrowser(cfg, monotonic=lambda: clock[0])
    launches: list[float] = []
    pages: list[object] = [RuntimeError("launch"), RuntimeError("launch")]

    async def launch():
        launches.append(clock[0])
        outcome = pages.pop(0) if pages else _FakePage({"status": 200, "body": '{"data": []}'})
        if isinstance(outcome, Exception):
            raise outcome
        return outcome

    browser._launch = launch
    train = DueTrain("167", "2026-10-17", 60.0)

    async def attempt():
        return await browser.fetch(cfg, train)

    with pytest.raises(BrowserUnavailable):
        asyncio.run(attempt())
    assert browser._backoff == 60
    clock[0] += 30
    with pytest.raises(BrowserUnavailable, match="backoff"):
        asyncio.run(attempt())
    assert len(launches) == 1
    clock[0] += 31
    with pytest.raises(BrowserUnavailable):
        asyncio.run(attempt())
    assert browser._backoff == 120
    clock[0] += 121
    assert asyncio.run(attempt()) == {"data": []}
    assert browser._backoff == 0
    assert len(launches) == 3

    # A hung or failed in-page fetch closes the browser and backs off again; capped at the max.
    browser._page = _FakePage(asyncio.TimeoutError())
    with pytest.raises(BrowserUnavailable):
        asyncio.run(attempt())
    assert browser._page is None
    browser._backoff = 120
    browser._next_launch = 0
    pages.append(RuntimeError("launch"))
    with pytest.raises(BrowserUnavailable):
        asyncio.run(attempt())
    assert browser._backoff == 150


def test_the_browser_is_recycled_after_its_maximum_age():
    clock = [0.0]
    cfg = Config(browser_max_age_secs=100)
    browser = ResidentBrowser(cfg, monotonic=lambda: clock[0])
    launches: list[float] = []

    async def launch():
        launches.append(clock[0])
        return _FakePage({"status": 200, "body": '{"data": []}'})

    browser._launch = launch
    train = DueTrain("167", "2026-10-17", 60.0)
    asyncio.run(browser.fetch(cfg, train))
    clock[0] = 50
    asyncio.run(browser.fetch(cfg, train))
    clock[0] = 100
    asyncio.run(browser.fetch(cfg, train))
    assert launches == [0.0, 100]
