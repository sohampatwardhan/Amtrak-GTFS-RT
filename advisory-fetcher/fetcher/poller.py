"""The poll loop: launch a browser per cycle, fetch the notices page, store the HTML.

Design decisions embodied here (Requirements 1.1–1.3, 4.1/4.2, 8.1/8.3):

* **Launch-per-poll, single browser** — each cycle launches exactly one browser, uses it, and
  closes it in a ``finally`` (no resident browser between polls; peak memory bounded to one browser).
* **Subresource-blocking** — when enabled, image/font/media/CSS requests are aborted while
  script/xhr/fetch are allowed, so Akamai's sensor JS still runs but the fetch is lean.
* **Fail-open** — a cycle that times out, is blocked, or errors returns ``None`` and never
  overwrites a good snapshot; the loop never raises.

The Playwright work lives in the injectable :func:`_default_launcher` so the loop's control flow is
unit-testable offline; the real browser path is exercised on-device (task 5.1).
"""

from __future__ import annotations

import asyncio
import logging
import os
import re
import signal
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from urllib.parse import urljoin

from .config import Config
from .store import SnapshotStore, safe_detail_name

logger = logging.getLogger(__name__)

# Cap detail navigations so a notices page cannot turn one poll into an unbounded crawl.
MAX_DETAIL_PAGES = 40
_DETAIL_SELECTOR = ".alerts-details-minimum"
_DATA_HREF = re.compile(r"""data-href=["']([^"']+)["']""")

_LAUNCH_ARGS = ["--disable-dev-shm-usage", "--disable-gpu"]
_BLOCKED_RESOURCE_TYPES = frozenset({"image", "font", "media", "stylesheet"})
_STEALTH_UA = (
    "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
)

@dataclass(frozen=True)
class FetchedAdvisories:
    """One successful poll: the notices list plus any detail pages captured in the same session."""

    list_html: str
    details: dict[str, str]


# A launcher returns the notices list and detail pages, or ``None`` when the list gate is missing.
Launcher = Callable[[Config], Awaitable[FetchedAdvisories | None]]


def extract_detail_paths(html: str) -> list[str]:
    """Return unique `/alert/<slug>.html` paths from list-page ``data-href`` values, in order.

    Absolute URLs, query strings, and anything that is not a safe alert path are skipped. The
    result is capped at :data:`MAX_DETAIL_PAGES`.
    """
    found: list[str] = []
    seen: set[str] = set()
    for match in _DATA_HREF.finditer(html):
        raw = match.group(1).split("?", 1)[0].split("#", 1)[0]
        if safe_detail_name(raw) is None or raw in seen:
            continue
        seen.add(raw)
        found.append(raw)
        if len(found) >= MAX_DETAIL_PAGES:
            break
    return found


def detail_paths_due(paths: list[str], fresh: set[str]) -> list[str]:
    """Detail paths that still need a browser render. ``fresh`` paths are served from cache."""
    return [path for path in paths if path not in fresh]


@dataclass
class RenderStability:
    """Tracks whether rendered HTML has stopped growing and the gate selector is present.

    Amtrak pages keep timer-driven requests alive for about 20 seconds after the advisory
    markup is on the page. Sampling the HTML size and stopping at the first stable window
    avoids waiting those timers out.
    """

    last_size: int | None = None
    stable_for: float = 0.0

    def observe(self, size: int, gate_present: bool, elapsed: float, required: float) -> bool:
        """Return whether this sample completes a stable window of ``required`` seconds."""
        if gate_present and size > 0 and size == self.last_size:
            self.stable_for += elapsed
            return self.stable_for >= required
        self.last_size = size if gate_present and size > 0 else None
        self.stable_for = 0.0
        return False


def should_block(resource_type: str) -> bool:
    """Return whether a subresource of this type should be aborted during the fetch.

    Blocks image/font/media/CSS (pure page weight we do not need) while allowing document, script,
    xhr, and fetch so Akamai's sensor JS still executes and the advisory markup still renders.
    """
    return resource_type in _BLOCKED_RESOURCE_TYPES


async def _render_until_stable(page, url: str, selector: str, cfg: Config) -> str | None:
    """Load ``url`` and return its HTML once the size is stable and ``selector`` is present.

    Navigation waits only until the response is committed. Later timer-driven requests do not
    extend the wait. ``None`` means the gate never became stable before the navigation timeout.
    """
    try:
        await page.goto(url, wait_until="commit", timeout=cfg.nav_timeout_secs * 1000)
    except Exception:  # noqa: BLE001 — a navigation failure is one missed page
        return None
    stability = RenderStability()
    deadline = time.monotonic() + cfg.nav_timeout_secs
    poll = cfg.render_poll_ms / 1000
    required = cfg.render_stable_ms / 1000
    html = ""
    while time.monotonic() < deadline:
        try:
            html = await page.content()
            gate = await page.query_selector(selector) is not None
        except Exception:  # noqa: BLE001 — the page can be closing or mid-navigation
            gate = False
            html = ""
        if stability.observe(len(html), gate, poll, required):
            return html
        await asyncio.sleep(poll)
    return None


def _driver_pid(playwright) -> int | None:
    """Best-effort pid of the Playwright driver so a hung renderer can be killed."""
    try:
        return playwright._impl_obj._connection._transport._proc.pid  # noqa: SLF001
    except Exception:  # noqa: BLE001 — private layout differs across Playwright releases
        return None


def _kill_process_tree(pid: int) -> None:
    """SIGKILL ``pid`` and its children. Used when ``browser.close()`` waits on page timers."""
    children: list[int] = []
    try:
        raw = open(f"/proc/{pid}/task/{pid}/children", encoding="ascii").read().split()
        children = [int(child) for child in raw]
    except (OSError, ValueError):
        children = []
    for child in children:
        _kill_process_tree(child)
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass


async def _terminate_browser(browser, playwright, cfg: Config) -> None:
    """Close the browser, then kill the driver tree if shutdown exceeds the budget."""
    try:
        await asyncio.wait_for(browser.close(), cfg.browser_close_secs)
    except Exception:  # noqa: BLE001 — close can hang while pages keep timers armed
        logger.warning("browser close exceeded %ss; killing renderer", cfg.browser_close_secs)
        pid = _driver_pid(playwright)
        if pid is not None:
            _kill_process_tree(pid)
    try:
        stop = getattr(playwright, "stop", None)
        if stop is not None:
            await asyncio.wait_for(stop(), cfg.browser_close_secs)
    except Exception:  # noqa: BLE001 — the driver may already be dead
        pid = _driver_pid(playwright)
        if pid is not None:
            _kill_process_tree(pid)


async def _default_launcher(
    cfg: Config, store: SnapshotStore | None = None
) -> FetchedAdvisories | None:
    """Launch Chromium once, render the notices list, then any stale detail pages.

    Uses ``chromium-headless-shell`` (headless, no channel) by default, or full Chromium under Xvfb
    when ``cfg.browser_mode == "xvfb-headful"``. Playwright is imported lazily so the module (and
    its tests) load without a browser installed. Each page is snapshotted when its HTML size is
    stable and the renderer is then closed, so timer-driven requests cannot hold the browser.
    Detail pages already stored within ``detail_ttl_secs`` are not rendered again. A detail that
    fails is skipped; the list is still returned. The browser is always released in ``finally``.
    """
    from playwright.async_api import async_playwright  # lazy: only needed for a real fetch

    manager = async_playwright()
    playwright = await manager.start()
    try:
        if cfg.browser_mode == "xvfb-headful":
            browser = await playwright.chromium.launch(
                headless=False, channel="chromium", args=_LAUNCH_ARGS
            )
        else:
            browser = await playwright.chromium.launch(headless=True, args=_LAUNCH_ARGS)
        # Everything after a successful launch is inside try/finally so the renderer is always
        # released, even if new_context() itself fails.
        try:
            if cfg.browser_mode == "xvfb-headful":
                context = await browser.new_context(
                    user_agent=_STEALTH_UA, viewport={"width": 1280, "height": 800}
                )
            else:
                context = await browser.new_context()
            if cfg.block_subresources:

                async def _route(route_obj):
                    if should_block(route_obj.request.resource_type):
                        await route_obj.abort()
                    else:
                        await route_obj.continue_()

                await context.route("**/*", _route)
            page = await context.new_page()
            list_html = await _render_until_stable(page, cfg.source_url, cfg.gate_selector, cfg)
            await page.close()
            if list_html is None:
                return None
            now = time.time()
            fresh = {
                path
                for path in extract_detail_paths(list_html)
                if store is not None and store.detail_is_fresh(path, cfg.detail_ttl_secs, now)
            }
            details: dict[str, str] = {}
            for path in detail_paths_due(extract_detail_paths(list_html), fresh):
                detail_page = await context.new_page()
                try:
                    html = await _render_until_stable(
                        detail_page, urljoin(cfg.source_url, path), _DETAIL_SELECTOR, cfg
                    )
                finally:
                    await detail_page.close()
                if html is None:
                    logger.warning("advisory detail page not obtained: %s", path)
                else:
                    details[path] = html
            if fresh:
                logger.info("advisory details served from cache: %s", len(fresh))
            return FetchedAdvisories(list_html, details)
        finally:
            await _terminate_browser(browser, playwright, cfg)
    finally:
        try:
            await asyncio.wait_for(manager.__aexit__(None, None, None), cfg.browser_close_secs)
        except Exception:  # noqa: BLE001 — driver shutdown is best-effort after the renderer dies
            pid = _driver_pid(playwright)
            if pid is not None:
                _kill_process_tree(pid)


async def poll_once(cfg: Config, *, launcher: Launcher = _default_launcher) -> FetchedAdvisories | None:
    """Run one fetch cycle; return the list and detail pages, or ``None`` if the list was blocked."""
    return await launcher(cfg)


async def run_forever(
    store: SnapshotStore,
    cfg: Config,
    *,
    launcher: Launcher = _default_launcher,
    sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    iterations: int | None = None,
) -> None:
    """Poll forever (or ``iterations`` times, for tests), storing each successful fetch.

    Fail-open: any error in a cycle is logged and swallowed so a transient failure never crashes the
    loop or overwrites the last good snapshot. Between cycles it sleeps ``poll_interval_secs``
    (Requirement 6.1). ``launcher`` and ``sleep`` are injectable so the loop is testable offline.
    """
    async def launch(cfg: Config) -> FetchedAdvisories | None:
        if launcher is _default_launcher:
            return await _default_launcher(cfg, store)
        return await launcher(cfg)

    count = 0
    while iterations is None or count < iterations:
        try:
            fetched = await poll_once(cfg, launcher=launch)
            if fetched is not None:
                changed = store.update(fetched.list_html)
                stored_details = 0
                for path, detail_html in fetched.details.items():
                    try:
                        store.update_detail(path, detail_html)
                        stored_details += 1
                    except ValueError:
                        logger.warning("skipped unsafe advisory detail path")
                logger.info(
                    "advisories fetched (changed=%s details=%s)", changed, stored_details
                )
            else:
                logger.warning("advisories markup not obtained this cycle; keeping last snapshot")
        except Exception:  # noqa: BLE001 — fail-open: never let a cycle crash the loop
            logger.exception("poll cycle failed; keeping last snapshot")
        count += 1
        if iterations is not None and count >= iterations:
            break
        await sleep(cfg.poll_interval_secs)
