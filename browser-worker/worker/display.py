"""Virtual X display for ``BROWSER_MODE=xvfb-headful``.

Full Chromium needs an X server when it is not headless. The default image ships only
``chromium-headless-shell``, which needs none. An image built with
``--build-arg BROWSER_BUNDLE=full`` adds full Chromium and Xvfb, and this module starts Xvfb before
the first launch so no ``xvfb-run`` wrapper is needed.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import time
from collections.abc import Callable, MutableMapping

HEADFUL_MODE = "xvfb-headful"
DISPLAY = ":99"
_SOCKET = "/tmp/.X11-unix/X99"
_READY_TIMEOUT_SECS = 10.0


class DisplayError(RuntimeError):
    """The virtual display cannot start. The message says how to fix the image."""


def start_virtual_display(
    mode: str,
    env: MutableMapping[str, str] = os.environ,
    *,
    which: Callable[[str], str | None] = shutil.which,
    popen: Callable[..., subprocess.Popen] = subprocess.Popen,
    ready: Callable[[], bool] = lambda: os.path.exists(_SOCKET),
    sleep: Callable[[float], None] = time.sleep,
) -> subprocess.Popen | None:
    """Start Xvfb for headful mode and point ``DISPLAY`` at it.

    Returns ``None`` when no display is needed: headless mode, or ``DISPLAY`` already set by the
    operator. Raises :class:`DisplayError` when Xvfb is missing or exits before it is ready.
    """
    if mode != HEADFUL_MODE or env.get("DISPLAY"):
        return None
    binary = which("Xvfb")
    if binary is None:
        raise DisplayError(
            "BROWSER_MODE=xvfb-headful needs full Chromium and Xvfb; "
            "rebuild the image with --build-arg BROWSER_BUNDLE=full"
        )
    process = popen(
        [binary, DISPLAY, "-screen", "0", "1280x800x24", "-nolisten", "tcp"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    waited = 0.0
    while not ready():
        if process.poll() is not None:
            raise DisplayError(f"Xvfb exited with status {process.returncode}")
        if waited >= _READY_TIMEOUT_SECS:
            process.kill()
            raise DisplayError("Xvfb did not start within 10 seconds")
        sleep(0.1)
        waited += 0.1
    env["DISPLAY"] = DISPLAY
    return process


def stop_virtual_display(process: subprocess.Popen | None) -> None:
    """Stop a display started by :func:`start_virtual_display`."""
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
