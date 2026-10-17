"""Environment defaults keep the near-ETA poll unless an operator overrides them."""

import pytest

from fetcher.config import Config, ConfigError


def test_defaults_poll_near_arrival_and_cap_each_cycle():
    cfg = Config.from_env({})
    assert cfg.trains_url.endswith("/trains/getTrainsData")
    assert cfg.page_url.endswith("/tickets/train-status.html")
    assert cfg.poll_interval_secs == 60
    assert cfg.window_before_secs == 1800
    assert cfg.window_after_secs == 600
    assert cfg.hot_window_secs == 720
    assert cfg.cooldown_secs == 90
    assert cfg.max_fetches_per_cycle == 24
    assert cfg.listen_port == 8081
    assert cfg.serve_path == "/tracks.json"
    assert cfg.browser_mode == "headless-shell"
    assert cfg.block_subresources is True
    assert cfg.max_stale_secs == 300


def test_overrides_and_rejections():
    cfg = Config.from_env(
        {
            "POLL_INTERVAL_SECS": "30",
            "MAX_FETCHES_PER_CYCLE": "8",
            "TRACK_WINDOW_BEFORE_SECS": "1200",
            "BLOCK_SUBRESOURCES": "off",
            "LISTEN_PORT": "0",
        }
    )
    assert cfg.poll_interval_secs == 30
    assert cfg.max_fetches_per_cycle == 8
    assert cfg.window_before_secs == 1200
    assert cfg.block_subresources is False
    assert cfg.listen_port == 0
    with pytest.raises(ConfigError, match="MAX_FETCHES_PER_CYCLE"):
        Config.from_env({"MAX_FETCHES_PER_CYCLE": "0"})
    with pytest.raises(ConfigError, match="BROWSER_MODE"):
        Config.from_env({"BROWSER_MODE": "lynx"})
