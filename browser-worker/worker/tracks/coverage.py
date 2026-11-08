"""Station coverage for Amtrak track and gate numbers.

``www.amtrak.com`` answers the train-status page's own fetches and resets other clients, so this
opens ``https://www.amtrak.com/tickets/train-status.html`` once in headless Chromium and calls
the same status URL the page calls, from that page. It does not impersonate a browser, copy
cookies, or fall back to another HTTP client. The maps train list is plain HTTP and is decrypted
with :mod:`worker.tracks.decrypt`. Parsing is :mod:`worker.tracks.status_parse`, shared with
``python -m worker.tracks.dump``.

The same inputs produce the same request order: origin service date, then train number. Output
rows are sorted by station, date, train, and stop index. The delay between www.amtrak.com
requests is fixed (at least one second, default two). There is no jitter and no retry.

``survey`` writes an ``amtrak-station-lead-time/v1`` profile. Trip updates load that file from
``AMTRAK_TRACKS_LEAD_TIME_FILE`` and adopt stations whose confidence is ``observed`` and whose
``compared_to_default`` is ``wider``.

Usage::

    python -m worker.tracks.coverage tracks 66 --date 2026-10-31 --station KIN --out train66.json
    python -m worker.tracks.coverage survey --out survey.json
    python -m worker.tracks.coverage survey --interval 900 --duration 3600 --out-dir snapshots
    python -m worker.tracks.coverage merge snapshots --out coverage.json
"""

from __future__ import annotations

import argparse
import asyncio
import csv
import json
import os
import re
import sys
import time
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from urllib.error import URLError

from .config import DEFAULT_PAGE_URL, DEFAULT_TRAINS_URL, Config
from .decrypt import DecryptError
from .dump import new_york_today, service_date, train_number
from .poller import _FETCH_JS, _LAUNCH_ARGS, is_first_party, load_trains, should_block, status_url
from .select import FleetTrain, list_fleet
from .status_parse import (
    PROFILE_SCHEMA,
    STOP_FIELDS,
    STATION_FIELDS,
    board_field_counts,
    coverage_report,
    merge_stop_rows,
    sort_stops,
    stops_from_document,
)

SCHEMA_VERSION = 1
DEFAULT_DELAY_SECS = 2.0
MIN_DELAY_SECS = 1.0
MAX_DELAY_SECS = 60.0
TRACKS_MAX_REQUESTS = 40
SURVEY_MAX_REQUESTS = 300
MAX_REQUESTS_LIMIT = 500
MIN_INTERVAL_SECS = 60.0
MAX_BOX_SECS = 86400.0
MAX_CYCLES = 96
_FATAL_STATUS = frozenset({0, 401, 403, 429})
_STATION = re.compile(r"[A-Za-z0-9]{3,5}")
STOPS_ENDPOINT_NOTE = (
    "The train-status page requests this before the per-train document. A check on "
    "2026-11-01 found no track or gate fields, so this tool does not call it."
)
BOARD_NOTE = (
    "Optional corridor board (/dotcom/travel-service/statuses/stations). Survey probes one "
    "pair per run (default NYP to WAS) and records whether track and gate numbers are present. "
    "Tracks mode does not call it. On 2026-10-17 NYP-WAS carried them; on 2026-11-01 a browser "
    "check did not. Rows below come only from per-train status documents."
)

EXIT_OK = 0
EXIT_EMPTY = 1
EXIT_BLOCKED = 3


@dataclass(frozen=True)
class JobPlan:
    """Trains this run will fetch, in request order, plus the ones the cap left out."""

    jobs: list[tuple[str, str]]
    not_fetched: list[tuple[str, str]]
    matched: int
    bus_only: int
    missing_trains: list[str]
    fleet_error: str | None


def station_code(raw: str) -> str:
    """A station code: three to five letters or digits, stored uppercase."""
    if not _STATION.fullmatch(raw):
        raise argparse.ArgumentTypeError(f"not a station code: {raw!r}")
    return raw.upper()


def station_board_url(origin: str, destination: str, departure_date: str) -> str:
    """The corridor board the train-status page also requests. Secondary to per-train status."""
    return (
        "https://www.amtrak.com/dotcom/travel-service/statuses/stations"
        f"?origin-code={origin}&destination-code={destination}&departure-date={departure_date}"
    )


def jobs_from_fleet(
    fleet: Sequence[FleetTrain],
    *,
    trains: set[str] | None,
    stations: set[str] | None,
    offset: int,
    limit: int,
) -> tuple[list[tuple[str, str]], list[tuple[str, str]], int]:
    """``(chosen, not_fetched, matched)`` from an already sorted fleet.

    ``stations`` keeps a train when any of its rail codes is in the set. ``offset`` skips that
    many matches before ``limit`` is applied, so two runs can cover a fleet without overlap.
    """
    matched: list[tuple[str, str]] = []
    for train in fleet:
        if trains is not None and train.train_number not in trains:
            continue
        if stations and stations.isdisjoint(train.station_codes):
            continue
        matched.append((train.train_number, train.service_date))
    if limit < 0:
        limit = 0
    chosen = matched[offset : offset + limit]
    return chosen, matched[offset + limit :], len(matched)


def jobs_from_dates(
    trains: Sequence[str], dates: Sequence[str], *, offset: int, limit: int
) -> tuple[list[tuple[str, str]], list[tuple[str, str]], int]:
    """Each named train on each date, sorted by date then train number, then capped."""
    pairs: list[tuple[str, str]] = []
    seen: set[tuple[str, str]] = set()
    for day in sorted(set(dates)):
        for number in sorted(set(trains), key=lambda item: (int(item), item)):
            pair = (number, day)
            if pair in seen:
                continue
            seen.add(pair)
            pairs.append(pair)
    chosen = pairs[offset : offset + limit]
    return chosen, pairs[offset + limit :], len(pairs)


def next_pause(*, elapsed: float, interval: float, time_left: float) -> float | None:
    """Seconds to sleep before another cycle, or ``None`` when the time box is over.

    ``time_left`` is what remains after the cycle that just finished. The cycle itself is not
    cut short. A pause that would start the next cycle on or after the deadline is not taken.
    """
    if time_left <= 0:
        return None
    pause = interval - elapsed
    if pause < 0:
        pause = 0.0
    if pause >= time_left:
        return None
    return pause


def read_fetch_result(raw: object) -> dict:
    """One in-page ``fetch`` result: ``http_status``, ``document``, ``error``, ``fatal``.

    ``fatal`` means this client is being refused (403, 401, 429, or no HTTP status) and the
    rest of the run should stop. A train that does not operate that day is not fatal.
    """
    if not isinstance(raw, dict):
        return {"http_status": None, "document": None, "error": "no response", "fatal": True}
    status = raw.get("status")
    if isinstance(status, bool) or not isinstance(status, int):
        status = None
    if status != 200:
        label = "no response" if status is None else f"http {status}"
        return {
            "http_status": status,
            "document": None,
            "error": label,
            "fatal": status in _FATAL_STATUS or status is None,
        }
    body = raw.get("body")
    if not isinstance(body, str):
        return {"http_status": status, "document": None, "error": "no body", "fatal": False}
    try:
        document = json.loads(body)
    except json.JSONDecodeError:
        return {"http_status": status, "document": None, "error": "not json", "fatal": False}
    if not isinstance(document, dict):
        return {"http_status": status, "document": None, "error": "not json", "fatal": False}
    if "data" not in document:
        message = "no data"
        error = document.get("error")
        if isinstance(error, dict):
            text = error.get("message")
            if isinstance(text, str) and text.strip():
                message = text.strip()
        return {"http_status": status, "document": None, "error": message, "fatal": False}
    return {"http_status": status, "document": document, "error": None, "fatal": False}


def abort_message(reason: str) -> str:
    """What to print when the browser or Akamai stops a run."""
    if reason in {"http 403", "http 401", "http 429", "http 0", "no response"}:
        return (
            f"www.amtrak.com refused the in-page fetch ({reason}). Akamai blocks clients that "
            "are not a real page on https://www.amtrak.com/tickets/train-status.html. This run "
            "opened that page in headless Chromium, the same way the track-fetcher does, and "
            "did not retry or switch to another HTTP client. Re-run where that page loads."
        )
    if reason == "playwright is not installed":
        return (
            "Playwright is not installed, so the status page could not be opened. "
            "From browser-worker/: python -m pip install playwright==1.64.1 && "
            "python -m playwright install --only-shell chromium"
        )
    return (
        f"The status browser stopped ({reason}). Requests are made from "
        "https://www.amtrak.com/tickets/train-status.html and were not retried with another client."
    )


def fleet_error_message(error: BaseException) -> str:
    """Plain-HTTP failure of the maps train list. www.amtrak.com was not called."""
    return (
        "Could not read the maps train list "
        f"({type(error).__name__}: {error}). That host answers plain HTTP and is decrypted "
        "locally. The status pages on www.amtrak.com were not called."
    )


def filter_station_rows(stops: list[dict], stations: set[str] | None) -> list[dict]:
    """Keep stops whose code is in ``stations``. ``None`` or empty keeps every stop."""
    if not stations:
        return stops
    return [row for row in stops if row["station"] in stations]


def build_payload(
    stops: list[dict],
    *,
    kind: str,
    fetched_at: str,
    delay_secs: float | None,
    max_requests: int | None,
    offset: int,
    truncated: bool,
    matched_trains: int,
    station_filter: list[str],
    missing_trains: list[str],
    not_fetched: list[dict],
    bus_only_skipped: int,
    bus_services_skipped: int,
    status_documents: int,
    aborted: dict | None,
    sources: list[str],
    input_files: list[str],
    inputs_aborted: int,
    requests: list[dict],
    station_board: dict,
    snapshot_count: int,
) -> dict:
    """The JSON document for one run or a merge. Stops and stations are sorted."""
    ordered = sort_stops(stops)
    profile = coverage_report(ordered, generated_at=fetched_at, snapshot_count=snapshot_count)
    return {
        "schema_version": SCHEMA_VERSION,
        "kind": kind,
        "fetched_at": fetched_at,
        "delay_secs": delay_secs,
        "max_requests": max_requests,
        "offset": offset,
        "truncated": truncated,
        "matched_trains": matched_trains,
        "station_filter": list(station_filter),
        "missing_trains": list(missing_trains),
        "not_fetched": not_fetched,
        "bus_only_skipped": bus_only_skipped,
        "bus_services_skipped": bus_services_skipped,
        "status_documents": status_documents,
        "aborted": aborted,
        "sources": list(sources),
        "input_files": list(input_files),
        "inputs_aborted": inputs_aborted,
        "snapshot_count": snapshot_count,
        "requests": requests,
        "station_board": station_board,
        "stops_endpoint": {
            "url_template": (
                "https://www.amtrak.com/dotcom/travel-service/statuses/stops/"
                "{station}?service-numbers={train}&departure-date={date}"
            ),
            "probed": False,
            "note": STOPS_ENDPOINT_NOTE,
        },
        "stops": ordered,
        "stations": profile["stations"],
        "profile": profile,
    }


def unprobed_board() -> dict:
    """Board object used when this run does not call the corridor endpoint."""
    return {
        "probed": False,
        "origin": None,
        "destination": None,
        "departure_date": None,
        "url": None,
        "http_status": None,
        "has_track": None,
        "has_gate": None,
        "track_count": None,
        "gate_count": None,
        "error": None,
        "note": BOARD_NOTE,
    }


def format_duration(seconds: int | None) -> str:
    """``4h10m``, ``15m``, ``12m05s``, or a leading minus. Blank when unknown."""
    if seconds is None:
        return ""
    sign = "-" if seconds < 0 else ""
    remaining = abs(int(seconds))
    hours, rest = divmod(remaining, 3600)
    minutes, secs = divmod(rest, 60)
    if hours:
        return f"{sign}{hours}h{minutes:02d}m"
    if minutes and secs:
        return f"{sign}{minutes}m{secs:02d}s"
    if minutes:
        return f"{sign}{minutes}m"
    return f"{sign}{secs}s"


def format_station_table(stations: Sequence[dict]) -> str:
    """The per-station coverage table. Ahead is how long before the stop a track was posted."""
    if not stations:
        return "No stops."
    header = [
        "Station",
        "Name",
        "Trains",
        "Stops",
        "Canceled",
        "Track%",
        "Gate%",
        "Ahead min",
        "Ahead med",
        "Ahead max",
        "Window",
        "Hot",
        "Confidence",
        "Gates",
    ]
    body = []
    for station in stations:
        body.append(
            [
                station["station"],
                station["station_name"],
                str(station["trains_seen"]),
                str(station["stops_seen"]),
                str(station["canceled_stops"]),
                _percent_cell(station["pct_with_track"]),
                _percent_cell(station["pct_with_gate"]),
                format_duration(station["lead_min_secs"]),
                format_duration(station["lead_median_secs"]),
                format_duration(station["lead_max_secs"]),
                format_duration(station["suggested_window_before_secs"]),
                format_duration(station["suggested_hot_window_secs"]),
                station["confidence"],
                ",".join(station["gates_seen"]),
            ]
        )
    return _aligned(header, body)


def format_stop_table(stops: Sequence[dict]) -> str:
    """Chosen track (departure, else arrival) for each stop, plus both sides."""
    if not stops:
        return "No stops."
    header = [
        "Train",
        "Date",
        "Station",
        "Track",
        "Gate",
        "Side",
        "Lead",
        "Arrival",
        "Departure",
    ]
    body = []
    for row in stops:
        body.append(
            [
                str(row["train"]),
                str(row["service_date"]),
                row["station"],
                row["chosen_track"],
                row["chosen_gate"],
                row["chosen_side"],
                format_duration(row["chosen_lead_time_secs"]),
                _side_cell(row["arrival_track"], row["arrival_gate"]),
                _side_cell(row["departure_track"], row["departure_gate"]),
            ]
        )
    return _aligned(header, body)


def stable_json(payload: object) -> str:
    """Indented JSON with sorted keys and a trailing newline."""
    return json.dumps(payload, indent=2, sort_keys=True) + "\n"


def write_report(path: Path, payload: dict) -> None:
    """Write ``path``, the stop CSV, the station CSV, and the profile JSON beside it."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(stable_json(payload), encoding="utf-8")
    _write_csv(path.with_suffix(".csv"), STOP_FIELDS, payload["stops"])
    _write_csv(path.with_suffix(".stations.csv"), STATION_FIELDS, payload["stations"])
    path.with_suffix(".profile.json").write_text(stable_json(payload["profile"]), encoding="utf-8")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    """CLI arguments. Exits via ``SystemExit`` when a value is out of range."""
    parser = argparse.ArgumentParser(
        prog="python -m worker.tracks.coverage",
        description=(
            "Fetch Amtrak track and gate numbers and summarize which stations publish them. "
            "Requests run in sorted order through one headless browser page, with a fixed delay."
        ),
    )
    sub = parser.add_subparsers(dest="command", required=True)

    tracks = sub.add_parser("tracks", help="status documents for specific train numbers")
    tracks.add_argument("trains", nargs="+", type=train_number, help="train numbers, e.g. 66 2150")
    tracks.add_argument(
        "--date",
        dest="dates",
        action="append",
        type=service_date,
        help="origin service date (YYYY-MM-DD); default is each train's origin date on the maps feed",
    )
    _add_fetch_args(tracks, default_max=TRACKS_MAX_REQUESTS)

    survey = sub.add_parser("survey", help="every active train, then a per-station profile")
    _add_fetch_args(survey, default_max=SURVEY_MAX_REQUESTS)
    survey.add_argument("--interval", type=float, help="seconds between survey starts")
    survey.add_argument("--duration", type=float, help="stop starting cycles after this many seconds")
    survey.add_argument("--max-cycles", type=int, default=24, help="cap on cycles (default 24)")
    survey.add_argument("--out-dir", help="directory for survey-<stamp>-<cycle>.json when watching")
    survey.add_argument("--no-board", action="store_true", help="do not probe the NYP-WAS corridor board")
    survey.add_argument("--board-origin", type=station_code, default="NYP")
    survey.add_argument("--board-destination", type=station_code, default="WAS")
    survey.add_argument("--board-date", type=service_date, help="board departure-date; default is today in New York")

    merge = sub.add_parser("merge", help="fold survey snapshots into one coverage report")
    merge.add_argument("inputs", nargs="+", help="snapshot JSON files or a directory of them")
    merge.add_argument("--out", required=True, help="merged JSON path")

    args = parser.parse_args(list(argv) if argv is not None else None)
    _validate(parser, args)
    return args


def main(argv: Sequence[str] | None = None) -> int:
    """Run tracks, survey, watch, or merge. See the module docstring for examples."""
    args = parse_args(argv)
    try:
        if args.command == "merge":
            return merge_command(args)
        if args.command == "survey" and args.interval is not None:
            return watch_command(args)
        return run_command(args)
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return EXIT_BLOCKED


def run_command(args: argparse.Namespace, *, out_path: Path | None = None) -> int:
    """One tracks or survey invocation."""
    trains_url, page_url = _endpoint_urls()
    plan = _resolve(args, trains_url)
    if plan.fleet_error:
        print(plan.fleet_error, file=sys.stderr)
        return EXIT_BLOCKED
    for number in plan.missing_trains:
        print(
            f"train {number} is not in the maps feed; pass --date to query it anyway",
            file=sys.stderr,
        )
    stations = set(args.station or [])
    board = _board_spec(args)
    target = out_path if out_path is not None else (Path(args.out) if args.out else None)
    return _execute(
        args,
        plan,
        stations=stations,
        board=board,
        page_url=page_url,
        out_path=target,
        kind=args.command,
    )


def watch_command(args: argparse.Namespace) -> int:
    """Repeat survey until ``--duration`` elapses, then merge the snapshots."""
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + args.duration
    written: list[Path] = []
    worst = EXIT_OK
    for cycle in range(1, args.max_cycles + 1):
        if time.monotonic() >= deadline:
            break
        started = time.monotonic()
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        path = out_dir / f"survey-{stamp}-{cycle:02d}.json"
        print(f"cycle {cycle} -> {path}", file=sys.stderr)
        code = run_command(args, out_path=path)
        written.append(path)
        worst = max(worst, code)
        if code == EXIT_BLOCKED:
            break
        pause = next_pause(
            elapsed=time.monotonic() - started,
            interval=args.interval,
            time_left=deadline - time.monotonic(),
        )
        if pause is None:
            break
        time.sleep(pause)
    existing = [(path, True) for path in written if path.is_file()]
    if existing:
        merged = out_dir / "merged.json"
        code = _merge_paths(existing, merged)
        worst = max(worst, code)
        print(f"merged {len(existing)} snapshots -> {merged}", file=sys.stderr)
    return worst


def merge_command(args: argparse.Namespace) -> int:
    """Aggregate snapshot files. Explicit paths may include an earlier merged report."""
    paths = _snapshot_paths(args.inputs)
    if not paths:
        print("no snapshot JSON found", file=sys.stderr)
        return EXIT_EMPTY
    return _merge_paths(paths, Path(args.out))


def _execute(
    args: argparse.Namespace,
    plan: JobPlan,
    *,
    stations: set[str],
    board: dict | None,
    page_url: str,
    out_path: Path | None,
    kind: str,
) -> int:
    started = _utc_now()
    if plan.jobs or board is not None:
        fetched = asyncio.run(
            fetch_jobs(
                plan.jobs,
                board=board,
                delay=args.delay,
                timeout=args.timeout,
                nav_timeout=args.nav_timeout,
                page_url=page_url,
            )
        )
    else:
        fetched = _empty_fetch()
    stops = filter_station_rows(fetched["stops"], stations or None)
    payload = build_payload(
        stops,
        kind=kind,
        fetched_at=started,
        delay_secs=args.delay,
        max_requests=args.max_requests,
        offset=args.offset,
        truncated=bool(plan.not_fetched),
        matched_trains=plan.matched,
        station_filter=sorted(stations),
        missing_trains=plan.missing_trains,
        not_fetched=_pair_dicts(plan.not_fetched),
        bus_only_skipped=plan.bus_only,
        bus_services_skipped=fetched["bus_services"],
        status_documents=fetched["documents"],
        aborted=fetched["aborted"],
        sources=[started],
        input_files=[],
        inputs_aborted=0,
        requests=fetched["requests"],
        station_board=fetched["board"],
        snapshot_count=1,
    )
    _emit(payload, out_path, tracks=kind == "tracks")
    if fetched["aborted"] is not None:
        return EXIT_BLOCKED
    board_ok = bool(fetched["board"].get("probed") and fetched["board"].get("http_status") == 200)
    if fetched["documents"] == 0 and not board_ok:
        return EXIT_EMPTY
    return EXIT_OK


def _emit(payload: dict, out_path: Path | None, *, tracks: bool) -> None:
    if tracks:
        print(format_stop_table(payload["stops"]), flush=True)
        print(flush=True)
    print(format_station_table(payload["stations"]), flush=True)
    print(
        "Ahead is event time minus track.asOf for the earlier of arrival and departure. "
        "The Track column is the side the track-fetcher would publish (departure, else arrival).",
        file=sys.stderr,
    )
    print(
        f"{payload['status_documents']} status documents, {len(payload['requests'])} "
        f"www.amtrak.com requests, {len(payload['stops'])} stops, "
        f"{sum(1 for row in payload['stations'] if row['stops_with_track'])} stations with a track",
        file=sys.stderr,
    )
    if payload["truncated"]:
        print(
            f"truncated: {payload['matched_trains']} trains matched, "
            f"{len(payload['not_fetched'])} left for a later --offset",
            file=sys.stderr,
        )
    if out_path is not None:
        write_report(out_path, payload)
        print(
            f"wrote {out_path}, {out_path.with_suffix('.csv')}, "
            f"{out_path.with_suffix('.stations.csv')}, {out_path.with_suffix('.profile.json')}",
            file=sys.stderr,
        )
    aborted = payload.get("aborted")
    if isinstance(aborted, dict) and aborted.get("detail"):
        print(aborted["detail"], file=sys.stderr)


async def fetch_jobs(
    jobs: Sequence[tuple[str, str]],
    *,
    board: dict | None,
    delay: float,
    timeout: float,
    nav_timeout: float,
    page_url: str,
) -> dict:
    """Fetch the board (if asked) and each train-day through one page, in order.

    A 403, 401, 429, reset, or browser failure stops the queue. Other per-train misses are
    recorded and the next train is still requested, after ``delay`` seconds.
    """
    queue: list[tuple[str, str, str, str]] = []
    if board is not None:
        queue.append(
            (
                "station_board",
                station_board_url(board["origin"], board["destination"], board["departure_date"]),
                "",
                board["departure_date"],
            )
        )
    for train, day in jobs:
        queue.append(("status", status_url(train, day), train, day))
    result = _empty_fetch()
    if board is not None:
        result["board"] = {
            **unprobed_board(),
            "probed": True,
            "origin": board["origin"],
            "destination": board["destination"],
            "departure_date": board["departure_date"],
            "url": queue[0][1],
        }
    if not queue:
        return result
    try:
        from playwright.async_api import async_playwright  # lazy: tests never import it
    except ImportError:
        result["aborted"] = _abort("playwright is not installed")
        return result

    browser = None
    try:
        playwright = await async_playwright().start()
    except Exception as error:  # noqa: BLE001 — the driver failed to start
        result["aborted"] = _abort(type(error).__name__)
        return result
    try:
        browser = await playwright.chromium.launch(headless=True, args=_LAUNCH_ARGS)
        context = await browser.new_context()

        async def _route(route):
            request = route.request
            if should_block(request.resource_type) or not is_first_party(request.url):
                await route.abort()
            else:
                await route.continue_()

        await context.route("**/*", _route)
        page = await context.new_page()
        try:
            await page.goto(page_url, wait_until="domcontentloaded", timeout=nav_timeout * 1000)
        except Exception as error:  # noqa: BLE001 — launch and navigation failures stop the run
            result["aborted"] = _abort(type(error).__name__)
            return result
        for index, (kind, url, train, day) in enumerate(queue):
            if index:
                await asyncio.sleep(delay)
            started = _utc_now()
            try:
                raw = await asyncio.wait_for(
                    page.evaluate(_FETCH_JS, [url, timeout * 1000]),
                    timeout + 5,
                )
            except Exception as error:  # noqa: BLE001 — a dead page or hung request
                fetched_at = _utc_now()
                interpreted = {
                    "http_status": None,
                    "document": None,
                    "error": type(error).__name__,
                    "fatal": True,
                }
                result["requests"].append(
                    _request_row(index + 1, kind, train, day, url, started, fetched_at, interpreted)
                )
                result["aborted"] = _abort(type(error).__name__)
                break
            fetched_at = _utc_now()
            interpreted = read_fetch_result(raw)
            result["requests"].append(
                _request_row(index + 1, kind, train, day, url, started, fetched_at, interpreted)
            )
            if kind == "station_board":
                _fill_board(result["board"], interpreted)
            elif interpreted["document"] is not None:
                rows, buses = stops_from_document(
                    interpreted["document"],
                    train=train,
                    service_date=day,
                    fetched_at=fetched_at,
                )
                result["stops"].extend(rows)
                result["bus_services"] += buses
                result["documents"] += 1
            if interpreted["fatal"]:
                result["aborted"] = _abort(interpreted["error"] or "no response")
                break
    except Exception as error:  # noqa: BLE001 — launch failed before a page existed
        if result["aborted"] is None:
            result["aborted"] = _abort(type(error).__name__)
    finally:
        if browser is not None:
            try:
                await browser.close()
            except Exception:  # noqa: BLE001 — closing a crashed browser is best-effort
                pass
        try:
            await playwright.stop()
        except Exception:  # noqa: BLE001
            pass
    return result


def _fill_board(board: dict, interpreted: dict) -> None:
    board["http_status"] = interpreted["http_status"]
    board["error"] = interpreted["error"]
    document = interpreted["document"]
    if document is None:
        return
    counts = board_field_counts(document)
    board.update(counts)


def _resolve(args: argparse.Namespace, trains_url: str) -> JobPlan:
    stations = set(args.station or [])
    if args.command == "tracks" and args.dates:
        chosen, skipped, matched = jobs_from_dates(
            args.trains, args.dates, offset=args.offset, limit=args.max_requests
        )
        return JobPlan(chosen, skipped, matched, 0, [], None)
    try:
        timeout = max(1, int(args.nav_timeout))
        geojson = load_trains(Config(trains_url=trains_url, nav_timeout_secs=timeout))
    except (OSError, URLError, json.JSONDecodeError, DecryptError, TimeoutError, ValueError) as error:
        return JobPlan([], [], 0, 0, [], fleet_error_message(error))
    fleet, bus_only = list_fleet(geojson)
    only = set(args.trains) if args.command == "tracks" else None
    board_cost = 0
    if args.command == "survey" and not args.no_board:
        board_cost = 1
    limit = max(0, args.max_requests - board_cost)
    chosen, skipped, matched = jobs_from_fleet(
        fleet,
        trains=only,
        stations=stations or None,
        offset=args.offset,
        limit=limit,
    )
    missing: list[str] = []
    if only is not None:
        present = {train.train_number for train in fleet}
        missing = sorted(only - present, key=lambda number: (int(number), number))
    return JobPlan(chosen, skipped, matched, bus_only, missing, None)


def _board_spec(args: argparse.Namespace) -> dict | None:
    if args.command != "survey" or args.no_board:
        return None
    return {
        "origin": args.board_origin,
        "destination": args.board_destination,
        "departure_date": args.board_date or new_york_today(),
    }


def _merge_paths(paths: Sequence[tuple[Path, bool]], out: Path) -> int:
    loaded: list[tuple[Path, dict]] = []
    for path, direct in paths:
        payload = _read_snapshot(path, direct=direct)
        if payload is not None:
            loaded.append((path, payload))
    if not loaded:
        print("no snapshot JSON with stops found", file=sys.stderr)
        return EXIT_EMPTY
    groups = [payload["stops"] for _, payload in loaded]
    merged = merge_stop_rows(groups)
    sources = sorted({str(payload.get("fetched_at") or "") for _, payload in loaded if payload.get("fetched_at")})
    generated = sources[-1] if sources else _utc_now()
    filters = [tuple(payload.get("station_filter") or []) for _, payload in loaded]
    station_filter = list(filters[0]) if filters and all(item == filters[0] for item in filters) else []
    payload = build_payload(
        merged,
        kind="merged",
        fetched_at=generated,
        delay_secs=None,
        max_requests=None,
        offset=0,
        truncated=any(bool(item.get("truncated")) for _, item in loaded),
        matched_trains=sum(int(item.get("matched_trains") or 0) for _, item in loaded),
        station_filter=station_filter,
        missing_trains=[],
        not_fetched=[],
        bus_only_skipped=sum(int(item.get("bus_only_skipped") or 0) for _, item in loaded),
        bus_services_skipped=sum(int(item.get("bus_services_skipped") or 0) for _, item in loaded),
        status_documents=sum(int(item.get("status_documents") or 0) for _, item in loaded),
        aborted=None,
        sources=sources,
        input_files=sorted(path.name for path, _ in loaded),
        inputs_aborted=sum(1 for _, item in loaded if item.get("aborted")),
        requests=[],
        station_board=_merged_board(loaded),
        snapshot_count=len(loaded),
    )
    _emit(payload, out, tracks=False)
    return EXIT_OK


def _merged_board(loaded: Sequence[tuple[Path, dict]]) -> dict:
    """The newest probed board, if any snapshot called it."""
    probed = [
        (str(payload.get("fetched_at") or ""), payload.get("station_board"))
        for _, payload in loaded
        if isinstance(payload.get("station_board"), dict) and payload["station_board"].get("probed")
    ]
    if not probed:
        return unprobed_board()
    probed.sort(key=lambda item: item[0])
    board = dict(probed[-1][1])
    board["note"] = BOARD_NOTE
    return board


def _snapshot_paths(inputs: Sequence[str]) -> list[tuple[Path, bool]]:
    """``(path, direct)``. Directory scans are not direct, so an older merged file is skipped."""
    found: list[tuple[Path, bool]] = []
    seen: set[Path] = set()
    for raw in inputs:
        path = Path(raw)
        if path.is_dir():
            children = sorted(
                child for child in path.glob("*.json") if not child.name.endswith(".profile.json")
            )
            for child in children:
                _add_path(found, seen, child, direct=False)
        else:
            _add_path(found, seen, path, direct=True)
    return found


def _add_path(found: list[tuple[Path, bool]], seen: set[Path], path: Path, *, direct: bool) -> None:
    try:
        key = path.resolve()
    except OSError:
        key = path
    if key in seen:
        return
    seen.add(key)
    found.append((path, direct))


def _read_snapshot(path: Path, *, direct: bool) -> dict | None:
    """A snapshot dict, or ``None`` when the file is not one.

    ``direct`` is true for paths the user named (a previous merged report counts) and false for
    files discovered in a directory (``kind: merged`` and profiles are skipped).
    """
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        print(f"{path}: {error}", file=sys.stderr)
        return None
    if not isinstance(payload, dict) or not isinstance(payload.get("stops"), list):
        print(f"{path}: not a coverage snapshot", file=sys.stderr)
        return None
    if not direct and payload.get("kind") == "merged":
        return None
    return payload


def _endpoint_urls() -> tuple[str, str]:
    return (
        os.environ.get("TRAINS_URL", DEFAULT_TRAINS_URL),
        os.environ.get("AMTRAK_PAGE_URL", DEFAULT_PAGE_URL),
    )


def _empty_fetch() -> dict:
    return {
        "requests": [],
        "stops": [],
        "bus_services": 0,
        "documents": 0,
        "aborted": None,
        "board": unprobed_board(),
    }


def _abort(reason: str) -> dict:
    return {"reason": reason, "detail": abort_message(reason)}


def _request_row(seq, kind, train, day, url, started, fetched_at, interpreted) -> dict:
    return {
        "seq": seq,
        "kind": kind,
        "train": train,
        "service_date": day,
        "url": url,
        "started_at": started,
        "fetched_at": fetched_at,
        "http_status": interpreted["http_status"],
        "error": interpreted["error"],
    }


def _pair_dicts(pairs: Sequence[tuple[str, str]]) -> list[dict]:
    return [{"service_date": day, "train": train} for train, day in pairs]


def _utc_now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _percent_cell(value: float | None) -> str:
    if value is None:
        return ""
    return f"{value:.1f}%"


def _side_cell(track: str, gate: str) -> str:
    if track and gate:
        return f"{track}/{gate}"
    return track or gate or ""


def _aligned(header: list[str], rows: list[list[str]]) -> str:
    widths = [len(cell) for cell in header]
    for row in rows:
        for index, cell in enumerate(row):
            widths[index] = max(widths[index], len(cell))

    def line(cells: list[str]) -> str:
        return "  ".join(cell.ljust(widths[index]) for index, cell in enumerate(cells)).rstrip()

    return "\n".join([line(header), *(line(row) for row in rows)])


def _write_csv(path: Path, fields: Sequence[str], rows: Sequence[dict]) -> None:
    with path.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=list(fields), lineterminator="\n", extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow({key: _csv_cell(row.get(key)) for key in fields})


def _csv_cell(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, list):
        return "|".join(str(item) for item in value)
    return str(value)


def _add_fetch_args(parser: argparse.ArgumentParser, *, default_max: int) -> None:
    parser.add_argument("--delay", type=float, default=DEFAULT_DELAY_SECS, help="seconds between www.amtrak.com requests (default 2, minimum 1)")
    parser.add_argument("--max-requests", type=int, default=default_max, help=f"cap on www.amtrak.com requests this run (default {default_max})")
    parser.add_argument("--timeout", type=float, default=20.0, help="seconds per request (default 20)")
    parser.add_argument("--nav-timeout", type=float, default=45.0, help="seconds to open the status page and the maps feed (default 45)")
    parser.add_argument("--offset", type=int, default=0, help="skip this many trains after sorting, before the cap")
    parser.add_argument("--station", action="append", type=station_code, default=None, help="keep these station codes; repeat to list several")
    parser.add_argument("--out", help="JSON path; CSV, station CSV, and the profile are written beside it")


def _validate(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    if args.command == "merge":
        return
    _range(parser, "--delay", args.delay, MIN_DELAY_SECS, MAX_DELAY_SECS)
    _range(parser, "--max-requests", args.max_requests, 1, MAX_REQUESTS_LIMIT)
    _range(parser, "--timeout", args.timeout, 0.1, 120)
    _range(parser, "--nav-timeout", args.nav_timeout, 1, 180)
    if args.offset < 0:
        parser.error("--offset must be >= 0")
    if args.command != "survey":
        return
    if args.interval is None and args.duration is not None:
        parser.error("--duration requires --interval")
    if args.interval is not None:
        if args.duration is None:
            parser.error("--interval requires --duration")
        if not args.out_dir:
            parser.error("--interval requires --out-dir")
        if args.out:
            parser.error("use --out-dir with --interval, not --out")
        _range(parser, "--interval", args.interval, MIN_INTERVAL_SECS, MAX_BOX_SECS)
        _range(parser, "--duration", args.duration, MIN_INTERVAL_SECS, MAX_BOX_SECS)
        _range(parser, "--max-cycles", args.max_cycles, 1, MAX_CYCLES)
    elif not args.out:
        parser.error("survey requires --out")


def _range(parser: argparse.ArgumentParser, name: str, value: float, low: float, high: float) -> None:
    if value < low or value > high:
        parser.error(f"{name} must be between {low} and {high}")


if __name__ == "__main__":
    sys.exit(main())
