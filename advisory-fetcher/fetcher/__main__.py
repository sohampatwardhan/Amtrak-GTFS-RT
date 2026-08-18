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
from .poller import run_forever
from .server import make_server
from .store import SnapshotStore

logger = logging.getLogger("advisory_fetcher")


def build_components(env: Mapping[str, str]) -> tuple[Config, SnapshotStore, ThreadingHTTPServer]:
    """Construct the config, store, and (bound, not-yet-serving) HTTP server from ``env``.

    Factored out of :func:`main` so the wiring can be exercised in tests without running the loop.
    """
    cfg = Config.from_env(env)
    store = SnapshotStore(cfg.snapshot_dir)
    server = make_server(store, cfg.serve_path, cfg.max_stale_secs, cfg.listen_port)
    return cfg, store, server


async def _serve(cfg: Config, store: SnapshotStore, server: ThreadingHTTPServer) -> None:
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    logger.info("serving snapshot on :%d%s", cfg.listen_port, cfg.serve_path)

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
    """Program entry point: configure logging, build components, and run until terminated."""
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s %(message)s")
    cfg, store, server = build_components(os.environ)
    asyncio.run(_serve(cfg, store, server))


if __name__ == "__main__":
    main()
