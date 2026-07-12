"""Focused tests for the persisted recent-document switcher."""

from __future__ import annotations

from pathlib import Path

from textual.pilot import Pilot
from textual.widgets import OptionList

from termwriter.app import TermWriterApp
from termwriter.config import load_config
from termwriter.models.workspace import Workspace
from termwriter.screens.recent_documents import RecentDocumentsDialog
from termwriter.services.recovery import RecoveryJournal
from termwriter.services.session import (
    MAX_SESSION_DOCUMENTS,
    DocumentViewState,
    SessionState,
    SessionStore,
)


def _app(
    target: Path,
    store: SessionStore,
    *,
    config_root: Path | None = None,
) -> TermWriterApp:
    config = None if config_root is None else load_config(config_root)
    return TermWriterApp(
        Workspace.from_target(target),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(target.parent / ".test-recovery"),
        session_store=store,
        config=config,
    )


async def _wait_for_document(
    app: TermWriterApp,
    pilot: Pilot[None],
    expected: Path,
) -> None:
    for _ in range(100):
        document = app.document
        if document is not None and document.path == expected:
            return
        await pilot.pause(0.01)
    raise AssertionError(f"document did not open: {expected}")


async def test_recent_switcher_uses_and_persists_mru_order(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    third = tmp_path / "third.md"
    for path in (first, second, third):
        path.write_text(path.stem, encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    app = _app(first, store)

    async with app.run_test(size=(100, 30)) as pilot:
        app._request_open(second)
        await _wait_for_document(app, pilot, second)
        app._request_open(third)
        await _wait_for_document(app, pilot, third)

        await pilot.press("ctrl+o")

        assert isinstance(app.screen, RecentDocumentsDialog)
        assert app.screen.paths == (third, second, first)
        assert "current" in str(
            app.screen.query_one("#recent-documents-list", OptionList).get_option_at_index(0).prompt
        )

    state = store.load(tmp_path).state
    assert state is not None
    assert state.active_path == third
    assert tuple(view.path for view in state.documents) == (third, second, first)


async def test_recent_switching_preserves_dirty_source_in_an_open_tab(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            first,
            (DocumentViewState(first), DocumentViewState(second)),
        )
    )
    app = _app(first, store)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x", "ctrl+o", "down", "enter")
        await _wait_for_document(app, pilot, second)

        assert app.document is not None
        assert app.document.path == second
        assert first.read_text(encoding="utf-8") == "first"
        first_buffer = app._open_document_for_path(first)
        assert first_buffer is not None
        assert first_buffer.text == "xfirst"
        assert first_buffer.dirty

        await pilot.press("ctrl+o", "down", "enter")
        await _wait_for_document(app, pilot, first)

        assert app.editor.text == "xfirst"
        assert app.document.dirty
        assert first.read_text(encoding="utf-8") == "first"


async def test_recent_switcher_skips_missing_session_paths(tmp_path: Path) -> None:
    active = tmp_path / "active.md"
    missing = tmp_path / "missing.md"
    active.write_text("active", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            active,
            (DocumentViewState(active), DocumentViewState(missing)),
        )
    )
    app = _app(active, store)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+o")

        assert isinstance(app.screen, RecentDocumentsDialog)
        assert app.screen.paths == (active,)
        for _ in range(100):
            state = store.load(tmp_path).state
            if state is not None and tuple(view.path for view in state.documents) == (active,):
                break
            await pilot.pause(0.01)
        else:
            raise AssertionError("pruned MRU session was not persisted")


def test_recent_document_cache_evicts_oldest_view_at_limit(tmp_path: Path) -> None:
    active = tmp_path / "active.md"
    active.write_text("active", encoding="utf-8")
    app = _app(active, SessionStore(tmp_path / "sessions"))
    paths = [tmp_path / f"note-{index}.md" for index in range(MAX_SESSION_DOCUMENTS + 1)]

    for path in paths:
        app._session_views[path] = DocumentViewState(path)
        app._mark_document_recent(path)

    assert len(app._recent_paths) == MAX_SESSION_DOCUMENTS
    assert paths[0] not in app._recent_paths
    assert paths[0] not in app._session_views
    assert app._recent_paths[0] == paths[-1]


async def test_recent_binding_is_configurable_and_command_is_discoverable(
    tmp_path: Path,
) -> None:
    active = tmp_path / "active.md"
    active.write_text("active", encoding="utf-8")
    config_root = tmp_path / "config"
    config_root.mkdir()
    (config_root / "config.toml").write_text(
        '[keybindings]\nrecent_documents = "ctrl+r"\n',
        encoding="utf-8",
    )
    app = _app(active, SessionStore(tmp_path / "sessions"), config_root=config_root)

    async with app.run_test(size=(100, 30)) as pilot:
        command_titles = {command.title for command in app.get_system_commands(app.screen)}
        assert "Open recent document" in command_titles

        await pilot.press("ctrl+o")
        assert not isinstance(app.screen, RecentDocumentsDialog)

        await pilot.press("ctrl+r")
        assert isinstance(app.screen, RecentDocumentsDialog)


async def test_recent_switcher_waits_for_critical_file_io(tmp_path: Path) -> None:
    active = tmp_path / "active.md"
    other = tmp_path / "other.md"
    active.write_text("active", encoding="utf-8")
    other.write_text("other", encoding="utf-8")
    store = SessionStore(tmp_path / "sessions")
    store.save(
        SessionState(
            tmp_path,
            active,
            (DocumentViewState(active), DocumentViewState(other)),
        )
    )
    app = _app(active, store)

    async with app.run_test(size=(100, 30)) as pilot:
        app._critical_io = True
        await pilot.press("ctrl+o")

        assert not isinstance(app.screen, RecentDocumentsDialog)
        assert app.document is not None
        assert app.document.path == active

        app._critical_io = False
