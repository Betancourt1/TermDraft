"""Typed modal decisions used by the TermWriter coordinator."""

from __future__ import annotations

from enum import Enum, auto
from pathlib import Path
from typing import ClassVar

from textual import on, work
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.screen import ModalScreen
from textual.widgets import Button, Input, OptionList, Static
from textual.widgets.option_list import Option
from textual.worker import get_current_worker

from termwriter.models.workspace import Workspace
from termwriter.services.file_search import search_files
from termwriter.services.text_search import (
    TextSearchMatch,
    TextSearchOverride,
    TextSearchResult,
    search_text,
)


class UnsavedDecision(Enum):
    SAVE = auto()
    DISCARD = auto()
    CANCEL = auto()


class ConflictDecision(Enum):
    SAVE_AS = auto()
    RELOAD = auto()
    DISCARD = auto()
    CANCEL = auto()


class RecoveryDecision(Enum):
    RESTORE = auto()
    DISCARD = auto()
    CANCEL = auto()


class RecoveryDialog(ModalScreen[RecoveryDecision | None]):
    """Offer a crash journal without silently replacing the disk version."""

    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(
        self,
        path: Path,
        updated_at: str,
        *,
        disk_changed: bool,
        source_missing: bool = False,
    ) -> None:
        self.path = path
        self.updated_at = updated_at
        self.disk_changed = disk_changed
        self.source_missing = source_missing
        super().__init__(id="recovery-dialog-screen")

    def compose(self) -> ComposeResult:
        detail = f"A crash-recovery draft from {self.updated_at} is available for {self.path.name}."
        if self.source_missing:
            detail += (
                " The original Markdown file is missing or cannot be safely read; "
                "restore the draft to save a copy."
            )
        elif self.disk_changed:
            detail += " The Markdown file also changed; restoring will require conflict recovery."
        with Vertical(classes="dialog", id="recovery-dialog"):
            yield Static("Recover unsaved draft", classes="dialog-title", markup=False)
            yield Static(detail, classes="dialog-message", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("Restore draft", id="recovery-restore", variant="primary")
                discard_label = "Discard draft" if self.source_missing else "Use disk version"
                yield Button(discard_label, id="recovery-discard", variant="warning")
                yield Button("Cancel opening", id="recovery-cancel")

    @on(Button.Pressed)
    def choose(self, event: Button.Pressed) -> None:
        decisions = {
            "recovery-restore": RecoveryDecision.RESTORE,
            "recovery-discard": RecoveryDecision.DISCARD,
            "recovery-cancel": RecoveryDecision.CANCEL,
        }
        if event.button.id in decisions:
            self.dismiss(decisions[event.button.id])

    def action_cancel(self) -> None:
        self.dismiss(RecoveryDecision.CANCEL)


class MixedLineEndingsDialog(ModalScreen[bool]):
    """Require consent before an edit can normalize mixed separators."""

    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(self, path: Path, target: str, *, cancel_label: str = "Cancel opening") -> None:
        self.path = path
        self.target = target
        self.cancel_label = cancel_label
        super().__init__(id="mixed-line-endings-dialog-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="mixed-line-endings-dialog"):
            yield Static("Mixed line endings", classes="dialog-title", markup=False)
            yield Static(
                f"{self.path.name} mixes line-ending styles. Textual will normalize them to "
                f"{self.target} after the first edit. The file stays byte-for-byte unchanged "
                "until you edit and save.",
                classes="dialog-message",
                markup=False,
            )
            with Horizontal(classes="dialog-buttons"):
                yield Button("Edit and normalize", id="mixed-normalize", variant="primary")
                yield Button(self.cancel_label, id="mixed-cancel")

    @on(Button.Pressed)
    def choose(self, event: Button.Pressed) -> None:
        if event.button.id == "mixed-normalize":
            self.dismiss(True)
        elif event.button.id == "mixed-cancel":
            self.dismiss(False)

    def action_cancel(self) -> None:
        self.dismiss(False)


class UnsavedChangesDialog(ModalScreen[UnsavedDecision | None]):
    """Require an actual decision before a dirty document is left behind."""

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

    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(
        self,
        path: Path,
        *,
        can_reload: bool,
        unavailable: bool = False,
        allow_discard: bool = False,
    ) -> None:
        self.path = path
        self.can_reload = can_reload
        self.unavailable = unavailable
        self.allow_discard = allow_discard
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
                if self.allow_discard:
                    yield Button("Continue without copy", id="conflict-discard", variant="warning")
                else:
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
            "conflict-discard": ConflictDecision.DISCARD,
            "conflict-cancel": ConflictDecision.CANCEL,
        }
        if event.button.id in decisions:
            self.dismiss(decisions[event.button.id])

    def action_cancel(self) -> None:
        self.dismiss(ConflictDecision.CANCEL)


class SaveAsDialog(ModalScreen[bool]):
    """Collect a new workspace-relative Markdown path."""

    BINDINGS: ClassVar[list[BindingType]] = [Binding("escape", "cancel", "Cancel", show=False)]

    class Submitted(Message):
        """Request that the coordinator validate and save the entered path."""

        def __init__(self, dialog: SaveAsDialog, value: str) -> None:
            self.dialog = dialog
            self.value = value
            super().__init__()

        @property
        def control(self) -> SaveAsDialog:
            return self.dialog

    def __init__(self, suggested_path: str, error: str | None = None) -> None:
        self.suggested_path = suggested_path
        self.error = error
        super().__init__()

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
        self.post_message(self.Submitted(self, event.value.strip()))

    @on(Button.Pressed, "#save-as-confirm")
    def submit_button(self) -> None:
        value = self.query_one("#save-as-input", Input).value.strip()
        self.post_message(self.Submitted(self, value))

    @on(Button.Pressed, "#save-as-cancel")
    def cancel_button(self) -> None:
        self.dismiss(False)

    def action_cancel(self) -> None:
        self.dismiss(False)

    def show_error(self, error: str) -> None:
        """Keep the modal open and report a recoverable validation/save error."""
        self.error = error
        self.query_one("#save-as-error", Static).update(error)
        self.query_one("#save-as-input", Input).focus()


class FileSearchDialog(ModalScreen[Path | None]):
    """Search and select Markdown files from the validated workspace index."""

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


class TextSearchDialog(ModalScreen[TextSearchMatch | None]):
    """Search source text without blocking Textual's UI thread."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "cancel", "Cancel", show=False),
        Binding("down", "focus_results", "Results", show=False),
    ]

    def __init__(
        self,
        workspace: Workspace,
        *,
        active_override: TextSearchOverride | None = None,
    ) -> None:
        self.workspace = workspace
        self.root = workspace.root
        self.active_override = active_override
        self.matches: tuple[TextSearchMatch, ...] = ()
        super().__init__(id="text-search-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="text-search-dialog"):
            yield Static("Search workspace text", classes="dialog-title", markup=False)
            yield Input(
                placeholder="Type literal text and press Enter…",
                id="text-search-input",
            )
            yield Static("Enter a query to search Markdown source.", id="text-search-status")
            yield OptionList(id="text-search-results", markup=False)

    def on_mount(self) -> None:
        self._set_placeholder("No search yet")
        self.query_one("#text-search-input", Input).focus()

    @on(Input.Submitted, "#text-search-input")
    def submit_search(self, event: Input.Submitted) -> None:
        query = event.value
        if not query:
            self.matches = ()
            self.query_one("#text-search-status", Static).update("Enter a non-empty query.")
            self._set_placeholder("No search yet")
            return
        self.query_one("#text-search-status", Static).update("Searching…")
        self._set_placeholder("Searching…")
        self._search_in_background(query)

    @work(group="text-search", exclusive=True, thread=True, exit_on_error=False)
    def _search_in_background(self, query: str) -> None:
        worker = get_current_worker()
        try:
            scan = self.workspace.scan(should_cancel=lambda: worker.is_cancelled)
            result = search_text(
                scan.files,
                query,
                active_override=self.active_override,
                should_cancel=lambda: worker.is_cancelled,
            )
            result = TextSearchResult(
                result.matches,
                (*scan.warnings, *result.warnings),
            )
        except Exception as error:
            if not worker.is_cancelled:
                self.app.call_from_thread(self._show_error, query, str(error))
            return
        if not worker.is_cancelled:
            self.app.call_from_thread(self._show_results, query, result)

    def _show_results(self, query: str, result: TextSearchResult) -> None:
        if not self.is_mounted or self.query_one("#text-search-input", Input).value != query:
            return
        self.matches = result.matches
        options = [
            Option(
                f"{match.path.relative_to(self.root).as_posix()}:{match.line + 1}:"
                f"{match.column + 1}  {match.preview}",
                id=str(index),
            )
            for index, match in enumerate(self.matches)
        ]
        results = self.query_one("#text-search-results", OptionList)
        if options:
            results.set_options(options)
            results.highlighted = 0
            results.focus()
        else:
            self._set_placeholder("No matching source lines")
        match_word = "match" if len(self.matches) == 1 else "matches"
        status = f"{len(self.matches)} {match_word}"
        if result.warnings:
            status += f" · skipped {len(result.warnings)} unreadable path(s)"
        self.query_one("#text-search-status", Static).update(status)

    def _show_error(self, query: str, error: str) -> None:
        if not self.is_mounted or self.query_one("#text-search-input", Input).value != query:
            return
        self.matches = ()
        self._set_placeholder("Search failed")
        self.query_one("#text-search-status", Static).update(f"Search failed: {error}")

    def _set_placeholder(self, message: str) -> None:
        results = self.query_one("#text-search-results", OptionList)
        results.set_options([Option(message, disabled=True)])
        results.highlighted = None

    @on(OptionList.OptionSelected, "#text-search-results")
    def open_selected(self, event: OptionList.OptionSelected) -> None:
        if 0 <= event.option_index < len(self.matches):
            self.dismiss(self.matches[event.option_index])

    def action_focus_results(self) -> None:
        if self.matches:
            self.query_one("#text-search-results", OptionList).focus()

    def action_cancel(self) -> None:
        self.dismiss(None)


class HelpDialog(ModalScreen[None]):
    """Display the centralized shortcut list."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Close", show=False),
        Binding("f1", "close", "Close", show=False),
    ]

    def __init__(self, content: str, *, title: str = "TermWriter shortcuts") -> None:
        self.content = content
        self.dialog_title = title
        super().__init__(id="help-dialog-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="help-dialog"):
            yield Static(self.dialog_title, classes="dialog-title", markup=False)
            yield Static(self.content, id="help-shortcuts", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("Close", id="help-close", variant="primary")

    @on(Button.Pressed, "#help-close")
    def close_button(self) -> None:
        self.dismiss(None)

    def action_close(self) -> None:
        self.dismiss(None)
