"""Repeatable development benchmarks for TermWriter's real coordination paths."""

from __future__ import annotations

import argparse
import asyncio
import gc
import json
import math
import platform
import resource
import statistics
import sys
import tempfile
import time
import tracemalloc
from dataclasses import asdict, dataclass
from pathlib import Path

import textual

from termwriter import __version__
from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.services.recovery import RecoveryJournal
from termwriter.services.semantic_blocks import map_semantic_blocks
from termwriter.services.session import SessionStore

_MEBIBYTE = 1024 * 1024
_MARKDOWN_SAMPLE = """\
# Benchmark heading

Unicode paragraph: café, 日本語, 👩🏽‍💻, and an [inert link][reference].

> [!NOTE]
> Alert body with **bold** text.

- first item
  - nested item

| Column | Value |
|---|---:|
| one | 1 |

Term
: Definition body

```python
print("source is inert")
```

Footnote reference[^note].

[^note]: Footnote body.
[reference]: https://example.invalid/

"""


@dataclass(frozen=True, slots=True)
class BenchmarkConfig:
    """Sizes and sample counts for one benchmark run."""

    semantic_kib: int = 256
    tab_kib: int = 32
    tabs: int = 10
    watch_kib: int = 256
    iterations: int = 20
    warmup: int = 1

    def __post_init__(self) -> None:
        if min(self.semantic_kib, self.tab_kib, self.watch_kib, self.iterations) <= 0:
            raise ValueError("sizes and iterations must be positive")
        if not 2 <= self.tabs <= 100:
            raise ValueError("tabs must be between 2 and 100")
        if self.warmup < 0:
            raise ValueError("warmup cannot be negative")


@dataclass(frozen=True, slots=True)
class TimingSummary:
    """Stable summary without machine-specific pass/fail thresholds."""

    samples: int
    minimum_ms: float
    median_ms: float
    p95_ms: float


@dataclass(frozen=True, slots=True)
class SemanticBenchmark:
    source_bytes: int
    blocks: int
    timing: TimingSummary
    median_mib_per_second: float


@dataclass(frozen=True, slots=True)
class TabMemoryBenchmark:
    tabs: int
    bytes_per_tab_source: int
    one_tab_traced_bytes: int
    all_tabs_traced_bytes: int
    added_tabs_traced_bytes: int
    traced_bytes_per_added_tab: float
    traced_peak_growth_bytes: int
    process_peak_rss_before_bytes: int
    process_peak_rss_after_bytes: int
    process_peak_rss_high_water_delta_bytes: int
    mount_seconds: float


@dataclass(frozen=True, slots=True)
class WatcherBenchmark:
    files_per_pass: int
    bytes_per_pass: int
    timing: TimingSummary
    median_passes_per_second: float


@dataclass(frozen=True, slots=True)
class BenchmarkReport:
    termwriter_version: str
    textual_version: str
    python_version: str
    platform: str
    config: BenchmarkConfig
    semantic: SemanticBenchmark
    mounted_tabs: TabMemoryBenchmark
    watcher: WatcherBenchmark


def make_markdown(target_bytes: int) -> str:
    """Build deterministic Markdown at least as large as the requested byte count."""
    if target_bytes <= 0:
        raise ValueError("target_bytes must be positive")
    sample_bytes = len(_MARKDOWN_SAMPLE.encode("utf-8"))
    repetitions = math.ceil(target_bytes / sample_bytes)
    return _MARKDOWN_SAMPLE * repetitions


def summarize_timings(samples: list[float]) -> TimingSummary:
    """Summarize elapsed seconds using a nearest-rank p95."""
    if not samples:
        raise ValueError("at least one timing sample is required")
    ordered = sorted(samples)
    p95_index = math.ceil(0.95 * len(ordered)) - 1
    return TimingSummary(
        samples=len(samples),
        minimum_ms=ordered[0] * 1000,
        median_ms=statistics.median(ordered) * 1000,
        p95_ms=ordered[p95_index] * 1000,
    )


async def run_benchmarks(config: BenchmarkConfig) -> BenchmarkReport:
    """Run all benchmarks and return a JSON-serializable report."""
    semantic = _benchmark_semantic(config)
    with tempfile.TemporaryDirectory(prefix="termwriter-benchmark-") as temporary:
        root = Path(temporary)
        mounted_tabs = await _benchmark_mounted_tabs(config, root / "tabs")
        watcher = await _benchmark_watcher(config, root / "watcher")
    return BenchmarkReport(
        termwriter_version=__version__,
        textual_version=textual.__version__,
        python_version=platform.python_version(),
        platform=platform.platform(),
        config=config,
        semantic=semantic,
        mounted_tabs=mounted_tabs,
        watcher=watcher,
    )


def _benchmark_semantic(config: BenchmarkConfig) -> SemanticBenchmark:
    source = make_markdown(config.semantic_kib * 1024)
    for _ in range(config.warmup):
        map_semantic_blocks(source)

    samples: list[float] = []
    block_count = 0
    for _ in range(config.iterations):
        started = time.perf_counter()
        mapping = map_semantic_blocks(source)
        samples.append(time.perf_counter() - started)
        block_count = len(mapping.blocks)
    timing = summarize_timings(samples)
    source_bytes = len(source.encode("utf-8"))
    median_seconds = timing.median_ms / 1000
    return SemanticBenchmark(
        source_bytes=source_bytes,
        blocks=block_count,
        timing=timing,
        median_mib_per_second=(source_bytes / _MEBIBYTE) / median_seconds,
    )


async def _benchmark_mounted_tabs(config: BenchmarkConfig, root: Path) -> TabMemoryBenchmark:
    root.mkdir(parents=True)
    source = make_markdown(config.tab_kib * 1024)
    paths = [root / f"document-{index:03}.md" for index in range(config.tabs)]
    for path in paths:
        path.write_text(source, encoding="utf-8", newline="")

    app = _benchmark_app(paths[0], root)
    owns_tracing = not tracemalloc.is_tracing()
    if owns_tracing:
        tracemalloc.start()
    traced_baseline, traced_peak_baseline = tracemalloc.get_traced_memory()
    rss_before = _process_peak_rss_bytes()
    try:
        async with app.run_test(size=(100, 30)):
            await _wait_for_idle(app)
            gc.collect()
            one_tab_total, _ = tracemalloc.get_traced_memory()
            one_tab_bytes = max(0, one_tab_total - traced_baseline)
            started = time.perf_counter()
            for path in paths[1:]:
                worker = app._open_file_now(path)
                if worker is None:
                    raise RuntimeError(f"could not start opening benchmark tab {path.name}")
                await worker.wait()
                await _wait_for_idle(app)
            mount_seconds = time.perf_counter() - started
            gc.collect()
            all_tabs_total, traced_peak_total = tracemalloc.get_traced_memory()
            all_tabs_bytes = max(0, all_tabs_total - traced_baseline)
            traced_peak_growth = max(0, traced_peak_total - traced_peak_baseline)
            if len(app._open_documents) != config.tabs:
                raise RuntimeError("benchmark did not mount every requested tab")
    finally:
        if owns_tracing:
            tracemalloc.stop()

    added_tabs_bytes = max(0, all_tabs_bytes - one_tab_bytes)
    rss_after = _process_peak_rss_bytes()
    return TabMemoryBenchmark(
        tabs=config.tabs,
        bytes_per_tab_source=len(source.encode("utf-8")),
        one_tab_traced_bytes=one_tab_bytes,
        all_tabs_traced_bytes=all_tabs_bytes,
        added_tabs_traced_bytes=added_tabs_bytes,
        traced_bytes_per_added_tab=added_tabs_bytes / (config.tabs - 1),
        traced_peak_growth_bytes=traced_peak_growth,
        process_peak_rss_before_bytes=rss_before,
        process_peak_rss_after_bytes=rss_after,
        process_peak_rss_high_water_delta_bytes=max(0, rss_after - rss_before),
        mount_seconds=mount_seconds,
    )


async def _benchmark_watcher(config: BenchmarkConfig, root: Path) -> WatcherBenchmark:
    root.mkdir(parents=True)
    source = make_markdown(config.watch_kib * 1024)
    paths = (root / "active.md", root / "inactive.md")
    for path in paths:
        path.write_text(source, encoding="utf-8", newline="")

    app = _benchmark_app(paths[0], root)
    async with app.run_test(size=(100, 30)):
        await _wait_for_idle(app)
        worker = app._open_file_now(paths[1])
        if worker is None:
            raise RuntimeError("could not mount the inactive watcher tab")
        await worker.wait()
        await _wait_for_idle(app)

        for _ in range(config.warmup):
            await _run_watcher_pass(app)
        samples: list[float] = []
        for _ in range(config.iterations):
            started = time.perf_counter()
            await _run_watcher_pass(app)
            samples.append(time.perf_counter() - started)

    timing = summarize_timings(samples)
    median_seconds = timing.median_ms / 1000
    return WatcherBenchmark(
        files_per_pass=2,
        bytes_per_pass=sum(path.stat().st_size for path in paths),
        timing=timing,
        median_passes_per_second=1 / median_seconds,
    )


def _benchmark_app(initial_path: Path, state_root: Path) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(initial_path),
        preview_debounce=3600,
        external_poll_interval=3600,
        recovery_journal=RecoveryJournal(state_root / "recovery"),
        session_store=SessionStore(state_root / "sessions"),
    )


async def _wait_for_idle(app: TermWriterApp) -> None:
    for _ in range(2000):
        if (
            not app._critical_io
            and not app._session_save_in_flight
            and app._pending_session_state is None
        ):
            return
        await asyncio.sleep(0.001)
    raise RuntimeError("TermWriter did not become idle during the benchmark")


async def _run_watcher_pass(app: TermWriterApp) -> None:
    app._check_external_in_background()
    worker = app._watch_probe_worker
    if worker is None or not worker.is_running:
        raise RuntimeError("watcher benchmark pass did not start")
    await worker.wait()
    await asyncio.sleep(0)


def _process_peak_rss_bytes() -> int:
    peak = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    return peak if sys.platform == "darwin" else peak * 1024


def build_parser() -> argparse.ArgumentParser:
    defaults = BenchmarkConfig()
    parser = argparse.ArgumentParser(
        prog="termwriter-benchmark",
        description="Measure TermWriter semantic parsing, mounted tabs, and watcher passes.",
    )
    parser.add_argument("--semantic-kib", type=_positive_int, default=defaults.semantic_kib)
    parser.add_argument("--tab-kib", type=_positive_int, default=defaults.tab_kib)
    parser.add_argument("--tabs", type=_tab_count, default=defaults.tabs)
    parser.add_argument("--watch-kib", type=_positive_int, default=defaults.watch_kib)
    parser.add_argument("--iterations", type=_positive_int, default=defaults.iterations)
    parser.add_argument("--warmup", type=_nonnegative_int, default=defaults.warmup)
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = build_parser().parse_args(argv)
    config = BenchmarkConfig(**vars(arguments))
    report = asyncio.run(run_benchmarks(config))
    print(json.dumps(asdict(report), indent=2, sort_keys=True, allow_nan=False))
    return 0


def _positive_int(value: str) -> int:
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def _nonnegative_int(value: str) -> int:
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("cannot be negative")
    return number


def _tab_count(value: str) -> int:
    number = int(value)
    if not 2 <= number <= 100:
        raise argparse.ArgumentTypeError("must be between 2 and 100")
    return number


if __name__ == "__main__":  # pragma: no cover - console entry point
    raise SystemExit(main())
