"""One-shot dump of the tracks Amtrak has posted for given trains.

    python -m fetcher.dump 66 67 2150 --date 2026-10-19

``www.amtrak.com`` resets plain HTTP clients, so this opens the train-status page once in headless
Chromium, the way the sidecar does, and then fetches each train-day's status document from inside
that page: one request at a time, with a pause between them, and nothing else. Images, fonts,
stylesheets, media, and every host outside ``amtrak.com`` are blocked. Unlike the sidecar it does
not poll, keep a browser, or serve anything; it prints every posted track and exits.

A service date is the day the train left its origin, so an overnight train still running after
midnight needs yesterday's date as well. Without ``--date`` the current date in New York is used.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
import sys
from collections.abc import Iterable, Sequence
from datetime import date, datetime
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from .config import DEFAULT_PAGE_URL
from .poller import _FETCH_JS, _LAUNCH_ARGS, StatusMiss, document_from_eval, is_first_party
from .poller import should_block, status_url

MAX_REQUESTS = 40
"""Status requests one run may make unless ``--max-requests`` raises it."""

_TRAIN = re.compile(r"[0-9]{1,4}")
_SIDES = ("arrival", "departure")
_COLUMNS = (
    ("train", "Train"),
    ("service_date", "Service date"),
    ("station", "Station"),
    ("side", "Side"),
    ("track", "Track"),
    ("gate", "Gate"),
    ("scheduled", "Scheduled"),
    ("as_of", "As of"),
    ("station_name", "Station name"),
)


def posted_tracks(train: str, service_date: str, document: dict) -> list[dict]:
    """Every arrival or departure in ``document`` with a posted track, in stop order.

    A track Amtrak has not assigned yet (an empty number or ``TBD``) is left out.
    """
    rows = []
    for service in document.get("data") or []:
        if not isinstance(service, dict):
            continue
        for stop in service.get("stops") or []:
            if not isinstance(stop, dict):
                continue
            station = stop.get("station") if isinstance(stop.get("station"), dict) else {}
            code = station.get("code") or stop.get("code") or ""
            for side in _SIDES:
                event = stop.get(side)
                if not isinstance(event, dict):
                    continue
                track = event.get("track")
                if not isinstance(track, dict):
                    continue
                number = str(track.get("number") or "").strip()
                if not number or number.upper() == "TBD":
                    continue
                gate = event.get("gate") or track.get("gate")
                schedule = event.get("schedule")
                rows.append(
                    {
                        "train": train,
                        "service_date": service_date,
                        "station": code,
                        "station_name": station.get("name") or "",
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


def format_table(rows: Sequence[dict]) -> str:
    """``rows`` as an aligned plain-text table, or a note when there are none."""
    if not rows:
        return "No posted tracks."
    widths = {
        key: max(len(title), *(len(str(row[key])) for row in rows)) for key, title in _COLUMNS
    }
    lines = ["  ".join(title.ljust(widths[key]) for key, title in _COLUMNS).rstrip()]
    for row in rows:
        lines.append("  ".join(str(row[key]).ljust(widths[key]) for key, _ in _COLUMNS).rstrip())
    return "\n".join(lines)


def train_number(raw: str) -> str:
    """A train number as Amtrak's status URL takes it: one to four digits, no leading zeros."""
    if not _TRAIN.fullmatch(raw) or int(raw) == 0:
        raise argparse.ArgumentTypeError(f"not a train number: {raw!r}")
    return str(int(raw))


def service_date(raw: str) -> str:
    """A ``YYYY-MM-DD`` service date."""
    try:
        return date.fromisoformat(raw).isoformat()
    except ValueError:
        raise argparse.ArgumentTypeError(f"not a YYYY-MM-DD date: {raw!r}") from None


def parse_args(argv: Sequence[str] | None, today: str) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="python -m fetcher.dump",
        description="Print every track Amtrak has posted for the given trains.",
    )
    parser.add_argument("trains", nargs="+", type=train_number, help="train numbers, e.g. 66 2150")
    parser.add_argument(
        "--date",
        dest="dates",
        action="append",
        type=service_date,
        help=f"service date (YYYY-MM-DD); repeat for several; default {today} (New York)",
    )
    parser.add_argument(
        "--delay",
        type=float,
        default=1.5,
        help="seconds to wait between status requests (default 1.5)",
    )
    parser.add_argument(
        "--timeout", type=float, default=20.0, help="seconds per request (default 20)"
    )
    parser.add_argument(
        "--max-requests",
        type=int,
        default=MAX_REQUESTS,
        help=f"refuse to run more status requests than this (default {MAX_REQUESTS})",
    )
    parser.add_argument(
        "--json", action="store_true", help="print one JSON object per track instead of a table"
    )
    args = parser.parse_args(argv)
    args.dates = args.dates or [today]
    if args.delay < 0 or args.timeout <= 0:
        parser.error("--delay must be >= 0 and --timeout > 0")
    count = len(request_pairs(args.trains, args.dates))
    if count > args.max_requests:
        parser.error(f"{count} train-days exceed --max-requests {args.max_requests}")
    return args


def request_pairs(trains: Iterable[str], dates: Iterable[str]) -> list[tuple[str, str]]:
    """Each (train, service date) once, in the order given."""
    seen: dict[tuple[str, str], None] = {}
    for day in dates:
        for train in trains:
            seen.setdefault((train, day), None)
    return list(seen)


async def fetch_documents(
    pairs: Sequence[tuple[str, str]], *, delay: float, timeout: float
) -> list[tuple[str, str, dict | None, str | None]]:
    """``(train, date, document, error)`` for each pair, all through one page load."""
    from playwright.async_api import async_playwright  # lazy: tests never import it

    results: list[tuple[str, str, dict | None, str | None]] = []
    async with async_playwright() as playwright:
        browser = await playwright.chromium.launch(headless=True, args=_LAUNCH_ARGS)
        try:
            context = await browser.new_context()

            async def _route(route):
                request = route.request
                if should_block(request.resource_type) or not is_first_party(request.url):
                    await route.abort()
                else:
                    await route.continue_()

            await context.route("**/*", _route)
            page = await context.new_page()
            # domcontentloaded lets Amtrak's first-party scripts set their cookies first.
            await page.goto(
                DEFAULT_PAGE_URL, wait_until="domcontentloaded", timeout=timeout * 1000
            )
            for index, (train, day) in enumerate(pairs):
                if index:
                    await asyncio.sleep(delay)
                try:
                    raw = await asyncio.wait_for(
                        page.evaluate(_FETCH_JS, [status_url(train, day), timeout * 1000]),
                        timeout + 5,
                    )
                    results.append((train, day, document_from_eval(raw), None))
                except StatusMiss as miss:
                    results.append((train, day, None, str(miss)))
                except Exception as error:  # noqa: BLE001 — report and keep going
                    results.append((train, day, None, type(error).__name__))
        finally:
            await browser.close()
    return results


def new_york_today() -> str:
    """Today's date in New York, or the local date where no time-zone database is installed."""
    try:
        return datetime.now(ZoneInfo("America/New_York")).date().isoformat()
    except ZoneInfoNotFoundError:
        return date.today().isoformat()


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv, new_york_today())
    pairs = request_pairs(args.trains, args.dates)
    results = asyncio.run(fetch_documents(pairs, delay=args.delay, timeout=args.timeout))
    rows = []
    for train, day, document, error in results:
        if error is not None:
            print(f"train {train} on {day}: {error}", file=sys.stderr)
            continue
        rows.extend(posted_tracks(train, day, document))
    if args.json:
        for row in rows:
            print(json.dumps(row))
    else:
        print(format_table(rows))
    fetched = sum(document is not None for _, _, document, _ in results)
    print(f"{fetched}/{len(results)} status documents fetched, {len(rows)} tracks", file=sys.stderr)
    return 0 if fetched else 1


if __name__ == "__main__":
    sys.exit(main())
