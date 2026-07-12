"""Coordinate contracts for exact source and wrapped editor positions."""

from pathlib import Path

import pytest
from textual.app import App, ComposeResult

from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.screens.coordinate_inspector import CoordinateInspectorDialog
from termwriter.services.coordinate_diagnostic import diagnose_coordinate
from termwriter.services.recovery import RecoveryJournal
from termwriter.widgets.editor import MarkdownEditor


@pytest.mark.parametrize(
    ("source", "location", "expected_offset"),
    (
        ("a\nb", (1, 0), 2),
        ("a\rb", (1, 0), 2),
        ("a\r\nb", (1, 0), 3),
        ("a\r\nb\nc\rd", (3, 1), 8),
        ("a\r\n", (1, 0), 3),
        ("", (0, 0), 0),
    ),
)
def test_preserves_exact_line_endings(
    source: str,
    location: tuple[int, int],
    expected_offset: int,
) -> None:
    diagnostic = diagnose_coordinate(source, location, wrap_width=20)

    assert diagnostic.source_offset == expected_offset


def test_distinguishes_python_and_utf8_offsets() -> None:
    diagnostic = diagnose_coordinate("a🙂", (0, 2), wrap_width=20)

    assert diagnostic.source_offset == 2
    assert diagnostic.utf8_byte_offset == 5


@pytest.mark.parametrize(
    ("source", "location", "width", "expected"),
    (
        ("abcdefgh", (0, 6), 4, (1, 2)),
        ("a\tb", (0, 2), 8, (0, 4)),
        ("a\tb", (0, 2), 4, (1, 0)),
        ("界界", (0, 1), 2, (1, 0)),
        ("🙂🙂", (0, 1), 2, (1, 0)),
        ("👨‍👩‍👧‍👦x", (0, 7), 4, (0, 2)),
    ),
)
def test_uses_textual_wrapped_cell_coordinates(
    source: str,
    location: tuple[int, int],
    width: int,
    expected: tuple[int, int],
) -> None:
    diagnostic = diagnose_coordinate(source, location, wrap_width=width)

    assert (diagnostic.visual_row, diagnostic.visual_cell) == expected


def test_flags_cursor_inside_combining_grapheme() -> None:
    inside = diagnose_coordinate("e\u0301x", (0, 1), wrap_width=4)
    boundary = diagnose_coordinate("e\u0301x", (0, 2), wrap_width=4)

    assert inside.visual_cell == boundary.visual_cell == 1
    assert not inside.grapheme_boundary
    assert boundary.grapheme_boundary


def test_flags_textual_narrow_wrap_that_splits_zwj_grapheme() -> None:
    diagnostic = diagnose_coordinate("👨‍👩‍👧‍👦x", (0, 7), wrap_width=2)

    assert (diagnostic.visual_row, diagnostic.visual_cell) == (4, 0)
    assert diagnostic.grapheme_boundary
    assert diagnostic.wrap_splits_grapheme


@pytest.mark.parametrize(
    ("source", "location", "wrap_width", "tab_width"),
    (
        ("", (-1, 0), 1, 4),
        ("", (1, 0), 1, 4),
        ("x", (0, -1), 1, 4),
        ("x", (0, 2), 1, 4),
        ("x", (0, 0), -1, 4),
        ("x", (0, 0), 1, 0),
    ),
)
def test_rejects_invalid_coordinates_and_dimensions(
    source: str,
    location: tuple[int, int],
    wrap_width: int,
    tab_width: int,
) -> None:
    with pytest.raises(ValueError):
        diagnose_coordinate(
            source,
            location,
            wrap_width=wrap_width,
            tab_width=tab_width,
        )


class _EditorApp(App[None]):
    def compose(self) -> ComposeResult:
        yield MarkdownEditor("abcdefgh", read_only=False)


async def test_matches_live_textual_wrapped_document() -> None:
    app = _EditorApp()

    async with app.run_test(size=(20, 8)) as pilot:
        await pilot.pause()
        editor = app.query_one(MarkdownEditor)
        editor.move_cursor((0, 6))
        await pilot.pause()
        diagnostic = diagnose_coordinate(
            editor.text,
            editor.cursor_location,
            wrap_width=editor.wrap_width,
            tab_width=editor.indent_width,
        )
        live = editor.wrapped_document.location_to_offset(editor.cursor_location)

        assert (diagnostic.visual_cell, diagnostic.visual_row) == live


async def test_command_opens_read_only_coordinate_snapshot(tmp_path: Path) -> None:
    path = tmp_path / "unicode.md"
    path.write_text("a🙂\n", encoding="utf-8")
    app = TermWriterApp(
        Workspace.from_target(path),
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
    )

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.pause()
        app.editor.move_cursor((0, 2))
        app.action_inspect_cursor_coordinates()
        await pilot.pause()

        dialog = app.screen
        assert isinstance(dialog, CoordinateInspectorDialog)
        assert dialog.diagnostic.source_offset == 2
        assert dialog.diagnostic.utf8_byte_offset == 5
        assert app.document is not None
        assert app.document.text == "a🙂\n"
        assert not app.document.dirty
