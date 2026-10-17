"""One poll cycle with injected train data and status responses. No browser, no network."""

import asyncio
from datetime import datetime, timedelta, timezone

from fetcher.config import Config
from fetcher.poller import poll_once, run_forever, should_block, status_url
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
