"""Keyboard-first recent document selection."""

from __future__ import annotations

from pathlib import Path
from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.screen import ModalScreen
from textual.widgets import OptionList, Static
from textual.widgets.option_list import Option

from termdraft.widgets.dialog import TerminalDialog


class RecentDocumentsDialog(ModalScreen[Path | None]):
    """Select one validated document from a most-recently-used list."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "cancel", "Cancel", show=False),
    ]

    CSS = """
    #recent-documents-dialog {
        height: 22;
    }

    #recent-documents-list {
        height: 1fr;
        min-height: 5;
        border: round $panel-lighten-2;
    }
    """

    def __init__(
        self,
        paths: tuple[Path, ...],
        workspace_root: Path,
        active_path: Path | None,
    ) -> None:
        self.paths = paths
        self.workspace_root = workspace_root
        self.active_path = active_path
        super().__init__(id="recent-documents-screen")

    def compose(self) -> ComposeResult:
        options = [
            Option(self._label(path), id=str(index)) for index, path in enumerate(self.paths)
        ]
        with TerminalDialog("Recent documents", id="recent-documents-dialog"):
            yield Static(
                "Most recently used first. Select a document and press Enter.",
                classes="dialog-message",
                markup=False,
            )
            yield OptionList(*options, id="recent-documents-list", markup=False)

    def on_mount(self) -> None:
        documents = self.query_one("#recent-documents-list", OptionList)
        documents.highlighted = 0
        documents.focus()

    def _label(self, path: Path) -> str:
        label = path.relative_to(self.workspace_root).as_posix()
        return f"{label} · current" if path == self.active_path else label

    @on(OptionList.OptionSelected, "#recent-documents-list")
    def open_selected(self, event: OptionList.OptionSelected) -> None:
        if 0 <= event.option_index < len(self.paths):
            self.dismiss(self.paths[event.option_index])

    def action_cancel(self) -> None:
        self.dismiss(None)
