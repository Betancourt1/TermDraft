"""Coverage for the default inline Markdown presentation."""

from __future__ import annotations

import pytest

from termdraft.services.inline_preview import render_inline_preview_line
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

    assert editor.get_line(0).plain == "= Heading"
    assert editor.get_line(1).plain == "This is **bold**."


def test_editor_can_switch_from_split_source_to_inline_preview() -> None:
    editor = MarkdownEditor("active\n# Inactive", read_only=False)
    assert editor.get_line(1).plain == "# Inactive"

    editor.set_inline_preview(True)

    assert editor.get_line(1).plain == "= Inactive"


@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("# Document title", "= Document title"),
        ("## Main section", " - Main section"),
        ("### Subsection", "  : Subsection"),
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


def test_inline_heading_styles_have_a_clear_visual_scale() -> None:
    title = render_inline_preview_line("# Title")
    section = render_inline_preview_line("## Section")
    subsection = render_inline_preview_line("### Subsection")

    assert title.spans[0].style.reverse
    assert section.spans[0].style.bold
    assert subsection.spans[0].style.italic
