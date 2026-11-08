"""Entrypoint: serve both sources' snapshots and run each enabled poll loop until SIGTERM.

``python -m worker`` reads :class:`~worker.config.WorkerConfig` from the environment, binds one
listener, and runs the track loop, the notices loop, or both, sharing one Chromium
(:class:`~worker.browser.SharedBrowser`). With both sources off it logs that and exits ``0``.

On SIGTERM or SIGINT the loops stop, the browser and its driver are killed, and the process exits
promptly so ``docker stop`` never waits on a navigation stuck in the driver.
"""

from __future__ import annotations

import asyncio
import logging
import os
import signal
import threading
from collections.abc import Mapping
from dataclasses import dataclass
from http.server import ThreadingHTTPServer

from .browser import SharedBrowser, kill_browser_processes
from .config import WorkerConfig
from .display import DisplayError, start_virtual_display, stop_virtual_display
from .notices import poller as notices_poller
from .notices.store import SnapshotStore
from .server import Snapshots, make_server
from .tracks import poller as tracks_poller
from .tracks.store import TrackStore

logger = logging.getLogger("browser_worker")


@dataclass(frozen=True)
class Components:
    """Everything :func:`main` runs, built without starting anything."""

    config: WorkerConfig
    snapshots: Snapshots
    server: ThreadingHTTPServer


def build_components(env: Mapping[str, str]) -> Components | None:
    """Construct the config, each enabled source's store, and a bound (not serving) server.

    Returns ``None`` when both sources are off. Factored out of :func:`main` so the wiring can be
    tested without running the loops.
    """
    config = WorkerConfig.from_env(env)
    if config.tracks is None and config.notices is None:
        return None
    snapshots = Snapshots(
        config=config,
        tracks=TrackStore() if config.tracks is not None else None,
        notices=SnapshotStore(config.notices.snapshot_dir) if config.notices is not None else None,
    )
    return Components(config, snapshots, make_server(snapshots))


async def _serve(components: Components) -> None:
    config, snapshots, server = components.config, components.snapshots, components.server
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    logger.info(
        "browser worker serving %s on %s:%d",
        " and ".join(config.sources()),
        config.listen_host,
        config.listen_port,
    )

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()

    def _request_stop() -> None:
        kill_browser_processes()
        if stop.is_set():
            return
        stop.set()
        # If the loop never resumes the coroutine, still die inside the smoke test's budget.
        loop.call_later(5, os._exit, 0)

    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, _request_stop)

    shared = SharedBrowser(config.browser_mode, config.browser_close_secs)
    polls: set[asyncio.Task] = set()
    if config.tracks is not None and snapshots.tracks is not None:
        polls.add(
            asyncio.create_task(
                tracks_poller.run_forever(snapshots.tracks, config.tracks, shared=shared)
            )
        )
    if config.notices is not None and snapshots.notices is not None:
        polls.add(
            asyncio.create_task(
                notices_poller.run_forever(snapshots.notices, config.notices, shared=shared)
            )
        )
    stop_task = asyncio.create_task(stop.wait())
    try:
        # A poll loop never returns on its own. If one does, exit so the supervisor restarts us.
        await asyncio.wait({*polls, stop_task}, return_when=asyncio.FIRST_COMPLETED)
    finally:
        kill_browser_processes()
        for task in polls:
            task.cancel()
        try:
            await asyncio.wait_for(asyncio.gather(*polls, return_exceptions=True), timeout=2)
        except (asyncio.CancelledError, TimeoutError):
            pass
        # server.shutdown() can block in the listening thread, and a stuck driver task makes
        # asyncio.run wait past `docker stop`. The server thread is a daemon; exit now.
        os._exit(0)


def main(env: Mapping[str, str] | None = None) -> None:
    """Configure logging, build the components, and run until terminated."""
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s %(message)s")
    components = build_components(os.environ if env is None else env)
    if components is None:
        logger.warning(
            "no browser source is on (tracks and notices are both off); exiting"
        )
        return
    try:
        display = start_virtual_display(components.config.browser_mode)
    except DisplayError as error:
        logger.error("%s", error)
        raise SystemExit(2) from None
    try:
        asyncio.run(_serve(components))
    finally:
        stop_virtual_display(display)


if __name__ == "__main__":
    main()
