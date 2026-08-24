"""Runtime configuration for the advisory-fetcher.

All settings come from the environment so the same image runs unchanged across deployments; every
value has a safe default so a bare `docker run` works. The defaults encode the design decisions:
a respectful 15-minute poll cadence (Requirements 6.1/6.2), the efficient `headless-shell` browser
mode with subresource-blocking on (Requirement 8), and an HTTP endpoint whose freshness window the
service reads through its own TTL (Requirement 2.3).
"""

from __future__ import annotations

import dataclasses
from collections.abc import Mapping

DEFAULT_SOURCE_URL = "https://www.amtrak.com/service-alerts-and-notices"
DEFAULT_GATE_SELECTOR = "[class*='na-service-alert']"
DEFAULT_SERVE_PATH = "/service-alerts-and-notices"
DEFAULT_SNAPSHOT_DIR = "/snapshot"
BROWSER_MODES = ("headless-shell", "xvfb-headful")
_TRUE = {"1", "true", "on", "yes"}
_FALSE = {"0", "false", "off", "no"}


class ConfigError(ValueError):
    """Raised when an environment value is malformed, naming the offending key and reason."""


@dataclasses.dataclass(frozen=True)
class Config:
    """Immutable fetcher configuration.

    Fields:
        source_url: the Amtrak notices page to load.
        poll_interval_secs: minimum seconds between successive page loads (R6.1); the floor the
            poll loop sleeps for.
        nav_timeout_secs: per-cycle navigation/selector timeout; on expiry the cycle is abandoned
            without overwriting a good snapshot (R1.3).
        max_stale_secs: how long a snapshot is served as fresh; past it the HTTP layer returns 503
            so the service's fail-open path engages (R2.3/R3.1).
        listen_port: port the snapshot HTTP server binds.
        snapshot_dir: directory holding the atomically-replaced snapshot file.
        serve_path: HTTP path the service GETs (its `AMTRAK_ADVISORIES_URL`).
        gate_selector: CSS selector whose presence proves Akamai let the advisory markup render.
        browser_mode: `headless-shell` (efficient default) or `xvfb-headful` (fallback).
        block_subresources: when true, abort image/font/media/CSS requests during the fetch for
            efficiency, keeping script/xhr so Akamai's sensor JS still runs (R8).
    """

    source_url: str = DEFAULT_SOURCE_URL
    poll_interval_secs: int = 900
    nav_timeout_secs: int = 45
    max_stale_secs: int = 3600
    listen_port: int = 8080
    snapshot_dir: str = DEFAULT_SNAPSHOT_DIR
    serve_path: str = DEFAULT_SERVE_PATH
    gate_selector: str = DEFAULT_GATE_SELECTOR
    browser_mode: str = "headless-shell"
    block_subresources: bool = True

    @staticmethod
    def from_env(env: Mapping[str, str]) -> "Config":
        """Build a :class:`Config` from an environment mapping, applying defaults and validating.

        Integer fields must parse as positive integers and `BROWSER_MODE` must be a known mode;
        anything else raises :class:`ConfigError` naming the key, so a misconfiguration fails fast
        at startup rather than surfacing as a confusing runtime error later.
        """
        return Config(
            source_url=env.get("AMTRAK_SOURCE_URL", DEFAULT_SOURCE_URL),
            poll_interval_secs=_pos_int(env, "POLL_INTERVAL_SECS", 900),
            nav_timeout_secs=_pos_int(env, "NAV_TIMEOUT_SECS", 45),
            max_stale_secs=_pos_int(env, "MAX_STALE_SECS", 3600),
            listen_port=_port(env, "LISTEN_PORT", 8080),
            snapshot_dir=env.get("SNAPSHOT_DIR", DEFAULT_SNAPSHOT_DIR),
            serve_path=env.get("SERVE_PATH", DEFAULT_SERVE_PATH),
            gate_selector=env.get("GATE_SELECTOR", DEFAULT_GATE_SELECTOR),
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
    if not 0 <= value <= 65535:  # 0 = OS-assigned ephemeral port
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
