"""Pilot coverage for independent in-memory document tabs."""

from __future__ import annotations

from pathlib import Path

from textual.pilot import Pilot
from textual.widgets import Tab

from termwriter.app import TermWriterApp
from termwriter.config import load_config
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import (
    RecoveryManagerDialog,
    TextSearchDialog,
    UnsavedChangesDialog,
)
from termwriter.services.recovery import RecoveryJournal


def _app(
    first: Path,
    *,
    recovery_debounce: float = 0.01,
) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=recovery_debounce,
        recovery_journal=RecoveryJournal(first.parent / ".test-recovery"),
    )


async def _wait_for_document(
    app: TermWriterApp,
    pilot: Pilot[None],
    path: Path,
) -> None:
    for _ in range(200):
        if app.document is not None and app.document.path == path:
            return
        await pilot.pause(0.01)
    raise AssertionError(f"document did not activate: {path}")


async def _open(app: TermWriterApp, pilot: Pilot[None], path: Path) -> None:
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
        await pilot.press("x")
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


async def test_switching_tabs_clears_shared_editor_undo_without_cross_buffer_edit(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await _open(app, pilot, second)
        await pilot.press("y", "ctrl+pageup", "ctrl+z")

        assert app.document is not None
        assert app.document.path == first
        assert app.document.text == "xfirst"
        second_buffer = app._open_document_for_path(second)
        assert second_buffer is not None
        assert second_buffer.text == "ysecond"


async def test_saving_active_tab_does_not_write_or_clean_another_buffer(
    tmp_path: Path,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
        await _open(app, pilot, second)
        await pilot.press("y", "ctrl+s")
        for _ in range(200):
            if second.read_text(encoding="utf-8") == "ysecond":
                break
            await pilot.pause(0.01)

        first_buffer = app._open_document_for_path(first)
        second_buffer = app._open_document_for_path(second)
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
    app = TermWriterApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=10,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
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
    app = TermWriterApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await _open(app, pilot, second)
        await pilot.press("x", "ctrl+f4")
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.click("#unsaved-cancel")

        assert len(app._open_documents) == 2
        assert app.document is not None and app.document.path == second

        await pilot.press("ctrl+f4")
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.click("#unsaved-discard")
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
        await pilot.press("x")
        await _open(app, pilot, second)
        await pilot.press("y", "ctrl+q")
        assert isinstance(app.screen, UnsavedChangesDialog)
        assert app.document is not None and app.document.path == second

        await pilot.click("#unsaved-save")
        for _ in range(200):
            if isinstance(app.screen, UnsavedChangesDialog) and app.document.path == first:
                break
            await pilot.pause(0.01)

        assert second.read_text(encoding="utf-8") == "ysecond"
        assert isinstance(app.screen, UnsavedChangesDialog)
        await pilot.click("#unsaved-cancel")

        assert app.is_running
        assert app.document.path == first
        assert app.document.dirty
        assert first.read_text(encoding="utf-8") == "first"


async def test_reactivating_dirty_tab_detects_external_conflict(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    app = _app(first)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("x")
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
    app = TermWriterApp(
        Workspace.from_target(first),
        preview_debounce=0.01,
        recovery_debounce=0.01,
        recovery_journal=journal,
    )

    async with app.run_test(size=(100, 34)) as pilot:
        await pilot.press("x")
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
        first_tab = next(opened for opened in app._open_documents if opened.document.path == first)
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
    app = TermWriterApp(
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
