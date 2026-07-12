"""Functional tests for customization, Markdown editing, and command discovery."""

from __future__ import annotations

from pathlib import Path

from textual.color import Color

from termwriter.app import TermWriterApp
from termwriter.config import EditorConfig, TermWriterConfig, load_config
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import HelpDialog
from termwriter.services.recovery import RecoveryJournal


def _app(path: Path, config: TermWriterConfig | None = None) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(path.parent / ".test-recovery"),
        config=config,
    )


async def test_enter_continues_task_list_and_undo_reverts_the_whole_edit(tmp_path: Path) -> None:
    path = tmp_path / "tasks.md"
    path.write_text("- [x] done", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((0, len("- [x] done")))
        await pilot.press("enter")

        assert app.editor.text == "- [x] done\n- [ ] "
        assert app.document is not None
        assert app.document.text == "- [x] done\n- [ ] "

        await pilot.press("ctrl+z")
        assert app.editor.text == "- [x] done"
        assert not app.document.dirty


async def test_enter_on_empty_marker_ends_the_list(tmp_path: Path) -> None:
    path = tmp_path / "list.md"
    path.write_text("- item\n- ", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((1, 2))
        await pilot.press("enter")

        assert app.editor.text == "- item\n\n"
        assert app.editor.cursor_location == (2, 0)


async def test_list_continuation_can_be_disabled(tmp_path: Path) -> None:
    path = tmp_path / "list.md"
    path.write_text("- item", encoding="utf-8")
    config = TermWriterConfig(
        root=tmp_path / "config",
        editor=EditorConfig(auto_continue_lists=False),
    )
    app = _app(path, config)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((0, len("- item")))
        await pilot.press("enter")

        assert app.editor.text == "- item\n"


async def test_remapped_save_and_undo_replace_their_default_keys(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\nsave = "ctrl+g"\nundo = "ctrl+u"\n',
        encoding="utf-8",
    )
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+z")
        assert app.editor.text == "xbase"
        assert path.read_text(encoding="utf-8") == "base"

        await pilot.press("ctrl+u")
        assert app.editor.text == "base"

        await pilot.press("y", "ctrl+s")
        assert path.read_text(encoding="utf-8") == "base"

        await pilot.press("ctrl+g")
        assert path.read_text(encoding="utf-8") == "ybase"


async def test_configuration_reload_updates_keys_and_editor_options(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    config_path = config_root / "config.toml"
    config_path.write_text('[keybindings]\nsave = "ctrl+g"\n', encoding="utf-8")
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)) as pilot:
        config_path.write_text(
            """\
[editor]
auto_continue_lists = false
soft_wrap = false
show_line_numbers = false

[keybindings]
save = "ctrl+r"
""",
            encoding="utf-8",
        )
        app.action_reload_config()

        assert app.config.keybindings["save"] == "ctrl+r"
        assert not app.editor.auto_continue_lists
        assert not app.editor.soft_wrap
        assert not app.editor.show_line_numbers

        await pilot.press("x", "ctrl+g")
        assert path.read_text(encoding="utf-8") == "base"
        await pilot.press("ctrl+r")
        assert path.read_text(encoding="utf-8") == "xbase"


async def test_user_theme_overrides_bundled_css(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "theme.tcss").write_text(
        "#title-bar { background: #010203; }\n",
        encoding="utf-8",
    )
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)):
        title = app.query_one("#title-bar")
        assert title.styles.background == Color(1, 2, 3)


async def test_command_palette_and_help_expose_product_actions(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        command_titles = {command.title for command in app.get_system_commands(app.screen)}
        assert {
            "Save document",
            "Reload configuration",
            "Shortcut help",
            "Markdown syntax help",
            "Quit safely",
        } <= command_titles

        await pilot.press("ctrl+backslash")
        assert app.screen.id == "--command-palette"
        await pilot.press("escape")

        app.action_show_markdown_help()
        assert isinstance(app.screen, HelpDialog)
        assert app.screen.dialog_title == "Markdown syntax"
        assert "double underscores mean bold" in app.screen.content
