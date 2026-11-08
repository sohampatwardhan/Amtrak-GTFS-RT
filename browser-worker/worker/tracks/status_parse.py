"""Parse Amtrak train-status documents for tracks, gates, and how far ahead they posted.

The worker publishes ``departure.track.number`` when that number is assigned, and otherwise
``arrival.track.number``. A blank number or ``TBD`` is not a track. Gates are ``gate.number`` on
that side, or ``track.gate`` when the gate is nested there. ``python -m worker.tracks.dump`` and
``python -m worker.tracks.coverage`` both use this module. The hot-set check in
``store.tracked_codes`` is separate and is not changed here: it still treats any non-empty number,
including ``TBD``, as present.

Lead time is the stop's event time minus ``track.asOf``, in seconds. The event time is
``statusInfo.dateTime`` when Amtrak sent one, otherwise ``schedule.dateTime``. Positive means the
track was posted before the event. ``lead_time_secs`` is the earlier of the arrival and departure
postings (the greater lead). ``chosen_lead_time_secs`` is the side the worker would publish.
"""

from __future__ import annotations

from datetime import datetime

from .config import Config

_DEFAULTS = Config()
DEFAULT_WINDOW_BEFORE_SECS = _DEFAULTS.window_before_secs
DEFAULT_WINDOW_AFTER_SECS = _DEFAULTS.window_after_secs
DEFAULT_HOT_WINDOW_SECS = _DEFAULTS.hot_window_secs
MIN_WINDOW_BEFORE_SECS = 600
MAX_WINDOW_BEFORE_SECS = 12 * 3600
HOT_LEAD_SECS = 15 * 60
OBSERVED_LEAD_COUNT = 3

PROFILE_SCHEMA = "amtrak-station-lead-time/v1"
LEAD_TIME_DEFINITION = (
    "Seconds from track.asOf to the event time. The event time is statusInfo.dateTime when "
    "Amtrak sent one, otherwise schedule.dateTime. lead_time_secs is the earlier posting of "
    "the arrival and departure sides (the greater lead). chosen_lead_time_secs is the side the "
    "track-fetcher publishes: departure when that track is posted, otherwise arrival. Positive "
    "means the track was posted before the event."
)
CONSUMER_RULE = (
    "Point AMTRAK_TRACKS_LEAD_TIME_FILE at this file. The loader adopts a station only when "
    "confidence is observed and compared_to_default is wider, using suggested_window_before_secs "
    "and suggested_hot_window_secs. sparse, no_track, none, covered, unknown, and missing "
    f"stations keep the global TRACK_WINDOW_BEFORE_SECS ({DEFAULT_WINDOW_BEFORE_SECS}) and "
    f"TRACK_HOT_WINDOW_SECS ({DEFAULT_HOT_WINDOW_SECS}). Covered stations are not narrowed. "
    f"The lookback stays TRACK_WINDOW_AFTER_SECS ({DEFAULT_WINDOW_AFTER_SECS})."
)

STOP_FIELDS = (
    "fetched_at",
    "train",
    "service_date",
    "stop_index",
    "station",
    "station_name",
    "canceled",
    "has_track",
    "has_gate",
    "side",
    "track",
    "track_as_of",
    "gate",
    "gate_as_of",
    "scheduled",
    "estimated",
    "lead_time_secs",
    "lead_time_basis",
    "chosen_side",
    "chosen_track",
    "chosen_track_as_of",
    "chosen_gate",
    "chosen_lead_time_secs",
    "arrival_scheduled",
    "arrival_estimated",
    "arrival_time_basis",
    "arrival_track",
    "arrival_track_as_of",
    "arrival_gate",
    "arrival_gate_as_of",
    "arrival_canceled",
    "arrival_lead_time_secs",
    "departure_scheduled",
    "departure_estimated",
    "departure_time_basis",
    "departure_track",
    "departure_track_as_of",
    "departure_gate",
    "departure_gate_as_of",
    "departure_canceled",
    "departure_lead_time_secs",
)

STATION_FIELDS = (
    "station",
    "station_name",
    "trains_seen",
    "train_numbers",
    "stops_seen",
    "canceled_stops",
    "stops_with_track",
    "pct_with_track",
    "stops_with_gate",
    "pct_with_gate",
    "gates_seen",
    "tracks_seen",
    "lead_n",
    "lead_min_secs",
    "lead_median_secs",
    "lead_max_secs",
    "chosen_lead_n",
    "chosen_lead_min_secs",
    "chosen_lead_median_secs",
    "chosen_lead_max_secs",
    "suggested_window_before_secs",
    "suggested_hot_window_secs",
    "compared_to_default",
    "confidence",
)


def posted_track_number(number: object) -> str:
    """A platform token, or ``""`` when the number is missing, blank, or ``TBD``.

    Integers are accepted. The original case is kept after stripping; callers that want the
    worker's uppercase label apply ``str.upper`` themselves.
    """
    if isinstance(number, bool) or number is None:
        return ""
    if isinstance(number, int):
        text = str(number)
    elif isinstance(number, str):
        text = number.strip()
    else:
        return ""
    if not text or text.upper() == "TBD":
        return ""
    return text


def parse_instant(text: str) -> datetime | None:
    """An aware ``datetime`` from ``YYYY-MM-DDTHH:MM:SS`` with ``Z`` or ``±HH:MM``.

    Fractional seconds are accepted. A value with no offset is rejected so a lead time is never
    computed by assuming a zone.
    """
    if not isinstance(text, str):
        return None
    raw = text.strip()
    if not raw:
        return None
    if raw.endswith("Z"):
        raw = raw[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(raw)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        return None
    return parsed


def seconds_between(start_iso: str, end_iso: str) -> int | None:
    """Whole seconds from ``start_iso`` to ``end_iso``, truncating toward zero."""
    start = parse_instant(start_iso)
    end = parse_instant(end_iso)
    if start is None or end is None:
        return None
    return int((end - start).total_seconds())


def posted_track_rows(train: str, service_date: str, document: dict) -> list[dict]:
    """Every arrival or departure with a posted track, in stop order.

    This is the row shape ``python -m worker.tracks.dump`` prints. A track Amtrak has not assigned
    (an empty number or ``TBD``) is left out. Both sides are listed when both have a number.
    """
    rows: list[dict] = []
    for service in _services(document):
        for stop in _stop_dicts(service):
            station = stop.get("station") if isinstance(stop.get("station"), dict) else {}
            code = station.get("code") or stop.get("code") or ""
            if not isinstance(code, str):
                code = ""
            name = station.get("name") or ""
            if not isinstance(name, str):
                name = ""
            for side in ("arrival", "departure"):
                event = stop.get(side)
                if not isinstance(event, dict):
                    continue
                track = event.get("track")
                if not isinstance(track, dict):
                    continue
                number = posted_track_number(track.get("number"))
                if not number:
                    continue
                gate = event.get("gate") or track.get("gate")
                schedule = event.get("schedule")
                rows.append(
                    {
                        "train": train,
                        "service_date": service_date,
                        "station": code,
                        "station_name": name,
                        "side": side,
                        "track": number,
                        "gate": (gate.get("number") if isinstance(gate, dict) else gate) or "",
                        "scheduled": (
                            schedule.get("dateTime") if isinstance(schedule, dict) else None
                        )
                        or "",
                        "as_of": track.get("asOf") or "",
                    }
                )
    return rows


def stops_from_document(
    document: dict, *, train: str, service_date: str, fetched_at: str
) -> tuple[list[dict], int]:
    """One row per stop, plus how many ``BUS`` services in ``document`` were skipped.

    Rows stay in document order. :func:`sort_stops` is what makes a report stable.
    """
    rows: list[dict] = []
    bus_services = 0
    index = 0
    for service in _services(document):
        if _service_type(service) == "BUS":
            bus_services += 1
            continue
        for stop in _stop_dicts(service):
            rows.append(
                compose_stop(
                    train=train,
                    service_date=service_date,
                    stop_index=index,
                    station=_station_code(stop),
                    station_name=_station_name(stop),
                    fetched_at=fetched_at,
                    arrival=_named_side(stop.get("arrival"), "arrival"),
                    departure=_named_side(stop.get("departure"), "departure"),
                )
            )
            index += 1
    return rows, bus_services


def compose_stop(
    *,
    train: str,
    service_date: str,
    stop_index: int,
    station: str,
    station_name: str,
    fetched_at: str,
    arrival: dict | None,
    departure: dict | None,
) -> dict:
    """One stop row from already-read sides. Lead times are computed here."""
    arrival_lead, arrival_basis = _side_lead(arrival)
    departure_lead, departure_basis = _side_lead(departure)
    if arrival is not None:
        arrival = {**arrival, "lead_time_secs": arrival_lead, "lead_time_basis": arrival_basis}
    if departure is not None:
        departure = {
            **departure,
            "lead_time_secs": departure_lead,
            "lead_time_basis": departure_basis,
        }
    posted = [side for side in (arrival, departure) if side and side["track"]]
    if posted:
        first = min(posted, key=lambda side: _posting_rank(side))
    else:
        first = departure or arrival
    if departure and departure["track"]:
        chosen = departure
    elif arrival and arrival["track"]:
        chosen = arrival
    else:
        chosen = departure or arrival
    present = [side for side in (arrival, departure) if side is not None]
    canceled = bool(present) and all(side["canceled"] for side in present)
    gates = [side["gate"] for side in present if side["gate"]]
    row = {
        "fetched_at": fetched_at,
        "train": train,
        "service_date": service_date,
        "stop_index": stop_index,
        "station": station,
        "station_name": station_name,
        "canceled": canceled,
        "has_track": bool(posted),
        "has_gate": bool(gates),
        "side": first["side"] if first else "",
        "track": first["track"] if first else "",
        "track_as_of": first["track_as_of"] if first else "",
        "gate": first["gate"] if first else "",
        "gate_as_of": first["gate_as_of"] if first else "",
        "scheduled": first["scheduled"] if first else "",
        "estimated": first["estimated"] if first else "",
        "lead_time_secs": first["lead_time_secs"] if first else None,
        "lead_time_basis": first["lead_time_basis"] if first else "",
        "chosen_side": chosen["side"] if chosen else "",
        "chosen_track": chosen["track"] if chosen else "",
        "chosen_track_as_of": chosen["track_as_of"] if chosen else "",
        "chosen_gate": chosen["gate"] if chosen else "",
        "chosen_lead_time_secs": chosen["lead_time_secs"] if chosen else None,
    }
    row.update(_flat_side("arrival", arrival))
    row.update(_flat_side("departure", departure))
    return row


def sort_stops(stops: list[dict]) -> list[dict]:
    """Station, origin date, train number, then the stop's index in its document."""
    return sorted(stops, key=_stop_key)


def coverage_report(stops: list[dict], *, generated_at: str, snapshot_count: int) -> dict:
    """Per-station stats and the lead-time profile the worker could load later.

    ``stations`` inside the profile is the same list as the top-level return's stations would
    be; callers embed ``profile`` and may also copy ``profile["stations"]``.
    """
    stations = summarize_stations(stops)
    return {
        "schema": PROFILE_SCHEMA,
        "generated_at": generated_at,
        "snapshot_count": snapshot_count,
        "lead_time_definition": LEAD_TIME_DEFINITION,
        "defaults": {
            "window_before_secs": DEFAULT_WINDOW_BEFORE_SECS,
            "window_after_secs": DEFAULT_WINDOW_AFTER_SECS,
            "hot_window_secs": DEFAULT_HOT_WINDOW_SECS,
        },
        "loaded_by_sidecar": False,
        "consumer": CONSUMER_RULE,
        "stations": stations,
    }


def summarize_stations(stops: list[dict]) -> list[dict]:
    """One row per station code, sorted by that code.

    Percentages and lead times ignore canceled stops. A station whose stops are all canceled
    stays in the list with ``stops_seen`` 0 so the snapshot still shows it was read.
    """
    grouped: dict[str, list[dict]] = {}
    for stop in stops:
        grouped.setdefault(stop["station"], []).append(stop)
    stations = [_station_summary(code, grouped[code]) for code in sorted(grouped)]
    return stations


def merge_stop_rows(groups: list[list[dict]]) -> list[dict]:
    """One row per train-day stop across snapshots.

    The track number and gate are the newest snapshot that still showed a number. ``asOf`` is
    the earliest stamp of that same number, so the lead is when Amtrak first posted it, not when
    a later poll repeated it. Event times come from the newest snapshot. A stop that had a track
    in any snapshot keeps it after a later document clears the field.
    """
    buckets: dict[tuple, list[dict]] = {}
    for stops in groups:
        for row in stops:
            key = (row["train"], row["service_date"], row["station"], row["stop_index"])
            buckets.setdefault(key, []).append(row)
    merged = [_merge_bucket(buckets[key]) for key in sorted(buckets, key=_bucket_key)]
    return sort_stops(merged)


def board_field_counts(document: object) -> dict:
    """How many posted track and gate numbers a station-board document carries.

    The corridor board is not a source of rows. Survey records this so a run can say whether
    that response still includes the fields it had on 2026-10-17.
    """
    tracks = 0
    gates = 0
    stack = [document]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            track = node.get("track")
            if isinstance(track, dict) and posted_track_number(track.get("number")):
                tracks += 1
            gate = node.get("gate")
            if isinstance(gate, dict) and _gate_token(gate.get("number")):
                gates += 1
            stack.extend(node.values())
        elif isinstance(node, list):
            stack.extend(node)
    return {
        "has_track": tracks > 0,
        "has_gate": gates > 0,
        "track_count": tracks,
        "gate_count": gates,
    }


def median_seconds(values: list[int]) -> int | None:
    """Median of whole seconds. An even count rounds the mean half away from zero."""
    if not values:
        return None
    ordered = sorted(values)
    mid = len(ordered) // 2
    if len(ordered) % 2 == 1:
        return ordered[mid]
    return _mean_half_away(ordered[mid - 1], ordered[mid])


def percent(part: int, whole: int) -> float | None:
    """``part / whole`` as a percent with one decimal, half up. ``None`` when ``whole`` is 0."""
    if whole <= 0:
        return None
    tenths = (part * 1000 + whole // 2) // whole
    return tenths / 10


def suggested_window_before(max_lead: int) -> int | None:
    """Lookahead covering ``max_lead``, plus 10 percent (at least a minute), clamped.

    Zero or negative means the stamp is not before the event, so this snapshot cannot say how
    early to poll. Otherwise the result is rounded up to the next minute and kept between 10
    minutes and 12 hours.
    """
    if max_lead <= 0:
        return None
    headroom = max(60, (max_lead + 9) // 10)
    return _clamp(
        _ceil_minute(max_lead + headroom), MIN_WINDOW_BEFORE_SECS, MAX_WINDOW_BEFORE_SECS
    )


def suggested_hot_window(chosen_median: int | None) -> int | None:
    """Hot-window length. Short leads widen it to the median; long leads keep the default."""
    if chosen_median is None:
        return None
    if chosen_median <= HOT_LEAD_SECS:
        return max(DEFAULT_HOT_WINDOW_SECS, _ceil_minute(max(chosen_median, 0)))
    return DEFAULT_HOT_WINDOW_SECS


def _services(document: object):
    if not isinstance(document, dict):
        return
    data = document.get("data")
    if not isinstance(data, list):
        return
    for service in data:
        if isinstance(service, dict):
            yield service


def _stop_dicts(service: dict):
    stops = service.get("stops")
    if not isinstance(stops, list):
        return
    for stop in stops:
        if isinstance(stop, dict):
            yield stop


def _service_type(service: dict) -> str:
    travel = service.get("travelService")
    if not isinstance(travel, dict):
        return ""
    kind = travel.get("type")
    if not isinstance(kind, dict):
        return ""
    code = kind.get("code")
    return code.strip().upper() if isinstance(code, str) else ""


def _station_code(stop: dict) -> str:
    station = stop.get("station") if isinstance(stop.get("station"), dict) else {}
    raw = station.get("code") if isinstance(station.get("code"), str) else ""
    if not raw and isinstance(stop.get("code"), str):
        raw = stop["code"]
    return raw.strip().upper()


def _station_name(stop: dict) -> str:
    station = stop.get("station") if isinstance(stop.get("station"), dict) else {}
    name = station.get("name")
    return name.strip() if isinstance(name, str) else ""


def _read_side(event: object) -> dict | None:
    if not isinstance(event, dict):
        return None
    info = event.get("statusInfo") if isinstance(event.get("statusInfo"), dict) else {}
    schedule = event.get("schedule") if isinstance(event.get("schedule"), dict) else {}
    scheduled = _text(schedule.get("dateTime"))
    estimated = _text(info.get("dateTime"))
    kind = _text(info.get("dateTimeType")).upper()
    if estimated:
        basis = {"ESTIMATE": "estimated", "ACTUAL": "actual", "SCHEDULED": "scheduled"}.get(
            kind, "estimated"
        )
    elif scheduled:
        basis = "scheduled"
    else:
        basis = ""
    track = event.get("track") if isinstance(event.get("track"), dict) else {}
    number = posted_track_number(track.get("number")).upper()
    gate_obj = event.get("gate") or track.get("gate")
    gate = ""
    gate_as_of = ""
    if isinstance(gate_obj, dict):
        gate = _gate_token(gate_obj.get("number"))
        gate_as_of = _text(gate_obj.get("asOf"))
    elif gate_obj is not None:
        gate = _gate_token(gate_obj)
    return {
        "side": "",
        "scheduled": scheduled,
        "estimated": estimated,
        "time_basis": basis,
        "track": number,
        "track_as_of": _text(track.get("asOf")) if number else "",
        "gate": gate,
        "gate_as_of": gate_as_of if gate else "",
        "canceled": _event_canceled(info),
        "lead_time_secs": None,
        "lead_time_basis": "",
    }


def _event_canceled(info: dict) -> bool:
    for key in ("status", "displayStatus"):
        text = _text(info.get(key)).upper()
        if text in {"CANCELED", "CANCELLED"}:
            return True
    return False


def _gate_token(number: object) -> str:
    if isinstance(number, bool) or number is None:
        return ""
    if isinstance(number, int):
        text = str(number)
    elif isinstance(number, str):
        text = number.strip()
    else:
        return ""
    if not text or text.upper() == "TBD":
        return ""
    return text.upper()


def _text(value: object) -> str:
    return value.strip() if isinstance(value, str) else ""


def _side_lead(side: dict | None) -> tuple[int | None, str]:
    if side is None or not side["track"] or not side["track_as_of"]:
        return None, ""
    clock = side["estimated"] or side["scheduled"]
    if not clock:
        return None, ""
    lead = seconds_between(side["track_as_of"], clock)
    if lead is None:
        return None, ""
    basis = side["time_basis"] or ("estimated" if side["estimated"] else "scheduled")
    return lead, basis


def _posting_rank(side: dict) -> tuple:
    lead = side["lead_time_secs"]
    return (lead is None, -(lead or 0), 0 if side["side"] == "departure" else 1)


def _flat_side(name: str, side: dict | None) -> dict:
    prefix = f"{name}_"
    if side is None:
        return {
            f"{prefix}scheduled": "",
            f"{prefix}estimated": "",
            f"{prefix}time_basis": "",
            f"{prefix}track": "",
            f"{prefix}track_as_of": "",
            f"{prefix}gate": "",
            f"{prefix}gate_as_of": "",
            f"{prefix}canceled": False,
            f"{prefix}lead_time_secs": None,
        }
    return {
        f"{prefix}scheduled": side["scheduled"],
        f"{prefix}estimated": side["estimated"],
        f"{prefix}time_basis": side["time_basis"],
        f"{prefix}track": side["track"],
        f"{prefix}track_as_of": side["track_as_of"],
        f"{prefix}gate": side["gate"],
        f"{prefix}gate_as_of": side["gate_as_of"],
        f"{prefix}canceled": side["canceled"],
        f"{prefix}lead_time_secs": side["lead_time_secs"],
    }


def _named_side(event: object, side_name: str) -> dict | None:
    side = _read_side(event)
    if side is not None:
        side["side"] = side_name
    return side


def _stop_key(row: dict) -> tuple:
    train = str(row["train"])
    train_ord = int(train) if train.isdigit() else 10**9
    return (row["station"], row["service_date"], train_ord, train, row["stop_index"])


def _bucket_key(key: tuple) -> tuple:
    train, service_date, station, index = key
    text = str(train)
    train_ord = int(text) if text.isdigit() else 10**9
    return (station, service_date, train_ord, text, index)


def _station_summary(code: str, rows: list[dict]) -> dict:
    ordered = sorted(rows, key=_stop_key)
    name = next((row["station_name"] for row in ordered if row["station_name"]), "")
    live = [row for row in ordered if not row["canceled"]]
    train_keys = sorted(
        {(row["train"], row["service_date"]) for row in live},
        key=lambda item: (
            item[1],
            int(item[0]) if str(item[0]).isdigit() else 10**9,
            str(item[0]),
        ),
    )
    numbers = sorted(
        {str(train) for train, _ in train_keys},
        key=lambda number: (int(number) if number.isdigit() else 10**9, number),
    )
    with_track = [row for row in live if row["has_track"]]
    with_gate = [row for row in live if row["has_gate"]]
    leads = [row["lead_time_secs"] for row in with_track if row["lead_time_secs"] is not None]
    chosen = [
        row["chosen_lead_time_secs"]
        for row in with_track
        if row["chosen_lead_time_secs"] is not None
    ]
    gates = sorted(
        {
            gate
            for row in live
            for gate in (row["arrival_gate"], row["departure_gate"], row["gate"])
            if gate
        }
    )
    tracks = sorted(
        {
            track
            for row in live
            for track in (row["arrival_track"], row["departure_track"], row["track"])
            if track
        }
    )
    lead = _lead_stats(leads)
    chosen_lead = _lead_stats(chosen)
    window = suggested_window_before(lead["max"]) if lead["max"] is not None else None
    hot = suggested_hot_window(chosen_lead["median"])
    if window is None:
        compared = "unknown"
    elif window > DEFAULT_WINDOW_BEFORE_SECS:
        compared = "wider"
    else:
        compared = "covered"
    if not live:
        confidence = "none"
    elif not with_track:
        confidence = "no_track"
    elif len(leads) < OBSERVED_LEAD_COUNT:
        confidence = "sparse"
    else:
        confidence = "observed"
    return {
        "station": code,
        "station_name": name,
        "trains_seen": len(train_keys),
        "train_numbers": numbers,
        "stops_seen": len(live),
        "canceled_stops": len(rows) - len(live),
        "stops_with_track": len(with_track),
        "pct_with_track": percent(len(with_track), len(live)),
        "stops_with_gate": len(with_gate),
        "pct_with_gate": percent(len(with_gate), len(live)),
        "gates_seen": gates,
        "tracks_seen": tracks,
        "lead_n": lead["n"],
        "lead_min_secs": lead["min"],
        "lead_median_secs": lead["median"],
        "lead_max_secs": lead["max"],
        "chosen_lead_n": chosen_lead["n"],
        "chosen_lead_min_secs": chosen_lead["min"],
        "chosen_lead_median_secs": chosen_lead["median"],
        "chosen_lead_max_secs": chosen_lead["max"],
        "suggested_window_before_secs": window,
        "suggested_hot_window_secs": hot,
        "compared_to_default": compared,
        "confidence": confidence,
    }


def _lead_stats(values: list[int]) -> dict:
    if not values:
        return {"n": 0, "min": None, "median": None, "max": None}
    return {
        "n": len(values),
        "min": min(values),
        "median": median_seconds(values),
        "max": max(values),
    }


def _merge_bucket(rows: list[dict]) -> dict:
    ordered = sorted(rows, key=_fetched_key)
    last = ordered[-1]
    name = ""
    for row in reversed(ordered):
        if row.get("station_name"):
            name = row["station_name"]
            break
    return compose_stop(
        train=str(last["train"]),
        service_date=str(last["service_date"]),
        stop_index=int(last["stop_index"]),
        station=str(last["station"]),
        station_name=name,
        fetched_at=str(last.get("fetched_at") or ""),
        arrival=_merge_side(ordered, "arrival"),
        departure=_merge_side(ordered, "departure"),
    )


def _merge_side(rows: list[dict], name: str) -> dict | None:
    prefix = f"{name}_"

    def present(row: dict) -> bool:
        return any(
            (
                row.get(f"{prefix}scheduled"),
                row.get(f"{prefix}estimated"),
                row.get(f"{prefix}track"),
                row.get(f"{prefix}gate"),
                row.get(f"{prefix}canceled"),
                row.get(f"{prefix}time_basis"),
            )
        )

    if not any(present(row) for row in rows):
        return None
    newest = None
    for row in rows:
        if row.get(f"{prefix}track"):
            newest = row
    last = rows[-1]
    if newest is None:
        track = ""
        as_of = ""
        gate = ""
        gate_as_of = ""
    else:
        track = str(newest.get(f"{prefix}track") or "")
        gate = str(newest.get(f"{prefix}gate") or "")
        gate_as_of = str(newest.get(f"{prefix}gate_as_of") or "")
        as_of = _earliest_as_of(rows, prefix, track)
    return {
        "side": name,
        "scheduled": str(last.get(f"{prefix}scheduled") or ""),
        "estimated": str(last.get(f"{prefix}estimated") or ""),
        "time_basis": str(last.get(f"{prefix}time_basis") or ""),
        "track": track,
        "track_as_of": as_of,
        "gate": gate,
        "gate_as_of": gate_as_of,
        "canceled": bool(last.get(f"{prefix}canceled")),
        "lead_time_secs": None,
        "lead_time_basis": "",
    }


def _earliest_as_of(rows: list[dict], prefix: str, number: str) -> str:
    best: datetime | None = None
    best_iso = ""
    fallback = ""
    for row in rows:
        if row.get(f"{prefix}track") != number:
            continue
        iso = str(row.get(f"{prefix}track_as_of") or "")
        if iso and not fallback:
            fallback = iso
        instant = parse_instant(iso)
        if instant is None:
            continue
        if best is None or instant < best:
            best = instant
            best_iso = iso
    return best_iso or fallback


def _fetched_key(row: dict) -> tuple:
    iso = str(row.get("fetched_at") or "")
    instant = parse_instant(iso)
    stamp = instant.timestamp() if instant is not None else float("-inf")
    return (stamp, iso)


def _mean_half_away(left: int, right: int) -> int:
    total = left + right
    if total % 2 == 0:
        return total // 2
    if total > 0:
        return total // 2 + 1
    return total // 2


def _ceil_minute(seconds: int) -> int:
    if seconds <= 0:
        return 0
    return ((seconds + 59) // 60) * 60


def _clamp(value: int, low: int, high: int) -> int:
    return max(low, min(high, value))
