"""Runtime configuration for the track source of the browser worker.

Every value comes from the environment and has a default, so a bare ``python -m worker`` polls
Amtrak without further setup. The listen address and the on/off flags belong to the worker as a
whole (:mod:`worker.config`).

The defaults follow the Amtrak app's behavior on 2026-10-17: train 167's Providence track appeared
about ten minutes before arrival, so the poll looks about 30 minutes ahead and 10 minutes behind,
and it does not fetch the whole active fleet every minute.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Mapping

from ..env import BROWSER_MODES, ConfigError, browser_mode, flag, pos_int

__all__ = ["BROWSER_MODES", "Config", "ConfigError", "DEFAULT_PAGE_URL", "DEFAULT_TRAINS_URL"]

DEFAULT_TRAINS_URL = "https://maps.amtrak.com/services/MapDataService/trains/getTrainsData"
DEFAULT_PAGE_URL = "https://www.amtrak.com/tickets/train-status.html"


@dataclasses.dataclass(frozen=True)
class Config:
    """Immutable fetcher configuration."""

    trains_url: str = DEFAULT_TRAINS_URL
    page_url: str = DEFAULT_PAGE_URL
    poll_interval_secs: int = 60
    window_before_secs: int = 1800
    window_after_secs: int = 600
    hot_window_secs: int = 720
    cooldown_secs: int = 90
    max_fetches_per_cycle: int = 24
    document_ttl_secs: int = 7200
    nav_timeout_secs: int = 45
    fetch_timeout_secs: int = 20
    browser_max_age_secs: int = 21600
    relaunch_backoff_max_secs: int = 900
    max_stale_secs: int = 300
    liveness_secs: int = 900
    browser_mode: str = "headless-shell"
    block_subresources: bool = True
    block_third_party: bool = True
    lead_time_file: str = ""
    base_interval_secs: int = 900
    debounce_secs: int = 5
    min_gap_secs: int = 90

    @staticmethod
    def from_env(env: Mapping[str, str]) -> "Config":
        """Build a :class:`Config` from ``env``, applying defaults and validating."""
        return Config(
            trains_url=env.get("TRAINS_URL", DEFAULT_TRAINS_URL),
            page_url=env.get("AMTRAK_PAGE_URL", DEFAULT_PAGE_URL),
            poll_interval_secs=pos_int(env, "POLL_INTERVAL_SECS", 60),
            window_before_secs=pos_int(env, "TRACK_WINDOW_BEFORE_SECS", 1800),
            window_after_secs=pos_int(env, "TRACK_WINDOW_AFTER_SECS", 600),
            hot_window_secs=pos_int(env, "TRACK_HOT_WINDOW_SECS", 720),
            cooldown_secs=pos_int(env, "TRACK_COOLDOWN_SECS", 90),
            max_fetches_per_cycle=pos_int(env, "MAX_FETCHES_PER_CYCLE", 24),
            document_ttl_secs=pos_int(env, "DOCUMENT_TTL_SECS", 7200),
            nav_timeout_secs=pos_int(env, "NAV_TIMEOUT_SECS", 45),
            fetch_timeout_secs=pos_int(env, "FETCH_TIMEOUT_SECS", 20),
            browser_max_age_secs=pos_int(env, "BROWSER_MAX_AGE_SECS", 21600),
            relaunch_backoff_max_secs=pos_int(env, "RELAUNCH_BACKOFF_MAX_SECS", 900),
            max_stale_secs=pos_int(env, "MAX_STALE_SECS", 300),
            liveness_secs=pos_int(env, "LIVENESS_SECS", 900),
            browser_mode=browser_mode(env),
            block_subresources=flag(env, "BLOCK_SUBRESOURCES", True),
            block_third_party=flag(env, "BLOCK_THIRD_PARTY", True),
            lead_time_file=env.get("AMTRAK_TRACKS_LEAD_TIME_FILE", ""),
            base_interval_secs=pos_int(env, "AMTRAK_TRACKS_BASE_INTERVAL_SECS", 900),
            debounce_secs=pos_int(env, "AMTRAK_TRACKS_DEBOUNCE_SECS", 5),
            min_gap_secs=pos_int(env, "AMTRAK_TRACKS_MIN_GAP_SECS", 90),
        )

