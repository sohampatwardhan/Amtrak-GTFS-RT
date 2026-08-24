"""Atomic, churn-free on-disk snapshot store for the advisories HTML.

The fetcher writes the latest good advisories HTML here and the HTTP server reads it. Two
properties matter (Requirements 2.2, 2.3, 8.2):

* **Atomic writes** — a reader (the HTTP server) never observes a half-written file, because a new
  snapshot is written to a temp file and `os.replace`d into place (an atomic rename on the same
  filesystem).
* **No churn** — when a freshly fetched page is byte-identical to the current snapshot, the payload
  is not rewritten; only the freshness marker (the file mtime) is bumped. This avoids needless I/O
  and inode churn every poll when advisories rarely change.

Freshness is the snapshot file's mtime, so it survives a fetcher restart with no extra state.
"""

from __future__ import annotations

import os
import time
from pathlib import Path

SNAPSHOT_NAME = "advisories.html"


class SnapshotStore:
    """Persist and read the latest advisories HTML with atomic, churn-free updates."""

    def __init__(self, snapshot_dir: str | os.PathLike[str]) -> None:
        """Create the store rooted at ``snapshot_dir`` (created if absent)."""
        self._dir = Path(snapshot_dir)
        self._path = self._dir / SNAPSHOT_NAME
        self._dir.mkdir(parents=True, exist_ok=True)

    @property
    def path(self) -> Path:
        """Absolute path of the snapshot file (may not exist yet)."""
        return self._path

    def update(self, html: str) -> bool:
        """Store ``html`` as the current snapshot; return whether the payload changed.

        If ``html`` is identical to the existing snapshot, the file is left in place and only its
        mtime is refreshed (no rewrite), returning ``False``. Otherwise the new content is written
        atomically (temp file + ``os.replace``) and ``True`` is returned. The write is atomic so a
        concurrent reader sees either the whole old file or the whole new file, never a partial one.
        """
        data = html.encode("utf-8")
        if self._path.exists() and self._path.read_bytes() == data:
            now = time.time()
            os.utime(self._path, (now, now))  # refresh freshness marker without rewriting payload
            return False
        tmp = self._path.with_name(f"{SNAPSHOT_NAME}.{os.getpid()}.tmp")
        tmp.write_bytes(data)
        os.replace(tmp, self._path)  # atomic on the same filesystem
        return True

    def read(self) -> tuple[bytes, float] | None:
        """Return ``(snapshot_bytes, last_success_epoch)`` or ``None`` if no snapshot exists yet."""
        try:
            data = self._path.read_bytes()
        except FileNotFoundError:
            return None
        return data, self._path.stat().st_mtime
