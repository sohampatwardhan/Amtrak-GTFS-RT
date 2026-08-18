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
from collections.abc import Awaitable, Callable

from .config import Config
from .store import SnapshotStore

logger = logging.getLogger(__name__)

_LAUNCH_ARGS = ["--disable-dev-shm-usage", "--disable-gpu"]
_BLOCKED_RESOURCE_TYPES = frozenset({"image", "font", "media", "stylesheet"})
_STEALTH_UA = (
    "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
)

# A launcher performs one browser fetch and returns (gate_present, html).
Launcher = Callable[[Config], Awaitable[tuple[bool, str]]]


def should_block(resource_type: str) -> bool:
    """Return whether a subresource of this type should be aborted during the fetch.

    Blocks image/font/media/CSS (pure page weight we do not need) while allowing document, script,
    xhr, and fetch so Akamai's sensor JS still executes and the advisory markup still renders.
    """
    return resource_type in _BLOCKED_RESOURCE_TYPES


async def _default_launcher(cfg: Config) -> tuple[bool, str]:
    """Launch Chromium once, load the notices page, and return ``(gate_present, html)``.

    Uses ``chromium-headless-shell`` (headless, no channel) by default, or full Chromium under Xvfb
    when ``cfg.browser_mode == "xvfb-headful"``. Playwright is imported lazily so the module (and
    its tests) load without a browser installed. The browser is always closed in ``finally``.
    """
    from playwright.async_api import async_playwright  # lazy: only needed for a real fetch

    async with async_playwright() as p:
        if cfg.browser_mode == "xvfb-headful":
            browser = await p.chromium.launch(headless=False, channel="chromium", args=_LAUNCH_ARGS)
        else:
            browser = await p.chromium.launch(headless=True, args=_LAUNCH_ARGS)
        # Everything after a successful launch is inside try/finally so browser.close() always runs,
        # even if new_context() itself fails — otherwise a failed context would leak a Chromium
        # process, defeating the launch-per-poll memory guarantee.
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
            await page.goto(
                cfg.source_url, wait_until="domcontentloaded", timeout=cfg.nav_timeout_secs * 1000
            )
            try:
                await page.wait_for_selector(cfg.gate_selector, timeout=cfg.nav_timeout_secs * 1000)
            except Exception:  # noqa: BLE001 — a missing gate selector is a blocked cycle
                return False, ""
            return True, await page.content()
        finally:
            await browser.close()  # release all browser memory every cycle


async def poll_once(cfg: Config, *, launcher: Launcher = _default_launcher) -> str | None:
    """Run one fetch cycle; return the advisories HTML, or ``None`` if the markup was not obtained."""
    gate_present, html = await launcher(cfg)
    return html if gate_present else None


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
    count = 0
    while iterations is None or count < iterations:
        try:
            html = await poll_once(cfg, launcher=launcher)
            if html is not None:
                changed = store.update(html)
                logger.info("advisories fetched (changed=%s)", changed)
            else:
                logger.warning("advisories markup not obtained this cycle; keeping last snapshot")
        except Exception:  # noqa: BLE001 — fail-open: never let a cycle crash the loop
            logger.exception("poll cycle failed; keeping last snapshot")
        count += 1
        if iterations is not None and count >= iterations:
            break
        await sleep(cfg.poll_interval_secs)
