"""Modal display for read-only cursor coordinate diagnostics."""

from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal
from textual.geometry import Offset
from textual.screen import ModalScreen
from textual.widgets import Button, Static

from termdraft.services.coordinate_diagnostic import CoordinateDiagnostic
from termdraft.widgets.dialog import TerminalDialog


class CoordinateInspectorDialog(ModalScreen[None]):
    """Show the current cursor in source, wrapped, and screen coordinates."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Close", show=False),
    ]

    def __init__(self, diagnostic: CoordinateDiagnostic, screen_offset: Offset) -> None:
        self.diagnostic = diagnostic
        self.screen_offset = screen_offset
        super().__init__(id="coordinate-inspector-screen")

    def compose(self) -> ComposeResult:
        diagnostic = self.diagnostic
        split_warning = (
            "yes — unsafe for block editing" if diagnostic.wrap_splits_grapheme else "no"
        )
        with TerminalDialog("Cursor coordinate diagnostic", id="coordinate-inspector-dialog"):
            yield Static(
                "\n".join(
                    (
                        f"Source character offset: {diagnostic.source_offset}",
                        f"UTF-8 byte offset: {diagnostic.utf8_byte_offset}",
                        f"Logical location: line {diagnostic.logical_line}, "
                        f"column {diagnostic.logical_column}",
                        f"Wrapped location: row {diagnostic.visual_row}, "
                        f"cell {diagnostic.visual_cell}",
                        f"Terminal screen: row {self.screen_offset.y}, cell {self.screen_offset.x}",
                        f"At grapheme boundary: {'yes' if diagnostic.grapheme_boundary else 'no'}",
                        f"Wrap splits a grapheme: {split_warning}",
                    )
                ),
                classes="dialog-message",
                markup=False,
            )
            yield Static(
                "Coordinates are a read-only snapshot. Terminal width rules, IME input, and "
                "bidirectional text remain outside this diagnostic.",
                classes="dialog-message",
                markup=False,
            )
            with Horizontal(classes="dialog-buttons"):
                yield Button("Close", id="coordinate-inspector-close", variant="primary")

    @on(Button.Pressed, "#coordinate-inspector-close")
    def close_button(self) -> None:
        self.dismiss(None)

    def action_close(self) -> None:
        self.dismiss(None)
