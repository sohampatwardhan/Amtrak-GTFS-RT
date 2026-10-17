"""In-memory status documents served to the feed producer.

A document is kept until ``document_ttl_secs`` after it was fetched. The Rust service keeps a
track it has already seen until the train departs, so dropping an old document here does not
clear a published track.
"""

from __future__ import annotations

import threading


class TrackStore:
    """Latest successful status document per train-day, plus the last completed poll."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._documents: dict[tuple[str, str], tuple[float, dict]] = {}
        self._last_success: float | None = None

    def put(self, train_number: str, service_date: str, document: dict, now: float) -> None:
        """Store one status document. A later document for the same train-day replaces it."""
        with self._lock:
            self._documents[(train_number, service_date)] = (now, document)

    def mark_success(self, now: float) -> None:
        """Record that a poll finished, including a poll that found nothing due."""
        with self._lock:
            self._last_success = now

    def snapshot(self, now: float, ttl_secs: float) -> tuple[float, list[dict]] | None:
        """``(last_success, documents)`` with documents older than ``ttl_secs`` removed.

        ``None`` means no poll has completed yet.
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
                    fresh.append(document)
            for key in expired:
                del self._documents[key]
            return self._last_success, fresh
