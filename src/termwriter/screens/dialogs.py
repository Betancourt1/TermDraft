"""Typed modal decisions used by the TermWriter coordinator."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from enum import Enum, auto
from pathlib import Path
from typing import ClassVar

from textual import events, on, work
from textual.app import ComposeResult
from textual.binding import Binding, BindingType
from textual.containers import Grid, Horizontal, Vertical
from textual.message import Message
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, Input, OptionList, Select, Static
from textual.widgets.option_list import Option
from textual.worker import get_current_worker

from termwriter.models.workspace import Workspace
from termwriter.services.file_search import search_files
from termwriter.services.path_filter import PathFilterError, parse_path_filter
from termwriter.services.recovery import RecoveryRecord
from termwriter.services.text_search import (
    TextSearchMatch,
    TextSearchMode,
    TextSearchOptions,
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


class RecoveryManagerAction(Enum):
    """Explicit operations available from the recovery inventory."""

    OPEN = auto()
    RETARGET = auto()
    QUARANTINE = auto()
    RESTORE_QUARANTINED = auto()
    EXPORT_QUARANTINED = auto()
    DELETE_QUARANTINED = auto()


@dataclass(frozen=True, slots=True)
class RecoveryManagerRequest:
    """One recovery operation chosen by the user."""

    action: RecoveryManagerAction
    record: RecoveryRecord
    target: str | None = None


@dataclass(frozen=True, slots=True)
class RecoveryRetentionRequest:
    """Exact quarantine records confirmed for age-based cleanup."""

    cutoff: datetime
    records: tuple[RecoveryRecord, ...]
    retention_days: int


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


class RecoveryManagerDialog(ModalScreen[RecoveryManagerRequest | RecoveryRetentionRequest | None]):
    """Inspect, reopen, retarget, or safely archive recovery journals."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "close", "Close", show=False),
    ]

    def __init__(
        self,
        records: tuple[RecoveryRecord, ...],
        workspace_root: Path,
        *,
        protected_journal_path: Path | None = None,
        retention_days: int = 30,
    ) -> None:
        self.records = records
        self.workspace_root = workspace_root
        self.protected_journal_path = protected_journal_path
        self.retention_days = retention_days
        super().__init__(id="recovery-manager-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="recovery-manager-dialog"):
            yield Static("Manage recovery drafts", classes="dialog-title", markup=False)
            yield Static(
                "Archive preserves bytes in quarantine. Quarantined entries can be restored, "
                "exported as Markdown, or deleted forever.",
                classes="dialog-message",
                markup=False,
            )
            options = [
                Option(self._record_label(record), id=str(index))
                for index, record in enumerate(self.records)
            ]
            if not options:
                options = [Option("No recovery entries", disabled=True)]
            yield OptionList(*options, id="recovery-manager-records", markup=False)
            yield Static("", id="recovery-manager-detail", markup=False)
            yield Input(
                placeholder="Retarget to a workspace path, e.g. notes/renamed.md",
                id="recovery-manager-target",
            )
            with Grid(classes="dialog-buttons", id="recovery-manager-buttons"):
                yield Button("Open draft", id="recovery-manager-open", variant="primary")
                yield Button("Retarget", id="recovery-manager-retarget")
                yield Button("Archive", id="recovery-manager-archive", variant="warning")
                yield Button("Delete expired", id="recovery-manager-retention", variant="error")
                yield Button("Close", id="recovery-manager-close")

    def on_mount(self) -> None:
        records = self.query_one("#recovery-manager-records", OptionList)
        records.highlighted = 0 if self.records else None
        records.focus()
        self._refresh_selection()

    def on_descendant_focus(self, event: events.DescendantFocus) -> None:
        """Keep focused recovery controls visible in short terminals."""
        self.call_after_refresh(event.widget.scroll_visible)

    def _record_label(self, record: RecoveryRecord) -> str:
        location = "QUARANTINE · " if record.quarantined else ""
        if record.entry is None:
            active = " · active" if record.journal_path == self.protected_journal_path else ""
            return f"{location}CORRUPT · {record.journal_path.name}{active}"
        entry = record.entry
        try:
            label = entry.document_path.relative_to(self.workspace_root).as_posix()
        except ValueError:
            label = str(entry.document_path)
        flags: list[str] = []
        if record.journal_path == self.protected_journal_path:
            flags.append("active")
        if not entry.document_path.exists():
            flags.append("missing")
        if entry.updated_at < datetime.now(UTC) - timedelta(days=self.retention_days):
            flags.append("old")
        suffix = f" · {', '.join(flags)}" if flags else ""
        return f"{location}{label}{suffix}"

    def _selected_record(self) -> RecoveryRecord | None:
        index = self.query_one("#recovery-manager-records", OptionList).highlighted
        if index is None or not 0 <= index < len(self.records):
            return None
        return self.records[index]

    def _expired_records(self) -> tuple[RecoveryRecord, ...]:
        cutoff = datetime.now(UTC) - timedelta(days=self.retention_days)
        return tuple(
            record
            for record in self.records
            if record.quarantined and record.entry is not None and record.entry.updated_at < cutoff
        )

    def _refresh_selection(self) -> None:
        record = self._selected_record()
        detail = self.query_one("#recovery-manager-detail", Static)
        target = self.query_one("#recovery-manager-target", Input)
        open_button = self.query_one("#recovery-manager-open", Button)
        retarget_button = self.query_one("#recovery-manager-retarget", Button)
        archive_button = self.query_one("#recovery-manager-archive", Button)
        retention_button = self.query_one("#recovery-manager-retention", Button)
        expired_count = len(self._expired_records())
        retention_button.label = f"Delete >{self.retention_days}d ({expired_count})"
        retention_button.disabled = expired_count == 0
        open_button.label = "Open draft"
        retarget_button.label = "Retarget"
        archive_button.label = "Archive"
        target.placeholder = "Retarget to a workspace path, e.g. notes/renamed.md"
        if record is None:
            detail.update("Nothing to manage in this recovery directory.")
            target.disabled = True
            open_button.disabled = True
            retarget_button.disabled = True
            archive_button.disabled = True
            return

        entry = record.entry
        protected = record.journal_path == self.protected_journal_path
        if record.quarantined:
            target.disabled = entry is None
            target.placeholder = "Export copy as Markdown, e.g. recovered/draft.md"
            open_button.label = "Restore"
            retarget_button.label = "Delete forever"
            archive_button.label = "Export copy"
            open_button.disabled = entry is None
            retarget_button.disabled = False
            archive_button.disabled = entry is None
            if entry is None:
                detail.update(
                    (record.error or "This quarantined entry could not be validated.")
                    + " It cannot be restored or exported, but it can be permanently deleted."
                )
            else:
                updated = entry.updated_at.astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")
                detail.update(f"Quarantined · {entry.document_path} · updated {updated}")
            return
        target.disabled = entry is None or protected
        open_button.disabled = entry is None or protected
        retarget_button.disabled = entry is None or protected
        archive_button.disabled = protected
        if protected:
            detail.update("This draft belongs to the active dirty document and cannot be moved.")
        elif entry is None:
            detail.update(record.error or "This recovery entry could not be validated.")
        else:
            updated = entry.updated_at.astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")
            detail.update(f"{entry.document_path} · updated {updated}")

    @on(OptionList.OptionHighlighted, "#recovery-manager-records")
    def highlight_record(self) -> None:
        self._refresh_selection()

    @on(Button.Pressed, "#recovery-manager-open")
    def open_record(self) -> None:
        record = self._selected_record()
        if record is not None and record.entry is not None:
            action = (
                RecoveryManagerAction.RESTORE_QUARANTINED
                if record.quarantined
                else RecoveryManagerAction.OPEN
            )
            self.dismiss(RecoveryManagerRequest(action, record))

    @on(Button.Pressed, "#recovery-manager-retarget")
    def retarget_record(self) -> None:
        record = self._selected_record()
        if record is not None and record.quarantined:
            self.dismiss(
                RecoveryManagerRequest(
                    RecoveryManagerAction.DELETE_QUARANTINED,
                    record,
                )
            )
            return
        target = self.query_one("#recovery-manager-target", Input).value.strip()
        if record is None or record.entry is None:
            return
        if not target:
            self.query_one("#recovery-manager-detail", Static).update(
                "Enter a new workspace-relative Markdown path."
            )
            return
        self.dismiss(
            RecoveryManagerRequest(
                RecoveryManagerAction.RETARGET,
                record,
                target,
            )
        )

    @on(Button.Pressed, "#recovery-manager-archive")
    def archive_or_export_record(self) -> None:
        record = self._selected_record()
        if record is None:
            return
        if not record.quarantined:
            self.dismiss(RecoveryManagerRequest(RecoveryManagerAction.QUARANTINE, record))
            return
        target = self.query_one("#recovery-manager-target", Input).value.strip()
        if record.entry is None:
            return
        if not target:
            self.query_one("#recovery-manager-detail", Static).update(
                "Enter a new workspace-relative Markdown path for the exported copy."
            )
            return
        self.dismiss(
            RecoveryManagerRequest(
                RecoveryManagerAction.EXPORT_QUARANTINED,
                record,
                target,
            )
        )

    @on(Input.Submitted, "#recovery-manager-target")
    def submit_target(self) -> None:
        record = self._selected_record()
        if record is not None and record.quarantined:
            self.archive_or_export_record()
        else:
            self.retarget_record()

    @on(Button.Pressed, "#recovery-manager-retention")
    def cleanup_expired(self) -> None:
        records = self._expired_records()
        if not records:
            return
        self.dismiss(
            RecoveryRetentionRequest(
                cutoff=datetime.now(UTC) - timedelta(days=self.retention_days),
                records=records,
                retention_days=self.retention_days,
            )
        )

    @on(Button.Pressed, "#recovery-manager-close")
    def close_button(self) -> None:
        self.dismiss(None)

    def action_close(self) -> None:
        self.dismiss(None)


class RecoveryDeleteDialog(ModalScreen[bool]):
    """Confirm irreversible deletion of one quarantined recovery entry."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "cancel", "Cancel", show=False),
    ]

    def __init__(self, record: RecoveryRecord) -> None:
        self.record = record
        if record.entry is None:
            self.description = record.journal_path.name
        else:
            self.description = str(record.entry.document_path)
        super().__init__(id="recovery-delete-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="recovery-delete-dialog"):
            yield Static("Permanently delete recovery?", classes="dialog-title", markup=False)
            yield Static(
                f"Delete quarantined bytes for {self.description}? This cannot be undone.",
                classes="dialog-message",
                markup=False,
            )
            with Horizontal(classes="dialog-buttons"):
                yield Button(
                    "Delete forever",
                    id="recovery-delete-confirm",
                    variant="error",
                )
                yield Button("Cancel", id="recovery-delete-cancel")

    def on_mount(self) -> None:
        self.query_one("#recovery-delete-cancel", Button).focus()

    @on(Button.Pressed, "#recovery-delete-confirm")
    def confirm(self) -> None:
        self.dismiss(True)

    @on(Button.Pressed, "#recovery-delete-cancel")
    def cancel_button(self) -> None:
        self.dismiss(False)

    def action_cancel(self) -> None:
        self.dismiss(False)


class RecoveryRetentionDialog(ModalScreen[bool]):
    """Confirm deletion of the exact expired quarantine inventory shown."""

    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("escape", "cancel", "Cancel", show=False),
    ]

    def __init__(self, request: RecoveryRetentionRequest) -> None:
        self.request = request
        super().__init__(id="recovery-retention-screen")

    def compose(self) -> ComposeResult:
        count = len(self.request.records)
        noun = "draft" if count == 1 else "drafts"
        options = []
        for record in self.request.records:
            path = record.journal_path if record.entry is None else record.entry.document_path
            options.append(Option(f"{path.name} · {path}", disabled=True))
        with Vertical(classes="dialog", id="recovery-retention-dialog"):
            yield Static("Delete expired recoveries?", classes="dialog-title", markup=False)
            yield Static(
                f"Permanently delete {count} quarantined {noun} older than "
                f"{self.request.retention_days} days? This cannot be undone.",
                classes="dialog-message",
                markup=False,
            )
            yield OptionList(
                *options,
                id="recovery-retention-records",
                markup=False,
            )
            with Grid(classes="dialog-buttons", id="recovery-retention-buttons"):
                yield Button(
                    "Delete expired",
                    id="recovery-retention-confirm",
                    variant="error",
                )
                yield Button("Cancel", id="recovery-retention-cancel")

    def on_mount(self) -> None:
        self.query_one("#recovery-retention-cancel", Button).focus()

    @on(Button.Pressed, "#recovery-retention-confirm")
    def confirm(self) -> None:
        self.dismiss(True)

    @on(Button.Pressed, "#recovery-retention-cancel")
    def cancel_button(self) -> None:
        self.dismiss(False)

    def action_cancel(self) -> None:
        self.dismiss(False)


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
        self._busy = False
        super().__init__()

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="save-as-dialog"):
            yield Static("Save local version as", classes="dialog-title", markup=False)
            yield Input(
                self.suggested_path,
                placeholder="notes/local-copy.md",
                id="save-as-input",
                disabled=self._busy,
            )
            yield Static(self.error or "", id="save-as-error", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button(
                    "Save copy",
                    id="save-as-confirm",
                    variant="primary",
                    disabled=self._busy,
                )
                yield Button("Cancel", id="save-as-cancel", disabled=self._busy)

    def on_mount(self) -> None:
        if not self._busy:
            self.query_one("#save-as-input", Input).focus()

    @on(Input.Submitted, "#save-as-input")
    def submit_input(self, event: Input.Submitted) -> None:
        if not self._busy:
            self.post_message(self.Submitted(self, event.value.strip()))

    @on(Button.Pressed, "#save-as-confirm")
    def submit_button(self) -> None:
        if not self._busy:
            value = self.query_one("#save-as-input", Input).value.strip()
            self.post_message(self.Submitted(self, value))

    @on(Button.Pressed, "#save-as-cancel")
    def cancel_button(self) -> None:
        if not self._busy:
            self.dismiss(False)

    def action_cancel(self) -> None:
        if not self._busy:
            self.dismiss(False)

    def set_busy(self, busy: bool) -> None:
        """Lock or unlock the modal while a save is being published."""
        self._busy = busy
        if not self.is_mounted:
            return
        self.query_one("#save-as-input", Input).disabled = busy
        self.query_one("#save-as-confirm", Button).disabled = busy
        self.query_one("#save-as-cancel", Button).disabled = busy

    def show_error(self, error: str) -> None:
        """Keep the modal open and report a recoverable validation/save error."""
        self.set_busy(False)
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
            yield Input(
                placeholder="Includes/excludes, e.g. notes/**, !notes/archive/**",
                id="file-search-filter",
            )
            yield Static("Fuzzy path matching", id="file-search-status", markup=False)
            yield OptionList(id="search-results", markup=False)

    def on_mount(self) -> None:
        self._set_query("")
        self.query_one("#search-input", Input).focus()

    def _set_query(self, query: str) -> None:
        filter_expression = self.query_one("#file-search-filter", Input).value
        try:
            path_filter = parse_path_filter(filter_expression)
        except PathFilterError as error:
            self.matches = ()
            self.query_one("#file-search-status", Static).update(f"Invalid file filter: {error}")
            results = self.query_one("#search-results", OptionList)
            results.set_options([Option("Invalid file filter", disabled=True)])
            results.highlighted = None
            return
        self.matches = search_files(
            self.files,
            query,
            root=self.root,
            path_filter=path_filter,
        )
        options = [
            Option(path.relative_to(self.root).as_posix(), id=str(index))
            for index, path in enumerate(self.matches)
        ]
        if not options:
            options = [Option("No matching Markdown files", disabled=True)]
        results = self.query_one("#search-results", OptionList)
        results.set_options(options)
        results.highlighted = 0 if self.matches else None
        count = len(self.matches)
        noun = "file" if count == 1 else "files"
        status = f"{count} {noun} · fuzzy path matching"
        if filter_expression.strip():
            status += f" · {filter_expression.strip()}"
        self.query_one("#file-search-status", Static).update(status)

    @on(Input.Changed)
    def search(self, event: Input.Changed) -> None:
        del event
        self._set_query(self.query_one("#search-input", Input).value)

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
        self._search_revision = 0
        super().__init__(id="text-search-screen")

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog", id="text-search-dialog"):
            yield Static("Search workspace text", classes="dialog-title", markup=False)
            yield Input(
                placeholder="Type a query and press Enter…",
                id="text-search-input",
            )
            with Vertical(id="text-search-options"):
                yield Select(
                    (
                        ("Literal", TextSearchMode.LITERAL.value),
                        ("Fuzzy", TextSearchMode.FUZZY.value),
                        ("Whole word", TextSearchMode.WHOLE_WORD.value),
                        ("Regular expression", TextSearchMode.REGEX.value),
                    ),
                    value=TextSearchMode.LITERAL.value,
                    allow_blank=False,
                    compact=True,
                    id="text-search-mode",
                )
                yield Checkbox("Match case", compact=True, id="text-search-case")
            yield Input(
                placeholder="Includes/excludes, e.g. notes/**/*.md, !notes/drafts/**",
                id="text-search-filter",
            )
            yield Static(
                "Enter a query to search Markdown source.",
                id="text-search-status",
                markup=False,
            )
            yield OptionList(id="text-search-results", markup=False)

    def on_mount(self) -> None:
        self._set_placeholder("No search yet")
        self.query_one("#text-search-input", Input).focus()

    @on(Input.Submitted, "#text-search-input")
    def submit_search(self, event: Input.Submitted) -> None:
        self._search_revision += 1
        revision = self._search_revision
        query = event.value
        if not query:
            self.matches = ()
            self.query_one("#text-search-status", Static).update("Enter a non-empty query.")
            self._set_placeholder("No search yet")
            return
        self.query_one("#text-search-status", Static).update("Searching…")
        self._set_placeholder("Searching…")
        mode_value = self.query_one("#text-search-mode", Select).value
        mode = (
            TextSearchMode.LITERAL if mode_value is Select.NULL else TextSearchMode(str(mode_value))
        )
        file_filter = self.query_one("#text-search-filter", Input).value.strip() or None
        options = TextSearchOptions(
            mode=mode,
            file_filter=file_filter,
            case_sensitive=self.query_one("#text-search-case", Checkbox).value,
        )
        self._search_in_background(revision, query, options)

    @work(group="text-search", exclusive=True, thread=True, exit_on_error=False)
    def _search_in_background(
        self,
        revision: int,
        query: str,
        options: TextSearchOptions,
    ) -> None:
        worker = get_current_worker()
        try:
            scan = self.workspace.scan(should_cancel=lambda: worker.is_cancelled)
            result = search_text(
                scan.files,
                query,
                active_override=self.active_override,
                should_cancel=lambda: worker.is_cancelled,
                options=options,
                root=self.root,
            )
            result = TextSearchResult(
                result.matches,
                (*scan.warnings, *result.warnings),
                result.error,
            )
        except Exception as error:
            if not worker.is_cancelled:
                self.app.call_from_thread(self._show_error, revision, query, str(error))
            return
        if not worker.is_cancelled:
            self.app.call_from_thread(self._show_results, revision, query, options, result)

    def _show_results(
        self,
        revision: int,
        query: str,
        options: TextSearchOptions,
        result: TextSearchResult,
    ) -> None:
        if (
            revision != self._search_revision
            or not self.is_mounted
            or self.query_one("#text-search-input", Input).value != query
        ):
            return
        if result.error is not None:
            self._show_error(revision, query, result.error)
            return
        self.matches = result.matches
        result_options = [
            Option(
                f"{match.path.relative_to(self.root).as_posix()}:{match.line + 1}:"
                f"{match.column + 1}  {match.preview}",
                id=str(index),
            )
            for index, match in enumerate(self.matches)
        ]
        results = self.query_one("#text-search-results", OptionList)
        if result_options:
            results.set_options(result_options)
            results.highlighted = 0
            results.focus()
        else:
            self._set_placeholder("No matching source lines")
        match_word = "match" if len(self.matches) == 1 else "matches"
        mode_label = {
            TextSearchMode.LITERAL: "literal",
            TextSearchMode.FUZZY: "fuzzy",
            TextSearchMode.WHOLE_WORD: "whole word",
            TextSearchMode.REGEX: "regular expression",
        }[options.mode]
        status = f"{len(self.matches)} {match_word} · {mode_label}"
        if options.file_filter:
            status += f" · {options.file_filter}"
        if result.warnings:
            warning_word = "warning" if len(result.warnings) == 1 else "warnings"
            status += f" · {len(result.warnings)} {warning_word}"
        self.query_one("#text-search-status", Static).update(status)

    def _show_error(self, revision: int, query: str, error: str) -> None:
        if (
            revision != self._search_revision
            or not self.is_mounted
            or self.query_one("#text-search-input", Input).value != query
        ):
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
