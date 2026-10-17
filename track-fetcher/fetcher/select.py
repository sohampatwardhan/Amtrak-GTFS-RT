"""Choose which trains to ask Amtrak about.

Amtrak's status API often publishes a track only in the last minutes before a train reaches a
station (Providence showed track 1 about eight minutes before train 167 arrived). Some stations
publish earlier, and that value is already on the document once the train enters the window, so
polling trains with a stop near its ETA collects both kinds. A train is hot, and fetched every
cycle, when its next stop is inside the hot window and an upcoming stop still has no track in its
last document. Every other due train, including one that has just left a stop, is rotated under a
cooldown so a busy hour does not fetch every active train every minute.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo

_ZONES = {
    "E": ZoneInfo("America/New_York"),
    "C": ZoneInfo("America/Chicago"),
    "M": ZoneInfo("America/Denver"),
    "P": ZoneInfo("America/Los_Angeles"),
}
# Arizona does not observe daylight saving time, but the train feed labels its stations "M".
_PHOENIX = ZoneInfo("America/Phoenix")
_ARIZONA_STATIONS = frozenset({"BEN", "FLG", "KNG", "MRC", "TUS", "WLO", "WMJ", "YUM"})
_TIME_FORMATS = ("%m/%d/%Y %H:%M:%S", "%m/%d/%Y %I:%M:%S %p")


@dataclass(frozen=True)
class DueTrain:
    """One train-day with a stop inside the polling window.

    ``soonest_secs`` is the distance to the closest in-window event, past or future.
    ``next_secs`` is the time until the next future in-window event, or ``None`` when every
    in-window event is already past. ``upcoming`` holds the codes of the stations those future
    events belong to.
    """

    train_number: str
    service_date: str
    soonest_secs: float
    next_secs: float | None = None
    upcoming: frozenset[str] = frozenset()

    @property
    def key(self) -> tuple[str, str]:
        return (self.train_number, self.service_date)


def due_trains(
    geojson: dict,
    now: datetime,
    *,
    before: timedelta,
    after: timedelta,
) -> list[DueTrain]:
    """Trains with a rail stop whose ETA is between ``now - after`` and ``now + before``.

    An estimated time replaces the scheduled time for that arrival or departure. Bus stops are
    ignored. ``soonest_secs`` is the absolute distance from ``now`` to the closest such event.
    """
    if now.tzinfo is None:
        raise ValueError("now must be timezone-aware")
    before_secs = before.total_seconds()
    after_secs = after.total_seconds()
    found: dict[tuple[str, str], DueTrain] = {}
    for feature in geojson.get("features") or []:
        properties = feature.get("properties") or {}
        number = _train_number(properties.get("TrainNum"))
        service_date = _service_date(properties)
        if number is None or service_date is None:
            continue
        distances: list[float] = []
        ahead: list[float] = []
        upcoming: set[str] = set()
        for station in _stations(properties):
            if station.get("bus") is True:
                continue
            code = _station_code(station)
            for moment in _event_times(station):
                delta = (moment - now).total_seconds()
                if -after_secs <= delta <= before_secs:
                    distances.append(abs(delta))
                    if delta >= 0:
                        ahead.append(delta)
                        if code is not None:
                            upcoming.add(code)
        if not distances:
            continue
        train = DueTrain(
            number,
            service_date,
            min(distances),
            min(ahead) if ahead else None,
            frozenset(upcoming),
        )
        current = found.get(train.key)
        if current is None or train.soonest_secs < current.soonest_secs:
            found[train.key] = train
    return list(found.values())


def is_hot(train: DueTrain, tracked: Mapping[tuple[str, str], frozenset[str]], hot_secs: float) -> bool:
    """Whether ``train`` reaches a stop within ``hot_secs`` and an upcoming stop lacks a track."""
    if train.next_secs is None or train.next_secs > hot_secs:
        return False
    known = tracked.get(train.key, frozenset())
    return not train.upcoming or not train.upcoming <= known


def choose_fetches(
    due: list[DueTrain],
    last_fetched: dict[tuple[str, str], float],
    now: datetime,
    *,
    hot_secs: float,
    cooldown_secs: float,
    max_fetches: int,
    tracked: Mapping[tuple[str, str], frozenset[str]] | None = None,
) -> list[DueTrain]:
    """Hot trains first, then cooled-down trains, never more than ``max_fetches``.

    Hot trains (see :func:`is_hot`) are taken next-stop-first on every cycle. Other trains are
    taken oldest-fetch-first, and a train attempted within ``cooldown_secs`` waits. ``tracked``
    maps a train-day to the station codes that already show a track in its last document.
    """
    if max_fetches <= 0:
        return []
    tracked = tracked or {}
    now_ts = now.timestamp()
    hot_keys = {train.key for train in due if is_hot(train, tracked, hot_secs)}
    hot = sorted(
        (train for train in due if train.key in hot_keys),
        key=lambda train: (train.next_secs, train.train_number, train.service_date),
    )
    chosen = hot[:max_fetches]
    ready = []
    for train in due:
        if train.key in hot_keys:
            continue
        previous = last_fetched.get(train.key)
        if previous is None or now_ts - previous >= cooldown_secs:
            ready.append(train)
    ready.sort(
        key=lambda train: (
            last_fetched.get(train.key, 0.0),
            train.soonest_secs,
            train.train_number,
            train.service_date,
        )
    )
    chosen.extend(ready[: max_fetches - len(chosen)])
    return chosen


def _train_number(raw: object) -> str | None:
    if not isinstance(raw, str) or not raw.strip():
        return None
    text = raw.strip()
    if text[:1].upper() == "A" and text[1:].isdigit():
        text = text[1:]
    if not text.isdigit():
        return None
    return text.lstrip("0") or "0"


def _service_date(properties: dict) -> str | None:
    raw = properties.get("OrigSchDep")
    if isinstance(raw, str) and len(raw) >= 10:
        parsed = _parse_date(raw[:10])
        if parsed is not None:
            return parsed
    for station in _stations(properties):
        for key in ("schdep", "scharr", "estdep", "estarr"):
            value = station.get(key)
            if isinstance(value, str) and len(value) >= 10:
                parsed = _parse_date(value[:10])
                if parsed is not None:
                    return parsed
    return None


def _parse_date(text: str) -> str | None:
    try:
        return datetime.strptime(text, "%m/%d/%Y").date().isoformat()
    except ValueError:
        return None


def _stations(properties: dict):
    numbered: list[tuple[int, object]] = []
    for key, value in properties.items():
        if isinstance(key, str) and key.startswith("Station") and key[7:].isdigit() and value:
            numbered.append((int(key[7:]), value))
    numbered.sort()
    for _, value in numbered:
        if isinstance(value, dict):
            yield value
        elif isinstance(value, str):
            try:
                parsed = json.loads(value)
            except json.JSONDecodeError:
                continue
            if isinstance(parsed, dict):
                yield parsed


def _station_code(station: dict) -> str | None:
    code = station.get("code")
    if isinstance(code, str) and code.strip():
        return code.strip().upper()
    return None


def _event_times(station: dict) -> list[datetime]:
    zone_code = station.get("tz")
    if _station_code(station) in _ARIZONA_STATIONS:
        zone = _PHOENIX
    else:
        code = zone_code.strip().upper() if isinstance(zone_code, str) else "E"
        zone = _ZONES.get(code, _ZONES["E"])
    moments: list[datetime] = []
    for estimated, scheduled in (("estarr", "scharr"), ("estdep", "schdep")):
        raw = station.get(estimated) or station.get(scheduled)
        if isinstance(raw, str) and raw.strip():
            moment = _parse_local(raw, zone)
            if moment is not None:
                moments.append(moment)
    return moments


def _parse_local(text: str, zone: ZoneInfo) -> datetime | None:
    naive = None
    for fmt in _TIME_FORMATS:
        try:
            naive = datetime.strptime(text.strip(), fmt)
            break
        except ValueError:
            continue
    if naive is None:
        return None
    return naive.replace(tzinfo=zone)
