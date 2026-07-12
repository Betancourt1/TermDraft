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
    result = main(["--config-dir", str(tmp_path / "config"), str(tmp_path / "missing")])

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

    result = main(["--config-dir", str(tmp_path / "config"), str(tmp_path)])

    assert result == 0
    assert launched == [tmp_path.resolve()]


def test_cli_initializes_and_prints_configuration_paths(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config_root = tmp_path / "config"

    result = main(["--config-dir", str(config_root), "--init-config"])

    assert result == 0
    assert (config_root / "config.toml").is_file()
    assert (config_root / "theme.tcss").is_file()
    output = capsys.readouterr().out
    assert str(config_root / "config.toml") in output
    assert str(config_root / "theme.tcss") in output

    result = main(["--config-dir", str(config_root), "--config-path"])
    assert result == 0
    assert capsys.readouterr().out.splitlines() == [
        str(config_root / "config.toml"),
        str(config_root / "theme.tcss"),
    ]


def test_cli_commands_show_effective_remapped_keys(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[editor]\nauto_continue_lists = false\n\n[keybindings]\nsave = "ctrl+g"\n',
        encoding="utf-8",
    )

    result = main(["--config-dir", str(config_root), "--commands"])

    assert result == 0
    output = capsys.readouterr().out
    assert "TermWriter commands" in output
    assert "Ctrl+G" in output
    assert "Open the command palette" in output
    assert "Search workspace text (literal / fuzzy / word / regex)" in output
    assert "manage recovery drafts" in output
    assert "inspect semantic blocks" in output
    assert "Esc in editor" in output
    assert "Tab / Shift+Tab in preview" in output
    assert "Select links or leave the preview" in output
    assert "Enter in preview" in output
    assert "Alt+Down" in output
    assert "next heading in the focused preview" in output
    assert "Alt+Up" in output
    assert "Enter in a list" not in output


def test_cli_rejects_invalid_configuration_before_launch(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\nsave = "ctrl+q"\n',
        encoding="utf-8",
    )

    result = main(["--config-dir", str(config_root), str(tmp_path)])

    assert result == 2
    assert "configuration error" in capsys.readouterr().err
