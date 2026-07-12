"""Pilot coverage for the opt-in read-only semantic reading experiment."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from threading import Event

import pytest
from textual.pilot import Pilot

from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.screens.semantic_reader import SemanticReaderDialog
from termwriter.services.recovery import RecoveryJournal
from termwriter.services.semantic_blocks import SemanticBlockMap, map_semantic_blocks


def _app(path: Path) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(path.parent / "recovery"),
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
    raise AssertionError("semantic reader did not reach the expected state")


async def test_reader_renders_only_simple_blocks_and_returns_to_exact_source(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    source = (
        "# Heading\n\n"
        "Paragraph with an [inert link](https://example.com).\n\n"
        "- list item\n\n"
        "```python\nprint('source')\n```\n"
    )
    path.write_text(source, encoding="utf-8")
    app = _app(path)

    async with app.run_test(size=(100, 38)) as pilot:
        titles = {command.title for command in app.get_system_commands(app.screen)}
        assert "Read semantic blocks (experimental)" in titles
        app.editor.move_cursor((2, 5))
        app.action_read_semantic_blocks()
        await _wait_until(pilot, lambda: isinstance(app.screen, SemanticReaderDialog))

        dialog = app.screen
        assert isinstance(dialog, SemanticReaderDialog)
        assert [segment.kind for segment in dialog.rendered_segments] == [
            "heading",
            "paragraph",
        ]
        assert [segment.kind for segment in dialog.fallback_segments] == [
            "bullet list",
            "fenced code",
        ]
        assert "".join(segment.source for segment in dialog.fallback_segments) == (
            "- list item\n\n```python\nprint('source')\n```\n"
        )
        assert len(dialog.query(".semantic-rendered-block")) == 2
        assert len(dialog.query(".semantic-source-fallback")) == 2
        assert app.document is not None
        assert app.document.text == source
        assert not app.document.dirty

        await pilot.press("escape")
        await _wait_until(pilot, lambda: not isinstance(app.screen, SemanticReaderDialog))

        assert app.editor.cursor_location == (2, 5)
        assert app.document.text == source
        assert path.read_text(encoding="utf-8") == source


async def test_reader_keeps_reference_definitions_as_exact_source_fallback(
    tmp_path: Path,
) -> None:
    path = tmp_path / "references.md"
    source = "Paragraph [ref].\r\n\r\n[ref]: https://example.com\r\n"
    path.write_bytes(source.encode("utf-8"))
    app = _app(path)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_read_semantic_blocks()
        await _wait_until(pilot, lambda: isinstance(app.screen, SemanticReaderDialog))

        dialog = app.screen
        assert isinstance(dialog, SemanticReaderDialog)
        assert [segment.kind for segment in dialog.fallback_segments] == [
            "link reference definition"
        ]
        assert dialog.fallback_segments[0].source == "[ref]: https://example.com\r\n"
        assert app.document is not None and app.document.text == source


async def test_stale_reader_result_is_discarded_after_edit(
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

    monkeypatch.setattr("termwriter.app.map_semantic_blocks", blocked_map)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_read_semantic_blocks()
        await _wait_until(pilot, started.is_set)
        try:
            await pilot.press("x")
            assert app.document is not None and app.document.dirty
        finally:
            release.set()
        await pilot.pause(0.05)

        assert not isinstance(app.screen, SemanticReaderDialog)
