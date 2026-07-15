"""Pilot coverage for independent in-memory document tabs."""

from __future__ import annotations

import signal
from pathlib import Path

import pytest
from textual.pilot import Pilot
from textual.widgets import Tab

from termdraft.app import TermDraftApp
from termdraft.config import load_config
from termdraft.models.workspace import Workspace, WorkspaceAccessError
from termdraft.screens.dialogs import (
    RecoveryManagerDialog,
    TextSearchDialog,
    UnsavedChangesDialog,
)
from termdraft.services.recovery import RecoveryJournal
from termdraft.services.session import DocumentViewState, SessionState, SessionStore


def _app(
    first: Path,
    *,
    recovery_debounce: float = 0.01,
) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=recovery_debounce,
        recovery_journal=RecoveryJournal(first.parent / ".test-recovery"),
    )


async def _wait_for_document(
    app: TermDraftApp,
    pilot: Pilot[None],
    path: Path,
) -> None:
    for _ in range(200):
        if app.document is not None and app.document.path == path:
            return
        await pilot.pause(0.01)
    raise AssertionError(f"document did not activate: {path}")


async def _open(app: TermDraftApp, pilot: Pilot[None], path: Path) -> None:
    app._request_open(path)
    await _wait_for_document(app, pilot, path)


async def test_tabs_preserve_independent_dirty_sources_and_view_positions(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("\n".join(f"first {index}" for index in range(40)), encoding="utf-8")
    second.write_text("\n".join(f"second {index}" for index in range(40)), encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 18)) as pilot:
        await pilot.press("i", "x")
        app.editor.move_cursor((20, 3))
        app.editor.scroll_to(y=12, animate=False, immediate=True)
        await _open(app, pilot, second)
        await pilot.press("y")

        assert len(app._open_documents) == 2
        assert app.document_tabs.display
        await pilot.press("ctrl+pageup")

        assert app.document is not None
        assert app.document.path == first
        assert app.document.dirty
        assert app.editor.text.startswith("xfirst 0")
        assert app.editor.cursor_location == (20, 3)
        assert app.editor.scroll_offset.y == 12

        await pilot.press("ctrl+pagedown")
        assert app.document.path == second
        assert app.document.dirty
        assert app.editor.text.startswith("ysecond 0")
        labels = [str(tab.label) for tab in app.document_tabs.query(Tab)]
        assert sum("●" in label for label in labels) == 2


async def test_tabs_keep_independent_editor_undo_histories(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        await _open(app, pilot, second)
        await pilot.press("y", "ctrl+pageup", "ctrl+z")

        assert app.document is not None
        assert app.document.path == first
        assert app.document.text == "first"
        second_buffer = app._open_document_for_path(second)
        assert second_buffer is not None
        assert second_buffer.text == "ysecond"

        await pilot.press("ctrl+pagedown", "ctrl+z")
        assert app.document.path == second
        assert app.document.text == "second"


async def test_saving_active_tab_does_not_write_or_clean_another_buffer(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        await _open(app, pilot, second)
        await pilot.press("y", "ctrl+s")
        for _ in range(200):
            second_buffer = app._open_document_for_path(second)
            if (
                second.read_text(encoding="utf-8") == "ysecond"
                and second_buffer is not None
                and not second_buffer.dirty
            ):
                break
            await pilot.pause(0.01)
        else:
            raise AssertionError("active tab did not finish saving")

        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None and first_buffer.dirty
        assert first_buffer.text == "xfirst"
        assert first.read_text(encoding="utf-8") == "first"
        assert second_buffer is not None and not second_buffer.dirty
        assert second.read_text(encoding="utf-8") == "ysecond"


async def test_switch_flushes_recovery_before_long_debounce_expires(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=10,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        await _open(app, pilot, second)
        for _ in range(200):
            recovered = journal.load(first)
            if recovered is not None:
                break
            await pilot.pause(0.01)

        assert recovered is not None
        assert recovered.text == "xfirst"


async def test_dirty_tab_close_uses_cancel_then_discard_guard(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await _open(app, pilot, second)
        await pilot.press("i", "x", "ctrl+f4")
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.press("escape")

        assert len(app._open_documents) == 2
        assert app.document is not None and app.document.path == second

        await pilot.press("ctrl+f4")
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.press("n")
        await _wait_for_document(app, pilot, first)
        for _ in range(200):
            if not app._recovery_mutation_in_flight and not app._recovery_mutation_queue:
                break
            await pilot.pause(0.01)

        assert len(app._open_documents) == 1
        assert app._open_document_for_path(second) is None
        assert second.read_text(encoding="utf-8") == "second"
        assert journal.load(second) is None
        assert not app.document_tabs.display


async def test_quit_guards_each_dirty_tab_and_cancel_stops_exit(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        await _open(app, pilot, second)
        await pilot.press("y", "ctrl+q")
        assert isinstance(app.screen, UnsavedChangesDialog)
        assert app.document is not None and app.document.path == second

        await pilot.press("y")
        for _ in range(200):
            if isinstance(app.screen, UnsavedChangesDialog) and app.document.path == first:
                break
            await pilot.pause(0.01)

        assert second.read_text(encoding="utf-8") == "ysecond"
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.press("escape")

        assert app.is_running
        assert app.document.path == first
        assert app.document.dirty
        assert first.read_text(encoding="utf-8") == "first"


async def test_orderly_signal_journals_every_dirty_tab_even_during_quit_dialog(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=60.0,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await _open(app, pilot, second)
        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None
        first_entry = app._open_entry_for_document(first_buffer)
        assert first_entry is not None
        first_entry.editor.insert("x", (0, 0))
        await pilot.press("i", "y", "ctrl+q")

        assert isinstance(app.screen, UnsavedChangesDialog)
        assert journal.load(first) is None
        assert journal.load(second) is None

        app.request_orderly_shutdown(signal.SIGTERM)
        for _ in range(200):
            if not app.is_running:
                break
            await pilot.pause(0.01)

        assert not app.is_running

    first_recovery = journal.load(first)
    second_recovery = journal.load(second)
    assert first_recovery is not None and first_recovery.text == "xfirst"
    assert second_recovery is not None and second_recovery.text == "ysecond"
    assert first.read_text(encoding="utf-8") == "first"
    assert second.read_text(encoding="utf-8") == "second"


async def test_reactivating_dirty_tab_detects_external_conflict(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        await _open(app, pilot, second)
        first.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+pageup")
        for _ in range(200):
            if app.document is not None and app.document.conflict:
                break
            await pilot.pause(0.01)

        assert app.document is not None
        assert app.document.path == first
        assert app.document.text == "xfirst"
        assert app.document.conflict
        assert first.read_text(encoding="utf-8") == "external"


async def test_inactive_clean_tab_reports_external_change_then_reloads_on_activation(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await _open(app, pilot, second)
        first.write_text("external", encoding="utf-8")
        app._check_external_in_background()
        first_buffer = app._open_document_for_path(first)
        for _ in range(200):
            if first_buffer is not None and first_buffer.conflict:
                break
            await pilot.pause(0.01)

        assert first_buffer is not None
        assert first_buffer.text == "first"
        assert first_buffer.last_save_status == "Changed externally"
        assert app.document is not None and app.document.path == second
        assert app.editor.text == "second"
        assert not app._has_modal
        assert any("! first.md" in str(tab.label) for tab in app.document_tabs.query(Tab))

        await pilot.press("ctrl+pageup")
        for _ in range(200):
            if first_buffer.text == "external" and not first_buffer.conflict:
                break
            await pilot.pause(0.01)

        assert app.document is first_buffer
        assert app.editor.text == "external"
        assert first_buffer.last_save_status == "Reloaded externally"


async def test_inactive_watcher_rotates_without_opening_conflict_dialogs(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    third = tmp_path / "third.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    third.write_text("third", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await _open(app, pilot, second)
        await _open(app, pilot, third)
        first_buffer = app._open_document_for_path(first)
        second_buffer = app._open_document_for_path(second)
        assert first_buffer is not None and second_buffer is not None
        first.write_text("external first", encoding="utf-8")
        second.write_text("external second", encoding="utf-8")

        app._check_external_in_background()
        for _ in range(200):
            if first_buffer.conflict:
                break
            await pilot.pause(0.01)
        assert first_buffer.conflict
        assert not second_buffer.conflict

        app._check_external_in_background()
        for _ in range(200):
            if second_buffer.conflict:
                break
            await pilot.pause(0.01)

        assert second_buffer.conflict
        assert app.document is not None and app.document.path == third
        assert app.editor.text == "third"
        assert not app._has_modal


async def test_workspace_search_uses_inactive_dirty_tab_source(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        app.editor.insert("memoryneedle ", (0, 0))
        await _open(app, pilot, second)
        app.action_search_text()
        await pilot.pause()
        assert isinstance(app.screen, TextSearchDialog)

        await pilot.press(*"memoryneedle", "enter")
        for _ in range(200):
            if app.screen.matches:
                break
            await pilot.pause(0.01)

        assert app.screen.matches[0].path == first
        assert "memoryneedle" in app.screen.matches[0].preview


async def test_recovery_manager_protects_every_dirty_open_tab(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 34)) as pilot:
        await pilot.press("i", "x")
        await _open(app, pilot, second)
        await pilot.press("y")
        for _ in range(200):
            if journal.load(first) is not None and journal.load(second) is not None:
                break
            await pilot.pause(0.01)

        app.action_manage_recovery()
        for _ in range(200):
            if isinstance(app.screen, RecoveryManagerDialog):
                break
            await pilot.pause(0.01)

        assert isinstance(app.screen, RecoveryManagerDialog)
        assert app.screen.protected_journal_paths == {
            journal.path_for(first),
            journal.path_for(second),
        }


async def test_tab_switch_is_ignored_during_critical_file_operation(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(70, 20)) as pilot:
        await _open(app, pilot, second)
        app._critical_io = True
        await pilot.press("ctrl+pageup")

        assert app.document is not None
        assert app.document.path == second
        assert app.document_tabs.display

        app._critical_io = False
        first_tab = next(opened for opened in app._open_documents if app._tab_path(opened) == first)
        await pilot.click(f"#{first_tab.tab_id}")
        assert app.document is not None
        assert app.document.path == first


async def test_tab_bindings_are_remappable_and_commands_are_discoverable(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\nprevious_tab = "alt+h"\n',
        encoding="utf-8",
    )
    app = TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        config=load_config(config_root),
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await _open(app, pilot, second)
        command_titles = {command.title for command in app.get_system_commands(app.screen)}
        assert {"Next document tab", "Previous document tab", "Close document tab"} <= (
            command_titles
        )

        await pilot.press("ctrl+pageup")
        assert app.document is not None and app.document.path == second
        await pilot.press("alt+h")
        await pilot.pause()
        assert app.document.path == first


async def test_directory_restart_restores_tab_order_and_active_tab(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            first,
            (
                DocumentViewState(first, line=0, column=3),
                DocumentViewState(second, line=0, column=4),
            ),
            (first, second),
        )
    )
    app = TermDraftApp(
        Workspace.from_target(tmp_path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(200):
            if len(app._open_documents) == 2 and not app._restoring_session_tabs:
                break
            await pilot.pause(0.01)

        assert [app._tab_path(opened) for opened in app._open_documents] == [first, second]
        assert len(app._materialized_open_documents()) == 1
        assert app.document is not None and app.document.path == first
        assert app.editor.cursor_location == (0, 3)
        assert app.editor.history.undo_stack == []

        await pilot.press("ctrl+pagedown")
        await _wait_for_document(app, pilot, second)

        assert len(app._materialized_open_documents()) == 2
        assert app.editor.text == "second"
        assert app.editor.cursor_location == (0, 4)


async def test_directory_restart_defers_every_inactive_tab(tmp_path: Path) -> None:
    paths = tuple(tmp_path / f"note-{index}.md" for index in range(10))
    for index, path in enumerate(paths):
        path.write_text(f"note {index}", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            paths[4],
            tuple(DocumentViewState(path) for path in paths),
            paths,
        )
    )
    app = TermDraftApp(
        Workspace.from_target(tmp_path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(200):
            if len(app._open_documents) == 10 and not app._restoring_session_tabs:
                break
            await pilot.pause(0.01)

        assert [app._tab_path(opened) for opened in app._open_documents] == list(paths)
        assert len(app._materialized_open_documents()) == 1
        assert app.document is not None and app.document.path == paths[4]


async def test_explicit_file_launch_does_not_restore_other_session_tabs(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            second,
            (DocumentViewState(first), DocumentViewState(second)),
            (first, second),
        )
    )
    app = TermDraftApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 30)):
        assert app.document is not None and app.document.path == first
        assert [app._tab_path(opened) for opened in app._open_documents] == [first]


async def test_missing_restored_tab_is_pruned_and_survivor_opens(tmp_path: Path) -> None:
    missing = tmp_path / "missing.md"
    survivor = tmp_path / "survivor.md"
    survivor.write_text("survivor", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            missing,
            (DocumentViewState(missing), DocumentViewState(survivor)),
            (missing, survivor),
        )
    )
    app = TermDraftApp(
        Workspace.from_target(tmp_path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(200):
            if app.document is not None and not app._restoring_session_tabs:
                break
            await pilot.pause(0.01)
        await _wait_until_session_saved(app, pilot)

        assert app.document is not None and app.document.path == survivor
        restored = store.load(tmp_path).state
        assert restored is not None
        assert restored.open_paths == (survivor,)


@pytest.mark.parametrize("replacement", ["missing", "directory"])
async def test_invalid_deferred_tab_is_pruned_after_startup(
    tmp_path: Path,
    replacement: str,
) -> None:
    first = tmp_path / "first.md"
    deferred = tmp_path / "deferred.md"
    first.write_text("first", encoding="utf-8")
    deferred.write_text("deferred", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            first,
            (DocumentViewState(first), DocumentViewState(deferred)),
            (first, deferred),
        )
    )
    app = TermDraftApp(
        Workspace.from_target(tmp_path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(200):
            if app.document is not None and not app._restoring_session_tabs:
                break
            await pilot.pause(0.01)
        deferred.unlink()
        if replacement == "directory":
            deferred.mkdir()

        deferred_tab = next(
            opened for opened in app._open_documents if app._tab_path(opened) == deferred
        )
        await pilot.click(f"#{deferred_tab.tab_id}")
        for _ in range(200):
            if all(app._tab_path(opened) != deferred for opened in app._open_documents):
                break
            await pilot.pause(0.01)
        await _wait_until_session_saved(app, pilot)

        assert app.document is not None and app.document.path == first
        assert [app._tab_path(opened) for opened in app._open_documents] == [first]
        restored = store.load(tmp_path).state
        assert restored is not None
        assert restored.open_paths == (first,)
        assert all(view.path != deferred for view in restored.documents)


async def test_transient_deferred_failure_can_be_retried_after_last_active_tab_closes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    deferred = tmp_path / "deferred.md"
    first.write_text("first", encoding="utf-8")
    deferred.write_text("deferred", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            first,
            (DocumentViewState(first), DocumentViewState(deferred)),
            (first, deferred),
        )
    )
    app = TermDraftApp(
        Workspace.from_target(tmp_path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
        session_store=store,
    )
    original_validate = Workspace.validate_document_path
    unavailable = True

    def transient_validate(
        workspace: Workspace,
        path: Path,
        *,
        must_exist: bool = True,
    ) -> Path:
        if unavailable and path == deferred:
            raise WorkspaceAccessError("injected transient access failure")
        return original_validate(workspace, path, must_exist=must_exist)

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(200):
            if app.document is not None and not app._restoring_session_tabs:
                break
            await pilot.pause(0.01)
        monkeypatch.setattr(Workspace, "validate_document_path", transient_validate)

        await pilot.press("ctrl+f4")
        for _ in range(200):
            if app.document is None and not app._critical_io:
                break
            await pilot.pause(0.01)

        assert [app._tab_path(opened) for opened in app._open_documents] == [deferred]
        assert app.document_tabs.display
        assert app.document_tabs.active == ""

        unavailable = False
        retained = app._open_documents[0]
        await pilot.click(f"#{retained.tab_id}")
        await _wait_for_document(app, pilot, deferred)

        assert app.editor.text == "deferred"
        assert app.document_tabs.active == retained.tab_id


async def _wait_until_session_saved(app: TermDraftApp, pilot: Pilot[None]) -> None:
    for _ in range(200):
        if not app._session_save_in_flight and app._pending_session_state is None:
            return
        await pilot.pause(0.01)
    raise AssertionError("session save did not finish")
