"""Pilot coverage for active-document find and replace."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from textual.pilot import Pilot
from textual.widgets import Button, Input, Static

from termdraft.app import TermDraftApp
from termdraft.models.workspace import Workspace
from termdraft.screens.document_find import DocumentFindDialog
from termdraft.services.recovery import RecoveryJournal


def _app(path: Path, journal: RecoveryJournal | None = None) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_debounce=0.01,
        recovery_journal=journal or RecoveryJournal(path.parent / ".test-recovery"),
    )


async def _wait_until(pilot: Pilot[None], condition: Callable[[], bool]) -> None:
    for _ in range(200):
        if condition():
            return
        await pilot.pause(0.01)
    raise AssertionError("condition did not become true")


def _selected_source(app: TermDraftApp) -> str:
    return app.editor.document.get_text_range(*app.editor.selection)


async def test_incremental_find_starts_at_cursor_and_wraps(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("one two one", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.move_cursor((0, 5))
        await pilot.press("ctrl+f", "o", "n", "e")
        await _wait_until(pilot, lambda: _selected_source(app) == "one")

        assert isinstance(app.screen, DocumentFindDialog)
        assert app.editor.selection == ((0, 8), (0, 11))
        assert str(app.screen.query_one("#document-find-status", Static).render()) == "2 of 2"

        await pilot.press("f3")
        assert app.editor.selection == ((0, 0), (0, 3))
        await pilot.press("shift+f3")
        assert app.editor.selection == ((0, 8), (0, 11))


async def test_single_replace_updates_dirty_preview_and_recovery(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("one one", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = _app(path, journal)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+f", "o", "n", "e")
        await pilot.click("#document-replace-input")
        await pilot.press("t", "w", "o", "enter")
        await _wait_until(pilot, lambda: app.editor.text == "two one")

        assert app.editor.selection == ((0, 4), (0, 7))
        await pilot.press("escape")
        await _wait_until(pilot, lambda: app.preview.source_text == "two one")
        await _wait_until(pilot, lambda: journal.load(path) is not None)

        assert app.document is not None and app.document.dirty
        assert path.read_text(encoding="utf-8") == "one one"
        recovered = journal.load(path)
        assert recovered is not None and recovered.text == "two one"


async def test_replace_all_is_one_undo_operation(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("one ONE one", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+f", "o", "n", "e")
        await pilot.click("#document-replace-input")
        await pilot.press("x")
        await _wait_until(
            pilot,
            lambda: (
                isinstance(app.screen, DocumentFindDialog)
                and len(app.screen.matches) == 3
                and not app.screen.query_one("#document-replace-all", Button).disabled
            ),
        )
        await pilot.click("#document-replace-all")
        await _wait_until(pilot, lambda: app.editor.text == "x x x")
        dialog = app.screen
        await pilot.press("escape")
        await _wait_until(pilot, lambda: app.screen is not dialog and app.focused is app.editor)
        await pilot.press("u")

        assert app.editor.text == "one ONE one"


async def test_read_only_document_can_find_but_not_replace(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("find me", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.read_only = True
        await pilot.press("ctrl+f", "f", "i", "n", "d")
        await _wait_until(pilot, lambda: _selected_source(app) == "find")

        assert isinstance(app.screen, DocumentFindDialog)
        assert app.screen.query_one("#document-replace-input", Input).disabled
        assert app.screen.query_one("#document-replace-one", Button).disabled
        assert app.screen.query_one("#document-replace-all", Button).disabled
        assert app.editor.text == "find me"


async def test_find_replace_is_palette_discoverable(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        titles = {command.title for command in app.get_system_commands(app.screen)}
        assert "Find and replace in document" in titles

        await pilot.press("ctrl+f")
        assert isinstance(app.screen, DocumentFindDialog)
