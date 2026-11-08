"""One Chromium for the whole worker.

Both sources borrow the same browser through :class:`SharedBrowser`. Each borrower takes a
:class:`Lease` and opens its own browser context, so tracks and notices never share cookies or
pages, and only one Chromium process ever runs:

* The track source holds a lease for as long as its page is up, so with tracks on the browser
  stays resident between polls.
* The notices source takes a lease for one cycle and returns it at the end. With tracks off, the
  last lease returned closes the browser, so a notices-only worker runs Chromium only during a
  cycle.

A browser that disconnects (a renderer crash or an out-of-memory kill) is dropped at the next
:meth:`SharedBrowser.acquire` and relaunched. :meth:`SharedBrowser.close` tears it down even
while leases are out; those leases become stale and returning one does nothing. Playwright is
imported only inside :func:`start_chromium`, so the module loads and its tests run without a
browser installed.
"""

from __future__ import annotations

import asyncio
import itertools
import logging
import os
import signal
from collections.abc import Awaitable, Callable
from dataclasses import dataclass

logger = logging.getLogger(__name__)

LAUNCH_ARGS = ["--disable-dev-shm-usage", "--disable-gpu"]

# A navigation stuck in the driver does not return when a poll task is cancelled. These markers
# match Playwright's Node driver and the Chromium builds it launches, and nothing else in the image.
_BROWSER_CMDLINE_MARKERS = (
    b"playwright/driver/node",
    b"chrome-headless-shell",
    b"headless_shell",
    b"/chrome",
)

# Starts Playwright and one browser: returns ``(playwright, browser)``.
Starter = Callable[[str], Awaitable[tuple[object, object]]]


@dataclass(frozen=True)
class Lease:
    """A borrower's hold on the shared browser. ``browser`` is the Playwright ``Browser``."""

    browser: object
    generation: int
    token: int


async def start_chromium(mode: str) -> tuple[object, object]:
    """Start Playwright and launch Chromium in ``mode``.

    ``headless-shell`` launches ``chromium-headless-shell``. ``xvfb-headful`` launches full
    Chromium on the display :mod:`worker.display` started. If the launch fails, the Playwright
    driver is stopped before the error propagates.
    """
    from playwright.async_api import async_playwright  # lazy: tests never import it

    playwright = await async_playwright().start()
    try:
        if mode == "xvfb-headful":
            browser = await playwright.chromium.launch(
                headless=False, channel="chromium", args=LAUNCH_ARGS
            )
        else:
            browser = await playwright.chromium.launch(headless=True, args=LAUNCH_ARGS)
    except BaseException:
        try:
            await playwright.stop()
        except Exception:  # noqa: BLE001 — the launch error is the one worth raising
            logger.debug("playwright stop after a failed launch failed", exc_info=True)
        raise
    return playwright, browser


class SharedBrowser:
    """Lends one Chromium to every source in the worker. See the module docstring."""

    def __init__(
        self,
        mode: str = "headless-shell",
        close_secs: float = 2,
        *,
        start: Starter = start_chromium,
    ) -> None:
        self._mode = mode
        self._close_secs = close_secs
        self._start = start
        self._lock = asyncio.Lock()
        self._playwright: object | None = None
        self._browser: object | None = None
        self._generation = 0
        self._leases: set[int] = set()
        self._tokens = itertools.count(1)
        self.launches = 0

    @property
    def running(self) -> bool:
        """Whether a browser is up right now."""
        return self._browser is not None

    @property
    def holders(self) -> int:
        """Leases currently out on the running browser."""
        return len(self._leases)

    async def acquire(self) -> Lease:
        """Borrow the browser, launching it first when none is running or it has disconnected.

        A failed launch propagates and leaves nothing held.
        """
        async with self._lock:
            if self._browser is not None and not _connected(self._browser):
                logger.warning("the shared browser disconnected; relaunching")
                await self._shutdown()
            if self._browser is None:
                self._playwright, self._browser = await self._start(self._mode)
                self._generation += 1
                self.launches += 1
            token = next(self._tokens)
            self._leases.add(token)
            return Lease(self._browser, self._generation, token)

    async def release(self, lease: Lease) -> None:
        """Return a lease. The last lease returned closes the browser.

        A lease from a browser that has since been closed or relaunched is ignored.
        """
        async with self._lock:
            if lease.generation != self._generation or lease.token not in self._leases:
                return
            self._leases.discard(lease.token)
            if not self._leases:
                await self._shutdown()

    async def close(self) -> None:
        """Close the browser now, whatever is borrowed. Outstanding leases become stale."""
        async with self._lock:
            await self._shutdown()

    async def _shutdown(self) -> None:
        browser, playwright = self._browser, self._playwright
        self._browser = None
        self._playwright = None
        self._leases.clear()
        if browser is None:
            return
        try:
            await asyncio.wait_for(browser.close(), self._close_secs)
        except Exception:  # noqa: BLE001 — close can hang while pages keep timers armed
            logger.warning("browser close exceeded %ss; killing the renderer", self._close_secs)
            kill_driver(playwright)
        if playwright is not None:
            try:
                await asyncio.wait_for(playwright.stop(), self._close_secs)
            except Exception:  # noqa: BLE001 — the driver may already be dead
                kill_driver(playwright)


def _connected(browser: object) -> bool:
    is_connected = getattr(browser, "is_connected", None)
    if is_connected is None:
        return True
    try:
        return bool(is_connected())
    except Exception:  # noqa: BLE001 — a dead connection can raise instead of answering
        return False


def driver_pid(playwright: object) -> int | None:
    """Best-effort pid of the Playwright driver, so a hung renderer can be killed."""
    try:
        return playwright._impl_obj._connection._transport._proc.pid  # noqa: SLF001
    except Exception:  # noqa: BLE001 — private layout differs across Playwright releases
        return None


def kill_driver(playwright: object) -> None:
    """SIGKILL the Playwright driver and the browsers it started, if its pid is known."""
    pid = driver_pid(playwright)
    if pid is not None:
        kill_process_tree(pid)


def kill_process_tree(pid: int) -> None:
    """SIGKILL ``pid`` and its children."""
    children: list[int] = []
    try:
        with open(f"/proc/{pid}/task/{pid}/children", encoding="ascii") as handle:
            children = [int(child) for child in handle.read().split()]
    except (OSError, ValueError):
        children = []
    for child in children:
        kill_process_tree(child)
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass


def kill_browser_processes() -> None:
    """SIGKILL Playwright's driver and browsers so a poll cannot hold ``docker stop``."""
    me = os.getpid()
    doomed: list[int] = []
    try:
        entries = os.listdir("/proc")
    except OSError:
        return
    for entry in entries:
        if not entry.isdigit():
            continue
        pid = int(entry)
        if pid == me:
            continue
        try:
            with open(f"/proc/{pid}/cmdline", "rb") as handle:
                command = handle.read()
        except OSError:
            continue
        if any(marker in command for marker in _BROWSER_CMDLINE_MARKERS):
            doomed.append(pid)
    for pid in doomed:
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass
