"""Opt-in read-only experiment for independently rendered source blocks."""

from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Markdown, Static

from termwriter.services.markdown_preview import preview_parser
from termwriter.services.semantic_blocks import SemanticBlock, SemanticBlockMap

_RENDERED_KINDS = frozenset({"heading", "paragraph"})


class SemanticReaderDialog(ModalScreen[None]):
    """Render only proven simple blocks and show every other block as source."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Return to source", show=False),
    ]

    def __init__(self, mapping: SemanticBlockMap) -> None:
        self.segments = tuple(
            segment for segment in mapping.segments if segment.kind != "separator"
        )
        self.rendered_segments = tuple(
            segment for segment in self.segments if segment.kind in _RENDERED_KINDS
        )
        self.fallback_segments = tuple(
            segment for segment in self.segments if segment.kind not in _RENDERED_KINDS
        )
        super().__init__(id="semantic-reader-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="semantic-reader-dialog"):
            yield Static("Experimental semantic reading", classes="dialog-title", markup=False)
            yield Static(
                "Headings and paragraphs render independently. Every other construct stays "
                "visible as exact Markdown source.",
                classes="dialog-message",
                markup=False,
            )
            with VerticalScroll(id="semantic-reader-content", can_focus=True):
                if not self.segments:
                    yield Static("This document has no visible source blocks.", markup=False)
                for index, segment in enumerate(self.segments):
                    with Vertical(classes="semantic-reading-block"):
                        yield Static(self._label(segment), classes="semantic-reading-label")
                        if segment.kind in _RENDERED_KINDS:
                            yield Markdown(
                                segment.source,
                                id=f"semantic-rendered-{index}",
                                classes="semantic-rendered-block",
                                parser_factory=preview_parser,
                                open_links=False,
                            )
                        else:
                            yield Static(
                                segment.source,
                                id=f"semantic-source-{index}",
                                classes="semantic-source-fallback",
                                markup=False,
                            )
            with Horizontal(classes="dialog-buttons"):
                yield Button(
                    "Return to source",
                    id="semantic-reader-close",
                    variant="primary",
                )

    @staticmethod
    def _label(segment: SemanticBlock) -> str:
        first_line = segment.start_line + 1
        last_line = max(first_line, segment.end_line)
        mode = "rendered" if segment.kind in _RENDERED_KINDS else "source fallback"
        return f"{segment.kind} · lines {first_line}-{last_line} · {mode}"

    def on_mount(self) -> None:
        self.query_one("#semantic-reader-content", VerticalScroll).focus()

    @on(Markdown.LinkClicked)
    def keep_links_inert(self, event: Markdown.LinkClicked) -> None:
        event.stop()

    @on(Button.Pressed, "#semantic-reader-close")
    def close_button(self) -> None:
        self.dismiss(None)

    def action_close(self) -> None:
        self.dismiss(None)
