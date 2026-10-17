"""ETA window, hot-set priority, and the per-cycle fetch cap."""

from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

from fetcher.select import choose_fetches, due_trains

EASTERN = ZoneInfo("America/New_York")
NOW = datetime(2026, 10, 17, 17, 0, tzinfo=EASTERN)
BEFORE = timedelta(minutes=30)
AFTER = timedelta(minutes=10)


def _feature(number, origin, stations):
    properties = {"TrainNum": number, "OrigSchDep": origin}
    for index, station in enumerate(stations, start=1):
        properties[f"Station{index}"] = station
    return {"properties": properties}


def _station(code, **times):
    payload = {"code": code, "tz": "E", "bus": False}
    payload.update(times)
    return payload


def _due(features):
    return due_trains({"features": features}, NOW, before=BEFORE, after=AFTER)


def test_a_stop_inside_thirty_minutes_is_due_and_a_far_one_is_not():
    due = _due(
        [
            _feature(
                "167",
                "10/17/2026 4:25:00 PM",
                [
                    _station("PVD", scharr="10/17/2026 17:06:00", schdep="10/17/2026 17:08:00"),
                    _station("NLC", scharr="10/17/2026 18:07:00"),
                ],
            ),
            _feature(
                "99",
                "10/17/2026 12:00:00 PM",
                [_station("BOS", schdep="10/17/2026 16:00:00")],
            ),
        ]
    )
    by_number = {train.train_number: train for train in due}
    assert set(by_number) == {"167"}
    assert by_number["167"].service_date == "2026-10-17"
    assert by_number["167"].soonest_secs == 6 * 60


def test_an_estimate_replaces_the_schedule_and_bus_stops_do_not_count():
    late = _due(
        [
            _feature(
                "54",
                "10/17/2026 8:00:00 AM",
                [_station("NYP", scharr="10/17/2026 17:05:00", estarr="10/17/2026 17:40:00")],
            )
        ]
    )
    assert late == []
    early_estimate = _due(
        [
            _feature(
                "54",
                "10/17/2026 8:00:00 AM",
                [_station("NYP", scharr="10/17/2026 18:00:00", estarr="10/17/2026 17:05:00")],
            )
        ]
    )
    assert [train.train_number for train in early_estimate] == ["54"]
    bus_only = _due(
        [
            _feature(
                "3167",
                "10/17/2026 4:00:00 PM",
                [
                    {
                        "code": "BUS",
                        "tz": "E",
                        "bus": True,
                        "scharr": "10/17/2026 17:05:00",
                    }
                ],
            )
        ]
    )
    assert bus_only == []


def test_a_stop_just_after_arrival_stays_due():
    due = _due(
        [
            _feature(
                "167",
                "10/17/2026 4:25:00 PM",
                [_station("PVD", scharr="10/17/2026 16:55:00")],
            )
        ]
    )
    assert len(due) == 1
    assert due[0].soonest_secs == 5 * 60


def test_hot_trains_are_fetched_every_cycle_and_the_outer_window_waits():
    hot = _feature("167", "10/17/2026 4:25:00 PM", [_station("PVD", scharr="10/17/2026 17:06:00")])
    outer = _feature("66", "10/17/2026 3:00:00 PM", [_station("KIN", scharr="10/17/2026 17:25:00")])
    due = _due([hot, outer])
    first = choose_fetches(due, {}, NOW, hot_secs=12 * 60, cooldown_secs=90, max_fetches=24)
    assert [train.train_number for train in first] == ["167", "66"]
    just_fetched = {train.key: NOW.timestamp() for train in first}
    again = choose_fetches(
        due, just_fetched, NOW + timedelta(seconds=30), hot_secs=12 * 60, cooldown_secs=90, max_fetches=24
    )
    assert [train.train_number for train in again] == ["167"]
    cooled = choose_fetches(
        due, just_fetched, NOW + timedelta(seconds=90), hot_secs=12 * 60, cooldown_secs=90, max_fetches=24
    )
    assert [train.train_number for train in cooled] == ["167", "66"]


def test_the_cap_keeps_the_soonest_hot_trains():
    features = []
    for index, number in enumerate(range(1, 31)):
        moment = NOW + timedelta(seconds=20 * index)
        features.append(
            _feature(
                str(number),
                "10/17/2026 8:00:00 AM",
                [_station("PVD", scharr=moment.strftime("%m/%d/%Y %H:%M:%S"))],
            )
        )
    due = _due(features)
    chosen = choose_fetches(due, {}, NOW, hot_secs=12 * 60, cooldown_secs=90, max_fetches=24)
    assert len(chosen) == 24
    assert [train.train_number for train in chosen] == [str(number) for number in range(1, 25)]
