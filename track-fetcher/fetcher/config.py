"""Runtime configuration for the track fetcher.

Every value comes from the environment and has a default, so a bare ``docker run`` polls Amtrak
without further setup. The defaults follow the Amtrak app's behavior on 2026-10-17: train 167's
Providence track appeared about ten minutes before arrival, so the poll looks about 30 minutes
ahead and 10 minutes behind, and it does not fetch the whole active fleet every minute.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Mapping

DEFAULT_TRAINS_URL = "https://maps.amtrak.com/services/MapDataService/trains/getTrainsData"
DEFAULT_PAGE_URL = "https://www.amtrak.com/tickets/train-status.html"
DEFAULT_SERVE_PATH = "/tracks.json"
BROWSER_MODES = ("headless-shell", "xvfb-headful")
_TRUE = {"1", "true", "on", "yes"}
_FALSE = {"0", "false", "off", "no"}


class ConfigError(ValueError):
    """An environment value is malformed. The message names the key."""


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
    max_stale_secs: int = 300
    listen_port: int = 8081
    serve_path: str = DEFAULT_SERVE_PATH
    browser_mode: str = "headless-shell"
    block_subresources: bool = True

    @staticmethod
    def from_env(env: Mapping[str, str]) -> "Config":
        """Build a :class:`Config` from ``env``, applying defaults and validating."""
        return Config(
            trains_url=env.get("TRAINS_URL", DEFAULT_TRAINS_URL),
            page_url=env.get("AMTRAK_PAGE_URL", DEFAULT_PAGE_URL),
            poll_interval_secs=_pos_int(env, "POLL_INTERVAL_SECS", 60),
            window_before_secs=_pos_int(env, "TRACK_WINDOW_BEFORE_SECS", 1800),
            window_after_secs=_pos_int(env, "TRACK_WINDOW_AFTER_SECS", 600),
            hot_window_secs=_pos_int(env, "TRACK_HOT_WINDOW_SECS", 720),
            cooldown_secs=_pos_int(env, "TRACK_COOLDOWN_SECS", 90),
            max_fetches_per_cycle=_pos_int(env, "MAX_FETCHES_PER_CYCLE", 24),
            document_ttl_secs=_pos_int(env, "DOCUMENT_TTL_SECS", 7200),
            nav_timeout_secs=_pos_int(env, "NAV_TIMEOUT_SECS", 45),
            max_stale_secs=_pos_int(env, "MAX_STALE_SECS", 300),
            listen_port=_port(env, "LISTEN_PORT", 8081),
            serve_path=env.get("SERVE_PATH", DEFAULT_SERVE_PATH),
            browser_mode=_browser_mode(env),
            block_subresources=_bool(env, "BLOCK_SUBRESOURCES", True),
        )


def _pos_int(env: Mapping[str, str], key: str, default: int) -> int:
    raw = env.get(key)
    if raw is None or raw == "":
        return default
    try:
        value = int(raw)
    except ValueError:
        raise ConfigError(f"{key} must be an integer, got {raw!r}") from None
    if value <= 0:
        raise ConfigError(f"{key} must be a positive integer, got {value}")
    return value


def _port(env: Mapping[str, str], key: str, default: int) -> int:
    raw = env.get(key)
    if raw is None or raw == "":
        return default
    try:
        value = int(raw)
    except ValueError:
        raise ConfigError(f"{key} must be an integer, got {raw!r}") from None
    if not 0 <= value <= 65535:
        raise ConfigError(f"{key} must be in 0..65535, got {value}")
    return value


def _bool(env: Mapping[str, str], key: str, default: bool) -> bool:
    raw = env.get(key)
    if raw is None or raw == "":
        return default
    lowered = raw.strip().casefold()
    if lowered in _TRUE:
        return True
    if lowered in _FALSE:
        return False
    raise ConfigError(f"{key} must be a boolean (true/false), got {raw!r}")


def _browser_mode(env: Mapping[str, str]) -> str:
    mode = env.get("BROWSER_MODE", "headless-shell")
    if mode not in BROWSER_MODES:
        raise ConfigError(f"BROWSER_MODE must be one of {BROWSER_MODES}, got {mode!r}")
    return mode
