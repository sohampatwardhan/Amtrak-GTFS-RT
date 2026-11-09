"""Notices configuration (task 3.1, Requirements 6.1/6.2). Its own variables carry NOTICES_."""

import pytest

from worker.notices.config import BROWSER_MODES, Config, ConfigError


def test_defaults_when_env_empty():
    cfg = Config.from_env({})
    assert cfg.source_url.endswith("/service-alerts-and-notices")
    assert cfg.poll_interval_secs == 900
    assert cfg.nav_timeout_secs == 45
    assert cfg.max_stale_secs == 3600
    assert cfg.snapshot_dir == "out/notices"
    assert cfg.browser_mode == "headless-shell"
    assert cfg.block_subresources is True
    assert cfg.detail_ttl_secs == 86400
    assert cfg.detail_max_stale_secs == 86400
    assert cfg.render_stable_ms == 400
    assert cfg.render_poll_ms == 200
    assert cfg.browser_close_secs == 2


def test_overrides_applied():
    cfg = Config.from_env(
        {
            "NOTICES_SOURCE_URL": "http://worker/notices",
            "NOTICES_POLL_INTERVAL_SECS": "300",
            "NOTICES_SNAPSHOT_DIR": "/snapshot",
            "BROWSER_MODE": "xvfb-headful",
            "BLOCK_SUBRESOURCES": "off",
        }
    )
    assert cfg.source_url == "http://worker/notices"
    assert cfg.poll_interval_secs == 300
    assert cfg.snapshot_dir == "/snapshot"
    assert cfg.browser_mode == "xvfb-headful"
    assert cfg.block_subresources is False


@pytest.mark.parametrize("value", ["", None])
def test_blank_falls_back_to_default(value):
    env = {} if value is None else {"NOTICES_POLL_INTERVAL_SECS": value}
    assert Config.from_env(env).poll_interval_secs == 900


def test_non_integer_rejected():
    with pytest.raises(ConfigError, match="NOTICES_POLL_INTERVAL_SECS"):
        Config.from_env({"NOTICES_POLL_INTERVAL_SECS": "soon"})


def test_non_positive_integer_rejected():
    with pytest.raises(ConfigError, match="positive"):
        Config.from_env({"NAV_TIMEOUT_SECS": "0"})


def test_unknown_browser_mode_rejected():
    with pytest.raises(ConfigError, match="BROWSER_MODE"):
        Config.from_env({"BROWSER_MODE": "webkit"})
    assert "headless-shell" in BROWSER_MODES


def test_detail_stale_window_must_cover_the_cache_ttl():
    with pytest.raises(ConfigError, match="NOTICES_DETAIL_MAX_STALE_SECS"):
        Config.from_env(
            {"NOTICES_DETAIL_TTL_SECS": "86400", "NOTICES_DETAIL_MAX_STALE_SECS": "3600"}
        )


def test_bad_boolean_rejected():
    with pytest.raises(ConfigError, match="BLOCK_SUBRESOURCES"):
        Config.from_env({"BLOCK_SUBRESOURCES": "maybe"})


def test_config_is_frozen():
    cfg = Config.from_env({})
    with pytest.raises(Exception):
        cfg.poll_interval_secs = 1  # type: ignore[misc]


def test_the_track_poll_interval_does_not_change_the_notices_cadence():
    """One process reads both sources' settings; POLL_INTERVAL_SECS is the track source's."""
    cfg = Config.from_env({"POLL_INTERVAL_SECS": "60", "MAX_STALE_SECS": "300"})
    assert cfg.poll_interval_secs == 900
    assert cfg.max_stale_secs == 3600


def test_snapshots_live_under_the_service_output_directory():
    assert Config.from_env({"AMTRAK_OUTPUT_DIR": "/data"}).snapshot_dir == "/data/notices"
