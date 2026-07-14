"""Compact find and replace controls for the active document."""

from __future__ import annotations

from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Grid, Vertical
from textual.message import Message
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, Input, Static

from termdraft.services.document_search import DocumentSearchMatch, find_document_matches


class DocumentFindDialog(ModalScreen[None]):
    """Incrementally search and optionally replace the active editor source."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Close", show=False),
        Binding("f3", "next_match", "Next", show=False),
        Binding("shift+f3", "previous_match", "Previous", show=False),
    ]

    class MatchSelected(Message):
        def __init__(self, dialog: DocumentFindDialog, match: DocumentSearchMatch) -> None:
            self.dialog = dialog
            self.match = match
            super().__init__()

    class ReplaceRequested(Message):
        def __init__(
            self,
            dialog: DocumentFindDialog,
            match: DocumentSearchMatch,
            replacement: str,
        ) -> None:
            self.dialog = dialog
            self.match = match
            self.replacement = replacement
            super().__init__()

    class ReplaceAllRequested(Message):
        def __init__(
            self,
            dialog: DocumentFindDialog,
            matches: tuple[DocumentSearchMatch, ...],
            replacement: str,
        ) -> None:
            self.dialog = dialog
            self.matches = matches
            self.replacement = replacement
            super().__init__()

    def __init__(self, source: str, cursor_offset: int, *, read_only: bool) -> None:
        self.source = source
        self.cursor_offset = cursor_offset
        self.read_only = read_only
        self.matches: tuple[DocumentSearchMatch, ...] = ()
        self.selected_index: int | None = None
        super().__init__(id="document-find-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="document-find-dialog"):
            yield Static("Find and replace", classes="dialog-title", markup=False)
            yield Input(placeholder="Find in the active document…", id="document-find-input")
            yield Input(
                placeholder="Replace with…",
                id="document-replace-input",
                disabled=self.read_only,
            )
            yield Checkbox("Match case", compact=True, id="document-find-case")
            yield Static("Enter text to find", id="document-find-status", markup=False)
            with Grid(id="document-find-buttons"):
                yield Button("Previous", id="document-find-previous")
                yield Button("Next", id="document-find-next", variant="primary")
                yield Button("Replace", id="document-replace-one", disabled=self.read_only)
                yield Button("Replace all", id="document-replace-all", disabled=self.read_only)

    def on_mount(self) -> None:
        self._refresh_matches(self.cursor_offset)
        self.query_one("#document-find-input", Input).focus()

    @on(Input.Changed, "#document-find-input")
    @on(Checkbox.Changed, "#document-find-case")
    def query_changed(self) -> None:
        self._refresh_matches(self.cursor_offset)

    @on(Input.Submitted, "#document-find-input")
    def find_submitted(self) -> None:
        self.action_next_match()

    @on(Input.Submitted, "#document-replace-input")
    def replace_submitted(self) -> None:
        self._request_replace()

    @on(Button.Pressed)
    def button_pressed(self, event: Button.Pressed) -> None:
        actions = {
            "document-find-previous": self.action_previous_match,
            "document-find-next": self.action_next_match,
            "document-replace-one": self._request_replace,
            "document-replace-all": self._request_replace_all,
        }
        action = actions.get(event.button.id or "")
        if action is not None:
            action()

    def _refresh_matches(self, anchor_offset: int) -> None:
        query = self.query_one("#document-find-input", Input).value
        case_sensitive = self.query_one("#document-find-case", Checkbox).value
        self.matches = find_document_matches(
            self.source,
            query,
            case_sensitive=case_sensitive,
        )
        self.selected_index = next(
            (index for index, match in enumerate(self.matches) if match.start >= anchor_offset),
            0 if self.matches else None,
        )
        self._show_selection()

    def _show_selection(self) -> None:
        status = self.query_one("#document-find-status", Static)
        has_match = self.selected_index is not None
        if not self.query_one("#document-find-input", Input).value:
            status.update("Enter text to find")
        elif not has_match:
            status.update("No matches")
        else:
            assert self.selected_index is not None
            status.update(f"{self.selected_index + 1} of {len(self.matches)}")
            self.post_message(self.MatchSelected(self, self.matches[self.selected_index]))

        self.query_one("#document-find-previous", Button).disabled = not has_match
        self.query_one("#document-find-next", Button).disabled = not has_match
        replacements_disabled = self.read_only or not has_match
        self.query_one("#document-replace-one", Button).disabled = replacements_disabled
        self.query_one("#document-replace-all", Button).disabled = replacements_disabled

    def action_next_match(self) -> None:
        if not self.matches:
            return
        current = -1 if self.selected_index is None else self.selected_index
        self.selected_index = (current + 1) % len(self.matches)
        self._show_selection()

    def action_previous_match(self) -> None:
        if not self.matches:
            return
        current = 0 if self.selected_index is None else self.selected_index
        self.selected_index = (current - 1) % len(self.matches)
        self._show_selection()

    def _request_replace(self) -> None:
        if self.read_only or self.selected_index is None:
            return
        replacement = self.query_one("#document-replace-input", Input).value
        self.post_message(
            self.ReplaceRequested(self, self.matches[self.selected_index], replacement)
        )

    def _request_replace_all(self) -> None:
        if self.read_only or not self.matches:
            return
        replacement = self.query_one("#document-replace-input", Input).value
        self.post_message(self.ReplaceAllRequested(self, self.matches, replacement))

    def update_source(self, source: str, anchor_offset: int) -> None:
        """Refresh matches after the coordinator applies one editor operation."""
        self.source = source
        self.cursor_offset = anchor_offset
        self._refresh_matches(anchor_offset)

    def action_close(self) -> None:
        self.dismiss(None)
