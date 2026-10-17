"""Xvfb startup for headful mode, with the process and filesystem faked."""

import pytest

from fetcher.display import DISPLAY, DisplayError, start_virtual_display, stop_virtual_display


class _FakeProcess:
    def __init__(self, exit_code=None):
        self.returncode = exit_code
        self.terminated = False

    def poll(self):
        return self.returncode

    def terminate(self):
        self.terminated = True
        self.returncode = 0

    def kill(self):
        self.returncode = -9

    def wait(self, timeout=None):
        return self.returncode


def test_headless_or_an_existing_display_needs_nothing():
    assert start_virtual_display("headless-shell", {}) is None
    env = {"DISPLAY": ":0"}
    assert start_virtual_display("xvfb-headful", env) is None
    assert env["DISPLAY"] == ":0"


def test_a_missing_xvfb_names_the_build_argument():
    with pytest.raises(DisplayError, match="BROWSER_BUNDLE=full"):
        start_virtual_display("xvfb-headful", {}, which=lambda _name: None)


def test_xvfb_is_started_and_display_is_set_once_its_socket_exists():
    env: dict[str, str] = {}
    launched: list[list[str]] = []
    checks = iter([False, False, True])

    def popen(args, **_kwargs):
        launched.append(args)
        return _FakeProcess()

    process = start_virtual_display(
        "xvfb-headful",
        env,
        which=lambda _name: "/usr/bin/Xvfb",
        popen=popen,
        ready=lambda: next(checks),
        sleep=lambda _secs: None,
    )
    assert launched[0][:2] == ["/usr/bin/Xvfb", DISPLAY]
    assert "-nolisten" in launched[0]
    assert env["DISPLAY"] == DISPLAY
    stop_virtual_display(process)
    assert process.terminated


def test_an_xvfb_that_exits_or_never_starts_is_an_error():
    with pytest.raises(DisplayError, match="status 1"):
        start_virtual_display(
            "xvfb-headful",
            {},
            which=lambda _name: "/usr/bin/Xvfb",
            popen=lambda *_a, **_k: _FakeProcess(exit_code=1),
            ready=lambda: False,
            sleep=lambda _secs: None,
        )
    hung = _FakeProcess()
    with pytest.raises(DisplayError, match="did not start"):
        start_virtual_display(
            "xvfb-headful",
            {},
            which=lambda _name: "/usr/bin/Xvfb",
            popen=lambda *_a, **_k: hung,
            ready=lambda: False,
            sleep=lambda _secs: None,
        )
    assert hung.returncode == -9
    stop_virtual_display(None)
