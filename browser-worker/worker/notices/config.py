"""Runtime configuration for the notices source of the browser worker.

The notices source shares its process with the track source, so every setting that only the
notices source reads carries a ``NOTICES_`` prefix. ``POLL_INTERVAL_SECS`` and ``MAX_STALE_SECS``
belong to the track source and to the Rust service; the notices cadence and freshness window are
``NOTICES_POLL_INTERVAL_SECS`` and ``NOTICES_MAX_STALE_SECS``. ``NAV_TIMEOUT_SECS``,
``BROWSER_MODE``, ``BLOCK_SUBRESOURCES``, and ``BROWSER_CLOSE_SECS`` are shared on purpose: there is
one browser.

The defaults encode the design decisions: a respectful 15-minute poll (Requirements 6.1/6.2), the
efficient ``headless-shell`` mode with subresource blocking on (Requirement 8), and an HTTP
endpoint whose freshness window the service reads through its own TTL (Requirement 2.3).
"""

from __future__ import annotations

import dataclasses
import os
from collections.abc import Mapping

from ..env import BROWSER_MODES, ConfigError, browser_mode, flag, pos_int

__all__ = ["BROWSER_MODES", "Config", "ConfigError", "DEFAULT_SOURCE_URL", "default_snapshot_dir"]

DEFAULT_SOURCE_URL = "https://www.amtrak.com/service-alerts-and-notices"
DEFAULT_GATE_SELECTOR = "[class*='na-service-alert']"


def default_snapshot_dir(env: Mapping[str, str]) -> str:
    """``{AMTRAK_OUTPUT_DIR}/notices``, the service's own output directory (``out`` on a host)."""
    return os.path.join(env.get("AMTRAK_OUTPUT_DIR") or "out", "notices")


@dataclasses.dataclass(frozen=True)
class Config:
    """Immutable notices configuration.

    Fields:
        source_url: the Amtrak notices page to load.
        poll_interval_secs: seconds between page loads (R6.1); the floor the poll loop sleeps for.
        nav_timeout_secs: per-page navigation and selector timeout; on expiry the cycle is
            abandoned without overwriting a good snapshot (R1.3).
        max_stale_secs: how long a snapshot is served as fresh; past it the HTTP layer returns 503
            so the service's fail-open path engages (R2.3/R3.1).
        snapshot_dir: directory holding the atomically replaced snapshot files.
        gate_selector: CSS selector whose presence proves Akamai let the advisory markup render.
        browser_mode: ``headless-shell`` (efficient default) or ``xvfb-headful`` (fallback).
        block_subresources: abort image/font/media/CSS requests during the fetch, keeping
            script/xhr so Akamai's sensor JS still runs (R8).
        detail_ttl_secs: skip re-rendering a detail page whose snapshot is younger than this.
        detail_max_stale_secs: how long a stored detail is served as fresh. Must be at least
            ``detail_ttl_secs`` so a cached page is not returned as 503 before it is due again.
        render_stable_ms: HTML size must stay unchanged this long, with the gate selector
            present, before the page is snapshotted.
        render_poll_ms: how often to sample ``page.content()`` while waiting for a stable size.
        browser_close_secs: graceful shutdown budget for a browser context; past it the
            renderer is killed.
    """

    source_url: str = DEFAULT_SOURCE_URL
    poll_interval_secs: int = 900
    nav_timeout_secs: int = 45
    max_stale_secs: int = 3600
    snapshot_dir: str = "out/notices"
    gate_selector: str = DEFAULT_GATE_SELECTOR
    browser_mode: str = "headless-shell"
    block_subresources: bool = True
    detail_ttl_secs: int = 86400
    detail_max_stale_secs: int = 86400
    render_stable_ms: int = 400
    render_poll_ms: int = 200
    browser_close_secs: int = 2

    @staticmethod
    def from_env(env: Mapping[str, str]) -> "Config":
        """Build a :class:`Config` from an environment mapping, applying defaults and validating.

        Raises :class:`ConfigError` naming the variable when a value is malformed.
        """
        detail_ttl_secs = pos_int(env, "NOTICES_DETAIL_TTL_SECS", 86400)
        detail_max_stale_secs = pos_int(env, "NOTICES_DETAIL_MAX_STALE_SECS", 86400)
        if detail_max_stale_secs < detail_ttl_secs:
            raise ConfigError(
                "NOTICES_DETAIL_MAX_STALE_SECS must be greater than or equal to "
                f"NOTICES_DETAIL_TTL_SECS ({detail_max_stale_secs} < {detail_ttl_secs})"
            )
        return Config(
            source_url=env.get("NOTICES_SOURCE_URL") or DEFAULT_SOURCE_URL,
            poll_interval_secs=pos_int(env, "NOTICES_POLL_INTERVAL_SECS", 900),
            nav_timeout_secs=pos_int(env, "NAV_TIMEOUT_SECS", 45),
            max_stale_secs=pos_int(env, "NOTICES_MAX_STALE_SECS", 3600),
            snapshot_dir=env.get("NOTICES_SNAPSHOT_DIR") or default_snapshot_dir(env),
            gate_selector=env.get("NOTICES_GATE_SELECTOR") or DEFAULT_GATE_SELECTOR,
            browser_mode=browser_mode(env),
            block_subresources=flag(env, "BLOCK_SUBRESOURCES", True),
            detail_ttl_secs=detail_ttl_secs,
            detail_max_stale_secs=detail_max_stale_secs,
            render_stable_ms=pos_int(env, "NOTICES_RENDER_STABLE_MS", 400),
            render_poll_ms=pos_int(env, "NOTICES_RENDER_POLL_MS", 200),
            browser_close_secs=pos_int(env, "BROWSER_CLOSE_SECS", 2),
        )
