"""Headless Textual Pilot tests for the protected editing workflow."""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest
from textual.widgets import Input, Static

from termwriter.app import TermWriterApp, _RecoveryCleanupWorkerResult
from termwriter.models.document import FileSnapshot
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import (
    ConflictDialog,
    FileSearchDialog,
    HelpDialog,
    MixedLineEndingsDialog,
    RecoveryDeleteDialog,
    RecoveryDialog,
    RecoveryManagerDialog,
    RecoveryRetentionDialog,
    SaveAsDialog,
    UnsavedChangesDialog,
)
from termwriter.services.external_changes import DiskProbe
from termwriter.services.persistence import (
    PersistenceError,
    SaveResult,
    atomic_save,
    load_file,
)
from termwriter.services.recovery import (
    RecoveryJournal,
    RecoveryRetentionOutcome,
    RecoveryRetentionResult,
)
from termwriter.services.session import DocumentViewState, SessionState, SessionStore


def app_for_file(
    path: Path,
    *,
    debounce: float = 0.01,
    external_poll_interval: float = 2.0,
    recovery_debounce: float = 0.5,
    recovery_journal: RecoveryJournal | None = None,
    session_store: SessionStore | None = None,
) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=debounce,
        external_poll_interval=external_poll_interval,
        recovery_debounce=recovery_debounce,
        recovery_journal=recovery_journal or RecoveryJournal(path.parent / ".test-recovery"),
        session_store=session_store,
    )


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


async def test_directory_session_reopens_last_document_and_view(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    first = workspace_root / "first.md"
    second = workspace_root / "second.md"
    first.write_text("\n".join(f"first {index}" for index in range(40)), encoding="utf-8")
    second.write_text("\n".join(f"second {index}" for index in range(40)), encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            workspace_root,
            second,
            (
                DocumentViewState(first, line=3, column=2, scroll_y=2),
                DocumentViewState(second, line=20, column=3, scroll_y=9),
            ),
        )
    )
    app = TermWriterApp(
        Workspace.from_target(workspace_root),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 16)) as pilot:
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.path == second
        assert app.editor.cursor_location == (20, 3)
        assert app.editor.scroll_offset.y == 9


async def test_directory_session_restore_still_offers_missing_recovery_drafts(
    tmp_path: Path,
) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    active = workspace_root / "active.md"
    active.write_text("active", encoding="utf-8")
    missing = workspace_root / "missing.md"
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            workspace_root,
            active,
            (DocumentViewState(active),),
        )
    )
    journal = RecoveryJournal(tmp_path / "recovery")
    journal.save(
        document_path=missing,
        workspace_root=workspace_root,
        text="missing draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    app = TermWriterApp(
        Workspace.from_target(workspace_root),
        preview_debounce=0.01,
        recovery_journal=journal,
        session_store=store,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(150):
            if isinstance(app.screen, RecoveryDialog):
                break
            await pilot.pause(0.01)

        assert isinstance(app.screen, RecoveryDialog)
        assert app.screen.path == missing
        assert app.screen.source_missing
        assert app.document is not None
        assert app.document.path == active


async def test_missing_session_active_path_is_pruned_without_opening_another_file(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    missing = workspace / "missing.md"
    other = workspace / "other.md"
    other.write_text("other", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            workspace,
            missing,
            (DocumentViewState(missing), DocumentViewState(other)),
        )
    )
    app = TermWriterApp(
        Workspace.from_target(workspace),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        assert app.document is None
        for _ in range(100):
            state = store.load(workspace).state
            if state is not None and state.active_path is None:
                break
            await pilot.pause(0.01)
        else:
            raise AssertionError("missing active session path was not pruned")

        assert tuple(view.path for view in state.documents) == (other,)


async def test_explicit_file_overrides_session_active_path_but_restores_its_view(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("\n".join(f"first {index}" for index in range(20)), encoding="utf-8")
    second.write_text("\n".join(f"second {index}" for index in range(20)), encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            second,
            (
                DocumentViewState(first, line=7, column=2, scroll_y=4),
                DocumentViewState(second, line=9, column=1, scroll_y=6),
            ),
        )
    )
    app = app_for_file(first, session_store=store)

    async with app.run_test(size=(100, 16)) as pilot:
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.path == first
        assert app.editor.cursor_location == (7, 2)
        assert app.editor.scroll_offset.y == 4


async def test_switch_and_clean_quit_persist_multiple_document_views(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("\n".join(f"first {index}" for index in range(30)), encoding="utf-8")
    second.write_text("\n".join(f"second {index}" for index in range(30)), encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    app = app_for_file(first, session_store=store)

    async with app.run_test(size=(100, 16)) as pilot:
        app.editor.move_cursor((12, 3))
        app.editor.scroll_to(y=8, animate=False, immediate=True)
        await pilot.press("ctrl+p")
        await pilot.press("s", "e", "c", "o", "n", "d", "enter")
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.path == second
        app.editor.move_cursor((15, 2))
        app.editor.scroll_to(y=10, animate=False, immediate=True)
        await pilot.press("ctrl+q")

    state = store.load(tmp_path).state
    assert state is not None
    assert state.active_path == second
    assert state.view_for(first) == DocumentViewState(first, 12, 3, 0, 8)
    assert state.view_for(second) == DocumentViewState(second, 15, 2, 0, 10)

    restored = app_for_file(
        first,
        session_store=store,
        recovery_journal=RecoveryJournal(tmp_path / "restored-recovery"),
    )
    async with restored.run_test(size=(100, 16)) as pilot:
        await pilot.pause(0.03)
        assert restored.editor.cursor_location == (12, 3)
        assert restored.editor.scroll_offset.y == 8


async def test_search_result_location_is_persisted_before_clean_quit(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("\n".join(f"second {index}" for index in range(40)), encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    app = app_for_file(first, session_store=store)

    async with app.run_test(size=(100, 16)) as pilot:
        app._request_open_at(second, 30, 4)
        for _ in range(100):
            if app.document is not None and app.document.path == second:
                break
            await pilot.pause(0.01)

        assert app.editor.cursor_location == (30, 4)
        state = store.load(tmp_path).state
        assert state is not None
        view = state.view_for(second)
        assert view is not None
        assert (view.line, view.column) == (30, 4)


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


async def test_opening_another_file_preserves_dirty_buffer_without_writing(tmp_path: Path) -> None:
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
        for _ in range(100):
            if app.document is not None and app.document.path == second:
                break
            await pilot.pause(0.01)

        assert app.document is not None
        assert app.document.path == second
        assert first.read_text(encoding="utf-8") == "one"
        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None
        assert first_buffer.text == "xone"
        assert first_buffer.dirty

        await pilot.press("ctrl+pageup")
        assert app.document.path == first
        assert app.editor.text == "xone"
        await pilot.press("ctrl+s")
        for _ in range(100):
            if first.read_text(encoding="utf-8") == "xone":
                break
            await pilot.pause(0.01)

        assert first.read_text(encoding="utf-8") == "xone"
        second_buffer = app._open_document_for_path(second)
        assert second_buffer is not None
        assert second_buffer.text == "two"


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


async def test_conflict_save_as_rejects_existing_name_without_crashing(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        await pilot.click("#conflict-save-as")
        assert isinstance(app.screen, SaveAsDialog)

        await pilot.press("n", "o", "t", "e", ".", "m", "d", "enter")
        await pilot.pause(0.03)

        assert isinstance(app.screen, SaveAsDialog)
        assert app.screen.error is not None
        assert "already exists" in app.screen.error
        assert path.read_text(encoding="utf-8") == "external"
        assert app.document is not None
        assert app.document.text == "xbase"


async def test_conflict_save_as_preserves_both_versions(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    local_copy = tmp_path / "note-local.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        await pilot.click("#conflict-save-as")
        await pilot.press("enter")
        await pilot.pause(0.03)

        assert path.read_text(encoding="utf-8") == "external"
        assert local_copy.read_text(encoding="utf-8") == "xbase"
        assert app.document is not None
        assert app.document.path == local_copy
        assert not app.document.dirty
        assert len(app._open_documents) == 1
        assert app._open_document_for_path(path) is None
        assert app._open_document_for_path(local_copy) is app.document


async def test_save_as_rejects_path_owned_by_deleted_open_tab(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = app_for_file(first)

    async with app.run_test(size=(100, 30)) as pilot:
        app._request_open(second)
        for _ in range(200):
            if app.document is not None and app.document.path == second:
                break
            await pilot.pause(0.01)
        second_buffer = app.document
        second.unlink()
        await pilot.press("ctrl+pageup", "x")
        first.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        for _ in range(200):
            if isinstance(app.screen, ConflictDialog):
                break
            await pilot.pause(0.01)
        await pilot.click("#conflict-save-as")
        dialog = app.screen
        assert isinstance(dialog, SaveAsDialog)
        dialog.query_one("#save-as-input", Input).value = "second.md"

        def forbidden_save(*args: object, **kwargs: object) -> SaveResult:
            del args, kwargs
            raise AssertionError("Save As must not publish onto an open buffer path")

        monkeypatch.setattr("termwriter.app.atomic_save", forbidden_save)
        await pilot.press("enter")
        for _ in range(200):
            if dialog.error is not None:
                break
            await pilot.pause(0.01)

        assert dialog.error is not None
        assert "already open in a tab" in dialog.error
        assert not second.exists()
        assert app.document is not None and app.document.path == first
        assert app._open_document_for_path(second) is second_buffer
        assert [opened.document.path for opened in app._open_documents] == [first, second]


async def test_save_as_replaces_pending_recovery_timer_for_future_edits(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    local_copy = tmp_path / "note-local.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(
        path,
        recovery_debounce=10.0,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        assert app._recovery_timer is not None
        path.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        await pilot.click("#conflict-save-as")
        await pilot.press("enter")

        assert app.document is not None
        assert app.document.path == local_copy
        assert app._recovery_timer is None
        assert journal.load(path) is None

        await pilot.press("y")
        new_timer = app._recovery_timer
        new_revision = app._recovery_revision
        assert new_timer is not None
        new_timer.stop()
        app._write_recovery(new_revision)

        recovered = None
        for _ in range(100):
            recovered = journal.load(local_copy)
            if recovered is not None:
                break
            await pilot.pause(0.01)
        assert recovered is not None
        assert recovered.text == "xybase"


async def test_ancestor_swap_during_save_as_cannot_escape_workspace(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = tmp_path / "workspace"
    moved_workspace = tmp_path / "workspace-moved"
    outside = tmp_path / "outside"
    workspace.mkdir()
    outside.mkdir()
    original = workspace / "note.md"
    outside_original = outside / "note.md"
    outside_copy = outside / "note-local.md"
    original.write_text("base", encoding="utf-8")
    outside_original.write_text("outside", encoding="utf-8")
    app = app_for_file(original)

    def redirected_save(
        path: Path,
        text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        workspace.rename(moved_workspace)
        workspace.symlink_to(outside, target_is_directory=True)
        return atomic_save(path, text, encoding=encoding, expected=expected)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        original.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        await pilot.click("#conflict-save-as")
        monkeypatch.setattr("termwriter.app.atomic_save", redirected_save)
        await pilot.press("enter")

        assert isinstance(app.screen, SaveAsDialog)
        assert app.screen.error is not None
        assert "Cannot open destination directory" in app.screen.error
        assert not outside_copy.exists()
        assert outside_original.read_text(encoding="utf-8") == "outside"
        assert (moved_workspace / "note.md").read_text(encoding="utf-8") == "external"
        assert app.document is not None
        assert app.document.text == "xbase"


async def test_dirty_persistence_error_keeps_source_and_disk(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    def inaccessible_save(
        _path: Path,
        _text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        del encoding, expected
        raise PersistenceError("permission denied")

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        monkeypatch.setattr("termwriter.app.atomic_save", inaccessible_save)
        await pilot.press("ctrl+s")
        await pilot.pause(0.1)

        assert app.document is not None
        assert app.document.text == "xbase"
        assert app.document.dirty
        assert app.document.last_save_status == "Save failed"
        assert path.read_text(encoding="utf-8") == "base"


async def test_dirty_inaccessible_file_offers_save_as(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    def inaccessible_save(
        _path: Path,
        _text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        del encoding, expected
        raise PersistenceError("permission denied")

    def inaccessible_probe(requested: Path) -> DiskProbe:
        return DiskProbe(requested, None, "permission denied")

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        monkeypatch.setattr("termwriter.app.atomic_save", inaccessible_save)
        monkeypatch.setattr("termwriter.app.probe_file", inaccessible_probe)
        await pilot.press("ctrl+s")
        await pilot.pause(0.1)

        assert isinstance(app.screen, ConflictDialog)
        assert not app.screen.can_reload
        assert app.document is not None
        assert app.document.text == "xbase"
        assert app.document.dirty
        assert path.read_text(encoding="utf-8") == "base"

        await pilot.click("#conflict-save-as")
        assert isinstance(app.screen, SaveAsDialog)


async def test_clean_deleted_file_can_be_explicitly_discarded_on_quit(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.unlink()
        await pilot.press("ctrl+q")

        assert isinstance(app.screen, ConflictDialog)
        assert app.screen.allow_discard

        await pilot.click("#conflict-discard")
        assert not app.is_running


async def test_parent_symlink_swap_cannot_redirect_save(tmp_path: Path) -> None:
    workspace_path = tmp_path / "workspace"
    moved_workspace = tmp_path / "workspace-moved"
    outside = tmp_path / "outside"
    workspace_path.mkdir()
    outside.mkdir()
    original = workspace_path / "note.md"
    outside_file = outside / "note.md"
    original.write_text("base", encoding="utf-8")
    outside_file.write_text("base", encoding="utf-8")
    app = app_for_file(original)

    async with app.run_test(size=(100, 30)) as pilot:
        workspace_path.rename(moved_workspace)
        workspace_path.symlink_to(outside, target_is_directory=True)
        await pilot.press("x", "ctrl+s")

        assert isinstance(app.screen, ConflictDialog)
        assert outside_file.read_text(encoding="utf-8") == "base"
        assert (moved_workspace / "note.md").read_text(encoding="utf-8") == "base"
        assert app.document is not None
        assert app.document.text == "xbase"


async def test_ancestor_swap_during_save_cannot_redirect_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace_path = tmp_path / "workspace"
    moved_workspace = tmp_path / "workspace-moved"
    outside = tmp_path / "outside"
    original_parent = workspace_path / "notes"
    outside_parent = outside / "notes"
    original_parent.mkdir(parents=True)
    outside_parent.mkdir(parents=True)
    original = original_parent / "note.md"
    outside_file = outside_parent / "note.md"
    original.write_text("base", encoding="utf-8")
    outside_file.write_text("base", encoding="utf-8")
    app = app_for_file(original)

    def redirected_save(
        path: Path,
        text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        workspace_path.rename(moved_workspace)
        workspace_path.symlink_to(outside, target_is_directory=True)
        return atomic_save(path, text, encoding=encoding, expected=expected)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        monkeypatch.setattr("termwriter.app.atomic_save", redirected_save)
        await pilot.press("ctrl+s")

        assert isinstance(app.screen, ConflictDialog)
        assert outside_file.read_text(encoding="utf-8") == "base"
        assert (moved_workspace / "notes" / "note.md").read_text(encoding="utf-8") == "base"
        assert app.document is not None
        assert app.document.text == "xbase"


async def test_recovered_baseline_clears_conflict_status(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        await pilot.click("#conflict-cancel")
        await pilot.press("ctrl+z")
        path.write_text("base", encoding="utf-8")
        await pilot.press("ctrl+s")

        assert app.document is not None
        assert not app.document.dirty
        assert not app.document.conflict
        assert app.document.last_save_status == "No changes"


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


async def test_file_search_combines_fuzzy_path_matching_and_compound_filter(
    tmp_path: Path,
) -> None:
    docs = tmp_path / "docs"
    drafts = docs / "drafts"
    drafts.mkdir(parents=True)
    active = tmp_path / "active.md"
    selected = docs / "research-summary.md"
    excluded = drafts / "research-summary.md"
    active.write_text("active", encoding="utf-8")
    selected.write_text("selected", encoding="utf-8")
    excluded.write_text("excluded", encoding="utf-8")
    app = app_for_file(active)

    async with app.run_test(size=(100, 32)) as pilot:
        await pilot.press("ctrl+p")
        assert isinstance(app.screen, FileSearchDialog)
        app.screen.query_one("#file-search-filter", Input).value = "docs/**/*.md, !docs/drafts/**"
        app.screen.query_one("#search-input", Input).value = "rsm"
        await pilot.pause()

        assert app.screen.matches == (selected,)
        await pilot.press("enter")
        await pilot.pause(0.03)

        assert app.document is not None
        assert app.document.path == selected


async def test_file_search_reports_invalid_compound_filter(tmp_path: Path) -> None:
    path = tmp_path / "active.md"
    path.write_text("active", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+p")
        assert isinstance(app.screen, FileSearchDialog)
        app.screen.query_one("#file-search-filter", Input).value = "*.md,,!archive/**"
        await pilot.pause()

        assert app.screen.matches == ()
        status = app.screen.query_one("#file-search-status", Static)
        assert "Invalid file filter" in str(status.render())


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
        assert app.focused is app.preview

        await pilot.press("ctrl+b")
        assert not app.explorer.display


async def test_showing_wide_preview_focuses_keyboard_link_navigation(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("[Reference](https://example.com)\n", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(120, 30)) as pilot:
        await pilot.pause(0.03)
        assert app.preview.display
        assert app.focused is app.editor

        await pilot.press("ctrl+e")
        assert not app.preview.display
        assert app.focused is app.editor

        await pilot.press("ctrl+e")
        assert app.preview.display
        assert app.focused is app.preview
        await pilot.press("tab")
        assert app.preview.query(".keyboard-link-selected")


async def test_preview_heading_navigation_announces_position_in_status(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("# Overview\n\n## Details\n", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(70, 24)) as pilot:
        await pilot.pause(0.03)
        await pilot.press("ctrl+e", "alt+down")
        await pilot.pause()

        status = app.query_one("#status-bar", Static)
        assert "H1 1/2 · Overview" in str(status.render())

        await pilot.press("alt+down")
        assert "H2 2/2 · Details" in str(status.render())

        await pilot.press("ctrl+e")
        assert app.focused is app.editor
        assert "H2 2/2 · Details" not in str(status.render())


async def test_stale_preview_timer_cannot_replace_new_file(tmp_path: Path) -> None:
    first = tmp_path / "alpha.md"
    second = tmp_path / "beta.md"
    first.write_text("alpha", encoding="utf-8")
    second.write_text("beta", encoding="utf-8")
    app = app_for_file(first, debounce=0.05)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+p")
        await pilot.press("b", "e", "t", "a", "enter")
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
        assert isinstance(app.screen, MixedLineEndingsDialog)
        await pilot.click("#mixed-normalize")
        await pilot.press("ctrl+s")

        assert app.document is not None
        assert not app.document.dirty
        assert app.document.text == source.decode()
        assert path.read_bytes() == source


async def test_mixed_line_ending_edit_requires_consent_and_normalizes(tmp_path: Path) -> None:
    path = tmp_path / "mixed.md"
    path.write_bytes(b"one\r\ntwo\nthree")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, MixedLineEndingsDialog)
        assert app.document is None

        await pilot.click("#mixed-normalize")
        await pilot.press("x", "ctrl+s")

        assert app.document is not None
        assert app.document.line_ending_label == "CRLF"
        assert path.read_bytes() == b"xone\r\ntwo\r\nthree"


async def test_mixed_line_ending_dialog_can_cancel_opening(tmp_path: Path) -> None:
    path = tmp_path / "mixed.md"
    source = b"one\r\ntwo\n"
    path.write_bytes(source)
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.click("#mixed-cancel")

        assert app.document is None
        assert path.read_bytes() == source


async def test_lf_first_mixed_source_reports_textuals_crlf_target(tmp_path: Path) -> None:
    path = tmp_path / "mixed.md"
    path.write_bytes(b"one\ntwo\r\nthree")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, MixedLineEndingsDialog)
        assert app.screen.target == "CRLF"
        await pilot.click("#mixed-normalize")
        await pilot.press("x", "ctrl+s")

        assert path.read_bytes() == b"xone\r\ntwo\r\nthree"


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


async def test_external_watcher_reloads_a_clean_document(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path, external_poll_interval=0.01)

    async with app.run_test(size=(100, 30)) as pilot:
        path.write_text("external", encoding="utf-8")
        await pilot.pause(0.06)

        assert app.document is not None
        assert app.document.text == "external"
        assert app.editor.text == "external"
        assert app.preview.source_text == "external"
        assert app.document.last_save_status == "Reloaded externally"


async def test_external_watcher_marks_dirty_conflict_without_opening_modal(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.write_text("external", encoding="utf-8")
        app._check_external_in_background()
        await pilot.pause(0.1)

        assert app.document is not None
        assert app.document.text == "xbase"
        assert app.document.conflict
        assert app.document.last_save_status == "External conflict"
        assert not isinstance(app.screen, ConflictDialog)
        assert path.read_text(encoding="utf-8") == "external"

        await pilot.press("ctrl+s")
        assert isinstance(app.screen, ConflictDialog)


async def test_external_watcher_marks_deletion_without_erasing_editor(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.unlink()
        app._check_external_in_background()
        await pilot.pause(0.1)

        assert app.document is not None
        assert app.document.text == "base"
        assert app.document.conflict
        assert app.document.last_save_status == "Deleted externally"
        assert not isinstance(app.screen, ConflictDialog)


async def test_external_watcher_pauses_for_modal_and_clears_reverted_conflict(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "f1")
        path.write_text("external", encoding="utf-8")
        app._check_external_in_background()

        assert isinstance(app.screen, HelpDialog)
        assert app.document is not None
        assert not app.document.conflict

        await pilot.press("escape")
        app._check_external_in_background()
        await pilot.pause(0.1)
        assert app.document.conflict

        path.write_text("base", encoding="utf-8")
        app._check_external_in_background()
        await pilot.pause(0.1)
        assert not app.document.conflict
        assert app.document.text == "xbase"
        assert app.document.dirty


async def test_external_watcher_requires_consent_for_mixed_line_ending_reload(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    source = b"one\r\ntwo\n"
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.write_bytes(source)
        app._check_external_in_background()
        await pilot.pause()

        assert isinstance(app.screen, MixedLineEndingsDialog)
        assert app.document is not None
        assert app.document.text == source.decode()
        assert app.editor.read_only

        await pilot.click("#mixed-cancel")
        assert app.editor.read_only
        assert path.read_bytes() == source

        path.write_text("uniform", encoding="utf-8")
        app._check_external_in_background()
        await pilot.pause(0.1)
        assert not app.editor.read_only
        await pilot.press("x")
        assert app.document.text == "xuniform"


async def test_external_watcher_marks_invalid_utf8_reload_as_persistent_conflict(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        path.write_bytes(b"\xff\xfe")
        app._check_external_in_background()
        await pilot.pause(0.1)

        assert app.document is not None
        assert app.document.text == "base"
        assert app.document.conflict
        assert app.document.last_save_status == "File unavailable"
        assert not isinstance(app.screen, ConflictDialog)

        await pilot.press("ctrl+s")
        assert isinstance(app.screen, ConflictDialog)
        assert path.read_bytes() == b"\xff\xfe"


async def test_dirty_edit_is_journaled_and_successful_save_clears_recovery(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(
        path,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await pilot.pause(0.04)

        recovered = journal.load(path)
        assert recovered is not None
        assert recovered.text == "xbase"
        assert app.document is not None
        assert app.document.recovery_saved

        await pilot.press("ctrl+s")

        assert path.read_text(encoding="utf-8") == "xbase"
        assert journal.load(path) is None
        assert not app.document.recovery_saved


async def test_continuous_edits_do_not_postpone_the_recovery_deadline(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(
        path,
        recovery_debounce=10.0,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        pending_timer = app._recovery_timer
        pending_revision = app._recovery_revision
        assert pending_timer is not None

        await pilot.press("y", "z")

        assert app._recovery_timer is pending_timer
        assert app._recovery_revision == pending_revision
        pending_timer.stop()
        app._write_recovery(pending_revision)
        recovered = None
        for _ in range(100):
            recovered = journal.load(path)
            if recovered is not None:
                break
            await pilot.pause(0.01)
        assert recovered is not None
        assert recovered.text == "xyzbase"


async def test_startup_can_restore_a_recovery_draft(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, RecoveryDialog)
        assert app.document is None

        await pilot.click("#recovery-restore")

        assert app.document is not None
        assert app.document.text == "draft"
        assert app.document.dirty
        assert app.document.recovery_saved
        assert not app.document.recovery_conflict

        await pilot.press("ctrl+s")
        assert path.read_text(encoding="utf-8") == "draft"
        assert journal.load(path) is None


async def test_recovered_draft_conflicting_with_disk_cannot_overwrite_silently(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    path.write_text("external", encoding="utf-8")
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, RecoveryDialog)
        assert app.screen.disk_changed
        await pilot.click("#recovery-restore")

        assert app.document is not None
        assert app.document.text == "draft"
        assert app.document.recovery_conflict

        await pilot.press("ctrl+s")
        assert isinstance(app.screen, ConflictDialog)
        assert path.read_text(encoding="utf-8") == "external"


async def test_recovery_detects_same_content_file_replacement_by_identity(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    replacement = tmp_path / "replacement.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    original_inode = loaded.snapshot.inode
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    replacement.write_text("base", encoding="utf-8")
    replacement.replace(path)
    assert path.stat().st_ino != original_inode
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, RecoveryDialog)
        assert app.screen.disk_changed
        await pilot.click("#recovery-restore")

        assert app.document is not None
        assert app.document.recovery_conflict
        await pilot.press("ctrl+s")

        assert isinstance(app.screen, ConflictDialog)
        assert path.read_text(encoding="utf-8") == "base"


async def test_recovered_conflict_keeps_original_baseline_across_two_restarts(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    original = load_file(path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=original.snapshot,
    )
    path.write_text("external", encoding="utf-8")

    first_app = app_for_file(
        path,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )
    async with first_app.run_test(size=(100, 30)) as pilot:
        await pilot.click("#recovery-restore")
        assert first_app.document is not None
        assert first_app.document.recovery_conflict
        await pilot.press("x")
        await pilot.pause(0.04)

    rewritten = journal.load(path)
    assert rewritten is not None
    assert rewritten.text == "xdraft"
    assert rewritten.base_snapshot == original.snapshot

    second_app = app_for_file(path, recovery_journal=journal)
    async with second_app.run_test(size=(100, 30)) as pilot:
        assert isinstance(second_app.screen, RecoveryDialog)
        assert second_app.screen.disk_changed
        await pilot.click("#recovery-restore")
        assert second_app.document is not None
        assert second_app.document.recovery_conflict

        await pilot.press("ctrl+s")
        assert isinstance(second_app.screen, ConflictDialog)
        assert path.read_text(encoding="utf-8") == "external"


async def test_workspace_startup_recovers_a_deleted_source_via_save_as(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    path = workspace / "deleted.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=workspace,
        text="draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    path.unlink()
    app = TermWriterApp(
        Workspace.from_target(workspace),
        preview_debounce=0.01,
        external_poll_interval=2.0,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, RecoveryDialog)
        assert app.screen.source_missing
        await pilot.click("#recovery-restore")

        assert app.document is not None
        assert app.document.path == path
        assert app.document.text == "draft"
        assert app.document.dirty
        assert app.document.recovery_conflict

        await pilot.press("ctrl+s")
        assert isinstance(app.screen, ConflictDialog)
        assert not app.screen.can_reload
        assert not path.exists()

        await pilot.click("#conflict-save-as")
        await pilot.press("enter")
        local_copy = workspace / "deleted-local.md"
        assert local_copy.read_text(encoding="utf-8") == "draft"
        assert journal.load(path) is None


async def test_workspace_startup_recovers_when_source_is_invalid_utf8(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    path = workspace / "broken.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=workspace,
        text="draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    path.write_bytes(b"\xff\xfe")
    app = TermWriterApp(
        Workspace.from_target(workspace),
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, RecoveryDialog)
        assert app.screen.source_missing
        await pilot.click("#recovery-restore")
        assert app.document is not None
        assert app.document.text == "draft"

        await pilot.press("ctrl+s")
        assert isinstance(app.screen, ConflictDialog)
        assert path.read_bytes() == b"\xff\xfe"


async def test_discarding_one_orphan_defers_the_next_recovery_dialog(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    paths = [workspace / "first.md", workspace / "second.md"]
    journal = RecoveryJournal(tmp_path / "state")
    for path in paths:
        path.write_text("base", encoding="utf-8")
        loaded = load_file(path)
        journal.save(
            document_path=path,
            workspace_root=workspace,
            text=f"draft for {path.stem}",
            encoding="utf-8",
            base_snapshot=loaded.snapshot,
        )
        path.unlink()
    app = TermWriterApp(
        Workspace.from_target(workspace),
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        assert isinstance(app.screen, RecoveryDialog)
        first_offered = app.screen.path
        await pilot.click("#recovery-discard")
        await pilot.pause()

        assert isinstance(app.screen, RecoveryDialog)
        assert app.screen.path != first_offered
        await pilot.click("#recovery-discard")
        await pilot.pause()

        assert app.document is None
        assert all(journal.load(path) is None for path in paths)


async def test_recovery_can_be_discarded_for_the_disk_version(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.click("#recovery-discard")

        assert app.document is not None
        assert app.document.text == "base"
        assert not app.document.dirty
        assert journal.load(path) is None


async def test_exact_mixed_recovery_survives_restore_without_an_edit(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    loaded = load_file(path)
    recovered_source = "one\r\ntwo\nlast"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text=recovered_source,
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.click("#recovery-restore")
        assert isinstance(app.screen, MixedLineEndingsDialog)
        await pilot.click("#mixed-normalize")
        await pilot.press("ctrl+s")

        assert path.read_bytes() == recovered_source.encode()
        assert journal.load(path) is None


async def test_permission_tightening_before_save_is_preserved(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    path.chmod(0o644)
    app = app_for_file(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.chmod(0o600)
        await pilot.press("ctrl+s")
        await pilot.pause(0.1)

        assert path.read_text(encoding="utf-8") == "xbase"
        assert path.stat().st_mode & 0o777 == 0o600


async def test_explicit_quit_discard_removes_recovery_journal(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(
        path,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await pilot.pause(0.04)
        assert journal.load(path) is not None

        await pilot.press("ctrl+q")
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.click("#unsaved-discard")

        assert journal.load(path) is None
        assert not app.is_running


async def test_cancelled_mixed_open_cannot_journal_the_wrong_document(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_bytes(b"one\r\ntwo\n")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(
        first,
        recovery_debounce=0.03,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+p")
        await pilot.press("s", "e", "c", "o", "n", "d", "enter")
        assert isinstance(app.screen, MixedLineEndingsDialog)

        await pilot.pause(0.06)
        recovered = journal.load(first)
        assert recovered is not None
        assert recovered.text == "xfirst"

        await pilot.click("#mixed-cancel")
        await pilot.pause(0.06)

        assert app.document is not None
        assert app.document.path == first
        assert app.document.text == "xfirst"
        recovered = journal.load(first)
        assert recovered is not None
        assert recovered.text == "xfirst"
        assert journal.load(second) is None


async def test_failed_open_keeps_the_active_dirty_document_and_recovery(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(
        first,
        recovery_debounce=0.02,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await pilot.pause(0.04)
        assert journal.load(first) is not None

        await pilot.press("ctrl+p")
        second.unlink()
        await pilot.press("s", "e", "c", "o", "n", "d", "enter")
        await pilot.pause(0.04)

        assert app.document is not None
        assert app.document.path == first
        assert app.document.text == "xfirst"
        recovered = journal.load(first)
        assert recovered is not None
        assert recovered.text == "xfirst"


async def test_recovery_manager_retargets_a_renamed_draft_and_opens_it(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    old_path = workspace / "old.md"
    new_path = workspace / "renamed.md"
    old_path.write_text("base", encoding="utf-8")
    loaded = load_file(old_path)
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=old_path,
        workspace_root=workspace,
        text="unsaved renamed draft",
        encoding="utf-8",
        base_snapshot=loaded.snapshot,
    )
    old_path.rename(new_path)
    app = app_for_file(new_path, recovery_journal=journal)

    async with app.run_test(size=(100, 32)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)

        await pilot.pause()
        app.screen.query_one("#recovery-manager-target", Input).value = "renamed.md"
        await pilot.click("#recovery-manager-retarget")
        for _ in range(500):
            if (
                journal.load(new_path) is not None
                and journal.load(old_path) is None
                and not app._critical_io
            ):
                break
            await pilot.pause(0.01)

        assert not app._critical_io
        assert journal.load(old_path) is None
        moved = journal.load(new_path)
        assert moved is not None
        assert moved.text == "unsaved renamed draft"

        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        await pilot.click("#recovery-manager-open")
        for _ in range(150):
            if isinstance(app.screen, RecoveryDialog):
                break
            await pilot.pause(0.01)

        assert isinstance(app.screen, RecoveryDialog)
        await pilot.click("#recovery-restore")
        assert app.document is not None
        assert app.document.path == new_path
        assert app.document.text == "unsaved renamed draft"
        assert app.document.dirty


async def test_recovery_manager_archives_corrupt_entry_without_changing_bytes(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    corrupt_path = journal.state_root / f"{'e' * 64}.json"
    corrupt_bytes = b"corrupt recovery bytes\x00\xff"
    corrupt_path.write_bytes(corrupt_bytes)
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 32)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        await pilot.click("#recovery-manager-archive")
        quarantine_path = journal.state_root / "quarantine" / corrupt_path.name
        for _ in range(500):
            if quarantine_path.exists() and not corrupt_path.exists():
                break
            await pilot.pause(0.01)

        assert not corrupt_path.exists()
        assert quarantine_path.read_bytes() == corrupt_bytes


async def test_recovery_manager_rechecks_active_dirty_draft_before_archive(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 32)) as pilot:
        assert app.document is not None
        journal.save(
            document_path=path,
            workspace_root=tmp_path,
            text="recoverable draft",
            encoding="utf-8",
            base_snapshot=app.document.snapshot,
        )
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        first_dialog = app.screen

        app.editor.insert("x", (0, 0))
        await pilot.pause()
        assert app.document.dirty
        await pilot.click("#recovery-manager-archive")

        for _ in range(150):
            if isinstance(app.screen, RecoveryManagerDialog) and app.screen is not first_dialog:
                break
            await pilot.pause(0.01)

        assert isinstance(app.screen, RecoveryManagerDialog)
        assert app.screen is not first_dialog
        recovered = journal.load(path)
        assert recovered is not None
        assert recovered.text == "recoverable draft"
        assert not app.editor.read_only


async def test_recovery_manager_restores_quarantine_into_guarded_open_flow(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("disk base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="archived unsaved draft",
        encoding="utf-8",
        base_snapshot=load_file(path).snapshot,
    )
    (active_record,) = journal.list_entries(tmp_path)
    journal.quarantine(active_record)
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 32)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        await pilot.click("#recovery-manager-open")

        for _ in range(200):
            if isinstance(app.screen, RecoveryDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryDialog)
        await pilot.click("#recovery-restore")

        assert app.document is not None
        assert app.document.text == "archived unsaved draft"
        assert app.document.dirty
        assert len(app._open_documents) == 1
        assert app._open_documents[0].document is app.document
        assert journal.list_quarantined(tmp_path) == ()
        assert journal.load(path) is not None


async def test_recovery_manager_exports_quarantine_without_consuming_it(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("disk base", encoding="utf-8")
    export_dir = tmp_path / "exports"
    export_dir.mkdir()
    destination = export_dir / "archived.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="archived unsaved draft\r\nwithout final newline",
        encoding="utf-8",
        base_snapshot=load_file(path).snapshot,
    )
    (active_record,) = journal.list_entries(tmp_path)
    quarantine_path = journal.quarantine(active_record)
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 32)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        app.screen.query_one("#recovery-manager-target", Input).value = "exports/archived.md"
        await pilot.click("#recovery-manager-archive")

        for _ in range(200):
            if destination.exists():
                break
            await pilot.pause(0.01)

        assert destination.read_bytes() == b"archived unsaved draft\r\nwithout final newline"
        assert quarantine_path.exists()
        assert len(journal.list_quarantined(tmp_path)) == 1


async def test_recovery_manager_permanently_deletes_quarantine_after_confirmation(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    quarantine_path = quarantine_root / f"{'f' * 64}.json"
    quarantine_path.write_bytes(b"corrupt archived bytes")
    app = app_for_file(path, recovery_journal=journal)

    async with app.run_test(size=(100, 32)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)

        await pilot.click("#recovery-manager-retarget")
        for _ in range(100):
            if isinstance(app.screen, RecoveryDeleteDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryDeleteDialog)
        assert quarantine_path.exists()
        await pilot.click("#recovery-delete-cancel")
        for _ in range(100):
            if not isinstance(app.screen, (RecoveryDeleteDialog, RecoveryManagerDialog)):
                break
            await pilot.pause(0.01)
        assert quarantine_path.exists()
        assert not isinstance(app.screen, (RecoveryDeleteDialog, RecoveryManagerDialog))

        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        delete_button = app.screen.query_one("#recovery-manager-retarget")
        delete_button.focus()
        await pilot.pause()
        assert delete_button.has_focus
        await pilot.press("enter")
        for _ in range(100):
            if isinstance(app.screen, RecoveryDeleteDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryDeleteDialog)
        await pilot.click("#recovery-delete-confirm")
        for _ in range(100):
            if not quarantine_path.exists():
                break
            await pilot.pause(0.01)

        assert not quarantine_path.exists()
        assert journal.list_quarantined(tmp_path) == ()


async def test_recovery_manager_cleans_only_confirmed_expired_quarantine(
    tmp_path: Path,
) -> None:
    active = tmp_path / "active.md"
    active.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    expired = tmp_path / "expired.md"
    journal.save(
        document_path=expired,
        workspace_root=tmp_path,
        text="expired draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    journal_path = journal.path_for(expired)
    payload = json.loads(journal_path.read_text(encoding="utf-8"))
    payload["updated_at"] = (datetime.now(UTC) - timedelta(days=90)).isoformat()
    journal_path.write_text(json.dumps(payload), encoding="utf-8")
    (record,) = journal.list_entries(tmp_path)
    quarantine_path = journal.quarantine(record)
    app = app_for_file(active, recovery_journal=journal)

    async with app.run_test(size=(100, 36)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        await pilot.click("#recovery-manager-retention")

        for _ in range(100):
            if isinstance(app.screen, RecoveryRetentionDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryRetentionDialog)
        assert app.screen.query_one("#recovery-retention-cancel").has_focus
        await pilot.click("#recovery-retention-cancel")
        for _ in range(100):
            if not isinstance(app.screen, (RecoveryManagerDialog, RecoveryRetentionDialog)):
                break
            await pilot.pause(0.01)
        assert quarantine_path.exists()

        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        await pilot.pause()
        await pilot.click("#recovery-manager-retention")
        for _ in range(100):
            if isinstance(app.screen, RecoveryRetentionDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryRetentionDialog)
        await pilot.click("#recovery-retention-confirm")

        for _ in range(200):
            if not quarantine_path.exists():
                break
            await pilot.pause(0.01)
        assert not quarantine_path.exists()


async def test_recovery_cleanup_notification_includes_every_failure(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    active = tmp_path / "active.md"
    active.write_text("base", encoding="utf-8")
    app = app_for_file(active)
    cutoff = datetime.now(UTC) - timedelta(days=30)
    result = RecoveryRetentionResult(
        cutoff,
        (
            RecoveryRetentionOutcome(
                tmp_path / "first.json",
                tmp_path / "first.md",
                cutoff,
                False,
                "fingerprint changed",
            ),
            RecoveryRetentionOutcome(
                tmp_path / "second.json",
                tmp_path / "second.md",
                cutoff,
                False,
                "permission denied",
            ),
        ),
    )
    notifications: list[str] = []

    async with app.run_test(size=(100, 32)):
        monkeypatch.setattr(
            app,
            "notify",
            lambda message, **_kwargs: notifications.append(str(message)),
        )
        app._handle_recovery_cleanup_result(_RecoveryCleanupWorkerResult(result=result))

    assert len(notifications) == 1
    assert "first.md" in notifications[0]
    assert "fingerprint changed" in notifications[0]
    assert "second.md" in notifications[0]
    assert "permission denied" in notifications[0]


async def test_recovery_manager_rechecks_dirty_document_before_quarantine_restore(
    tmp_path: Path,
) -> None:
    path = tmp_path / "active.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=path,
        workspace_root=tmp_path,
        text="archived draft",
        encoding="utf-8",
        base_snapshot=load_file(path).snapshot,
    )
    (active_record,) = journal.list_entries(tmp_path)
    quarantine_path = journal.quarantine(active_record)
    app = app_for_file(path, recovery_journal=journal, recovery_debounce=10)

    async with app.run_test(size=(100, 32)) as pilot:
        app.action_manage_recovery()
        for _ in range(100):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)
        assert isinstance(app.screen, RecoveryManagerDialog)
        first_dialog = app.screen

        app.editor.insert("x", (0, 0))
        await pilot.pause()
        await pilot.click("#recovery-manager-open")
        for _ in range(150):
            if isinstance(app.screen, RecoveryManagerDialog) and app.screen is not first_dialog:
                break
            await pilot.pause(0.01)

        assert isinstance(app.screen, RecoveryManagerDialog)
        assert app.screen is not first_dialog
        assert quarantine_path.exists()
        assert journal.load(path) is None
        assert app.document is not None
        assert app.document.text == "xbase"
