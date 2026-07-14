"""Tests for TermWriter's compact scrollbar renderer."""

from termwriter.widgets.scrollbar import THIN_SCROLLBAR_GLYPH, ThinScrollBarRender


def test_thin_scrollbar_uses_half_cell_thumb_and_preserves_mouse_actions() -> None:
    rendered = ThinScrollBarRender.render_bar(
        size=4,
        virtual_size=8,
        window_size=4,
        vertical=True,
    )

    assert [segment.text for segment in rendered.segments] == [
        THIN_SCROLLBAR_GLYPH,
        THIN_SCROLLBAR_GLYPH,
        " ",
        " ",
    ]
    styles = [segment.style for segment in rendered.segments]
    assert all(style is not None for style in styles)
    assert [style.meta["@mouse.down"] for style in styles if style is not None] == [
        "grab",
        "grab",
        "scroll_down",
        "scroll_down",
    ]
