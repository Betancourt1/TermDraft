"""Race-focused Pilot tests for background document hashing and publication."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from threading import Event, get_ident

import pytest
from textual.pilot import Pilot
from textual.widgets import Button, Input, TextArea

from termwriter.app import TermWriterApp
from termwriter.models.document import FileSnapshot
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import (
    ConflictDialog,
    MixedLineEndingsDialog,
    RecoveryDialog,
    SaveAsDialog,
    UnsavedChangesDialog,
)
from termwriter.services.external_changes import DiskProbe, probe_file
from termwriter.services.persistence import LoadedFile, SaveResult, atomic_save, load_file
from termwriter.services.recovery import RecoveryJournal
from termwriter.services.session import DocumentViewState, SessionState, SessionStore


def _app(
    path: Path,
    *,
    journal: RecoveryJournal | None = None,
    session_store: SessionStore | None = None,
) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=journal or RecoveryJournal(path.parent / ".test-recovery"),
        session_store=session_store,
    )


async def _wait_until(
    pilot: Pilot[None],
    predicate: Callable[[], bool],
    *,
    attempts: int = 200,
) -> None:
    for _ in range(attempts):
        if predicate():
            return
        await pilot.pause(0.01)
    raise AssertionError("Timed out waiting for background file operation")


async def test_initial_document_load_runs_off_the_ui_thread(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    ui_thread = get_ident()
    load_threads: list[int] = []

    def tracked_load(requested: Path) -> LoadedFile:
        load_threads.append(get_ident())
        return load_file(requested)

    monkeypatch.setattr("termwriter.app.load_file", tracked_load)
    app = _app(path)

    async with app.run_test(size=(100, 30)):
        assert app.document is not None
        assert app.document.text == "base"
        assert load_threads
        assert all(thread != ui_thread for thread in load_threads)


async def test_session_load_runs_off_the_ui_thread(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(SessionState(tmp_path, path, (DocumentViewState(path),)))
    ui_thread = get_ident()
    load_threads: list[int] = []
    real_load = store.load

    def tracked_load(workspace_root: Path):
        load_threads.append(get_ident())
        return real_load(workspace_root)

    monkeypatch.setattr(store, "load", tracked_load)
    app = _app(path, session_store=store)

    async with app.run_test(size=(100, 30)):
        assert app.document is not None
        assert load_threads
        assert all(thread != ui_thread for thread in load_threads)


async def test_session_saves_are_serialized_and_coalesce_to_latest_state(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    app = _app(path, session_store=store)
    started = Event()
    release = Event()
    saved_states: list[SessionState] = []
    save_threads: list[int] = []
    real_save = store.save

    async with app.run_test(size=(100, 30)) as pilot:
        await _wait_until(pilot, lambda: not app._session_save_in_flight)

        def blocked_save(state: SessionState) -> None:
            save_threads.append(get_ident())
            saved_states.append(state)
            if len(saved_states) == 1:
                started.set()
                assert release.wait(2)
            real_save(state)

        monkeypatch.setattr(store, "save", blocked_save)
        states = tuple(
            SessionState(
                tmp_path,
                tmp_path / f"state-{index}.md",
                (DocumentViewState(tmp_path / f"state-{index}.md"),),
            )
            for index in range(3)
        )
        app._queue_session_save(states[0])
        await _wait_until(pilot, started.is_set)
        app._queue_session_save(states[1])
        app._queue_session_save(states[2])

        release.set()
        await _wait_until(
            pilot,
            lambda: not app._session_save_in_flight and app._pending_session_state is None,
        )

        assert saved_states == [states[0], states[2]]
        assert all(thread != get_ident() for thread in save_threads)
        assert store.load(tmp_path).state == states[2]


async def test_recovery_publication_runs_off_ui_and_keeps_latest_edit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_debounce=10,
        recovery_journal=journal,
    )
    started = Event()
    release = Event()
    publication_threads: list[int] = []
    real_publish = journal.publish

    def blocked_publish(**kwargs):
        publication_threads.append(get_ident())
        if not started.is_set():
            started.set()
            assert release.wait(2)
        return real_publish(**kwargs)

    monkeypatch.setattr(journal, "publish", blocked_publish)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        assert app._recovery_timer is not None
        app._recovery_timer.stop()
        app._write_recovery(app._recovery_revision)
        await _wait_until(pilot, started.is_set)

        assert not app.editor.read_only
        await pilot.press("y")
        assert app.document is not None
        assert app.document.text == "xybase"
        assert app._recovery_timer is not None
        app._recovery_timer.stop()
        app._write_recovery(app._recovery_revision)

        release.set()
        await _wait_until(
            pilot,
            lambda: bool(
                app.document
                and app.document.recovery_saved
                and app._recovery_mutation_in_flight is None
                and not app._recovery_mutation_queue
            ),
        )

        recovered = journal.load(path)
        assert recovered is not None
        assert recovered.text == "xybase"
        assert publication_threads
        assert all(thread != get_ident() for thread in publication_threads)


async def test_discard_waits_for_recovery_save_then_delete_without_resurrection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    first.write_text("first", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = TermWriterApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=10,
        recovery_journal=journal,
    )
    started = Event()
    release = Event()
    real_publish = journal.publish

    def blocked_publish(**kwargs):
        started.set()
        assert release.wait(2)
        return real_publish(**kwargs)

    monkeypatch.setattr(journal, "publish", blocked_publish)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        assert app._recovery_timer is not None
        app._recovery_timer.stop()
        app._write_recovery(app._recovery_revision)
        await _wait_until(pilot, started.is_set)

        app.action_close_tab()
        await _wait_until(pilot, lambda: isinstance(app.screen, UnsavedChangesDialog))
        await pilot.pause()
        await pilot.click("#unsaved-discard")
        assert app.document is not None
        assert app.document.path == first
        assert app.editor.read_only
        before = app.editor.text
        await pilot.press("y", "ctrl+pageup")
        assert app.editor.text == before

        release.set()
        await _wait_until(pilot, lambda: app.document is None)

        assert journal.load(first) is None
        assert first.read_text(encoding="utf-8") == "first"


async def test_successful_save_stays_frozen_until_recovery_cleanup_finishes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = _app(path, journal=journal)
    started = Event()
    release = Event()
    real_delete = journal.delete_expected

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        assert app._recovery_timer is not None
        app._recovery_timer.stop()
        app._write_recovery(app._recovery_revision)
        await _wait_until(pilot, lambda: journal.load(path) is not None)

        def blocked_delete(document_path: Path, *, fingerprint: str | None) -> None:
            started.set()
            assert release.wait(2)
            real_delete(document_path, fingerprint=fingerprint)

        monkeypatch.setattr(journal, "delete_expected", blocked_delete)
        await pilot.press("ctrl+s")
        await _wait_until(pilot, started.is_set)

        assert app._critical_io
        assert app.editor.read_only
        assert path.read_text(encoding="utf-8") == "xbase"
        before = app.editor.text
        await pilot.press("y", "ctrl+z", "ctrl+y")
        assert app.editor.text == before

        release.set()
        await _wait_until(pilot, lambda: not app._critical_io)

        assert app.document is not None and not app.document.dirty
        assert not app.editor.read_only
        assert journal.load(path) is None


async def test_quit_drain_is_sealed_and_preserves_active_session_tab(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    app = _app(first, session_store=store)
    started = Event()
    release = Event()
    real_save = store.save

    async with app.run_test(size=(100, 30)) as pilot:
        app._request_open(second)
        await _wait_until(
            pilot,
            lambda: bool(app.document and app.document.path == second),
        )
        await pilot.press("ctrl+pageup")
        assert app.document is not None and app.document.path == first
        await _wait_until(
            pilot,
            lambda: not app._session_save_in_flight and app._pending_session_state is None,
        )

        def blocked_save(state: SessionState) -> None:
            started.set()
            assert release.wait(2)
            real_save(state)

        monkeypatch.setattr(store, "save", blocked_save)
        await pilot.press("ctrl+q")
        await _wait_until(pilot, started.is_set)

        assert app._exit_requested
        assert app.document is not None and app.document.path == first
        assert app.editor.read_only
        before = app.editor.text
        await pilot.press("ctrl+pagedown", "y")
        assert app.document.path == first
        assert app.editor.text == before

        release.set()
        await _wait_until(pilot, lambda: not app.is_running)

    state = store.load(tmp_path).state
    assert state is not None
    assert state.active_path == first
    assert tuple(view.path for view in state.documents) == (first, second)


async def test_reload_cleanup_keeps_its_tab_frozen_and_mixed_state_scoped(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first\n", encoding="utf-8")
    second.write_text("second\n", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = _app(first, journal=journal)
    started = Event()
    release = Event()
    real_delete = journal.delete_expected

    async with app.run_test(size=(100, 30)) as pilot:
        app._request_open(second)
        await _wait_until(
            pilot,
            lambda: bool(app.document and app.document.path == second),
        )
        await pilot.press("ctrl+pageup")
        assert app.document is not None and app.document.path == first
        record = journal.publish(
            document_path=first,
            workspace_root=tmp_path,
            text="draft",
            encoding="utf-8",
            base_snapshot=app.document.snapshot,
        )
        app._known_recovery_fingerprints[first] = record.fingerprint

        def blocked_delete(document_path: Path, *, fingerprint: str | None) -> None:
            started.set()
            assert release.wait(2)
            real_delete(document_path, fingerprint=fingerprint)

        monkeypatch.setattr(journal, "delete_expected", blocked_delete)
        first.write_text("external\r\nmixed\n", encoding="utf-8")
        app._reload_current_from_disk(automatic=True)
        await _wait_until(pilot, started.is_set)

        assert app._critical_io
        assert app.editor.read_only
        await pilot.press("ctrl+pagedown")
        assert app.document is not None and app.document.path == first

        release.set()
        await _wait_until(pilot, lambda: isinstance(app.screen, MixedLineEndingsDialog))
        assert app.document.path == first
        dialog = app.screen
        await pilot.pause()
        await pilot.click("#mixed-cancel")
        await _wait_until(pilot, lambda: app.screen is not dialog)
        assert app.document.read_only
        assert app.editor.read_only

        await pilot.press("ctrl+pagedown")
        assert app.document.path == second
        assert not app.document.read_only
        assert not app.editor.read_only


async def test_watcher_probe_allows_edit_and_classifies_result_against_latest_dirty_state(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)
    started = Event()
    release = Event()
    probe_threads: list[int] = []

    def blocked_probe(requested: Path) -> DiskProbe:
        probe_threads.append(get_ident())
        started.set()
        assert release.wait(2)
        return probe_file(requested)

    monkeypatch.setattr("termwriter.app.probe_file", blocked_probe)

    async with app.run_test(size=(100, 30)) as pilot:
        path.write_text("external", encoding="utf-8")
        app._check_external_in_background()
        await _wait_until(pilot, started.is_set)

        assert not app._critical_io
        assert not app.editor.read_only
        await pilot.press("x")
        assert app.document is not None
        assert app.document.text == "xbase"

        release.set()
        await _wait_until(pilot, lambda: bool(app.document and app.document.conflict))

        assert app.document.text == "xbase"
        assert app.document.last_save_status == "External conflict"
        assert path.read_text(encoding="utf-8") == "external"
        assert probe_threads and all(thread != get_ident() for thread in probe_threads)


async def test_edit_during_transition_probe_reenters_unsaved_guard(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    first.write_text("first", encoding="utf-8")
    app = _app(first)
    started = Event()
    release = Event()

    def blocked_probe(requested: Path) -> DiskProbe:
        started.set()
        assert release.wait(2)
        return probe_file(requested)

    monkeypatch.setattr("termwriter.app.probe_file", blocked_probe)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_close_tab()
        await _wait_until(pilot, started.is_set)

        assert app._critical_io
        assert not app.editor.read_only
        app.editor.focus()
        with app.editor.prevent(TextArea.Changed):
            app.editor.insert("x", (0, 0))
        assert app.editor.text == "xfirst"
        assert app.document is not None
        assert app.document.text == "first"
        release.set()
        await _wait_until(pilot, lambda: isinstance(app.screen, UnsavedChangesDialog))

        assert app.document is not None
        assert app.document.path == first
        assert app.document.text == "xfirst"
        assert first.read_text(encoding="utf-8") == "first"


async def test_blocked_save_is_read_only_and_cannot_be_quit_mid_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)
    started = Event()
    release = Event()
    save_threads: list[int] = []

    def blocked_save(
        path: Path,
        text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        save_threads.append(get_ident())
        started.set()
        assert release.wait(2)
        return atomic_save(path, text, encoding=encoding, expected=expected)

    monkeypatch.setattr("termwriter.app.atomic_save", blocked_save)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+s")
        await _wait_until(pilot, started.is_set)

        assert app._critical_io
        assert app.editor.read_only
        before = app.editor.text
        await pilot.press("x", "ctrl+z", "ctrl+y", "ctrl+q")
        assert app.is_running
        assert app.editor.text == before

        release.set()
        await _wait_until(pilot, lambda: bool(app.document and not app.document.dirty))

        assert path.read_text(encoding="utf-8") == "xbase"
        assert app.document is not None
        assert app.document.text == "xbase"
        assert app.document.saved_text == "xbase"
        assert app.editor.text == "xbase"
        assert not app.editor.read_only
        assert save_threads and all(thread != get_ident() for thread in save_threads)


async def test_external_write_during_background_save_opens_conflict(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)
    started = Event()
    release = Event()

    def blocked_save(
        path: Path,
        text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        started.set()
        assert release.wait(2)
        return atomic_save(path, text, encoding=encoding, expected=expected)

    monkeypatch.setattr("termwriter.app.atomic_save", blocked_save)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+s")
        await _wait_until(pilot, started.is_set)
        path.write_text("external", encoding="utf-8")
        release.set()
        await _wait_until(pilot, lambda: isinstance(app.screen, ConflictDialog))

        assert app.document is not None
        assert app.document.text == "xbase"
        assert app.document.dirty
        assert path.read_text(encoding="utf-8") == "external"
        assert not app.editor.read_only


async def test_unexpected_save_worker_error_restores_editor(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    def fail_save(
        _path: Path,
        _text: str,
        *,
        encoding: str,
        expected: FileSnapshot,
    ) -> SaveResult:
        del encoding, expected
        raise RuntimeError("worker exploded")

    monkeypatch.setattr("termwriter.app.atomic_save", fail_save)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+s")
        await _wait_until(
            pilot,
            lambda: bool(
                app.document
                and app.document.last_save_status == "Save failed"
                and not app._critical_io
            ),
        )

        assert app.document is not None
        assert app.document.dirty
        assert not app.editor.read_only
        assert path.read_text(encoding="utf-8") == "base"


async def test_save_as_dialog_stays_locked_until_background_publication_finishes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    copy = tmp_path / "note-local.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        path.write_text("external", encoding="utf-8")
        await pilot.press("ctrl+s")
        await _wait_until(pilot, lambda: isinstance(app.screen, ConflictDialog))
        await pilot.click("#conflict-save-as")
        assert isinstance(app.screen, SaveAsDialog)

        started = Event()
        release = Event()

        def blocked_save(
            target: Path,
            text: str,
            *,
            encoding: str,
            expected: FileSnapshot,
        ) -> SaveResult:
            started.set()
            assert release.wait(2)
            return atomic_save(target, text, encoding=encoding, expected=expected)

        monkeypatch.setattr("termwriter.app.atomic_save", blocked_save)
        await pilot.press("enter")
        await _wait_until(pilot, started.is_set)

        dialog = app.screen
        assert isinstance(dialog, SaveAsDialog)
        assert dialog.query_one("#save-as-input", Input).disabled
        assert dialog.query_one("#save-as-confirm", Button).disabled
        await pilot.press("escape", "enter")
        assert app.screen is dialog

        release.set()
        await _wait_until(pilot, lambda: bool(app.document and app.document.path == copy))

        assert copy.read_text(encoding="utf-8") == "xbase"
        assert path.read_text(encoding="utf-8") == "external"


async def test_orphan_source_validation_loads_off_the_ui_thread(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
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
    ui_thread = get_ident()
    load_threads: list[int] = []

    def tracked_load(requested: Path) -> LoadedFile:
        load_threads.append(get_ident())
        return load_file(requested)

    monkeypatch.setattr("termwriter.app.load_file", tracked_load)
    app = TermWriterApp(
        Workspace.from_target(workspace),
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)):
        assert isinstance(app.screen, RecoveryDialog)
        assert load_threads
        assert all(thread != ui_thread for thread in load_threads)
