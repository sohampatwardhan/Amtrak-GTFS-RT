"""Which browser sources run, and where the worker listens.

The worker reads the same on/off flags as the Rust service, which starts it with its own
environment:

* tracks run while ``AMTRAK_TRACKS`` and ``AMTRAK_TRACKS_AMTRAK`` are both on (both default on);
* notices run while ``AMTRAK_ADVISORIES`` is on (default on).

A source that is off has no config here (``None``): its loop does not start and its HTTP paths
answer ``404``. With both off nothing needs a browser and the worker exits at startup.

The worker binds ``127.0.0.1:8081`` by default. Only the service in the same container reads it.
"""

from __future__ import annotations

import dataclasses
from collections.abc import Mapping

from .env import ConfigError, browser_mode, flag, port, pos_int
from .notices.config import Config as NoticesConfig
from .tracks.config import Config as TracksConfig

__all__ = ["ConfigError", "DEFAULT_LISTEN_HOST", "DEFAULT_LISTEN_PORT", "WorkerConfig"]

DEFAULT_LISTEN_HOST = "127.0.0.1"
DEFAULT_LISTEN_PORT = 8081


@dataclasses.dataclass(frozen=True)
class WorkerConfig:
    """The worker's listener, its one browser, and each enabled source's settings."""

    listen_host: str = DEFAULT_LISTEN_HOST
    listen_port: int = DEFAULT_LISTEN_PORT
    browser_mode: str = "headless-shell"
    browser_close_secs: int = 2
    tracks: TracksConfig | None = None
    notices: NoticesConfig | None = None

    @staticmethod
    def from_env(env: Mapping[str, str]) -> "WorkerConfig":
        """Build the worker config from ``env``.

        Raises :class:`ConfigError` naming the variable when a flag or a value is malformed. A
        disabled source's own variables are not parsed.
        """
        tracks_on = flag(env, "AMTRAK_TRACKS", True)
        tracks_amtrak_on = flag(env, "AMTRAK_TRACKS_AMTRAK", True)
        notices_on = flag(env, "AMTRAK_ADVISORIES", True)
        return WorkerConfig(
            listen_host=env.get("LISTEN_HOST") or DEFAULT_LISTEN_HOST,
            listen_port=port(env, "LISTEN_PORT", DEFAULT_LISTEN_PORT),
            browser_mode=browser_mode(env),
            browser_close_secs=pos_int(env, "BROWSER_CLOSE_SECS", 2),
            tracks=TracksConfig.from_env(env) if tracks_on and tracks_amtrak_on else None,
            notices=NoticesConfig.from_env(env) if notices_on else None,
        )

    def sources(self) -> list[str]:
        """Names of the sources that run, for the startup log line."""
        return [
            name
            for name, config in (("tracks", self.tracks), ("notices", self.notices))
            if config is not None
        ]
