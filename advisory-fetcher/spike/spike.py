#!/usr/bin/env python3
"""Akamai-bypass + Pi-memory spike for the advisory-fetcher.

This is the gating measurement for the advisory-fetcher feature (Requirements 7.1, 7.2). It
answers one load-bearing question that no documentation can settle: can a real browser, running
on the target Raspberry Pi from a server/residential IP, repeatably retrieve Amtrak's
Akamai-protected Service Alerts page (the ``na-service-alert__*`` markup the service parses), and
does it fit the 1 GB container memory budget?

It is deliberately standalone and throwaway: no dependency on the ``fetcher`` package, no state
left behind. Run it on the Pi inside a throwaway ``docker run --rm`` container (see README).

Three modes (``--mode``):

* ``headless-shell`` — the efficient path: headless Chromium with no ``channel`` set, which
  Playwright resolves to ``chromium-headless-shell``. Tried first.
* ``xvfb-headful`` — the fallback: full Chromium (``channel="chromium"``) with basic stealth
  (a realistic user-agent and viewport). Wrap the whole invocation in ``xvfb-run``.
* ``fixture`` — offline self-test: reads a local HTML file instead of launching a browser, so the
  harness's reporting logic can be verified without a browser or network.

The report (``SpikeReport``) records, per cycle, whether the gate markup appeared, plus the
success rate, the longest consecutive-success streak (the repeatability proof for R7.2), and the
peak resident memory read from cgroup v2 (``/sys/fs/cgroup/memory.peak``) — the R4.1 / R7.1
measurement under the 1 GB cap.
"""

from __future__ import annotations

import argparse
import dataclasses
import sys
import time
from pathlib import Path

DEFAULT_URL = "https://www.amtrak.com/service-alerts-and-notices"
# Gate on any element carrying a na-service-alert* class: this is the DOM the PR #8 parsers
# consume, and its presence is the signal that Akamai let the real content render.
DEFAULT_GATE_SELECTOR = "[class*='na-service-alert']"
GATE_CLASS_TOKEN = "na-service-alert"
DEFAULT_CGROUP_PEAK = "/sys/fs/cgroup/memory.peak"
DEFAULT_NAV_TIMEOUT_MS = 45_000


@dataclasses.dataclass(frozen=True)
class SpikeConfig:
    """Inputs for a spike run.

    ``mode`` picks the fetch backend; ``fixture`` never touches the network so the harness can be
    self-tested offline. ``cycles`` is how many consecutive fetches to attempt.
    """

    mode: str = "headless-shell"
    url: str = DEFAULT_URL
    gate_selector: str = DEFAULT_GATE_SELECTOR
    cycles: int = 10
    fixture: Path | None = None
    cgroup_peak: Path = Path(DEFAULT_CGROUP_PEAK)
    nav_timeout_ms: int = DEFAULT_NAV_TIMEOUT_MS


@dataclasses.dataclass(frozen=True)
class SpikeReport:
    """Outcome of a spike run — the evidence Checkpoint 2 (R7.1/R7.2) is judged against.

    ``per_cycle`` is one boolean per attempted cycle (did the gate markup appear).
    ``max_consecutive`` is the longest run of ``True`` values, the repeatability proof required by
    R7.2 (at least three). ``peak_rss_bytes`` is the cgroup-v2 peak, or ``None`` when the peak
    file is unavailable (for example on a developer machine outside a memory-capped container).
    """

    mode: str
    per_cycle: list[bool]
    peak_rss_bytes: int | None

    @property
    def success_rate(self) -> float:
        """Fraction of cycles that returned the gate markup (0.0 when no cycles ran)."""
        if not self.per_cycle:
            return 0.0
        return sum(self.per_cycle) / len(self.per_cycle)

    @property
    def max_consecutive(self) -> int:
        """Longest streak of consecutive successful cycles."""
        best = run = 0
        for ok in self.per_cycle:
            run = run + 1 if ok else 0
            best = max(best, run)
        return best

    def to_text(self) -> str:
        """Render a human-readable summary for the spike log and ``results.md``."""
        rss = "unavailable" if self.peak_rss_bytes is None else f"{self.peak_rss_bytes / 2**20:.1f} MiB"
        marks = "".join("." if ok else "x" for ok in self.per_cycle)
        return (
            f"mode={self.mode} cycles={len(self.per_cycle)} "
            f"success_rate={self.success_rate:.0%} "
            f"max_consecutive={self.max_consecutive} peak_rss={rss}\n"
            f"per-cycle (.=markup, x=blocked): {marks}"
        )


def _gate_present(html: str) -> bool:
    """Return whether the advisory gate markup is present in the page HTML."""
    return GATE_CLASS_TOKEN in html


def _read_peak_rss(path: Path) -> int | None:
    """Read the cgroup-v2 peak memory in bytes, or ``None`` if the file is unavailable.

    ``memory.peak`` exists only inside a cgroup-v2 memory-capped container (the way the spike runs
    on the Pi). Outside one — e.g. a developer machine — it is absent, and the spike still reports
    the bypass result with peak memory marked unavailable rather than failing.
    """
    try:
        return int(path.read_text().strip())
    except (OSError, ValueError):
        return None


def _fetch_via_fixture(cfg: SpikeConfig) -> str:
    """Offline backend: return the fixture file's contents (no browser, no network)."""
    if cfg.fixture is None:
        raise ValueError("fixture mode requires --fixture PATH")
    return cfg.fixture.read_text(encoding="utf-8")


def _fetch_via_browser(cfg: SpikeConfig) -> str:
    """Browser backend: load the notices page in a real Chromium and return the rendered HTML.

    Imported lazily so the offline ``fixture`` mode needs no Playwright install. ``headless-shell``
    leaves ``channel`` unset (Playwright then uses ``chromium-headless-shell``); ``xvfb-headful``
    launches full Chromium with basic stealth and expects an X server (wrap in ``xvfb-run``).
    """
    from playwright.sync_api import sync_playwright  # lazy: only needed for real fetches

    launch_args = ["--disable-dev-shm-usage", "--disable-gpu"]
    with sync_playwright() as p:
        if cfg.mode == "xvfb-headful":
            browser = p.chromium.launch(headless=False, channel="chromium", args=launch_args)
            context = browser.new_context(
                user_agent=(
                    "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 "
                    "(KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
                ),
                viewport={"width": 1280, "height": 800},
            )
        else:
            browser = p.chromium.launch(headless=True, args=launch_args)
            context = browser.new_context()
        try:
            page = context.new_page()
            page.goto(cfg.url, wait_until="domcontentloaded", timeout=cfg.nav_timeout_ms)
            try:
                page.wait_for_selector(cfg.gate_selector, timeout=cfg.nav_timeout_ms)
            except Exception:  # noqa: BLE001 — a missing selector is a blocked cycle, not a crash
                return page.content()
            return page.content()
        finally:
            browser.close()


def run_spike(cfg: SpikeConfig) -> SpikeReport:
    """Run ``cfg.cycles`` fetch attempts and return the aggregated :class:`SpikeReport`.

    Each cycle fetches the page (via browser or fixture), records whether the gate markup appeared,
    and — in browser modes — launches then closes a fresh browser so the run mirrors the
    launch-per-poll production design and its memory profile. A single cycle's failure never aborts
    the run; it is recorded as a blocked cycle so the success rate and streak stay meaningful.
    """
    fetch = _fetch_via_fixture if cfg.mode == "fixture" else _fetch_via_browser
    per_cycle: list[bool] = []
    for _ in range(cfg.cycles):
        try:
            html = fetch(cfg)
            per_cycle.append(_gate_present(html))
        except Exception as exc:  # noqa: BLE001 — a failed fetch is a blocked cycle
            print(f"cycle error: {exc}", file=sys.stderr)
            per_cycle.append(False)
        time.sleep(0)  # placeholder; real cadence is the production poll interval, not the spike
    peak = _read_peak_rss(cfg.cgroup_peak)
    return SpikeReport(mode=cfg.mode, per_cycle=per_cycle, peak_rss_bytes=peak)


def _parse_args(argv: list[str] | None) -> SpikeConfig:
    parser = argparse.ArgumentParser(description="advisory-fetcher Akamai/memory spike")
    parser.add_argument(
        "--mode", choices=["headless-shell", "xvfb-headful", "fixture"], default="headless-shell"
    )
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--gate-selector", default=DEFAULT_GATE_SELECTOR)
    parser.add_argument("--cycles", type=int, default=10)
    parser.add_argument("--fixture", type=Path, default=None)
    parser.add_argument("--cgroup-peak", type=Path, default=Path(DEFAULT_CGROUP_PEAK))
    parser.add_argument("--nav-timeout-ms", type=int, default=DEFAULT_NAV_TIMEOUT_MS)
    args = parser.parse_args(argv)
    return SpikeConfig(
        mode=args.mode,
        url=args.url,
        gate_selector=args.gate_selector,
        cycles=args.cycles,
        fixture=args.fixture,
        cgroup_peak=args.cgroup_peak,
        nav_timeout_ms=args.nav_timeout_ms,
    )


def main(argv: list[str] | None = None) -> int:
    """CLI entry point: run the spike and print the report; exit non-zero on a total miss."""
    cfg = _parse_args(argv)
    report = run_spike(cfg)
    print(report.to_text())
    # A run that never saw the markup is a hard miss; a partial run leaves the go/no-go to a human.
    return 0 if report.max_consecutive > 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
