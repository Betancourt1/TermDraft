"""Tests for CLI validation without starting an interactive terminal."""

from __future__ import annotations

from pathlib import Path

import pytest

from termwriter.app import TermWriterApp
from termwriter.cli import main


def test_cli_reports_missing_path(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    result = main([str(tmp_path / "missing")])

    assert result == 2
    assert "does not exist" in capsys.readouterr().err


def test_cli_launches_valid_workspace(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    launched: list[Path] = []

    def fake_run(app: TermWriterApp) -> None:
        launched.append(app.workspace.root)

    monkeypatch.setattr(TermWriterApp, "run", fake_run)

    result = main([str(tmp_path)])

    assert result == 0
    assert launched == [tmp_path.resolve()]
