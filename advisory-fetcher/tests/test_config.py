"""Tests for advisory-fetcher configuration parsing (task 3.1, Requirements 6.1/6.2)."""

import pytest

from fetcher.config import BROWSER_MODES, Config, ConfigError


def test_defaults_when_env_empty():
    cfg = Config.from_env({})
    assert cfg.source_url.endswith("/service-alerts-and-notices")
    assert cfg.poll_interval_secs == 900
    assert cfg.nav_timeout_secs == 45
    assert cfg.max_stale_secs == 3600
    assert cfg.listen_port == 8080
    assert cfg.browser_mode == "headless-shell"
    assert cfg.block_subresources is True


def test_overrides_applied():
    cfg = Config.from_env(
        {
            "AMTRAK_SOURCE_URL": "http://fetcher/notices",
            "POLL_INTERVAL_SECS": "300",
            "LISTEN_PORT": "9000",
            "BROWSER_MODE": "xvfb-headful",
            "BLOCK_SUBRESOURCES": "off",
        }
    )
    assert cfg.source_url == "http://fetcher/notices"
    assert cfg.poll_interval_secs == 300
    assert cfg.listen_port == 9000
    assert cfg.browser_mode == "xvfb-headful"
    assert cfg.block_subresources is False


@pytest.mark.parametrize("value", ["", None])
def test_blank_falls_back_to_default(value):
    env = {} if value is None else {"POLL_INTERVAL_SECS": value}
    assert Config.from_env(env).poll_interval_secs == 900


def test_non_integer_rejected():
    with pytest.raises(ConfigError, match="POLL_INTERVAL_SECS"):
        Config.from_env({"POLL_INTERVAL_SECS": "soon"})


def test_non_positive_integer_rejected():
    with pytest.raises(ConfigError, match="positive"):
        Config.from_env({"NAV_TIMEOUT_SECS": "0"})


def test_unknown_browser_mode_rejected():
    with pytest.raises(ConfigError, match="BROWSER_MODE"):
        Config.from_env({"BROWSER_MODE": "webkit"})
    assert "headless-shell" in BROWSER_MODES


def test_bad_boolean_rejected():
    with pytest.raises(ConfigError, match="BLOCK_SUBRESOURCES"):
        Config.from_env({"BLOCK_SUBRESOURCES": "maybe"})


def test_config_is_frozen():
    cfg = Config.from_env({})
    with pytest.raises(Exception):
        cfg.poll_interval_secs = 1  # type: ignore[misc]
