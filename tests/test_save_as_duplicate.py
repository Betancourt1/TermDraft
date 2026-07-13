"""Normal Save As and non-retargeting duplicate workflows."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from textual.pilot import Pilot
from textual.widgets import Input

from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import SaveAsDialog, SaveAsOperation
from termwriter.services.recovery import RecoveryJournal


def _app(path: Path, journal: RecoveryJournal | None = None) -> TermWriterApp:
    return TermWriterApp(
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


async def _submit_path(pilot: Pilot[None], app: TermWriterApp, value: str) -> SaveAsDialog:
    dialog = app.screen
    assert isinstance(dialog, SaveAsDialog)
    dialog.query_one("#save-as-input", Input).value = value
    await pilot.press("enter")
    return dialog


async def test_normal_save_as_retargets_cleanly_and_preserves_old_disk_file(
    tmp_path: Path,
) -> None:
    original = tmp_path / "original.md"
    target = tmp_path / "renamed.md"
    original.write_text("base", encoding="utf-8")
    app = _app(original)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "ctrl+shift+s")
        assert isinstance(app.screen, SaveAsDialog)
        assert app.screen.operation is SaveAsOperation.RETARGET
        await _submit_path(pilot, app, target.name)
        await _wait_until(
            pilot,
            lambda: app.document is not None and app.document.path == target,
        )

        assert original.read_text(encoding="utf-8") == "base"
        assert target.read_text(encoding="utf-8") == "xbase"
        assert app.document is not None and not app.document.dirty
        assert app.editor.text == "xbase"


async def test_dirty_duplicate_preserves_original_tab_disk_and_recovery(tmp_path: Path) -> None:
    original = tmp_path / "original.md"
    duplicate = tmp_path / "duplicate.md"
    original.write_text("base", encoding="utf-8")
    journal = RecoveryJournal(tmp_path / "recovery")
    app = _app(original, journal)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        await _wait_until(pilot, lambda: journal.load(original) is not None)
        app.action_duplicate_document()
        await pilot.pause()
        assert isinstance(app.screen, SaveAsDialog)
        assert app.screen.operation is SaveAsOperation.DUPLICATE
        await _submit_path(pilot, app, duplicate.name)
        await _wait_until(pilot, duplicate.exists)

        assert app.document is not None and app.document.path == original
        assert app.document.dirty
        assert app.editor.text == "xbase"
        assert original.read_text(encoding="utf-8") == "base"
        assert duplicate.read_text(encoding="utf-8") == "xbase"
        recovered = journal.load(original)
        assert recovered is not None and recovered.text == "xbase"


async def test_clean_duplicate_keeps_original_clean(tmp_path: Path) -> None:
    original = tmp_path / "original.md"
    duplicate = tmp_path / "duplicate.md"
    original.write_text("base", encoding="utf-8")
    app = _app(original)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_duplicate_document()
        await pilot.pause()
        await _submit_path(pilot, app, duplicate.name)
        await _wait_until(pilot, duplicate.exists)

        assert app.document is not None and app.document.path == original
        assert not app.document.dirty
        assert duplicate.read_text(encoding="utf-8") == "base"


async def test_normal_save_as_rejects_an_existing_destination(tmp_path: Path) -> None:
    original = tmp_path / "original.md"
    existing = tmp_path / "existing.md"
    original.write_text("base", encoding="utf-8")
    existing.write_text("existing", encoding="utf-8")
    app = _app(original)

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("ctrl+shift+s")
        dialog = await _submit_path(pilot, app, existing.name)
        await _wait_until(pilot, lambda: dialog.error is not None)

        assert dialog.error is not None and "already exists" in dialog.error
        assert app.document is not None and app.document.path == original
        assert existing.read_text(encoding="utf-8") == "existing"


async def test_save_as_and_duplicate_are_palette_discoverable(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 30)):
        titles = {command.title for command in app.get_system_commands(app.screen)}

        assert {"Save document as…", "Duplicate document…"} <= titles
