"""Compact scrollbar rendering for terminal panes."""

from rich.color import Color
from rich.segment import Segment, Segments
from rich.style import Style
from textual.scrollbar import ScrollBarRender
from textual.widget import Widget

THIN_SCROLLBAR_GLYPH = "▐"
_DEFAULT_BACK_COLOR = Color.parse("#555555")
_DEFAULT_BAR_COLOR = Color.parse("bright_magenta")


class ThinScrollBarRender(ScrollBarRender):
    """Draw a vertical thumb in half of its interactive terminal cell."""

    @classmethod
    def render_bar(
        cls,
        size: int = 25,
        virtual_size: float = 50,
        window_size: float = 20,
        position: float = 0,
        thickness: int = 1,
        vertical: bool = True,
        back_color: Color = _DEFAULT_BACK_COLOR,
        bar_color: Color = _DEFAULT_BAR_COLOR,
    ) -> Segments:
        rendered = super().render_bar(
            size,
            virtual_size,
            window_size,
            position,
            thickness,
            vertical,
            back_color,
            bar_color,
        )
        if not vertical:
            return rendered

        segments: list[Segment] = []
        for segment in rendered.segments:
            meta = None if segment.style is None else segment.style.meta
            is_thumb = meta is not None and meta.get("@mouse.down") == "grab"
            segments.append(
                Segment(
                    (THIN_SCROLLBAR_GLYPH if is_thumb else " ") * thickness,
                    Style(
                        color=bar_color if is_thumb else None,
                        bgcolor=back_color,
                        meta=meta,
                    ),
                )
            )
        return Segments(segments, new_lines=True)


def use_thin_vertical_scrollbar(widget: Widget) -> None:
    """Install the compact renderer on one pane without affecting global scrollbars."""
    # Textual supports per-instance renderers even though its annotation is class-level.
    widget.vertical_scrollbar.renderer = ThinScrollBarRender  # type: ignore[misc]
