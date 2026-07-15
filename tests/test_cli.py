"""Tests for CLI validation without starting an interactive terminal."""

from __future__ import annotations

import signal
from pathlib import Path

import pytest

from termdraft.app import TermDraftApp
from termdraft.cli import SignalHandler, _run_with_shutdown_signals, main
from termdraft.models.workspace import Workspace


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

    def fake_run(app: TermDraftApp) -> None:
        launched.append(app.workspace.root)

    monkeypatch.setattr(TermDraftApp, "run", fake_run)

    result = main(["--config-dir", str(tmp_path / "config"), str(tmp_path)])

    assert result == 0
    assert launched == [tmp_path.resolve()]


def test_cli_safe_mode_ignores_only_the_user_theme(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        "[editor]\nsoft_wrap = false\n",
        encoding="utf-8",
    )
    (config_root / "theme.tcss").write_text(
        "#title-bar { background: #010203; }\n",
        encoding="utf-8",
    )
    launched: list[TermDraftApp] = []

    def fake_run(app: TermDraftApp) -> None:
        launched.append(app)

    monkeypatch.setattr(TermDraftApp, "run", fake_run)

    result = main(["--config-dir", str(config_root), "--safe-mode", str(tmp_path)])

    assert result == 0
    assert len(launched) == 1
    assert not launched[0].config.editor.soft_wrap
    assert launched[0]._theme_warning is None


def test_cli_forwards_shutdown_signal_and_restores_handlers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app = TermDraftApp(Workspace.from_target(tmp_path))
    calls: list[tuple[int, SignalHandler]] = []

    def fake_signal(signal_number: int, handler: SignalHandler) -> SignalHandler:
        calls.append((signal_number, handler))
        return signal.SIG_IGN

    def failed_run(running_app: TermDraftApp) -> None:
        installed_handler = calls[0][1]
        assert callable(installed_handler)
        installed_handler(signal.SIGTERM, None)
        assert running_app is app
        raise RuntimeError("run failed")

    monkeypatch.setattr("termdraft.cli.signal.signal", fake_signal)
    monkeypatch.setattr(TermDraftApp, "run", failed_run)

    with pytest.raises(RuntimeError, match="run failed"):
        _run_with_shutdown_signals(app)

    signal_count = 1 + int(getattr(signal, "SIGHUP", None) is not None)
    assert app._pending_shutdown_signal == signal.SIGTERM
    assert len(calls) == signal_count * 2
    assert all(handler == signal.SIG_IGN for _, handler in calls[signal_count:])


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
    assert "TermDraft commands" in output
    assert "Ctrl+G" in output
    assert "Open the command palette" in output
    assert "Search workspace text (literal / fuzzy / word / regex)" in output
    assert "Search headings in the active document" in output
    assert "manage recovery drafts" in output
    assert "inspect semantic blocks" in output
    assert "read semantic blocks experimentally" in output
    assert "Modes and COMMAND keys" in output
    assert "Focused Files keys" in output
    assert "Copy the selected file or folder" in output
    assert "Paste into the selected folder" in output
    assert "Enter COMMAND mode" in output
    assert "Enter WRITE mode" in output
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
