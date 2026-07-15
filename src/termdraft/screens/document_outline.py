"""Searchable navigation for headings in the active document."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal
from textual.screen import ModalScreen
from textual.widgets import Button, Input, OptionList, Static
from textual.widgets.option_list import Option

from termdraft.widgets.dialog import TerminalDialog
from termdraft.widgets.preview import PreviewHeading


class OutlineDestination(Enum):
    """The view that should receive the selected heading."""

    SOURCE = "source"
    PREVIEW = "preview"


@dataclass(frozen=True, slots=True)
class OutlineSelection:
    """One heading and the view where it should be revealed."""

    heading: PreviewHeading
    destination: OutlineDestination


class DocumentOutlineDialog(ModalScreen[OutlineSelection | None]):
    """Filter the rendered heading index and choose a navigation target."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "cancel", "Cancel", show=False),
        Binding("down", "focus_results", "Headings", show=False),
    ]

    def __init__(self, headings: tuple[PreviewHeading, ...]) -> None:
        self.headings = headings
        self.matches = headings
        super().__init__(id="document-outline-screen")

    def compose(self) -> ComposeResult:
        with TerminalDialog("Document outline", id="document-outline-dialog"):
            yield Input(placeholder="Filter headings…", id="document-outline-input")
            yield Static("", id="document-outline-status", markup=False)
            yield OptionList(id="document-outline-results", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button(
                    "Jump to source",
                    id="document-outline-source",
                    variant="primary",
                )
                yield Button("Show in preview", id="document-outline-preview")

    def on_mount(self) -> None:
        self._set_query("")
        self.query_one("#document-outline-input", Input).focus()

    def _set_query(self, query: str) -> None:
        folded_query = query.casefold()
        self.matches = tuple(
            heading for heading in self.headings if folded_query in heading.label.casefold()
        )
        options = [Option(self._label(heading)) for heading in self.matches]
        if not options:
            options = [Option("No matching headings", disabled=True)]

        results = self.query_one("#document-outline-results", OptionList)
        results.set_options(options)
        results.highlighted = 0 if self.matches else None

        count = len(self.matches)
        noun = "heading" if count == 1 else "headings"
        self.query_one("#document-outline-status", Static).update(f"{count} {noun}")
        disabled = not self.matches
        self.query_one("#document-outline-source", Button).disabled = disabled
        self.query_one("#document-outline-preview", Button).disabled = disabled

    @staticmethod
    def _label(heading: PreviewHeading) -> str:
        indentation = "  " * (heading.level - 1)
        return f"{indentation}H{heading.level} {heading.label} · line {heading.source_line + 1}"

    def _selected(self) -> PreviewHeading | None:
        index = self.query_one("#document-outline-results", OptionList).highlighted
        if index is None or not 0 <= index < len(self.matches):
            return None
        return self.matches[index]

    def _choose(self, destination: OutlineDestination) -> None:
        heading = self._selected()
        if heading is not None:
            self.dismiss(OutlineSelection(heading, destination))

    @on(Input.Changed, "#document-outline-input")
    def filter_headings(self, event: Input.Changed) -> None:
        self._set_query(event.value)

    @on(Input.Submitted, "#document-outline-input")
    def jump_to_first_match(self) -> None:
        self._choose(OutlineDestination.SOURCE)

    @on(OptionList.OptionSelected, "#document-outline-results")
    def jump_to_selected(self) -> None:
        self._choose(OutlineDestination.SOURCE)

    @on(Button.Pressed)
    def button_pressed(self, event: Button.Pressed) -> None:
        destinations = {
            "document-outline-source": OutlineDestination.SOURCE,
            "document-outline-preview": OutlineDestination.PREVIEW,
        }
        destination = destinations.get(event.button.id or "")
        if destination is not None:
            self._choose(destination)

    def action_focus_results(self) -> None:
        if self.matches:
            self.query_one("#document-outline-results", OptionList).focus()

    def action_cancel(self) -> None:
        self.dismiss(None)
