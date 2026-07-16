"""Coverage for the default inline Markdown presentation."""

from __future__ import annotations

import pytest
from rich.color import Color

from termdraft.services.inline_preview import render_inline_preview_line, table_line_kind
from termdraft.widgets.editor import MarkdownEditor


def test_inactive_line_preview_preserves_source_positions() -> None:
    source = "- [x] Read **carefully** and [follow](https://example.com)"

    rendered = render_inline_preview_line(source)

    assert len(rendered.plain) == len(source)
    assert rendered.plain.startswith("• ☑   Read   carefully")
    assert "follow" in rendered.plain
    assert "https://example.com" not in rendered.plain


def test_editor_keeps_only_the_cursor_line_as_exact_source() -> None:
    editor = MarkdownEditor(
        "# Heading\nThis is **bold**.",
        inline_preview=True,
        read_only=False,
    )

    assert editor.get_line(0).plain == "# Heading"
    assert editor.get_line(1).plain == "This is   bold  ."

    editor.move_cursor((1, 0))

    assert editor.get_line(0).plain == "| Heading"
    assert editor.get_line(1).plain == "This is **bold**."


def test_editor_can_switch_from_split_source_to_inline_preview() -> None:
    editor = MarkdownEditor("active\n# Inactive", read_only=False)
    assert editor.get_line(1).plain == "# Inactive"

    editor.set_inline_preview(True)

    assert editor.get_line(1).plain == "| Inactive"


def test_inline_preview_renders_table_rows_without_moving_source_positions() -> None:
    lines = [
        "| Name  | Status |",
        "| :---- | -----: |",
        "| Draft | **ready**  |",
    ]
    editor = MarkdownEditor("\n".join(["Before", *lines]), inline_preview=True, read_only=False)

    assert table_line_kind(lines, 0) == "header"
    assert table_line_kind(lines, 1) == "separator"
    assert table_line_kind(lines, 2) == "body"
    assert editor.get_line(1).plain == "│ Name  │ Status │"
    assert editor.get_line(2).plain == "├───────┼────────┤"
    assert editor.get_line(3).plain == "│ Draft │   ready    │"
    assert all(len(editor.get_line(index).plain) == len(lines[index - 1]) for index in range(1, 4))

    editor.move_cursor((3, 0))

    assert editor.get_line(3).plain == lines[2]


def test_inline_preview_leaves_non_table_pipes_unchanged() -> None:
    editor = MarkdownEditor(
        "Active\nThis is prose | with an aside.",
        inline_preview=True,
        read_only=False,
    )

    assert editor.get_line(1).plain == "This is prose | with an aside."


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("# Document title", "| Document title"),
        ("## Main section", " | Main section"),
        ("### Subsection", "  | Subsection"),
        ("#### Detail", "   . Detail"),
        ("##### Minor detail", "    . Minor detail"),
        ("###### Note", "     . Note"),
    ],
)
def test_inline_headings_show_level_without_moving_text(source: str, expected: str) -> None:
    rendered = render_inline_preview_line(source)

    assert rendered.plain == expected
    assert len(rendered.plain) == len(source)
    assert len(rendered.plain.encode()) == len(source.encode())


def test_editor_uses_a_bright_heading_syntax_color() -> None:
    editor = MarkdownEditor("# Heading", inline_preview=True, read_only=False)

    assert editor._theme is not None
    assert editor._theme.syntax_styles["heading"].color == Color.parse("#f2f2f2")
