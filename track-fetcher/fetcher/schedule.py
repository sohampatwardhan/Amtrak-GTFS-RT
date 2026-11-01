"""Debounce and cap early track refreshes.

The rules match :mod:`amtrak_gtfs_rt_service::schedule` in the Rust service:
a burst for one train collapses to a single refresh, two refreshes of that
train stay at least ``min_gap_secs`` apart, and one flush returns at most
``max_per_flush`` trains.
"""

from __future__ import annotations


def take_early(
    touches: dict[str, float],
    refreshed: dict[str, float],
    now: float,
    *,
    debounce_secs: float,
    min_gap_secs: float,
    max_per_flush: int,
) -> list[str]:
    """Trains whose last touch is at least ``debounce_secs`` old.

    ``touches`` maps a train to the latest request time and is updated: released
    trains are removed and the rest stay. ``refreshed`` records when a train was
    last released. A train still inside ``min_gap_secs`` stays in ``touches``.
    """
    if max_per_flush <= 0:
        return []
    ready = [
        train
        for train, touched in touches.items()
        if now - touched >= debounce_secs
        and now - refreshed.get(train, float("-inf")) >= min_gap_secs
    ]
    ready.sort()
    due = ready[:max_per_flush]
    for train in due:
        touches.pop(train, None)
        refreshed[train] = now
    return due
