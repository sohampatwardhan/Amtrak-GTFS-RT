"""Poll loop: decrypt the maps feed, then fetch status JSON for trains near a stop.

The maps feed (``maps.amtrak.com``) answers a plain HTTP client. ``www.amtrak.com`` does not, so
status documents are fetched with one resident headless browser: launching a browser on every
60-second cycle would thrash a Raspberry Pi. Image, font, stylesheet, and media requests are
blocked. A failed cycle keeps the last good documents.
"""

from __future__ import annotations

import asyncio
import json
import logging
import urllib.request
from collections.abc import Awaitable, Callable
from datetime import datetime, timedelta, timezone

from .config import Config
from .decrypt import DecryptError, decrypt_payload
from .select import DueTrain, choose_fetches, due_trains
from .store import TrackStore

logger = logging.getLogger(__name__)

_LAUNCH_ARGS = ["--disable-dev-shm-usage", "--disable-gpu"]
_BLOCKED_RESOURCE_TYPES = frozenset({"image", "font", "media", "stylesheet"})
_FETCH_JS = """async (url) => {
  const response = await fetch(url, {
    headers: { "Accept": "application/json", "Content-Type": "application/json" }
  });
  return { status: response.status, body: await response.text() };
}"""

TrainsFetcher = Callable[[Config], Awaitable[dict]]
StatusFetcher = Callable[[Config, DueTrain], Awaitable[dict | None]]
Clock = Callable[[], datetime]


def should_block(resource_type: str) -> bool:
    """Abort page weight the status fetch does not need. Script and fetch stay allowed."""
    return resource_type in _BLOCKED_RESOURCE_TYPES


def status_url(train_number: str, service_date: str) -> str:
    """Amtrak train-status URL for one train-day."""
    return (
        "https://www.amtrak.com/dotcom/travel-service/statuses/"
        f"{train_number}?service-date={service_date}"
    )


def load_trains(cfg: Config) -> dict:
    """GET the maps train feed and return its GeoJSON. A JSON body is used as-is."""
    request = urllib.request.Request(
        cfg.trains_url,
        headers={"Accept": "application/json", "User-Agent": "amtrak-gtfs-rt-track-fetcher"},
    )
    with urllib.request.urlopen(request, timeout=cfg.nav_timeout_secs) as response:
        text = response.read().decode("utf-8", "replace").strip()
    if text.startswith("{"):
        parsed = json.loads(text)
    else:
        parsed = json.loads(decrypt_payload(text))
    if not isinstance(parsed, dict):
        raise DecryptError("train feed is not an object")
    return parsed


async def _default_trains(cfg: Config) -> dict:
    return await asyncio.to_thread(load_trains, cfg)


class ResidentBrowser:
    """One headless browser, reused across polls and relaunched after a failed fetch."""

    def __init__(self, cfg: Config) -> None:
        self._cfg = cfg
        self._playwright = None
        self._browser = None
        self._page = None

    async def fetch(self, cfg: Config, train: DueTrain) -> dict | None:
        """Return one status document, or ``None`` when this train could not be read."""
        url = status_url(train.train_number, train.service_date)
        try:
            page = await self._ensure_page()
            raw = await page.evaluate(_FETCH_JS, url)
        except Exception:  # noqa: BLE001 — a dead renderer is one missed train
            logger.warning("status browser failed for train %s; relaunching next cycle", train.train_number)
            await self.close()
            return None
        return _document_from_eval(raw)

    async def close(self) -> None:
        """Release the browser. The next fetch launches a new one."""
        browser, playwright = self._browser, self._playwright
        self._page = None
        self._browser = None
        self._playwright = None
        if browser is not None:
            try:
                await browser.close()
            except Exception:  # noqa: BLE001
                logger.debug("browser close failed", exc_info=True)
        if playwright is not None:
            try:
                await playwright.stop()
            except Exception:  # noqa: BLE001
                logger.debug("playwright stop failed", exc_info=True)

    async def _ensure_page(self):
        if self._page is not None:
            return self._page
        from playwright.async_api import async_playwright  # lazy: tests never import it

        manager = async_playwright()
        playwright = await manager.start()
        self._playwright = playwright
        headless = self._cfg.browser_mode != "xvfb-headful"
        launch = {"headless": headless, "args": _LAUNCH_ARGS}
        if not headless:
            launch["channel"] = "chromium"
        browser = await playwright.chromium.launch(**launch)
        self._browser = browser
        context = await browser.new_context()
        if self._cfg.block_subresources:

            async def _route(route):
                if should_block(route.request.resource_type):
                    await route.abort()
                else:
                    await route.continue_()

            await context.route("**/*", _route)
        page = await context.new_page()
        await page.goto(
            self._cfg.page_url,
            wait_until="commit",
            timeout=self._cfg.nav_timeout_secs * 1000,
        )
        self._page = page
        return page


def _document_from_eval(raw: object) -> dict | None:
    if not isinstance(raw, dict) or raw.get("status") != 200:
        return None
    body = raw.get("body")
    if not isinstance(body, str):
        return None
    try:
        document = json.loads(body)
    except json.JSONDecodeError:
        return None
    if not isinstance(document, dict) or "data" not in document:
        return None
    return document


async def poll_once(
    store: TrackStore,
    cfg: Config,
    last_fetched: dict[tuple[str, str], float],
    *,
    now: datetime,
    trains_fetcher: TrainsFetcher,
    status_fetcher: StatusFetcher,
) -> None:
    """One cycle. Raises when the train list itself cannot be read; status misses are skipped."""
    geojson = await trains_fetcher(cfg)
    due = due_trains(
        geojson,
        now,
        before=timedelta(seconds=cfg.window_before_secs),
        after=timedelta(seconds=cfg.window_after_secs),
    )
    picks = choose_fetches(
        due,
        last_fetched,
        now,
        hot_secs=cfg.hot_window_secs,
        cooldown_secs=cfg.cooldown_secs,
        max_fetches=cfg.max_fetches_per_cycle,
    )
    stored = 0
    for train in picks:
        document = await status_fetcher(cfg, train)
        if document is None:
            continue
        store.put(train.train_number, train.service_date, document, now.timestamp())
        last_fetched[train.key] = now.timestamp()
        stored += 1
    store.mark_success(now.timestamp())
    logger.info("track poll due=%s fetched=%s stored=%s", len(due), len(picks), stored)


async def run_forever(
    store: TrackStore,
    cfg: Config,
    *,
    trains_fetcher: TrainsFetcher | None = None,
    status_fetcher: StatusFetcher | None = None,
    sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
    clock: Clock | None = None,
    iterations: int | None = None,
) -> None:
    """Poll until cancelled, or ``iterations`` times in tests.

    The default status fetcher keeps one :class:`ResidentBrowser` for the life of the loop.
    """
    trains = trains_fetcher or _default_trains
    browser = ResidentBrowser(cfg)
    status = status_fetcher or browser.fetch
    now_of = clock or (lambda: datetime.now(timezone.utc))
    last_fetched: dict[tuple[str, str], float] = {}
    count = 0
    try:
        while iterations is None or count < iterations:
            try:
                await poll_once(
                    store,
                    cfg,
                    last_fetched,
                    now=now_of(),
                    trains_fetcher=trains,
                    status_fetcher=status,
                )
            except Exception:  # noqa: BLE001 — a failed cycle must not crash the sidecar
                logger.exception("track poll failed; keeping the last snapshot")
            count += 1
            if iterations is not None and count >= iterations:
                break
            await sleep(cfg.poll_interval_secs)
    finally:
        await browser.close()
