"""Environment parsing shared by the worker, the track source, and the notices source.

Every value has a default, so a bare ``python -m worker`` runs. A malformed value raises
:class:`ConfigError` naming the variable, so a misconfiguration stops startup instead of
surfacing later as a confusing runtime error.
"""

from __future__ import annotations

from collections.abc import Mapping

BROWSER_MODES = ("headless-shell", "xvfb-headful")
_TRUE = {"1", "true", "on", "yes"}
_FALSE = {"0", "false", "off", "no"}


class ConfigError(ValueError):
    """An environment value is malformed. The message names the variable."""


def pos_int(env: Mapping[str, str], key: str, default: int) -> int:
    """A positive integer, or ``default`` when ``key`` is unset or empty."""
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


def port(env: Mapping[str, str], key: str, default: int) -> int:
    """A TCP port in ``0..65535`` (``0`` lets the OS choose), or ``default``."""
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


def flag(env: Mapping[str, str], key: str, default: bool) -> bool:
    """An on/off value. Accepts the same words as the Rust service's flags.

    ``1``, ``true``, ``on``, and ``yes`` are on; ``0``, ``false``, ``off``, and ``no`` are off,
    in any case. Unset or blank is ``default``. Anything else raises :class:`ConfigError`.
    """
    raw = env.get(key)
    if raw is None or raw.strip() == "":
        return default
    lowered = raw.strip().casefold()
    if lowered in _TRUE:
        return True
    if lowered in _FALSE:
        return False
    raise ConfigError(f"{key} must be on or off (true/false), got {raw!r}")


def browser_mode(env: Mapping[str, str]) -> str:
    """``BROWSER_MODE``: ``headless-shell`` (default) or ``xvfb-headful``."""
    mode = env.get("BROWSER_MODE") or "headless-shell"
    if mode not in BROWSER_MODES:
        raise ConfigError(f"BROWSER_MODE must be one of {BROWSER_MODES}, got {mode!r}")
    return mode
