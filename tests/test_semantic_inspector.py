"""Pilot coverage for the read-only semantic block inspector."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from threading import Event

import pytest
from textual.pilot import Pilot

from termdraft.app import TermDraftApp
from termdraft.models.workspace import Workspace
from termdraft.screens.dialogs import HelpDialog
from termdraft.screens.semantic_inspector import SemanticInspectorDialog
from termdraft.services.recovery import RecoveryJournal
from termdraft.services.semantic_blocks import SemanticBlockMap, map_semantic_blocks


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
    raise AssertionError("semantic inspector did not reach the expected state")


def _app(path: Path) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(path.parent / "recovery"),
    )


async def test_inspector_is_discoverable_read_only_and_can_jump(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    source = "# Heading\n\nParagraph\n"
    path.write_text(source, encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 38)) as pilot:
        titles = {command.title for command in app.get_system_commands(app.screen)}
        assert "Inspect semantic blocks" in titles
        app.action_inspect_semantic_blocks()
        await _wait_until(pilot, lambda: isinstance(app.screen, SemanticInspectorDialog))

        assert app.document is not None
        assert app.document.text == source
        assert not app.document.dirty
        await pilot.press("down", "enter")

        assert app.editor.cursor_location == (1, 0)
        assert app.document.text == source
        assert path.read_text(encoding="utf-8") == source


async def test_stale_semantic_result_is_discarded_after_edit(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("# Heading\n", encoding="utf-8")
    app = _app(path)
    started = Event()
    release = Event()

    def blocked_map(source: str) -> SemanticBlockMap:
        started.set()
        assert release.wait(2)
        return map_semantic_blocks(source)

    monkeypatch.setattr("termdraft.app.map_semantic_blocks", blocked_map)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_inspect_semantic_blocks()
        await _wait_until(pilot, started.is_set)
        try:
            await pilot.press("i", "x")
            assert app.document is not None and app.document.dirty
        finally:
            release.set()
        await pilot.pause(0.05)

        assert not isinstance(app.screen, SemanticInspectorDialog)


async def test_semantic_result_does_not_stack_over_an_existing_modal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("# Heading\n", encoding="utf-8")
    app = _app(path)
    started = Event()
    release = Event()

    def blocked_map(source: str) -> SemanticBlockMap:
        started.set()
        assert release.wait(2)
        return map_semantic_blocks(source)

    monkeypatch.setattr("termdraft.app.map_semantic_blocks", blocked_map)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_inspect_semantic_blocks()
        await _wait_until(pilot, started.is_set)
        try:
            app.action_show_help()
            assert isinstance(app.screen, HelpDialog)
        finally:
            release.set()
        await pilot.pause(0.05)

        assert isinstance(app.screen, HelpDialog)
        assert not any(isinstance(screen, SemanticInspectorDialog) for screen in app.screen_stack)
