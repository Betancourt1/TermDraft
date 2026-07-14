"""Keyboard-first COMMAND and WRITE mode behavior."""

from __future__ import annotations

from pathlib import Path

from textual.widgets import Input

from termdraft.app import TermDraftApp
from termdraft.bindings import format_shortcut_help
from termdraft.config import load_config
from termdraft.models.workspace import Workspace
from termdraft.screens.dialogs import FileSearchDialog
from termdraft.services.recovery import RecoveryJournal


def _app(path: Path, *, config_root: Path | None = None) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(path.parent / ".test-recovery"),
        config=None if config_root is None else load_config(config_root),
    )


async def test_command_navigation_moves_without_changing_source(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("alpha\nbeta\ncharlie", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((1, 2))

        await pilot.press("h")
        assert app.editor.cursor_location == (1, 1)
        await pilot.press("l", "j")
        assert app.editor.cursor_location == (2, 2)
        await pilot.press("k", "0")
        assert app.editor.cursor_location == (1, 0)
        await pilot.press("$")
        assert app.editor.cursor_location == (1, 4)
        await pilot.press("g")
        assert app.editor.cursor_location == (0, 0)
        await pilot.press("G")
        assert app.editor.cursor_location == (2, 7)

        assert app.editor.text == "alpha\nbeta\ncharlie"
        assert app.document is not None and not app.document.dirty


async def test_write_mode_inserts_navigation_letters_normally(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "h", "j", "k", "l")

        assert app.editor.text == "hjklsource"


async def test_command_navigation_remap_replaces_default_key(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\ncommand_cursor_left = "a"\n',
        encoding="utf-8",
    )
    app = _app(path, config_root=config_root)
    help_text = format_shortcut_help(app.config.keybindings)

    assert "a" in help_text
    assert "Move left" in help_text

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((0, 3))
        await pilot.press("h")
        assert app.editor.cursor_location == (0, 3)
        await pilot.press("a")
        assert app.editor.cursor_location == (0, 2)


async def test_command_navigation_does_not_steal_modal_input(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+p")
        assert isinstance(app.screen, FileSearchDialog)

        await pilot.press("j")

        assert app.screen.query_one(Input).value == "j"


async def test_write_startup_mode_accepts_text_without_mode_switch(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[editor]\nstartup_mode = "write"\n',
        encoding="utf-8",
    )
    app = _app(path, config_root=config_root)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")

        assert app.editor.text == "xsource"
        assert app._focus_mode() == "WRITE"


async def test_modifier_undo_and_redo_work_in_command_mode(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "escape", "ctrl+z")

        assert app.editor.text == "base"
        assert app._focus_mode() == "COMMAND"

        await pilot.press("ctrl+y")
        assert app.editor.text == "xbase"

        await pilot.press("backspace", "delete", "x")
        assert app.editor.text == "xbase"

        await pilot.press("u")
        assert app.editor.text == "base"
        await pilot.press("r")
        assert app.editor.text == "xbase"


async def test_remapped_modifier_undo_and_redo_work_in_command_mode(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\nundo = "ctrl+u"\nredo = "ctrl+shift+r"\n',
        encoding="utf-8",
    )
    app = _app(path, config_root=config_root)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "escape", "ctrl+z")
        assert app.editor.text == "xbase"

        await pilot.press("ctrl+u")
        assert app.editor.text == "base"

        await pilot.press("ctrl+shift+r")
        assert app.editor.text == "xbase"
