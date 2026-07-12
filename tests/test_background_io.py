"""Race-focused Pilot tests for background document hashing and publication."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from threading import Event, get_ident

import pytest
from textual.pilot import Pilot
from textual.widgets import Button, Input

from termwriter.app import TermWriterApp
from termwriter.models.document import FileSnapshot
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import (
    ConflictDialog,
    RecoveryDialog,
    SaveAsDialog,
    UnsavedChangesDialog,
)
from termwriter.services.external_changes import DiskProbe, probe_file
from termwriter.services.persistence import LoadedFile, SaveResult, atomic_save, load_file
from termwriter.services.recovery import RecoveryJournal


def _app(path: Path, *, journal: RecoveryJournal | None = None) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=journal or RecoveryJournal(path.parent / ".test-recovery"),
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
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)
    started = Event()
    release = Event()

    def blocked_probe(requested: Path) -> DiskProbe:
        started.set()
        assert release.wait(2)
        return probe_file(requested)

    monkeypatch.setattr("termwriter.app.probe_file", blocked_probe)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+p")
        await pilot.press("s", "e", "c", "o", "n", "d", "enter")
        await _wait_until(pilot, started.is_set)

        assert app._critical_io
        assert not app.editor.read_only
        app.editor.focus()
        await pilot.press("x")
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
        await pilot.press("x", "ctrl+q")
        assert app.is_running
        assert app.editor.text == before

        release.set()
        await _wait_until(pilot, lambda: bool(app.document and not app.document.dirty))

        assert path.read_text(encoding="utf-8") == "xbase"
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
