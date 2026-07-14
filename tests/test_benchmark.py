"""Contracts for the repeatable development benchmark CLI."""

import json
import math
import tracemalloc
from dataclasses import asdict

import pytest

from termdraft.benchmark import (
    BenchmarkConfig,
    build_parser,
    make_markdown,
    run_benchmarks,
    summarize_timings,
)


def test_workload_is_deterministic_and_meets_requested_size() -> None:
    first = make_markdown(2048)
    second = make_markdown(2048)

    assert first == second
    assert len(first.encode("utf-8")) >= 2048
    assert "日本語" in first
    assert "```python" in first
    assert "[^note]" in first


def test_timing_summary_uses_median_and_nearest_rank_p95() -> None:
    summary = summarize_timings([0.004, 0.001, 0.003, 0.002, 0.020])

    assert summary.samples == 5
    assert summary.minimum_ms == 1
    assert summary.median_ms == 3
    assert summary.p95_ms == 20


@pytest.mark.parametrize(
    "arguments",
    (
        ["--semantic-kib", "0"],
        ["--tab-kib", "-1"],
        ["--tabs", "1"],
        ["--tabs", "101"],
        ["--watch-kib", "0"],
        ["--iterations", "0"],
        ["--warmup", "-1"],
    ),
)
def test_parser_rejects_invalid_bounds(arguments: list[str]) -> None:
    with pytest.raises(SystemExit):
        build_parser().parse_args(arguments)


async def test_tiny_end_to_end_report_is_finite_and_serializable() -> None:
    config = BenchmarkConfig(
        semantic_kib=1,
        tab_kib=1,
        tabs=2,
        watch_kib=1,
        iterations=1,
        warmup=0,
    )

    owns_tracing = not tracemalloc.is_tracing()
    if owns_tracing:
        tracemalloc.start()
    try:
        report = await run_benchmarks(config)
        assert tracemalloc.is_tracing()
    finally:
        if owns_tracing:
            tracemalloc.stop()
    payload = asdict(report)
    encoded = json.dumps(payload, allow_nan=False)

    assert encoded
    assert payload["config"]["tabs"] == 2
    assert payload["semantic"]["timing"]["samples"] == 1
    assert payload["mounted_tabs"]["tabs"] == 2
    assert payload["watcher"]["files_per_pass"] == 2
    assert payload["watcher"]["timing"]["samples"] == 1
    assert _all_numbers_are_finite(payload)


def _all_numbers_are_finite(value: object) -> bool:
    if isinstance(value, dict):
        return all(_all_numbers_are_finite(item) for item in value.values())
    if isinstance(value, list):
        return all(_all_numbers_are_finite(item) for item in value)
    if isinstance(value, float):
        return math.isfinite(value) and value >= 0
    return True
