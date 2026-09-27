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
import re
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


def should_block(resource_type: str) -> bool:
    """Return whether a subresource of this type should be aborted during the fetch.

    Blocks image/font/media/CSS (pure page weight we do not need) while allowing document, script,
    xhr, and fetch so Akamai's sensor JS still executes and the advisory markup still renders.
    """
    return resource_type in _BLOCKED_RESOURCE_TYPES


async def _default_launcher(cfg: Config) -> FetchedAdvisories | None:
    """Launch Chromium once, load the notices page, then each linked detail page.

    Uses ``chromium-headless-shell`` (headless, no channel) by default, or full Chromium under Xvfb
    when ``cfg.browser_mode == "xvfb-headful"``. Playwright is imported lazily so the module (and
    its tests) load without a browser installed. The browser is always closed in ``finally``.
    Detail pages share that session so the Akamai cookie earned for the list still applies. A
    detail that times out or lacks the body markup is skipped; the list is still returned.
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
                return None
            list_html = await page.content()
            details: dict[str, str] = {}
            for path in extract_detail_paths(list_html):
                try:
                    await page.goto(
                        urljoin(cfg.source_url, path),
                        wait_until="domcontentloaded",
                        timeout=cfg.nav_timeout_secs * 1000,
                    )
                    await page.wait_for_selector(
                        _DETAIL_SELECTOR, timeout=cfg.nav_timeout_secs * 1000
                    )
                    details[path] = await page.content()
                except Exception:  # noqa: BLE001 — one detail miss must not drop the list
                    logger.warning("advisory detail page not obtained: %s", path)
            return FetchedAdvisories(list_html, details)
        finally:
            await browser.close()  # release all browser memory every cycle


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
    count = 0
    while iterations is None or count < iterations:
        try:
            fetched = await poll_once(cfg, launcher=launcher)
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
