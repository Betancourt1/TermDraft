"""Headless Textual Pilot tests for the protected editing workflow."""

from __future__ import annotations

from pathlib import Path

from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import (
    ConflictDialog,
    FileSearchDialog,
    HelpDialog,
    UnsavedChangesDialog,
)


def app_for_file(path: Path, *, debounce: float = 0.01) -> TermWriterApp:
    return TermWriterApp(Workspace.from_target(path), preview_debounce=debounce)


async def test_app_starts_and_opens_an_explicit_file(tmp_path: Path) -> None:
    path = tmp_path / "hello.md"
    path.write_text("# Hello\n", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(120, 36)) as pilot:
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.path == path
        assert app.editor.text == "# Hello\n"
        assert app.preview.source_text == "# Hello\n"
        assert not app.document.dirty


async def test_edit_marks_dirty_updates_preview_and_save_updates_file(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("Hello", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(120, 36)) as pilot:
        await pilot.press("x")
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.dirty
        assert app.document.text == "xHello"
        assert app.preview.source_text == "xHello"

        await pilot.press("ctrl+s")

        assert path.read_text(encoding="utf-8") == "xHello"
        assert not app.document.dirty
        assert app.document.last_save_status.startswith("Saved ")


async def test_switching_with_changes_requires_a_real_decision(tmp_path: Path) -> None:
    first = tmp_path / "one.md"
    second = tmp_path / "two.md"
    first.write_text("one", encoding="utf-8")
    second.write_text("two", encoding="utf-8")
    app = app_for_file(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await pilot.press("ctrl+p")
        assert isinstance(app.screen, FileSearchDialog)
        await pilot.press("t", "w", "o", "enter")

        assert isinstance(app.screen, UnsavedChangesDialog)
        assert app.document is not None
        assert app.document.path == first
        assert first.read_text(encoding="utf-8") == "one"

        await pilot.click("#unsaved-save")
        await pilot.pause(0.03)

        assert first.read_text(encoding="utf-8") == "xone"
        assert app.document.path == second
        assert app.editor.text == "two"


async def test_ctrl_q_does_not_discard_unsaved_content(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+q")

        assert isinstance(app.screen, UnsavedChangesDialog)
        assert app.is_running
        assert app.document is not None
        assert app.document.text == "xbase"
        assert path.read_text(encoding="utf-8") == "base"

        await pilot.click("#unsaved-cancel")

        assert app.is_running
        assert app.document.text == "xbase"


async def test_external_conflict_never_overwrites_disk_silently(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")

        assert isinstance(app.screen, ConflictDialog)
        assert path.read_text(encoding="utf-8") == "external"
        assert app.document is not None
        assert app.document.text == "xbase"
        assert app.document.conflict

        await pilot.click("#conflict-cancel")
        assert app.document.text == "xbase"


async def test_search_opens_selected_file_when_current_file_is_clean(tmp_path: Path) -> None:
    first = tmp_path / "one.md"
    second = tmp_path / "second.md"
    first.write_text("one", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = app_for_file(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+p")
        await pilot.press("s", "e", "c", "o", "n", "d", "enter")
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.path == second
        assert app.editor.text == "second"


async def test_narrow_layout_switches_between_editor_and_preview(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("content", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(70, 24)) as pilot:
        assert app.editor.display
        assert not app.preview.display

        await pilot.press("ctrl+e")
        assert not app.editor.display
        assert app.preview.display

        await pilot.press("ctrl+b")
        assert not app.explorer.display


async def test_stale_preview_timer_cannot_replace_new_file(tmp_path: Path) -> None:
    first = tmp_path / "alpha.md"
    second = tmp_path / "beta.md"
    first.write_text("alpha", encoding="utf-8")
    second.write_text("beta", encoding="utf-8")
    app = app_for_file(first, debounce=0.05)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+p")
        await pilot.press("b", "e", "t", "a", "enter")
        await pilot.click("#unsaved-discard")
        await pilot.pause(0.1)

        assert app.document is not None
        assert app.document.path == second
        assert app.preview.source_text == "beta"


async def test_undo_and_redo_use_text_area_history(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await pilot.press("ctrl+z")

        assert app.document is not None
        assert app.document.text == "base"
        assert not app.document.dirty

        await pilot.press("ctrl+y")
        assert app.document.text == "xbase"
        assert app.document.dirty


async def test_exact_mixed_newlines_remain_untouched_without_an_edit(tmp_path: Path) -> None:
    path = tmp_path / "mixed.md"
    source = b"one\r\ntwo\nthree"
    path.write_bytes(source)
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+s")

        assert app.document is not None
        assert not app.document.dirty
        assert app.document.text == source.decode()
        assert path.read_bytes() == source


async def test_crlf_is_preserved_when_editing_and_saving(tmp_path: Path) -> None:
    path = tmp_path / "crlf.md"
    path.write_bytes(b"one\r\ntwo\r\n")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+s")

        assert path.read_bytes() == b"xone\r\ntwo\r\n"


async def test_f1_opens_shortcut_help(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("f1")

        assert isinstance(app.screen, HelpDialog)
