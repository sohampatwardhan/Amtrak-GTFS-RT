"""Load an ``amtrak-station-lead-time/v1`` profile.

Stations that are missing, ``sparse``, ``no_track``, or ``none`` keep the global
window. ``covered`` stations are not narrowed. An ``observed`` station whose
``compared_to_default`` is ``wider`` uses the suggested lookahead and hot window.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path


SCHEMA = "amtrak-station-lead-time/v1"


@dataclass(frozen=True)
class StationWindow:
    """Lookahead and hot window, in seconds, for one station."""

    window_before_secs: int
    hot_window_secs: int


@dataclass(frozen=True)
class LeadProfile:
    """Adopted stations. Everyone else uses the globals passed to :meth:`window_for`."""

    stations: dict[str, StationWindow]

    @staticmethod
    def load(path: str | None, *, default_before: int, default_hot: int) -> "LeadProfile":
        """Read ``path``. A missing path or a bad file yields an empty profile."""
        if not path:
            return LeadProfile({})
        try:
            body = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return LeadProfile({})
        if not isinstance(body, dict) or body.get("schema") != SCHEMA:
            return LeadProfile({})
        stations: dict[str, StationWindow] = {}
        for row in body.get("stations") or []:
            window = _adopt(row, default_hot)
            if window is None:
                continue
            code, adopted = window
            stations[code] = adopted
        return LeadProfile(stations)

    def before_secs(self, station: str | None, default: int) -> int:
        """Lookahead for ``station``."""
        window = self._window(station)
        return default if window is None else window.window_before_secs

    def hot_secs(self, station: str | None, default: int) -> int:
        """Hot window for ``station``."""
        window = self._window(station)
        return default if window is None else window.hot_window_secs

    def _window(self, station: str | None) -> StationWindow | None:
        if not station:
            return None
        return self.stations.get(station.upper())


def _adopt(row: object, default_hot: int) -> tuple[str, StationWindow] | None:
    if not isinstance(row, dict):
        return None
    code = row.get("station")
    if not isinstance(code, str) or not code.strip():
        return None
    confidence = row.get("confidence")
    compared = row.get("compared_to_default")
    if confidence != "observed" or compared != "wider":
        return None
    before = row.get("suggested_window_before_secs")
    if not isinstance(before, int) or isinstance(before, bool) or before <= 0:
        return None
    hot = row.get("suggested_hot_window_secs")
    if not isinstance(hot, int) or isinstance(hot, bool) or hot <= 0:
        hot = default_hot
    return code.strip().upper(), StationWindow(before, hot)
