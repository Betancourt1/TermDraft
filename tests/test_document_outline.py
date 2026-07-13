"""Pilot coverage for searchable document heading navigation."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from textual.app import App
from textual.pilot import Pilot
from textual.widgets import Input, OptionList
from textual.widgets.markdown import MarkdownBlock

from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.screens.document_outline import (
    DocumentOutlineDialog,
    OutlineDestination,
    OutlineSelection,
)
from termwriter.services.recovery import RecoveryJournal
from termwriter.widgets.preview import PreviewHeading


class OutlineHarness(App[None]):
    """Mount one outline dialog and retain its typed result."""

    def __init__(self, headings: tuple[PreviewHeading, ...]) -> None:
        self.dialog = DocumentOutlineDialog(headings)
        self.result: OutlineSelection | None = None
        super().__init__(css_path=Path(__file__).parents[1] / "src" / "termwriter" / "default.tcss")

    def on_mount(self) -> None:
        self.push_screen(self.dialog, self._store_result)

    def _store_result(self, result: OutlineSelection | None) -> None:
        self.result = result


def _app(path: Path, *, debounce: float = 0.01) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=debounce,
        recovery_journal=RecoveryJournal(path.parent / ".test-recovery"),
    )


async def _wait_until(pilot: Pilot[None], condition: Callable[[], bool]) -> None:
    for _ in range(200):
        if condition():
            return
        await pilot.pause(0.01)
    raise AssertionError("condition did not become true")


async def test_outline_filters_labels_and_preserves_heading_identity() -> None:
    headings = (
        PreviewHeading(0, "Overview", 1, 0),
        PreviewHeading(1, "Résumé Details", 2, 4),
        PreviewHeading(2, "Finish", 3, 12),
    )
    app = OutlineHarness(headings)

    async with app.run_test(size=(80, 30)) as pilot:
        results = app.screen.query_one("#document-outline-results", OptionList)
        labels = [
            str(results.get_option_at_index(index).prompt) for index in range(results.option_count)
        ]
        assert labels == [
            "H1 Overview · line 1",
            "  H2 Résumé Details · line 5",
            "    H3 Finish · line 13",
        ]

        app.screen.query_one("#document-outline-input", Input).value = "RÉSUMÉ"
        await pilot.pause()

        assert app.screen.matches == (headings[1],)
        await pilot.press("enter")
        await pilot.pause()

    assert app.result == OutlineSelection(headings[1], OutlineDestination.SOURCE)


async def test_outline_includes_fresh_source_and_jumps_to_its_line(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("# Existing\n", encoding="utf-8")
    app = _app(path, debounce=10.0)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.pause(0.03)
        app.editor.insert("\n## Fresh\n", app.editor.document.end)
        await pilot.pause(0.01)
        assert app.preview.source_text == "# Existing\n"

        await pilot.press("ctrl+shift+o")
        await _wait_until(pilot, lambda: isinstance(app.screen, DocumentOutlineDialog))
        assert app.preview.source_text == "# Existing\n\n## Fresh\n"

        outline = app.screen
        assert isinstance(outline, DocumentOutlineDialog)
        outline.query_one("#document-outline-input", Input).value = "fresh"
        await pilot.pause()
        await pilot.press("enter")
        await _wait_until(pilot, lambda: app.screen is not outline)

        assert app.editor.cursor_location == (2, 0)
        assert app.focused is app.editor


async def test_outline_can_reveal_a_heading_in_narrow_preview(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("# Overview\n\n## Details\n", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(70, 24)) as pilot:
        await pilot.pause(0.03)
        await pilot.press("ctrl+shift+o")
        await _wait_until(pilot, lambda: isinstance(app.screen, DocumentOutlineDialog))
        outline = app.screen
        assert isinstance(outline, DocumentOutlineDialog)
        outline.query_one("#document-outline-input", Input).value = "details"
        await pilot.pause()
        await pilot.click("#document-outline-preview")
        await _wait_until(pilot, lambda: app.screen is not outline)

        assert app.preview.display
        assert not app.editor.display
        assert app.focused is app.preview
        selected = app.preview.query_one(".keyboard-heading-selected", MarkdownBlock)
        assert str(selected.render()) == "Details"


async def test_outline_warns_without_headings_and_is_palette_discoverable(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("Only a paragraph.\n", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        titles = {command.title for command in app.get_system_commands(app.screen)}
        assert "Open document outline" in titles

        await pilot.press("ctrl+shift+o")
        await pilot.pause()

        assert not isinstance(app.screen, DocumentOutlineDialog)
        notifications = list(app._notifications)
        assert notifications[-1].message == "The active document has no headings"
