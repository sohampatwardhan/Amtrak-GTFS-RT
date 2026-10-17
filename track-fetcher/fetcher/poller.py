"""Poll loop: decrypt the maps feed, then fetch status JSON for trains near a stop.

The maps feed (``maps.amtrak.com``) answers a plain HTTP client. ``www.amtrak.com`` does not, so
status documents are fetched with one resident headless browser: launching a browser on every
60-second cycle would thrash a Raspberry Pi. Image, font, stylesheet, and media requests are
blocked, and so is every host outside ``amtrak.com`` unless ``BLOCK_THIRD_PARTY`` is off.

A failed status fetch keeps the last good document. A browser failure ends the cycle's fetches
and the relaunch backs off, so a blocked or crashing browser is not relaunched once per train. A
cycle counts as a success only when it had nothing to fetch or stored a document, so a sidecar
Amtrak has stopped answering lets ``/tracks.json`` go stale instead of looking current.
"""

from __future__ import annotations

import asyncio
import json
import logging
import time
import urllib.request
from collections import Counter
from collections.abc import Awaitable, Callable
from datetime import datetime, timedelta, timezone
from urllib.parse import urlsplit

from .config import Config
from .decrypt import DecryptError, decrypt_payload
from .select import DueTrain, choose_fetches, due_trains
from .store import TrackStore

logger = logging.getLogger(__name__)

_LAUNCH_ARGS = ["--disable-dev-shm-usage", "--disable-gpu"]
_BLOCKED_RESOURCE_TYPES = frozenset({"image", "font", "media", "stylesheet"})
_FIRST_PARTY = "amtrak.com"
_RELAUNCH_BACKOFF_MIN_SECS = 60
_FETCH_JS = """async ([url, timeoutMs]) => {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(url, {
      headers: { "Accept": "application/json", "Content-Type": "application/json" },
      signal: controller.signal,
    });
    return { status: response.status, body: await response.text() };
  } finally {
    clearTimeout(timer);
  }
}"""


class BrowserUnavailable(Exception):
    """The browser could not run a fetch. The rest of the cycle is skipped."""


class StatusMiss(Exception):
    """Amtrak answered, but not with a usable status document. The message is the reason."""


TrainsFetcher = Callable[[Config], Awaitable[dict]]
StatusFetcher = Callable[[Config, DueTrain], Awaitable[dict | None]]
Clock = Callable[[], datetime]
Monotonic = Callable[[], float]


def should_block(resource_type: str) -> bool:
    """Abort page weight the status fetch does not need. Script and fetch stay allowed."""
    return resource_type in _BLOCKED_RESOURCE_TYPES


def is_first_party(url: str) -> bool:
    """Whether ``url`` is on ``amtrak.com`` or one of its subdomains."""
    host = (urlsplit(url).hostname or "").lower()
    return host == _FIRST_PARTY or host.endswith("." + _FIRST_PARTY)


def should_abort(cfg: Config, resource_type: str, url: str) -> bool:
    """Routing decision for one browser request."""
    if cfg.block_subresources and should_block(resource_type):
        return True
    return cfg.block_third_party and not is_first_party(url)


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
    """One headless browser, reused across polls.

    A failed fetch closes it, and the next launch waits an exponential backoff from one minute up
    to ``RELAUNCH_BACKOFF_MAX_SECS``, reset by the next successful fetch. The browser is also
    recycled after ``BROWSER_MAX_AGE_SECS`` to bound a resident page's memory growth.
    """

    def __init__(self, cfg: Config, *, monotonic: Monotonic = time.monotonic) -> None:
        self._cfg = cfg
        self._monotonic = monotonic
        self._playwright = None
        self._browser = None
        self._page = None
        self._launched_at = 0.0
        self._backoff = 0.0
        self._next_launch = 0.0

    async def fetch(self, cfg: Config, train: DueTrain) -> dict:
        """Return one status document.

        Raises :class:`StatusMiss` when Amtrak's answer is unusable and
        :class:`BrowserUnavailable` when the browser cannot run the request.
        """
        url = status_url(train.train_number, train.service_date)
        page = await self._ensure_page()
        timeout = cfg.fetch_timeout_secs
        try:
            raw = await asyncio.wait_for(
                page.evaluate(_FETCH_JS, [url, timeout * 1000]), timeout + 5
            )
        except Exception as error:  # noqa: BLE001 — a dead renderer or hung request
            await self._failed(f"status fetch failed: {type(error).__name__}")
            raise BrowserUnavailable(type(error).__name__) from None
        self._backoff = 0.0
        return document_from_eval(raw)

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

    async def _failed(self, reason: str) -> None:
        await self.close()
        self._backoff = min(
            max(self._backoff * 2, _RELAUNCH_BACKOFF_MIN_SECS),
            self._cfg.relaunch_backoff_max_secs,
        )
        self._next_launch = self._monotonic() + self._backoff
        logger.warning("%s; next browser launch in %ds", reason, self._backoff)

    async def _ensure_page(self):
        now = self._monotonic()
        if self._page is not None and now - self._launched_at >= self._cfg.browser_max_age_secs:
            logger.info("recycling the status browser after %ds", self._cfg.browser_max_age_secs)
            await self.close()
        if self._page is not None:
            return self._page
        if now < self._next_launch:
            raise BrowserUnavailable("relaunch backoff")
        try:
            page = await self._launch()
        except Exception as error:  # noqa: BLE001 — launch or first page load failed
            await self._failed(f"browser launch failed: {type(error).__name__}")
            raise BrowserUnavailable(type(error).__name__) from None
        self._launched_at = now
        self._page = page
        return page

    async def _launch(self):
        from playwright.async_api import async_playwright  # lazy: tests never import it

        cfg = self._cfg
        playwright = await async_playwright().start()
        self._playwright = playwright
        headless = cfg.browser_mode != "xvfb-headful"
        launch = {"headless": headless, "args": _LAUNCH_ARGS}
        if not headless:
            launch["channel"] = "chromium"
        browser = await playwright.chromium.launch(**launch)
        self._browser = browser
        context = await browser.new_context()
        if cfg.block_subresources or cfg.block_third_party:

            async def _route(route):
                request = route.request
                if should_abort(cfg, request.resource_type, request.url):
                    await route.abort()
                else:
                    await route.continue_()

            await context.route("**/*", _route)
        page = await context.new_page()
        # domcontentloaded lets the site's first-party scripts set their cookies before the
        # first status call; "commit" returned before they ran.
        await page.goto(
            cfg.page_url,
            wait_until="domcontentloaded",
            timeout=cfg.nav_timeout_secs * 1000,
        )
        return page


def document_from_eval(raw: object) -> dict:
    """The status document in an in-page fetch result, or :class:`StatusMiss` with the reason."""
    if not isinstance(raw, dict):
        raise StatusMiss("no response")
    status = raw.get("status")
    if status != 200:
        raise StatusMiss(f"http {status}")
    body = raw.get("body")
    if not isinstance(body, str):
        raise StatusMiss("no body")
    try:
        document = json.loads(body)
    except json.JSONDecodeError:
        raise StatusMiss("not json") from None
    if not isinstance(document, dict) or "data" not in document:
        raise StatusMiss("no data")
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
    """One cycle. Raises when the train list itself cannot be read; status misses are skipped.

    Every attempt, failed or not, starts the train's cooldown. ``last_fetched`` is pruned to the
    trains still due.
    """
    geojson = await trains_fetcher(cfg)
    due = due_trains(
        geojson,
        now,
        before=timedelta(seconds=cfg.window_before_secs),
        after=timedelta(seconds=cfg.window_after_secs),
    )
    due_keys = {train.key for train in due}
    for key in [key for key in last_fetched if key not in due_keys]:
        del last_fetched[key]
    picks = choose_fetches(
        due,
        last_fetched,
        now,
        hot_secs=cfg.hot_window_secs,
        cooldown_secs=cfg.cooldown_secs,
        max_fetches=cfg.max_fetches_per_cycle,
        tracked=store.tracked(),
    )
    stored = 0
    rejected: Counter[str] = Counter()
    browser_down = None
    for train in picks:
        last_fetched[train.key] = now.timestamp()
        try:
            document = await status_fetcher(cfg, train)
        except BrowserUnavailable as error:
            browser_down = str(error)
            break
        except StatusMiss as error:
            rejected[str(error)] += 1
            continue
        if document is None:
            rejected["empty"] += 1
            continue
        store.put(train.train_number, train.service_date, document, now.timestamp())
        stored += 1
    if not picks or stored > 0:
        store.mark_success(now.timestamp())
    logger.info(
        "track poll due=%s picked=%s stored=%s rejected=%s%s",
        len(due),
        len(picks),
        stored,
        dict(rejected) or 0,
        f" browser_unavailable={browser_down}" if browser_down else "",
    )


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
            store.mark_cycle(now_of().timestamp())
            count += 1
            if iterations is not None and count >= iterations:
                break
            await sleep(cfg.poll_interval_secs)
    finally:
        await browser.close()
