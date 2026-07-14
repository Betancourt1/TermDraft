"""Pilot coverage for workspace-wide source search and guarded opening."""

from __future__ import annotations

from pathlib import Path
from threading import Event
from typing import Any

import pytest
from textual.widgets import Checkbox, Input, Select, Static

from termdraft.app import TermDraftApp
from termdraft.models.workspace import Workspace
from termdraft.screens.dialogs import TextSearchDialog, UnsavedChangesDialog
from termdraft.services.recovery import RecoveryJournal
from termdraft.services.text_search import TextSearchMode


def _app(path: Path) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(path.parent / ".test-recovery"),
    )


async def test_text_search_opens_result_and_moves_to_match(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("Nothing here\n", encoding="utf-8")
    second.write_text("First line\nFind Needle here\n", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(110, 32)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)

        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)
        assert isinstance(app.screen, TextSearchDialog)
        assert [(match.path, match.line, match.column) for match in app.screen.matches] == [
            (second, 1, 5)
        ]

        await pilot.press("enter")
        await pilot.pause(0.05)
        assert app.document is not None
        assert app.document.path == second
        assert app.editor.cursor_location == (1, 5)


async def test_text_search_uses_dirty_active_source(tmp_path: Path) -> None:
    path = tmp_path / "active.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "n", "e", "e", "d", "l", "e", "space")
        await pilot.press("ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert [(match.path, match.preview) for match in app.screen.matches] == [
            (path, "needle base")
        ]
        await pilot.press("enter")

        assert app.document is not None
        assert app.document.path == path
        assert app.document.dirty
        assert app.editor.cursor_location == (0, 0)
        assert path.read_text(encoding="utf-8") == "base"


async def test_text_search_dialog_applies_regex_case_and_file_filter(
    tmp_path: Path,
) -> None:
    docs = tmp_path / "docs"
    docs.mkdir()
    active = tmp_path / "active.md"
    selected = docs / "selected.md"
    excluded = tmp_path / "excluded.md"
    active.write_text("nothing", encoding="utf-8")
    selected.write_text("Ticket id-42", encoding="utf-8")
    excluded.write_text("Ticket ID-99", encoding="utf-8")
    app = _app(active)

    async with app.run_test(size=(100, 32)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)
        app.screen.query_one("#text-search-mode", Select).value = TextSearchMode.REGEX.value
        app.screen.query_one("#text-search-case", Checkbox).value = False
        app.screen.query_one("#text-search-filter", Input).value = "docs/*.md"

        app.screen.query_one("#text-search-input", Input).value = r"id-\d+"
        await pilot.press("enter")
        await pilot.pause(0.15)

        assert [(match.path, match.column) for match in app.screen.matches] == [(selected, 7)]
        status = app.screen.query_one("#text-search-status", Static)
        assert "regular expression" in str(status.render())
        assert "docs/*.md" in str(status.render())


async def test_text_search_dialog_reports_invalid_regex(tmp_path: Path) -> None:
    path = tmp_path / "active.md"
    path.write_text("text", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)
        app.screen.query_one("#text-search-mode", Select).value = TextSearchMode.REGEX.value

        await pilot.press("[", "enter")
        await pilot.pause(0.1)

        assert app.screen.matches == ()
        status = app.screen.query_one("#text-search-status", Static)
        assert "Invalid regular expression" in str(status.render())


async def test_text_search_dialog_applies_fuzzy_ranking_and_compound_filter(
    tmp_path: Path,
) -> None:
    docs = tmp_path / "docs"
    drafts = docs / "drafts"
    drafts.mkdir(parents=True)
    active = tmp_path / "active.md"
    selected = docs / "selected.md"
    excluded = drafts / "excluded.md"
    active.write_text("nothing", encoding="utf-8")
    selected.write_text("Research summary", encoding="utf-8")
    excluded.write_text("Research memo", encoding="utf-8")
    app = _app(active)

    async with app.run_test(size=(100, 32)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)
        app.screen.query_one("#text-search-mode", Select).value = TextSearchMode.FUZZY.value
        app.screen.query_one("#text-search-filter", Input).value = "docs/**/*.md, !docs/drafts/**"
        app.screen.query_one("#text-search-input", Input).value = "rsm"
        await pilot.press("enter")
        await pilot.pause(0.15)

        assert [(match.path, match.column) for match in app.screen.matches] == [(selected, 5)]
        status = app.screen.query_one("#text-search-status", Static)
        assert "fuzzy" in str(status.render())
        assert "!docs/drafts/**" in str(status.render())


async def test_text_search_ignores_queued_result_from_an_older_same_query(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("Needle\nneedle\n", encoding="utf-8")
    app = _app(path)
    first_callback_started = Event()
    release_first_callback = Event()
    result_callbacks = 0

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)
        real_call_from_thread = app.call_from_thread

        def delay_first_result(*args: Any, **kwargs: Any) -> Any:
            nonlocal result_callbacks
            callback = args[0]
            if getattr(callback, "__name__", "") == "_show_results":
                result_callbacks += 1
                if result_callbacks == 1:
                    first_callback_started.set()
                    assert release_first_callback.wait(2)
            return real_call_from_thread(*args, **kwargs)

        monkeypatch.setattr(app, "call_from_thread", delay_first_result)
        query = app.screen.query_one("#text-search-input", Input)
        query.value = "needle"
        await pilot.press("enter")
        for _ in range(200):
            if first_callback_started.is_set():
                break
            await pilot.pause(0.01)
        assert first_callback_started.is_set()

        app.screen.query_one("#text-search-case", Checkbox).value = True
        query.focus()
        await pilot.press("enter")
        for _ in range(200):
            if [match.line for match in app.screen.matches] == [1]:
                break
            await pilot.pause(0.01)
        assert [match.line for match in app.screen.matches] == [1]

        release_first_callback.set()
        await pilot.pause(0.05)

        assert [match.line for match in app.screen.matches] == [1]
        assert "1 match · literal" in str(
            app.screen.query_one("#text-search-status", Static).render()
        )


async def test_text_search_dialog_reports_invalid_compound_filter(tmp_path: Path) -> None:
    path = tmp_path / "active.md"
    path.write_text("needle", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)
        app.screen.query_one("#text-search-filter", Input).value = "*.md,,!archive/**"
        app.screen.query_one("#text-search-input", Input).value = "needle"
        await pilot.press("enter")
        await pilot.pause(0.1)

        assert app.screen.matches == ()
        status = app.screen.query_one("#text-search-status", Static)
        assert "Invalid file filter" in str(status.render())


async def test_text_search_dialog_renders_filter_errors_as_plain_text(tmp_path: Path) -> None:
    path = tmp_path / "active.md"
    path.write_text("needle", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+shift+f")
        assert isinstance(app.screen, TextSearchDialog)
        app.screen.query_one("#text-search-filter", Input).value = "/[/]"
        app.screen.query_one("#text-search-input", Input).value = "needle"
        await pilot.press("enter")
        await pilot.pause(0.1)

        status = app.screen.query_one("#text-search-status", Static)
        assert "Invalid file filter" in str(status.render())
        assert "/[/]" in str(status.render())


async def test_text_search_options_remain_visible_in_a_narrow_terminal(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("text", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(24, 20)) as pilot:
        await pilot.press("ctrl+shift+f")

        assert isinstance(app.screen, TextSearchDialog)
        mode = app.screen.query_one("#text-search-mode", Select)
        case = app.screen.query_one("#text-search-case", Checkbox)
        assert mode.region.width > 0
        assert mode.region.height > 0
        assert case.region.width > 0
        assert case.region.height > 0


async def test_text_search_reads_new_disk_text_for_a_clean_active_document(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("old text", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.write_text("new needle", encoding="utf-8")
        await pilot.press("ctrl+shift+f")
        await pilot.press("n", "e", "w", "space", "n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert [(match.path, match.preview) for match in app.screen.matches] == [
            (path, "new needle")
        ]

        await pilot.press("enter")
        await pilot.pause(0.05)

        assert app.document is not None
        assert app.document.text == "new needle"
        assert app.editor.text == "new needle"
        assert app.editor.cursor_location == (0, 0)


async def test_text_search_can_focus_a_dirty_active_file_deleted_from_disk(
    tmp_path: Path,
) -> None:
    path = tmp_path / "deleted.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.unlink()
        await pilot.press("i", "n", "e", "e", "d", "l", "e", "space", "ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert [match.path for match in app.screen.matches] == [path]
        await pilot.press("enter")

        assert app.document is not None
        assert app.document.path == path
        assert app.document.text == "needle base"
        assert app.document.dirty
        assert app.editor.cursor_location == (0, 0)
        assert not path.exists()


async def test_text_result_opening_preserves_unsaved_source_in_a_tab(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("base", encoding="utf-8")
    second.write_text("target line", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "ctrl+shift+f")
        await pilot.press("t", "a", "r", "g", "e", "t", "enter")
        await pilot.pause(0.15)
        await pilot.press("enter")
        for _ in range(100):
            if app.document is not None and app.document.path == second:
                break
            await pilot.pause(0.01)

        assert app.document is not None
        assert app.document.path == second
        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None
        assert first_buffer.text == "xbase"
        assert first_buffer.dirty
        assert first.read_text(encoding="utf-8") == "base"


async def test_case_insensitive_alias_is_one_active_result(tmp_path: Path) -> None:
    real_path = tmp_path / "notes.md"
    alias_path = tmp_path / "NOTES.MD"
    real_path.write_text("needle", encoding="utf-8")
    if not alias_path.exists():
        pytest.skip("requires a case-insensitive filesystem")
    app = _app(alias_path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert [(match.path, match.preview) for match in app.screen.matches] == [
            (real_path, "xneedle")
        ]

        await pilot.press("enter")

        assert not isinstance(app.screen, UnsavedChangesDialog)
        assert app.document is not None
        assert app.document.path == alias_path
        assert app.document.text == "xneedle"
        assert app.document.dirty


async def test_unicode_normalization_alias_is_one_active_result(tmp_path: Path) -> None:
    real_path = tmp_path / "café.md"
    alias_path = tmp_path / "cafe\N{COMBINING ACUTE ACCENT}.md"
    real_path.write_text("needle", encoding="utf-8")
    if not alias_path.exists():
        pytest.skip("requires a Unicode-normalizing filesystem")
    app = _app(alias_path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert [(match.path, match.preview) for match in app.screen.matches] == [
            (real_path, "xneedle")
        ]


async def test_distinct_hardlink_result_opens_an_independent_tab(tmp_path: Path) -> None:
    first = tmp_path / "a.md"
    second = tmp_path / "b.md"
    first.write_text("needle", encoding="utf-8")
    second.hardlink_to(first)
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert [match.path for match in app.screen.matches] == [first, second]

        await pilot.press("down", "enter")
        for _ in range(100):
            if app.document is not None and app.document.path == second:
                break
            await pilot.pause(0.01)

        assert app.document is not None
        assert app.document.path == second
        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None and first_buffer.dirty


async def test_case_distinct_hardlink_result_opens_an_independent_tab_when_supported(
    tmp_path: Path,
) -> None:
    first = tmp_path / "a.md"
    second = tmp_path / "A.md"
    first.write_text("needle", encoding="utf-8")
    try:
        second.hardlink_to(first)
    except FileExistsError:
        pytest.skip("requires a case-sensitive filesystem")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        assert set(match.path for match in app.screen.matches) == {first, second}

        selected = next(
            index for index, match in enumerate(app.screen.matches) if match.path == second
        )
        if selected:
            await pilot.press("down")
        await pilot.press("enter")
        for _ in range(100):
            if app.document is not None and app.document.path == second:
                break
            await pilot.pause(0.01)

        assert app.document is not None
        assert app.document.path == second
        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None and first_buffer.dirty


async def test_clean_missing_active_search_marks_conflict_on_selection(
    tmp_path: Path,
) -> None:
    path = tmp_path / "missing.md"
    path.write_text("needle", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.unlink()
        await pilot.press("ctrl+shift+f")
        await pilot.press("n", "e", "e", "d", "l", "e", "enter")
        await pilot.pause(0.15)

        assert isinstance(app.screen, TextSearchDialog)
        status = app.screen.query_one("#text-search-status", Static)
        assert "1 warning" in str(status.render())
        await pilot.press("enter")

        assert app.document is not None
        assert app.document.text == "needle"
        assert app.document.conflict
        assert app.document.last_save_status == "Deleted externally"
