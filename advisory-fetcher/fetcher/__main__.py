"""Entrypoint: start the snapshot HTTP server and run the poll loop until terminated.

Wiring (task 3.5): build :class:`~fetcher.config.Config` from the environment, create the
:class:`~fetcher.store.SnapshotStore`, serve the latest snapshot over HTTP in a background thread,
and run the async poll loop in the foreground. On SIGTERM/SIGINT the loop stops and the server shuts
down cleanly so ``docker stop`` exits promptly.
"""

from __future__ import annotations

import asyncio
import logging
import os
import signal
import threading
from collections.abc import Mapping
from http.server import ThreadingHTTPServer

from .config import Config
from .display import DisplayError, start_virtual_display, stop_virtual_display
from .poller import run_forever
from .server import make_server
from .store import SnapshotStore

logger = logging.getLogger("advisory_fetcher")

# A navigation stuck in the driver does not return when the poll task is cancelled. These markers
# match Playwright's Node driver and the Chromium builds it launches, and nothing else in the image.
_BROWSER_CMDLINE_MARKERS = (
    b"playwright/driver/node",
    b"chrome-headless-shell",
    b"headless_shell",
    b"/chrome",
)


def _stop_browser_processes() -> None:
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


def build_components(env: Mapping[str, str]) -> tuple[Config, SnapshotStore, ThreadingHTTPServer]:
    """Construct the config, store, and (bound, not-yet-serving) HTTP server from ``env``.

    Factored out of :func:`main` so the wiring can be exercised in tests without running the loop.
    """
    cfg = Config.from_env(env)
    store = SnapshotStore(cfg.snapshot_dir)
    server = make_server(
        store, cfg.serve_path, cfg.max_stale_secs, cfg.listen_port, cfg.detail_max_stale_secs
    )
    return cfg, store, server


async def _serve(cfg: Config, store: SnapshotStore, server: ThreadingHTTPServer) -> None:
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    logger.info("serving snapshot on :%d%s", cfg.listen_port, cfg.serve_path)

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()

    def _request_stop() -> None:
        _stop_browser_processes()
        if stop.is_set():
            return
        stop.set()
        # If the loop never resumes the coroutine, still die inside the smoke test's budget.
        loop.call_later(5, os._exit, 0)

    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, _request_stop)

    poll_task = asyncio.create_task(run_forever(store, cfg))
    stop_task = asyncio.create_task(stop.wait())
    try:
        await asyncio.wait({poll_task, stop_task}, return_when=asyncio.FIRST_COMPLETED)
    finally:
        _stop_browser_processes()
        poll_task.cancel()
        try:
            await asyncio.wait_for(poll_task, timeout=2)
        except (asyncio.CancelledError, TimeoutError):
            pass
        # server.shutdown() can block in the listening thread, and a stuck driver task makes
        # asyncio.run wait past `docker stop`. The server thread is a daemon; exit now.
        os._exit(0)


def main() -> None:
    """Program entry point: configure logging, build components, and run until terminated."""
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s %(message)s")
    cfg, store, server = build_components(os.environ)
    try:
        display = start_virtual_display(cfg.browser_mode)
    except DisplayError as error:
        logger.error("%s", error)
        raise SystemExit(2) from None
    try:
        asyncio.run(_serve(cfg, store, server))
    finally:
        stop_virtual_display(display)


if __name__ == "__main__":
    main()
