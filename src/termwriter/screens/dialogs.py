"""Typed modal decisions used by the TermWriter coordinator."""

from __future__ import annotations

from enum import Enum, auto
from pathlib import Path
from typing import ClassVar

from textual import on
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, OptionList, Static
from textual.widgets.option_list import Option

from termwriter.services.file_search import search_files

_DIALOG_CSS = """
ModalScreen {
    align: center middle;
    background: $background 65%;
}

.dialog {
    width: 68;
    max-width: 94%;
    height: auto;
    max-height: 90%;
    padding: 1 2;
    background: $surface;
    border: round $primary;
}

.dialog-title {
    height: 1;
    margin-bottom: 1;
    text-style: bold;
}

.dialog-message {
    height: auto;
    margin-bottom: 1;
}

.dialog-buttons {
    height: 3;
    align-horizontal: right;
}

.dialog-buttons Button {
    margin-left: 1;
}

#search-dialog {
    height: 24;
}

#search-input {
    margin-bottom: 1;
}

#search-results {
    height: 1fr;
    border: round $panel-lighten-2;
}

#save-as-error {
    color: $error;
    height: auto;
    min-height: 1;
}

#help-shortcuts {
    height: auto;
}
"""


class UnsavedDecision(Enum):
    SAVE = auto()
    DISCARD = auto()
    CANCEL = auto()


class ConflictDecision(Enum):
    SAVE_AS = auto()
    RELOAD = auto()
    CANCEL = auto()


class UnsavedChangesDialog(ModalScreen[UnsavedDecision | None]):
    """Require an actual decision before a dirty document is left behind."""

    CSS = _DIALOG_CSS
    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(self, path: Path) -> None:
        self.path = path
        super().__init__(id="unsaved-dialog-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="unsaved-dialog"):
            yield Static("Unsaved changes", classes="dialog-title", markup=False)
            yield Static(
                f"Save changes to {self.path.name} before continuing?",
                classes="dialog-message",
                markup=False,
            )
            with Horizontal(classes="dialog-buttons"):
                yield Button("Save", id="unsaved-save", variant="primary")
                yield Button("Discard", id="unsaved-discard", variant="error")
                yield Button("Cancel", id="unsaved-cancel")

    @on(Button.Pressed)
    def choose(self, event: Button.Pressed) -> None:
        decisions = {
            "unsaved-save": UnsavedDecision.SAVE,
            "unsaved-discard": UnsavedDecision.DISCARD,
            "unsaved-cancel": UnsavedDecision.CANCEL,
        }
        if event.button.id in decisions:
            self.dismiss(decisions[event.button.id])

    def action_cancel(self) -> None:
        self.dismiss(UnsavedDecision.CANCEL)


class ConflictDialog(ModalScreen[ConflictDecision | None]):
    """Prevent an external version from being silently overwritten."""

    CSS = _DIALOG_CSS
    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(self, path: Path, *, can_reload: bool, unavailable: bool = False) -> None:
        self.path = path
        self.can_reload = can_reload
        self.unavailable = unavailable
        super().__init__(id="conflict-dialog-screen")

    def compose(self) -> ComposeResult:
        if self.can_reload:
            message = f"{self.path.name} changed outside TermWriter. Choose which version to keep."
        elif self.unavailable:
            message = (
                f"{self.path.name} cannot be read or verified. "
                "The original path will not be changed."
            )
        else:
            message = f"{self.path.name} no longer exists. The original path will not be recreated."
        with Vertical(classes="dialog", id="conflict-dialog"):
            yield Static("External change conflict", classes="dialog-title", markup=False)
            yield Static(message, classes="dialog-message", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("Save local as…", id="conflict-save-as", variant="primary")
                yield Button(
                    "Reload external",
                    id="conflict-reload",
                    disabled=not self.can_reload,
                )
                yield Button("Cancel", id="conflict-cancel")

    @on(Button.Pressed)
    def choose(self, event: Button.Pressed) -> None:
        decisions = {
            "conflict-save-as": ConflictDecision.SAVE_AS,
            "conflict-reload": ConflictDecision.RELOAD,
            "conflict-cancel": ConflictDecision.CANCEL,
        }
        if event.button.id in decisions:
            self.dismiss(decisions[event.button.id])

    def action_cancel(self) -> None:
        self.dismiss(ConflictDecision.CANCEL)


class SaveAsDialog(ModalScreen[str | None]):
    """Collect a new workspace-relative Markdown path."""

    CSS = _DIALOG_CSS
    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(self, suggested_path: str, error: str | None = None) -> None:
        self.suggested_path = suggested_path
        self.error = error
        super().__init__(id="save-as-dialog-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="save-as-dialog"):
            yield Static("Save local version as", classes="dialog-title", markup=False)
            yield Input(
                self.suggested_path,
                placeholder="notes/local-copy.md",
                id="save-as-input",
            )
            yield Static(self.error or "", id="save-as-error", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("Save copy", id="save-as-confirm", variant="primary")
                yield Button("Cancel", id="save-as-cancel")

    def on_mount(self) -> None:
        self.query_one("#save-as-input", Input).focus()

    @on(Input.Submitted, "#save-as-input")
    def submit_input(self, event: Input.Submitted) -> None:
        self.dismiss(event.value.strip())

    @on(Button.Pressed, "#save-as-confirm")
    def submit_button(self) -> None:
        self.dismiss(self.query_one("#save-as-input", Input).value.strip())

    @on(Button.Pressed, "#save-as-cancel")
    def cancel_button(self) -> None:
        self.dismiss(None)

    def action_cancel(self) -> None:
        self.dismiss(None)


class FileSearchDialog(ModalScreen[Path | None]):
    """Search and select Markdown files from the validated workspace index."""

    CSS = _DIALOG_CSS
    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "cancel", "Cancel", show=False),
        Binding("down", "focus_results", "Results", show=False),
    ]

    def __init__(self, files: tuple[Path, ...], root: Path) -> None:
        self.files = files
        self.root = root
        self.matches: tuple[Path, ...] = ()
        super().__init__(id="file-search-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="search-dialog"):
            yield Static("Find Markdown file", classes="dialog-title", markup=False)
            yield Input(placeholder="Type part of a path…", id="search-input")
            yield OptionList(id="search-results", markup=False)

    def on_mount(self) -> None:
        self._set_query("")
        self.query_one("#search-input", Input).focus()

    def _set_query(self, query: str) -> None:
        self.matches = search_files(self.files, query, root=self.root)
        options = [
            Option(path.relative_to(self.root).as_posix(), id=str(index))
            for index, path in enumerate(self.matches)
        ]
        if not options:
            options = [Option("No matching Markdown files", disabled=True)]
        results = self.query_one("#search-results", OptionList)
        results.set_options(options)
        results.highlighted = 0 if self.matches else None

    @on(Input.Changed, "#search-input")
    def search(self, event: Input.Changed) -> None:
        self._set_query(event.value)

    @on(Input.Submitted, "#search-input")
    def open_first(self) -> None:
        if self.matches:
            self.dismiss(self.matches[0])

    @on(OptionList.OptionSelected, "#search-results")
    def open_selected(self, event: OptionList.OptionSelected) -> None:
        if 0 <= event.option_index < len(self.matches):
            self.dismiss(self.matches[event.option_index])

    def action_focus_results(self) -> None:
        if self.matches:
            self.query_one("#search-results", OptionList).focus()

    def action_cancel(self) -> None:
        self.dismiss(None)


class HelpDialog(ModalScreen[None]):
    """Display the centralized shortcut list."""

    CSS = _DIALOG_CSS
    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Close", show=False),
        Binding("f1", "close", "Close", show=False),
    ]

    def __init__(self, shortcuts: str) -> None:
        self.shortcuts = shortcuts
        super().__init__(id="help-dialog-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="help-dialog"):
            yield Static("TermWriter shortcuts", classes="dialog-title", markup=False)
            yield Static(self.shortcuts, id="help-shortcuts", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("Close", id="help-close", variant="primary")

    @on(Button.Pressed, "#help-close")
    def close_button(self) -> None:
        self.dismiss(None)

    def action_close(self) -> None:
        self.dismiss(None)
