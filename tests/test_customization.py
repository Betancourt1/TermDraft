"""Functional tests for customization, Markdown editing, and command discovery."""

from __future__ import annotations

from pathlib import Path

from textual.color import Color
from textual.widgets import Input, Static

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


async def test_enter_after_thematic_break_does_not_start_a_list(tmp_path: Path) -> None:
    path = tmp_path / "rule.md"
    path.write_text("* * *", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((0, len("* * *")))
        await pilot.press("enter")

        assert app.editor.text == "* * *\n"


async def test_enter_does_not_continue_indented_code_but_keeps_nested_lists(
    tmp_path: Path,
) -> None:
    code_path = tmp_path / "code.md"
    code_path.write_text("    - literal", encoding="utf-8")
    code_app = _app(code_path)

    async with code_app.run_test(size=(100, 30)) as pilot:
        code_app.editor.move_cursor((0, len("    - literal")))
        await pilot.press("enter")
        assert code_app.editor.text == "    - literal\n"

    list_path = tmp_path / "list.md"
    list_path.write_text("- parent\n    - child", encoding="utf-8")
    list_app = _app(list_path)

    async with list_app.run_test(size=(100, 30)) as pilot:
        list_app.editor.move_cursor((1, len("    - child")))
        await pilot.press("enter")
        assert list_app.editor.text == "- parent\n    - child\n    - "


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
        commands = {command.title: command for command in app.get_system_commands(app.screen)}
        assert commands["Save document"].help.startswith("Keys: w · Ctrl+G  ·  ")


async def test_write_and_command_modes_protect_text_and_use_plain_keys(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        status = app.query_one("#status-bar", Static)
        assert str(status.render()).startswith("WRITE")

        await pilot.press("x", "escape", "z")
        assert app.editor.text == "xbase"
        assert str(status.render()).startswith("COMMAND")

        await pilot.press("u")
        assert app.editor.text == "base"

        await pilot.press("i", "w")
        assert app.editor.text == "wbase"
        assert str(status.render()).startswith("WRITE")


async def test_command_mode_arrows_navigate_without_modifying_text(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("one\nthree", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("escape", "right", "right", "down", "left")

        assert app.editor.cursor_location == (1, 1)
        assert app.editor.text == "one\nthree"

        await pilot.press("backspace", "delete", "enter", "x")
        assert app.editor.text == "one\nthree"


async def test_command_mode_keys_pause_while_a_dialog_accepts_text(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("escape", "f")
        await pilot.press("w", "q", "e", "n", "o")

        assert app.screen.id == "file-search-screen"
        assert app.screen.query_one("#search-input", Input).value == "wqeno"


async def test_command_mode_plain_w_saves_the_current_document(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "escape", "w")
        for _ in range(100):
            if path.read_text(encoding="utf-8") == "xbase":
                break
            await pilot.pause(0.01)

        assert path.read_text(encoding="utf-8") == "xbase"


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
    theme_path = config_root / "theme.tcss"
    theme_path.write_text(
        "#title-bar { background: #010203; }\n",
        encoding="utf-8",
    )
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)) as pilot:
        title = app.query_one("#title-bar")
        assert title.styles.background == Color(1, 2, 3)

        theme_path.write_text(
            "#title-bar { background: #040506; }\n",
            encoding="utf-8",
        )
        await pilot.pause(0.35)
        assert title.styles.background == Color(4, 5, 6)


async def test_command_palette_and_help_expose_product_actions(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        command_titles = {command.title for command in app.get_system_commands(app.screen)}
        assert {
            "Save document",
            "Search workspace text",
            "Reload configuration",
            "Manage recovery drafts",
            "Shortcut help",
            "Markdown syntax help",
            "Quit safely",
        } <= command_titles
        commands = {command.title: command for command in app.get_system_commands(app.screen)}
        assert commands["Save document"].help.startswith("Keys: w · Ctrl+S  ·  ")
        assert commands["Shortcut help"].help.startswith("Keys: ? · F1  ·  ")
        assert commands["Reload configuration"].help.startswith("Keys: Palette only  ·  ")
        assert all(command.help.startswith("Keys: ") for command in commands.values())

        await pilot.press("ctrl+backslash")
        assert app.screen.id == "--command-palette"
        await pilot.press("escape")

        app.action_show_markdown_help()
        assert isinstance(app.screen, HelpDialog)
        assert app.screen.dialog_title == "Markdown syntax"
        assert "double underscores mean bold" in app.screen.content
