"""In-memory status documents served to the feed producer.

A document is kept until ``document_ttl_secs`` after it was fetched. The Rust service keeps a
track it has already seen until the train departs, so dropping an old document here does not
clear a published track. Each served document carries ``fetchedAt``, the time it was read from
Amtrak, so the service can tell a fresh document from one this store keeps re-serving.
"""

from __future__ import annotations

import threading
import time
from datetime import datetime, timezone


def iso_utc(timestamp: float) -> str:
    """``YYYY-MM-DDTHH:MM:SSZ`` for a Unix timestamp."""
    return datetime.fromtimestamp(timestamp, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def tracked_codes(document: dict) -> frozenset[str]:
    """Station codes that already show a departure or arrival track in ``document``."""
    codes: set[str] = set()
    trains = document.get("data")
    if not isinstance(trains, list):
        return frozenset()
    for train in trains:
        stops = train.get("stops") if isinstance(train, dict) else None
        for stop in stops if isinstance(stops, list) else []:
            if not isinstance(stop, dict):
                continue
            station = stop.get("station")
            code = station.get("code") if isinstance(station, dict) else None
            if not isinstance(code, str) or not code.strip():
                continue
            for side in ("departure", "arrival"):
                event = stop.get(side)
                track = event.get("track") if isinstance(event, dict) else None
                number = track.get("number") if isinstance(track, dict) else None
                if isinstance(number, (str, int)) and str(number).strip():
                    codes.add(code.strip().upper())
                    break
    return frozenset(codes)


class TrackStore:
    """Latest successful status document per train-day, plus poll bookkeeping.

    ``last_success`` is the last poll that had nothing to fetch or stored a document; it decides
    whether ``/tracks.json`` is current. ``last_cycle`` is the last poll that finished at all; it
    decides whether ``/healthz`` reports the loop alive. ``consecutive_failures`` counts polls
    that finished without succeeding, so a loop that keeps running but never reads Amtrak can
    still fail the healthcheck.
    """

    def __init__(self, started_at: float | None = None) -> None:
        self._lock = threading.Lock()
        self._documents: dict[tuple[str, str], tuple[float, dict]] = {}
        self._tracked: dict[tuple[str, str], frozenset[str]] = {}
        self._last_success: float | None = None
        self._last_cycle: float = time.time() if started_at is None else started_at
        self._consecutive_failures = 0

    def put(self, train_number: str, service_date: str, document: dict, now: float) -> None:
        """Store one status document. A later document for the same train-day replaces it."""
        key = (train_number, service_date)
        codes = tracked_codes(document)
        with self._lock:
            self._documents[key] = (now, document)
            self._tracked[key] = codes

    def mark_success(self, now: float) -> None:
        """Record a poll that had nothing to fetch or stored at least one document."""
        with self._lock:
            self._last_success = now

    def mark_cycle(self, now: float) -> None:
        """Record that a poll finished, whatever its outcome."""
        with self._lock:
            self._last_cycle = now

    def record_poll(self, succeeded: bool) -> None:
        """Count a finished poll. A success clears the failure streak."""
        with self._lock:
            self._consecutive_failures = 0 if succeeded else self._consecutive_failures + 1

    def consecutive_failures(self) -> int:
        """Polls in a row that finished without succeeding."""
        with self._lock:
            return self._consecutive_failures

    def last_cycle(self) -> float:
        """When the last poll finished, or when the store was created."""
        with self._lock:
            return self._last_cycle

    def tracked(self) -> dict[tuple[str, str], frozenset[str]]:
        """Station codes with a track, per train-day, from each train's latest document."""
        with self._lock:
            return dict(self._tracked)

    def snapshot(self, now: float, ttl_secs: float) -> tuple[float, list[dict]] | None:
        """``(last_success, documents)`` with documents older than ``ttl_secs`` removed.

        Each document is a shallow copy with ``fetchedAt`` added. ``None`` means no poll has
        succeeded yet.
        """
        with self._lock:
            if self._last_success is None:
                return None
            fresh: list[dict] = []
            expired: list[tuple[str, str]] = []
            for key, (fetched_at, document) in self._documents.items():
                if now - fetched_at > ttl_secs:
                    expired.append(key)
                else:
                    fresh.append({**document, "fetchedAt": iso_utc(fetched_at)})
            for key in expired:
                del self._documents[key]
                self._tracked.pop(key, None)
            return self._last_success, fresh
