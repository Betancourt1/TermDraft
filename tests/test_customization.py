"""Functional tests for customization, Markdown editing, and command discovery."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from textual.color import Color
from textual.command import CommandInput, SearchIcon
from textual.filter import Monochrome
from textual.pilot import Pilot
from textual.widgets import Input, OptionList, Static

from termdraft.app import TermDraftApp
from termdraft.config import EditorConfig, TermDraftConfig, load_config
from termdraft.icons import SEARCH_ICON, SEARCH_ICON_COLOR
from termdraft.models.workspace import Workspace
from termdraft.screens.dialogs import HelpDialog
from termdraft.services.recovery import RecoveryJournal


def _app(
    path: Path,
    config: TermDraftConfig | None = None,
    *,
    use_user_theme: bool = True,
) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(path.parent / ".test-recovery"),
        config=config,
        use_user_theme=use_user_theme,
    )


async def _wait_until(pilot: Pilot[None], condition: Callable[[], bool]) -> None:
    for _ in range(200):
        if condition():
            return
        await pilot.pause(0.01)
    raise AssertionError("condition did not become true")


def test_default_theme_is_black_and_grayscale(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    theme = app.current_theme
    colors = (
        theme.primary,
        theme.secondary,
        theme.warning,
        theme.error,
        theme.success,
        theme.accent,
        theme.foreground,
        theme.background,
        theme.surface,
        theme.panel,
        theme.boost,
    )
    parsed_colors = [Color.parse(color) for color in colors if color is not None]

    assert theme.name == "termdraft-monochrome"
    assert theme.background == "#000000"
    assert len(parsed_colors) == len(colors)
    assert all(color.r == color.g == color.b for color in parsed_colors)
    assert any(isinstance(line_filter, Monochrome) for line_filter in app.get_line_filters())


def test_preview_headings_have_readable_monochrome_colors(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("# Heading", encoding="utf-8")
    app = _app(path)

    variables = app.current_theme.to_color_system().generate()

    assert variables["markdown-h1-color"] == "#e6e6e6"
    assert variables["markdown-h2-color"] == "#d0d0d0"
    assert variables["markdown-h3-color"] == "#b8b8b8"


async def test_enter_continues_task_list_and_undo_reverts_the_whole_edit(tmp_path: Path) -> None:
    path = tmp_path / "tasks.md"
    path.write_text("- [x] done", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i")
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
        await pilot.press("i")
        app.editor.move_cursor((1, 2))
        await pilot.press("enter")

        assert app.editor.text == "- item\n\n"
        assert app.editor.cursor_location == (2, 0)


async def test_enter_after_thematic_break_does_not_start_a_list(tmp_path: Path) -> None:
    path = tmp_path / "rule.md"
    path.write_text("* * *", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i")
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
        await pilot.press("i")
        code_app.editor.move_cursor((0, len("    - literal")))
        await pilot.press("enter")
        assert code_app.editor.text == "    - literal\n"

    list_path = tmp_path / "list.md"
    list_path.write_text("- parent\n    - child", encoding="utf-8")
    list_app = _app(list_path)

    async with list_app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i")
        list_app.editor.move_cursor((1, len("    - child")))
        await pilot.press("enter")
        assert list_app.editor.text == "- parent\n    - child\n    - "


async def test_list_continuation_can_be_disabled(tmp_path: Path) -> None:
    path = tmp_path / "list.md"
    path.write_text("- item", encoding="utf-8")
    config = TermDraftConfig(
        root=tmp_path / "config",
        editor=EditorConfig(auto_continue_lists=False),
    )
    app = _app(path, config)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i")
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
        await pilot.press("i", "x", "ctrl+z")
        assert app.editor.text == "xbase"
        assert path.read_text(encoding="utf-8") == "base"

        await pilot.press("ctrl+u")
        assert app.editor.text == "base"

        await pilot.press("y", "ctrl+s")
        assert path.read_text(encoding="utf-8") == "base"

        await pilot.press("ctrl+g")
        assert path.read_text(encoding="utf-8") == "ybase"
        commands = {command.title: command for command in app.get_system_commands(app.screen)}
        assert commands["Save document"].help.startswith("Keys: w  ·  ")


async def test_write_and_command_modes_protect_text_and_use_plain_keys(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        status = app.query_one("#status-bar", Static)
        assert str(status.render()).startswith("COMMAND")

        await pilot.press("x", "z")
        assert app.editor.text == "base"
        assert str(status.render()).startswith("COMMAND")

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
        await pilot.press("i", "x", "escape", "w")
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
    config_path.write_text(
        '[editor]\nview_mode = "split"\n\n[keybindings]\nsave = "ctrl+g"\n',
        encoding="utf-8",
    )
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)) as pilot:
        config_path.write_text(
            """\
[editor]
auto_continue_lists = false
soft_wrap = false
show_line_numbers = false
view_mode = "inline"

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
        assert app.editor.inline_preview
        assert not app.preview.display
        assert not app.workbench_resize_handle.display

        await pilot.press("i", "x", "ctrl+g")
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
        expected_background = Color(4, 5, 6)
        for _ in range(200):
            if title.styles.background == expected_background:
                break
            await pilot.pause(0.01)
        else:
            raise AssertionError("user theme did not reload")


async def test_invalid_user_theme_is_ignored_with_warning(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    theme_path = config_root / "theme.tcss"
    theme_path.write_text("#title-bar { background: ; }\n", encoding="utf-8")
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)):
        notifications = list(app._notifications)

        assert app.document is not None
        assert len(notifications) == 1
        assert notifications[0].title == "Custom theme ignored"
        assert str(theme_path) in notifications[0].message
        assert "restart TermDraft" in notifications[0].message


async def test_safe_mode_ignores_theme_but_keeps_editor_config(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
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
    app = _app(path, load_config(config_root), use_user_theme=False)

    async with app.run_test(size=(100, 30)):
        title = app.query_one("#title-bar")

        assert title.styles.background != Color(1, 2, 3)
        assert not app.editor.soft_wrap
        assert app._theme_warning is None


async def test_command_palette_and_help_expose_product_actions(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        commands = {command.title: command for command in app.get_system_commands(app.screen)}
        expected_keys = {
            "Enter WRITE mode": "i",
            "Enter COMMAND mode": "Esc",
            "Save document": "w",
            "Save document as…": "W",
            "Duplicate document…": "D",
            "Find file": "f",
            "Open recent document": "o",
            "Next document tab": "]",
            "Previous document tab": "[",
            "Close document tab": "C",
            "Search workspace text": "/",
            "Find and replace in document": "s",
            "Open document outline": "S",
            "Toggle file explorer": "e",
            "Create file or folder": "a",
            "Copy file or folder": "c",
            "Cut file or folder": "x",
            "Paste file or folder": "p",
            "Rename file or folder": "r",
            "Move file or folder": "m",
            "Move file or folder to Trash": "d",
            "Toggle preview": "v",
            "Undo": "u",
            "Redo": "U",
            "Reload configuration": "R",
            "Manage recovery drafts": "M",
            "Shortcut help": "?",
            "Markdown syntax help": "K",
            "Inspect semantic blocks": "b",
            "Read semantic blocks (experimental)": "B",
            "Inspect cursor coordinates": "I",
            "Quit safely": "q",
        }
        assert commands.keys() == expected_keys.keys()
        assert all(
            commands[title].help.startswith(f"Keys: {key}  ·  ")
            for title, key in expected_keys.items()
        )
        assert all("Palette only" not in command.help for command in commands.values())

        await pilot.press("ctrl+backslash")
        assert app.screen.id == "--command-palette"
        search_icon = app.screen.query_one(SearchIcon)
        group_prompts = {
            group: [
                str(option.prompt) for option in app.screen.query_one(selector, OptionList).options
            ]
            for group, selector in {
                "document": "#--command-document",
                "navigate": "#--command-navigate",
                "files": "#--command-files",
                "mode": "#--command-mode",
                "edit": "#--command-edit",
                "view": "#--command-view",
            }.items()
        }
        assert group_prompts == {
            "document": [
                "w  Save",
                "W  Save as",
                "D  Duplicate",
                "f  Find file",
                "o  Recent documents",
                "C  Close tab",
            ],
            "navigate": [
                "]  Next tab",
                "[  Previous tab",
                "/  Search workspace",
                "s  Find and replace",
                "S  Outline",
                "e  Explorer",
            ],
            "files": [
                "a  Create",
                "c  Copy",
                "x  Cut",
                "p  Paste",
                "r  Rename",
                "m  Move",
                "d  Trash",
            ],
            "mode": [
                "i  Write mode",
                "Esc  Command mode",
            ],
            "edit": [
                "u  Undo",
                "U  Redo",
                "R  Reload config",
                "b  Inspect blocks",
                "B  Read blocks",
            ],
            "view": [
                "v  Preview",
                "M  Recovery drafts",
                "?  Shortcut help",
                "K  Markdown help",
                "I  Cursor coordinates",
                "q  Quit",
            ],
        }
        assert all(
            "\n" not in prompt and "Keys:" not in prompt
            for prompts in group_prompts.values()
            for prompt in prompts
        )
        assert app.screen.query_one("#--command-document", OptionList).highlighted == 0
        assert all(
            app.screen.query_one(selector, OptionList).highlighted is None
            for selector in (
                "#--command-navigate",
                "#--command-files",
                "#--command-mode",
                "#--command-edit",
                "#--command-view",
            )
        )
        assert str(app.screen.query_one("#--command-description", Static).render()) == (
            "Save the open Markdown source"
        )
        assert search_icon.icon == SEARCH_ICON
        assert search_icon.styles.color == Color.parse(SEARCH_ICON_COLOR)
        await pilot.resize_terminal(56, 30)
        assert app.screen.has_class("-compact")
        await pilot.press("escape")

        app.action_show_markdown_help()
        assert isinstance(app.screen, HelpDialog)
        assert app.screen.dialog_title == "Markdown syntax"
        assert "double underscores mean bold" in app.screen.content


async def test_palette_filters_navigates_and_executes_commands(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+backslash", "down")
        assert app.screen.query_one("#--command-document", OptionList).highlighted == 1
        assert "new path" in str(app.screen.query_one("#--command-description", Static).render())

        command_input = app.screen.query_one(CommandInput)
        command_input.value = "toggle preview"
        lists = list(app.screen.query(".command-group-list").results(OptionList))
        await _wait_until(pilot, lambda: sum(widget.option_count for widget in lists) == 1)
        assert [widget.option_count for widget in lists] == [0, 0, 0, 0, 0, 1]
        assert str(lists[-1].options[0].prompt) == "v  Preview"

        preview_was_visible = app.preview.display
        await pilot.press("enter")
        assert app.preview.display is not preview_was_visible
        assert app.screen.id != "--command-palette"


async def test_palette_uses_effective_command_key_remaps(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\ncommand_save = "z"\ncommand_toggle_preview = "V"\n',
        encoding="utf-8",
    )
    app = _app(path, load_config(config_root))

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+backslash")
        document = app.screen.query_one("#--command-document", OptionList)
        view = app.screen.query_one("#--command-view", OptionList)

        assert str(document.options[0].prompt) == "z  Save"
        assert str(view.options[0].prompt) == "V  Preview"
