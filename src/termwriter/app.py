"""Textual application and the single document-transition coordinator."""

from __future__ import annotations

from collections.abc import Callable
from datetime import datetime
from pathlib import Path

from rich.markup import escape
from textual import events, on
from textual.app import App, ComposeResult
from textual.containers import Horizontal
from textual.screen import ModalScreen
from textual.timer import Timer
from textual.widgets import DirectoryTree, Static, TextArea

from termwriter.bindings import APP_BINDINGS, SHORTCUT_HELP
from termwriter.models.document import Document, FileSnapshot
from termwriter.models.workspace import Workspace, WorkspaceError, WorkspaceNotFoundError
from termwriter.screens.dialogs import (
    ConflictDecision,
    ConflictDialog,
    FileSearchDialog,
    HelpDialog,
    MixedLineEndingsDialog,
    RecoveryDecision,
    RecoveryDialog,
    SaveAsDialog,
    UnsavedChangesDialog,
    UnsavedDecision,
)
from termwriter.services.external_changes import (
    ExternalChange,
    ExternalChangeKind,
    detect_external_change,
)
from termwriter.services.persistence import (
    ExternalModificationError,
    PersistenceError,
    atomic_save,
    load_file,
    snapshot_file,
)
from termwriter.services.recovery import RecoveryEntry, RecoveryError, RecoveryJournal
from termwriter.widgets.editor import MarkdownEditor
from termwriter.widgets.file_tree import FileExplorer
from termwriter.widgets.preview import MarkdownPreview
from termwriter.widgets.status_bar import TermWriterStatusBar


class TermWriterApp(App[None]):
    """Local-first Markdown editor with guarded document transitions."""

    TITLE = "TermWriter"
    ENABLE_COMMAND_PALETTE = False
    BINDINGS = APP_BINDINGS

    CSS = """
    Screen {
        layout: vertical;
        background: $background;
    }

    #title-bar {
        height: 1;
        padding: 0 1;
        background: $primary;
        color: $text;
        text-style: bold;
    }

    #workspace {
        height: 1fr;
    }

    #file-explorer {
        width: 28;
        min-width: 20;
        height: 1fr;
        border-right: solid $panel-lighten-2;
        background: $surface;
    }

    #explorer-title {
        height: 1;
        padding: 0 1;
        background: $panel;
        text-style: bold;
        text-overflow: ellipsis;
    }

    #file-tree {
        height: 1fr;
    }

    #workbench {
        width: 1fr;
        height: 1fr;
    }

    #markdown-editor {
        width: 1fr;
        height: 1fr;
        border: none;
    }

    #markdown-preview {
        width: 1fr;
        height: 1fr;
        padding: 0 2;
        border-left: solid $panel-lighten-2;
        overflow-y: auto;
    }

    #status-bar {
        height: 1;
        padding: 0 1;
        background: $panel;
        color: $text-muted;
        text-overflow: ellipsis;
    }
    """

    def __init__(
        self,
        workspace: Workspace,
        *,
        preview_debounce: float = 0.2,
        external_poll_interval: float = 2.0,
        recovery_debounce: float = 0.5,
        recovery_journal: RecoveryJournal | None = None,
    ) -> None:
        super().__init__()
        self.workspace = workspace
        self.document: Document | None = None
        self.preview_debounce = preview_debounce
        self.external_poll_interval = external_poll_interval
        self.recovery_debounce = recovery_debounce
        self.recovery_journal = recovery_journal or RecoveryJournal()
        self.workspace_files: tuple[Path, ...] = ()
        self._preview_timer: Timer | None = None
        self._external_watch_timer: Timer | None = None
        self._recovery_timer: Timer | None = None
        self._recovery_revision = 0
        self._preview_revision = 0
        self._pending_transition: Callable[[], None] | None = None
        self._save_continuation: Callable[[], None] | None = None
        self._explorer_visible = True
        self._preview_visible = True
        self._narrow = False
        self._narrow_pane = "editor"
        self._editor_baseline_text = ""
        self._editor_baseline_source_text = ""
        self._pending_open_document: Document | None = None
        self._pending_recovery_entry: RecoveryEntry | None = None
        self._pending_recovery_is_orphan = False
        self._orphan_recoveries: list[RecoveryEntry] = []
        self._mixed_reload_continuation: Callable[[], None] | None = None

    def compose(self) -> ComposeResult:
        yield Static(f"TermWriter  ·  {self.workspace.root}", id="title-bar", markup=False)
        with Horizontal(id="workspace"):
            yield FileExplorer(self.workspace)
            with Horizontal(id="workbench"):
                yield MarkdownEditor()
                yield MarkdownPreview()
        yield TermWriterStatusBar()

    def on_mount(self) -> None:
        self._refresh_workspace_index()
        self._external_watch_timer = self.set_interval(
            self.external_poll_interval,
            self._check_external_in_background,
            name="external-file-watch",
        )
        self._narrow = self.size.width < 100
        self._apply_panel_visibility()
        if self.workspace.initial_file is not None:
            self._open_file_now(self.workspace.initial_file)
        else:
            self.explorer.directory_tree.focus()
            self._refresh_status()
            self._scan_orphan_recoveries()

    def on_resize(self, event: events.Resize) -> None:
        was_narrow = self._narrow
        self._narrow = event.size.width < 100
        if self._narrow and not was_narrow:
            self._narrow_pane = "editor"
        self._apply_panel_visibility()

    def on_app_focus(self, event: events.AppFocus) -> None:
        del event
        if self.document is not None and not self._has_modal:
            self.call_after_refresh(self._check_external_in_background)

    @property
    def _has_modal(self) -> bool:
        return isinstance(self.screen, ModalScreen)

    @property
    def editor(self) -> MarkdownEditor:
        return self.query_one(MarkdownEditor)

    @property
    def preview(self) -> MarkdownPreview:
        return self.query_one(MarkdownPreview)

    @property
    def explorer(self) -> FileExplorer:
        return self.query_one(FileExplorer)

    def _refresh_workspace_index(self) -> None:
        result = self.workspace.scan()
        self.workspace_files = result.files
        if result.warnings:
            self.notify(
                f"Skipped {len(result.warnings)} unreadable workspace location(s)",
                severity="warning",
            )

    def _apply_panel_visibility(self) -> None:
        self.explorer.display = self._explorer_visible
        if self._narrow:
            show_preview = self._preview_visible and self._narrow_pane == "preview"
            self.editor.display = not show_preview
            self.preview.display = show_preview
        else:
            self.editor.display = True
            self.preview.display = self._preview_visible
        self._refresh_status()

    def _sync_editor_state(self) -> None:
        document = self.document
        if document is None:
            return
        previous_text = document.text
        editor_text = self.editor.text
        if editor_text != self._editor_baseline_text:
            document.update_text(editor_text)
        else:
            document.update_text(self._editor_baseline_source_text)
        if document.text != previous_text:
            self._schedule_recovery()
        line, column = self.editor.cursor_location
        document.update_cursor(
            line,
            column,
            scroll_x=float(self.editor.scroll_offset.x),
            scroll_y=float(self.editor.scroll_offset.y),
        )

    @on(TextArea.Changed, "#markdown-editor")
    def editor_changed(self, event: TextArea.Changed) -> None:
        document = self.document
        if document is None:
            return
        if event.text_area.text == self._editor_baseline_text:
            document.update_text(self._editor_baseline_source_text)
        else:
            document.update_text(event.text_area.text)
        self._schedule_preview()
        self._schedule_recovery()
        self._refresh_status()

    @on(TextArea.SelectionChanged, "#markdown-editor")
    def cursor_changed(self, event: TextArea.SelectionChanged) -> None:
        document = self.document
        if document is None:
            return
        line, column = event.selection.end
        document.update_cursor(
            line,
            column,
            scroll_x=float(event.text_area.scroll_offset.x),
            scroll_y=float(event.text_area.scroll_offset.y),
        )
        self._refresh_status()

    @on(DirectoryTree.FileSelected)
    def file_selected(self, event: DirectoryTree.FileSelected) -> None:
        event.stop()
        self._request_open(event.path)

    def on_descendant_focus(self, event: events.DescendantFocus) -> None:
        del event
        self._refresh_status()

    def _request_open(self, path: Path) -> None:
        try:
            safe_path = self.workspace.validate_document_path(path)
        except WorkspaceError as error:
            self.notify(escape(str(error)), severity="error", title="Cannot open file")
            return
        if self.document is not None and safe_path == self.document.path:
            self.editor.focus()
            return
        self._request_transition(lambda: self._open_file_now(safe_path))

    def _request_transition(self, continuation: Callable[[], None]) -> None:
        self._sync_editor_state()
        document = self.document
        if document is not None and document.dirty:
            self._pending_transition = continuation
            self.push_screen(UnsavedChangesDialog(document.path), self._handle_unsaved_decision)
            return
        if document is not None:
            try:
                document.path = self.workspace.validate_document_path(document.path)
            except WorkspaceError as error:
                self._pending_transition = continuation
                self._show_conflict(
                    ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error)),
                    after=self._complete_pending_transition,
                )
                return
            change = detect_external_change(document)
            if change.kind is ExternalChangeKind.UNCHANGED and change.snapshot is not None:
                document.accept_unchanged_snapshot(change.snapshot)
            elif change.kind in {ExternalChangeKind.DELETED, ExternalChangeKind.INACCESSIBLE}:
                self._pending_transition = continuation
                self._show_conflict(change, after=self._complete_pending_transition)
                return
        continuation()

    def _handle_unsaved_decision(self, decision: UnsavedDecision | None) -> None:
        if decision is UnsavedDecision.SAVE:
            self._save_current(after=self._complete_pending_transition)
        elif decision is UnsavedDecision.DISCARD:
            if self.document is not None:
                self._clear_recovery(self.document.path)
            self._complete_pending_transition()
        else:
            self._cancel_pending_transition()

    def _complete_pending_transition(self) -> None:
        continuation = self._pending_transition
        self._pending_transition = None
        if continuation is not None:
            continuation()

    def _cancel_pending_transition(self) -> None:
        self._pending_transition = None
        self._save_continuation = None
        self._refresh_status()

    def _open_file_now(self, path: Path) -> None:
        try:
            safe_path = self.workspace.validate_document_path(path)
            loaded = load_file(safe_path)
        except (OSError, PersistenceError, WorkspaceError) as error:
            self.notify(escape(str(error)), severity="error", title="Cannot open file")
            self._cancel_pending_open()
            return

        document = Document(
            path=safe_path,
            text=loaded.text,
            saved_text=loaded.text,
            snapshot=loaded.snapshot,
            encoding=loaded.encoding,
        )
        self._pending_open_document = document
        self._pending_recovery_is_orphan = False
        try:
            recovery = self.recovery_journal.load(document.path)
        except RecoveryError as error:
            self.notify(escape(str(error)), severity="error", title="Recovery draft unavailable")
            recovery = None
        if recovery is not None and recovery.text == document.text:
            self._clear_recovery(document.path)
            recovery = None
        if recovery is not None:
            self._pending_recovery_entry = recovery
            updated_at = recovery.updated_at.astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")
            self.push_screen(
                RecoveryDialog(
                    document.path,
                    updated_at,
                    disk_changed=not (
                        recovery.base_snapshot.has_same_content(document.snapshot)
                        and recovery.base_snapshot.has_same_origin(document.snapshot)
                    ),
                ),
                self._handle_recovery_decision,
            )
            return
        self._continue_pending_open()

    def _handle_recovery_decision(self, decision: RecoveryDecision | None) -> None:
        document = self._pending_open_document
        recovery = self._pending_recovery_entry
        is_orphan = self._pending_recovery_is_orphan
        self._pending_recovery_is_orphan = False
        self._pending_recovery_entry = None
        if document is None or recovery is None:
            self._cancel_pending_open()
            return
        if decision is RecoveryDecision.RESTORE:
            document.restore_recovery(
                recovery.text,
                recovery.encoding,
                recovery.base_snapshot,
            )
            self._continue_pending_open()
        elif decision is RecoveryDecision.DISCARD:
            self._clear_recovery(document.path)
            if is_orphan:
                self._cancel_pending_open()
                self.call_after_refresh(self._offer_next_orphan_recovery)
            else:
                self._continue_pending_open()
        else:
            if is_orphan:
                self._orphan_recoveries.clear()
            self._cancel_pending_open()

    def _scan_orphan_recoveries(self) -> None:
        result = self.recovery_journal.scan_workspace(self.workspace.root)
        if result.warnings:
            self.notify(
                f"Skipped {len(result.warnings)} invalid recovery entry or entries",
                severity="warning",
                title="Recovery scan",
            )
        self._orphan_recoveries = [
            entry for entry in result.entries if self._recovery_source_is_unavailable(entry)
        ]
        self._offer_next_orphan_recovery()

    def _recovery_source_is_unavailable(self, entry: RecoveryEntry) -> bool:
        try:
            path = self.workspace.validate_document_path(entry.document_path)
            load_file(path)
        except (OSError, PersistenceError, WorkspaceError):
            return True
        return False

    def _offer_next_orphan_recovery(self) -> None:
        if self._has_modal or not self._orphan_recoveries:
            return
        recovery = self._orphan_recoveries.pop(0)
        document = Document(
            path=recovery.document_path,
            text=recovery.text,
            saved_text=recovery.text,
            snapshot=FileSnapshot.missing(),
            encoding=recovery.encoding,
        )
        document.restore_recovery(
            recovery.text,
            recovery.encoding,
            recovery.base_snapshot,
        )
        self._pending_open_document = document
        self._pending_recovery_entry = recovery
        self._pending_recovery_is_orphan = True
        updated_at = recovery.updated_at.astimezone().strftime("%Y-%m-%d %H:%M:%S %Z")
        self.push_screen(
            RecoveryDialog(
                document.path,
                updated_at,
                disk_changed=True,
                source_missing=True,
            ),
            self._handle_recovery_decision,
        )

    def _continue_pending_open(self) -> None:
        document = self._pending_open_document
        if document is None:
            return
        if document.has_mixed_line_endings:
            target = document.line_ending_label.removeprefix("MIXED→")
            self.push_screen(
                MixedLineEndingsDialog(document.path, target),
                self._handle_mixed_line_ending_decision,
            )
            return
        self._pending_open_document = None
        self._install_document(document)

    def _handle_mixed_line_ending_decision(self, accepted: bool | None) -> None:
        document = self._pending_open_document
        self._pending_open_document = None
        if accepted and document is not None:
            self._install_document(document)
            self.notify(
                f"Edits to {escape(document.path.name)} will normalize line endings to "
                f"{document.line_ending_label.removeprefix('MIXED→')}",
                severity="warning",
            )
            return
        self._cancel_pending_open()

    def _cancel_pending_open(self) -> None:
        self._pending_open_document = None
        self._pending_recovery_entry = None
        self._pending_recovery_is_orphan = False
        if self.document is not None:
            self.editor.focus()
            if self.document.dirty:
                self._schedule_recovery()
        self._refresh_status()

    def _install_document(self, document: Document) -> None:
        self._recovery_revision += 1
        if self._recovery_timer is not None:
            self._recovery_timer.stop()
            self._recovery_timer = None
        self.document = document
        with self.editor.prevent(TextArea.Changed):
            self.editor.load_text(document.text)
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self.editor.read_only = False
        self.editor.scroll_to(0, 0, animate=False, immediate=True)
        self.explorer.set_active(document.path)
        self._narrow_pane = "editor"
        self._apply_panel_visibility()
        self._schedule_preview(immediate=True)
        self.editor.focus()
        self._refresh_status()

    def _schedule_preview(self, *, immediate: bool = False) -> None:
        self._preview_revision += 1
        revision = self._preview_revision
        if self._preview_timer is not None:
            self._preview_timer.stop()
        delay = 0.001 if immediate else self.preview_debounce
        self._preview_timer = self.set_timer(delay, lambda: self._render_preview(revision))

    def _schedule_recovery(self) -> None:
        document = self.document
        if document is None:
            return
        if not document.dirty:
            self._clear_recovery(document.path)
            return
        document.recovery_saved = False
        if self._recovery_timer is not None:
            return
        self._recovery_revision += 1
        revision = self._recovery_revision
        self._recovery_timer = self.set_timer(
            self.recovery_debounce,
            lambda: self._write_recovery(revision),
        )

    def _write_recovery(self, revision: int) -> None:
        document = self.document
        if revision != self._recovery_revision:
            return
        self._recovery_timer = None
        if document is None or not document.dirty:
            return
        try:
            self.recovery_journal.save(
                document_path=document.path,
                workspace_root=self.workspace.root,
                text=document.text,
                encoding=document.encoding,
                base_snapshot=document.recovery_base_snapshot or document.snapshot,
            )
        except RecoveryError as error:
            document.recovery_saved = False
            self.notify(escape(str(error)), severity="error", title="Recovery draft failed")
        else:
            document.recovery_saved = True
        self._refresh_status()

    def _clear_recovery(self, path: Path) -> None:
        document = self.document
        if document is not None and document.path == path:
            self._recovery_revision += 1
            if self._recovery_timer is not None:
                self._recovery_timer.stop()
                self._recovery_timer = None
        try:
            self.recovery_journal.delete(path)
        except RecoveryError as error:
            self.notify(escape(str(error)), severity="warning", title="Recovery cleanup failed")
            return
        if document is not None and document.path == path:
            document.recovery_saved = False

    async def _render_preview(self, revision: int) -> None:
        if revision != self._preview_revision or self.document is None:
            return
        source = self.document.text
        try:
            await self.preview.render_source(source)
        except Exception as error:
            if revision == self._preview_revision:
                self.notify(
                    escape(str(error)),
                    severity="error",
                    title="Preview could not be rendered",
                )
            return
        if revision != self._preview_revision and self.document is not None:
            await self.preview.render_source(self.document.text)

    def action_save(self) -> None:
        if not self._has_modal:
            self._save_current()

    def _save_current(self, *, after: Callable[[], None] | None = None) -> None:
        self._sync_editor_state()
        document = self.document
        if document is None:
            self.notify("No Markdown file is open", severity="warning")
            return

        try:
            document.path = self.workspace.validate_document_path(document.path)
        except WorkspaceError as error:
            self._show_conflict(
                ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error)),
                after=after,
            )
            return

        if document.recovery_conflict:
            current = detect_external_change(document)
            if current.kind is ExternalChangeKind.INACCESSIBLE:
                self._show_conflict(current, after=after)
            else:
                self._show_conflict(
                    ExternalChange(ExternalChangeKind.CONFLICT, current.snapshot),
                    after=after,
                )
            return

        change = detect_external_change(document)
        if change.kind is ExternalChangeKind.UNCHANGED:
            if change.snapshot is not None:
                document.accept_unchanged_snapshot(change.snapshot)
            if not document.dirty:
                document.last_save_status = "No changes"
                self._clear_recovery(document.path)
                self._refresh_status()
                if after is not None:
                    after()
                return
        elif change.kind is ExternalChangeKind.MODIFIED:
            self._save_continuation = after
            self._reload_current_from_disk(automatic=True, failure_dialog=True)
            return
        elif change.kind is ExternalChangeKind.INACCESSIBLE:
            self._show_conflict(change, after=after)
            return
        else:
            self._show_conflict(change, after=after)
            return

        try:
            result = atomic_save(
                document.path,
                document.text,
                encoding=document.encoding,
                expected=document.snapshot,
            )
        except ExternalModificationError:
            self._show_conflict(detect_external_change(document), after=after)
            return
        except (OSError, PersistenceError) as error:
            document.last_save_status = "Save failed"
            self.notify(escape(str(error)), severity="error", title="Save failed")
            self._cancel_pending_transition()
            return

        timestamp = datetime.now().astimezone().strftime("Saved %H:%M:%S")
        document.mark_saved(result.snapshot, timestamp)
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self._clear_recovery(document.path)
        self._refresh_status()
        if result.warning:
            self.notify(result.warning, severity="warning")
        else:
            self.notify(f"Saved {escape(document.path.name)}")
        if after is not None:
            after()

    def _show_conflict(
        self,
        change: ExternalChange,
        *,
        after: Callable[[], None] | None,
    ) -> None:
        document = self.document
        if document is None:
            return
        document.conflict = True
        document.last_save_status = "Conflict"
        self._save_continuation = after
        can_reload = change.snapshot is not None and change.snapshot.exists
        allow_discard = after is not None and not document.dirty and not can_reload
        self._refresh_status()
        self.push_screen(
            ConflictDialog(
                document.path,
                can_reload=can_reload,
                unavailable=change.kind is ExternalChangeKind.INACCESSIBLE,
                allow_discard=allow_discard,
            ),
            self._handle_conflict_decision,
        )

    def _handle_conflict_decision(self, decision: ConflictDecision | None) -> None:
        if decision is ConflictDecision.SAVE_AS:
            self._open_save_as_dialog()
        elif decision is ConflictDecision.RELOAD:
            self._reload_current_from_disk(automatic=False)
        elif decision is ConflictDecision.DISCARD:
            continuation = self._save_continuation
            self._save_continuation = None
            if continuation is not None:
                continuation()
        else:
            self._cancel_pending_transition()

    def _open_save_as_dialog(self, error: str | None = None) -> None:
        document = self.document
        if document is None:
            self._cancel_pending_transition()
            return
        relative = document.path.relative_to(self.workspace.root)
        suggested = relative.with_name(f"{relative.stem}-local{relative.suffix}").as_posix()
        self.push_screen(SaveAsDialog(suggested, error), self._handle_save_as_closed)

    @on(SaveAsDialog.Submitted)
    def _handle_save_as_submission(self, event: SaveAsDialog.Submitted) -> None:
        document = self.document
        if document is None:
            event.dialog.dismiss(False)
            self._cancel_pending_transition()
            return
        if not event.value:
            event.dialog.show_error("Enter a Markdown filename.")
            return
        try:
            target = self.workspace.validate_document_path(Path(event.value), must_exist=False)
            expected_target = snapshot_file(target)
            target = self.workspace.validate_document_path(target, must_exist=False)
        except WorkspaceError as error:
            event.dialog.show_error(str(error))
            return
        except (OSError, PersistenceError) as error:
            event.dialog.show_error(str(error))
            return
        if expected_target.exists or target.exists() or target.is_symlink():
            event.dialog.show_error("That path already exists; choose a new filename.")
            return

        try:
            result = atomic_save(
                target,
                document.text,
                encoding=document.encoding,
                expected=expected_target,
            )
        except (OSError, PersistenceError) as error:
            event.dialog.show_error(str(error))
            return

        previous_path = document.path
        self._clear_recovery(previous_path)
        document.retarget(target)
        timestamp = datetime.now().astimezone().strftime("Saved %H:%M:%S")
        document.mark_saved(result.snapshot, timestamp)
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self.explorer.set_active(target)
        self._refresh_workspace_index()
        self.explorer.directory_tree.reload()
        self._refresh_status()
        if result.warning:
            self.notify(result.warning, severity="warning")
        else:
            self.notify(f"Saved local version as {escape(target.name)}")
        event.dialog.dismiss(True)

    def _handle_save_as_closed(self, saved: bool | None) -> None:
        if not saved:
            self._cancel_pending_transition()
            return
        continuation = self._save_continuation
        self._save_continuation = None
        if continuation is not None:
            continuation()

    def _reload_current_from_disk(
        self,
        *,
        automatic: bool,
        failure_dialog: bool = False,
    ) -> None:
        document = self.document
        if document is None:
            self._cancel_pending_transition()
            return
        try:
            document.path = self.workspace.validate_document_path(document.path)
            loaded = load_file(document.path)
        except (OSError, PersistenceError, WorkspaceError) as error:
            if failure_dialog:
                continuation = self._save_continuation
                self._save_continuation = None
                self._show_conflict(
                    ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error)),
                    after=continuation,
                )
                return
            if automatic:
                self._mark_external_warning(
                    ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error))
                )
            else:
                self.notify(escape(str(error)), severity="error", title="Reload failed")
            self._cancel_pending_transition()
            return

        previous_path = document.path
        document.replace_from_disk(loaded.text, loaded.snapshot, loaded.encoding)
        with self.editor.prevent(TextArea.Changed):
            self.editor.load_text(document.text)
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self._clear_recovery(previous_path)
        self._schedule_preview(immediate=True)
        if automatic:
            document.last_save_status = "Reloaded externally"
        self._refresh_status()
        if automatic:
            self.notify(f"Reloaded externally changed {escape(document.path.name)}")
        continuation = self._save_continuation
        self._save_continuation = None
        if document.has_mixed_line_endings:
            self.editor.read_only = True
            self._mixed_reload_continuation = continuation
            target = document.line_ending_label.removeprefix("MIXED→")
            self.push_screen(
                MixedLineEndingsDialog(
                    document.path,
                    target,
                    cancel_label="Keep read-only",
                ),
                self._handle_reloaded_mixed_line_ending_decision,
            )
            return
        self.editor.read_only = False
        if continuation is not None:
            continuation()

    def _handle_reloaded_mixed_line_ending_decision(self, accepted: bool | None) -> None:
        document = self.document
        if document is not None and accepted:
            self.editor.read_only = False
            self.notify(
                f"Edits to {escape(document.path.name)} will normalize line endings to "
                f"{document.line_ending_label.removeprefix('MIXED→')}",
                severity="warning",
            )
        elif document is not None:
            self.editor.read_only = True
            document.last_save_status = "Mixed line endings · read-only"
            self._refresh_status()
        continuation = self._mixed_reload_continuation
        self._mixed_reload_continuation = None
        if continuation is not None:
            continuation()

    def _check_external_in_background(self) -> None:
        document = self.document
        if (
            document is None
            or self._has_modal
            or self._pending_transition is not None
            or self._save_continuation is not None
        ):
            return
        self._sync_editor_state()
        try:
            document.path = self.workspace.validate_document_path(document.path)
        except WorkspaceNotFoundError:
            self._mark_external_warning(detect_external_change(document))
            return
        except WorkspaceError as error:
            self._mark_external_warning(
                ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error))
            )
            return
        change = detect_external_change(document)
        if change.kind is ExternalChangeKind.UNCHANGED:
            if change.snapshot is not None:
                document.accept_unchanged_snapshot(change.snapshot)
        elif change.kind is ExternalChangeKind.MODIFIED:
            self._reload_current_from_disk(automatic=True)
        else:
            self._mark_external_warning(change)
        self._refresh_status()

    def _mark_external_warning(self, change: ExternalChange) -> None:
        document = self.document
        if document is None:
            return
        was_conflicted = document.conflict
        document.conflict = True
        status_by_kind = {
            ExternalChangeKind.CONFLICT: "External conflict",
            ExternalChangeKind.DELETED: "Deleted externally",
            ExternalChangeKind.INACCESSIBLE: "File unavailable",
        }
        document.last_save_status = status_by_kind.get(change.kind, "External change")
        self._refresh_status()
        if was_conflicted:
            return
        if change.kind is ExternalChangeKind.DELETED:
            message = f"{document.path.name} was deleted outside TermWriter"
        elif change.kind is ExternalChangeKind.INACCESSIBLE:
            message = f"{document.path.name} cannot be read or verified"
        else:
            message = f"{document.path.name} changed outside TermWriter"
        self.notify(message, severity="warning", title="External change detected")

    def action_request_quit(self) -> None:
        if self._has_modal:
            return
        self._request_transition(self.exit)

    async def action_quit(self) -> None:
        """Defensively route Textual's inherited quit action through the safety gate."""
        self.action_request_quit()

    def action_toggle_explorer(self) -> None:
        if self._has_modal:
            return
        self._explorer_visible = not self._explorer_visible
        self._apply_panel_visibility()
        if self._explorer_visible:
            self.explorer.directory_tree.focus()
        elif self.document is not None:
            self.editor.focus()

    def action_toggle_preview(self) -> None:
        if self._has_modal:
            return
        if self._narrow:
            self._preview_visible = True
            self._narrow_pane = "preview" if self._narrow_pane == "editor" else "editor"
        else:
            self._preview_visible = not self._preview_visible
        self._apply_panel_visibility()
        if self._narrow_pane == "editor" and self.document is not None:
            self.editor.focus()
        elif self.preview.display:
            self.preview.focus()

    def action_find_file(self) -> None:
        if self._has_modal:
            return
        self._refresh_workspace_index()
        self.push_screen(
            FileSearchDialog(self.workspace_files, self.workspace.root),
            self._handle_search_result,
        )

    def _handle_search_result(self, path: Path | None) -> None:
        if path is not None:
            self._request_open(path)

    def action_editor_undo(self) -> None:
        if not self._has_modal and self.document is not None:
            self.editor.undo()

    def action_editor_redo(self) -> None:
        if not self._has_modal and self.document is not None:
            self.editor.redo()

    def action_show_help(self) -> None:
        if not self._has_modal:
            self.push_screen(HelpDialog(SHORTCUT_HELP))

    def _focus_mode(self) -> str:
        if self._narrow and self.preview.display:
            return "PREVIEW"
        focused = self.focused
        if isinstance(focused, MarkdownEditor):
            return "EDIT"
        if isinstance(focused, DirectoryTree):
            return "FILES"
        if isinstance(focused, MarkdownPreview):
            return "PREVIEW"
        return "COMMAND"

    def _refresh_status(self) -> None:
        status_bars = self.query(TermWriterStatusBar)
        if not status_bars:
            return
        status_bars.first(TermWriterStatusBar).show_document(
            self.document,
            root=self.workspace.root,
            mode=self._focus_mode(),
        )
