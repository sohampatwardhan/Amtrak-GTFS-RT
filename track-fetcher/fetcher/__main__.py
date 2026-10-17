"""Entrypoint: serve ``/tracks.json`` and poll until SIGTERM."""

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
from .store import TrackStore

logger = logging.getLogger("track_fetcher")


def build_components(env: Mapping[str, str]) -> tuple[Config, TrackStore, ThreadingHTTPServer]:
    """Construct config, store, and a bound HTTP server. The poll loop is not started."""
    cfg = Config.from_env(env)
    store = TrackStore()
    server = make_server(
        store,
        cfg.serve_path,
        cfg.max_stale_secs,
        cfg.document_ttl_secs,
        cfg.listen_port,
        host=cfg.listen_host,
        liveness_secs=cfg.liveness_secs,
    )
    return cfg, store, server


async def _serve(cfg: Config, store: TrackStore, server: ThreadingHTTPServer) -> None:
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    logger.info("serving train status on %s:%d%s", cfg.listen_host, cfg.listen_port, cfg.serve_path)

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, stop.set)

    poll_task = asyncio.create_task(run_forever(store, cfg))
    stop_task = asyncio.create_task(stop.wait())
    try:
        await asyncio.wait({poll_task, stop_task}, return_when=asyncio.FIRST_COMPLETED)
    finally:
        poll_task.cancel()
        server.shutdown()
        server_thread.join(timeout=5)


def main() -> None:
    """Configure logging and run until terminated."""
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
