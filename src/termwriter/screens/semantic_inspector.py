"""Read-only view of parser-backed Markdown source ranges."""

from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, OptionList, Static
from textual.widgets.option_list import Option

from termwriter.services.semantic_blocks import SemanticBlock, SemanticBlockMap


class SemanticInspectorDialog(ModalScreen[SemanticBlock | None]):
    """List mapped blocks and uncovered source without modifying either."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Close", show=False),
    ]

    def __init__(self, mapping: SemanticBlockMap) -> None:
        self.segments = mapping.segments
        super().__init__(id="semantic-inspector-screen")

    def compose(self) -> ComposeResult:
        options = [
            Option(self._label(segment), id=str(index))
            for index, segment in enumerate(self.segments)
        ]
        if not options:
            options = [Option("No semantic blocks in this document", disabled=True)]
        with Vertical(classes="dialog", id="semantic-inspector-dialog"):
            yield Static("Semantic source blocks", classes="dialog-title", markup=False)
            yield Static(
                "Parser ranges are read-only and use zero-based, end-exclusive internals.",
                classes="dialog-message",
                markup=False,
            )
            yield OptionList(*options, id="semantic-blocks", markup=False)
            yield Static("", id="semantic-block-detail", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("Jump to source", id="semantic-jump", variant="primary")
                yield Button("Close", id="semantic-close")

    def on_mount(self) -> None:
        options = self.query_one("#semantic-blocks", OptionList)
        options.highlighted = 0 if self.segments else None
        options.focus()
        self._refresh_detail()

    def _label(self, segment: SemanticBlock) -> str:
        first_line = segment.start_line + 1
        last_line = max(first_line, segment.end_line)
        detail = f" · {segment.detail}" if segment.detail else ""
        return (
            f"{segment.kind}{detail} · lines {first_line}-{last_line} · "
            f"chars {segment.start_offset}-{segment.end_offset}"
        )

    def _selected(self) -> SemanticBlock | None:
        index = self.query_one("#semantic-blocks", OptionList).highlighted
        if index is None or not 0 <= index < len(self.segments):
            return None
        return self.segments[index]

    def _refresh_detail(self) -> None:
        selected = self._selected()
        detail = self.query_one("#semantic-block-detail", Static)
        if selected is None:
            detail.update("Nothing mapped. Empty documents keep an empty source map.")
            return
        source = selected.source
        if len(source) > 1_200:
            source = source[:1_200] + "\n… preview truncated"
        detail.update(
            f"{selected.kind} · [{selected.start_line}, {selected.end_line}) lines · "
            f"[{selected.start_offset}, {selected.end_offset}) characters\n\n{source}"
        )

    @on(OptionList.OptionHighlighted, "#semantic-blocks")
    def highlight_block(self) -> None:
        self._refresh_detail()

    @on(OptionList.OptionSelected, "#semantic-blocks")
    def select_block(self, event: OptionList.OptionSelected) -> None:
        if 0 <= event.option_index < len(self.segments):
            self.dismiss(self.segments[event.option_index])

    @on(Button.Pressed, "#semantic-jump")
    def jump_to_block(self) -> None:
        selected = self._selected()
        if selected is not None:
            self.dismiss(selected)

    @on(Button.Pressed, "#semantic-close")
    def close_button(self) -> None:
        self.dismiss(None)

    def action_close(self) -> None:
        self.dismiss(None)
