"""Which sources the worker runs, read from the same flags as the Rust service."""

import pytest

from worker.config import WorkerConfig
from worker.env import ConfigError


def test_both_sources_run_by_default_on_loopback():
    cfg = WorkerConfig.from_env({})
    assert cfg.tracks is not None
    assert cfg.notices is not None
    assert (cfg.listen_host, cfg.listen_port) == ("127.0.0.1", 8081)
    assert cfg.sources() == ["tracks", "notices"]


@pytest.mark.parametrize("key", ["AMTRAK_TRACKS", "AMTRAK_TRACKS_AMTRAK"])
def test_either_track_flag_turns_tracks_off(key):
    cfg = WorkerConfig.from_env({key: "off"})
    assert cfg.tracks is None
    assert cfg.notices is not None
    assert cfg.sources() == ["notices"]


def test_advisories_flag_turns_notices_off():
    cfg = WorkerConfig.from_env({"AMTRAK_ADVISORIES": "FALSE"})
    assert cfg.notices is None
    assert cfg.sources() == ["tracks"]


@pytest.mark.parametrize("value", ["", "  "])
def test_blank_flags_keep_their_default(value):
    cfg = WorkerConfig.from_env({"AMTRAK_ADVISORIES": value, "AMTRAK_TRACKS_AMTRAK": value})
    assert cfg.sources() == ["tracks", "notices"]


@pytest.mark.parametrize("key", ["AMTRAK_TRACKS", "AMTRAK_TRACKS_AMTRAK", "AMTRAK_ADVISORIES"])
def test_an_unknown_flag_value_names_the_variable(key):
    with pytest.raises(ConfigError, match=key):
        WorkerConfig.from_env({key: "maybe"})


def test_a_disabled_source_does_not_parse_its_settings():
    cfg = WorkerConfig.from_env({"AMTRAK_ADVISORIES": "off", "NOTICES_POLL_INTERVAL_SECS": "soon"})
    assert cfg.notices is None
    with pytest.raises(ConfigError, match="NOTICES_POLL_INTERVAL_SECS"):
        WorkerConfig.from_env({"NOTICES_POLL_INTERVAL_SECS": "soon"})


def test_listener_and_browser_overrides():
    cfg = WorkerConfig.from_env(
        {
            "LISTEN_HOST": "0.0.0.0",
            "LISTEN_PORT": "8080",
            "BROWSER_MODE": "xvfb-headful",
            "BROWSER_CLOSE_SECS": "5",
        }
    )
    assert (cfg.listen_host, cfg.listen_port) == ("0.0.0.0", 8080)
    assert cfg.browser_mode == "xvfb-headful"
    assert cfg.browser_close_secs == 5
    assert cfg.tracks.browser_mode == cfg.notices.browser_mode == "xvfb-headful"
    with pytest.raises(ConfigError, match="LISTEN_PORT"):
        WorkerConfig.from_env({"LISTEN_PORT": "70000"})
