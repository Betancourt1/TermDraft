"""Textual application and the single document-transition coordinator."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass, replace
from datetime import datetime
from enum import Enum, auto
from pathlib import Path, PurePath

from rich.markup import escape
from textual import events, on, work
from textual.app import App, ComposeResult, SystemCommand
from textual.containers import Horizontal
from textual.screen import ModalScreen, Screen
from textual.timer import Timer
from textual.widgets import DirectoryTree, Static, Tab, Tabs, TextArea
from textual.worker import Worker, get_current_worker

from termwriter.bindings import (
    APP_BINDINGS,
    MARKDOWN_SYNTAX_HELP,
    format_shortcut_help,
)
from termwriter.config import ConfigError, TermWriterConfig, load_config
from termwriter.models.document import Document, FileSnapshot
from termwriter.models.workspace import (
    Workspace,
    WorkspaceError,
    WorkspaceNotFoundError,
    paths_are_spelling_aliases,
)
from termwriter.screens.dialogs import (
    ConflictDecision,
    ConflictDialog,
    FileSearchDialog,
    HelpDialog,
    MixedLineEndingsDialog,
    RecoveryDecision,
    RecoveryDeleteDialog,
    RecoveryDialog,
    RecoveryManagerAction,
    RecoveryManagerDialog,
    RecoveryManagerRequest,
    RecoveryRetentionDialog,
    RecoveryRetentionRequest,
    SaveAsDialog,
    TextSearchDialog,
    UnsavedChangesDialog,
    UnsavedDecision,
)
from termwriter.screens.recent_documents import RecentDocumentsDialog
from termwriter.services.external_changes import (
    DiskProbe,
    ExternalChange,
    ExternalChangeKind,
    classify_external_change,
    probe_file,
)
from termwriter.services.persistence import (
    ExternalModificationError,
    LoadedFile,
    PersistenceError,
    SaveResult,
    atomic_save,
    load_file,
    snapshot_file,
)
from termwriter.services.recovery import (
    RecoveryEntry,
    RecoveryError,
    RecoveryJournal,
    RecoveryRecord,
    RecoveryRetentionResult,
    RecoveryScan,
)
from termwriter.services.session import (
    MAX_SESSION_DOCUMENTS,
    DocumentViewState,
    SessionLoadResult,
    SessionState,
    SessionStore,
)
from termwriter.services.text_search import TextSearchMatch, TextSearchOverride
from termwriter.widgets.editor import MarkdownEditor
from termwriter.widgets.file_tree import FileExplorer
from termwriter.widgets.preview import MarkdownPreview
from termwriter.widgets.status_bar import TermWriterStatusBar


class _ProbePurpose(Enum):
    WATCH = auto()
    TRANSITION = auto()
    CURRENT_RESULT = auto()
    SAVE_CLEAN = auto()
    SAVE_RECOVERY = auto()


class _LoadPurpose(Enum):
    OPEN = auto()
    RELOAD_AUTOMATIC = auto()
    RELOAD_MANUAL = auto()


class _RecoveryMutationKind(Enum):
    SAVE = auto()
    DELETE = auto()


@dataclass(frozen=True, slots=True)
class _DocumentTicket:
    document: Document
    generation: int
    path: Path
    snapshot: FileSnapshot


@dataclass(frozen=True, slots=True)
class _SaveWorkerResult:
    saved: SaveResult | None = None
    external_snapshot: FileSnapshot | None = None
    error: str | None = None
    inaccessible: bool = False


@dataclass(frozen=True, slots=True)
class _SaveAsWorkerResult:
    target: Path | None = None
    saved: SaveResult | None = None
    error: str | None = None


@dataclass(frozen=True, slots=True)
class _OrphanWorkerResult:
    scan: RecoveryScan
    unavailable: tuple[RecoveryEntry, ...]


@dataclass(frozen=True, slots=True)
class _RecoveryManagementResult:
    action: RecoveryManagerAction
    entry: RecoveryEntry | None = None
    quarantine_path: Path | None = None
    target: Path | None = None
    warning: str | None = None
    source_unavailable: bool = False
    error: str | None = None


@dataclass(frozen=True, slots=True)
class _RecoveryCleanupWorkerResult:
    result: RecoveryRetentionResult | None = None
    error: str | None = None


@dataclass(frozen=True, slots=True)
class _RecoveryMutation:
    sequence: int
    kind: _RecoveryMutationKind
    path: Path
    document: Document | None = None
    text: str | None = None
    encoding: str | None = None
    base_snapshot: FileSnapshot | None = None
    fingerprint: str | None = None


@dataclass(frozen=True, slots=True)
class _RecoveryMutationResult:
    record: RecoveryRecord | None = None
    error: str | None = None


@dataclass(slots=True)
class _OpenDocument:
    tab_id: str
    document: Document


class TermWriterApp(App[None]):
    """Local-first Markdown editor with guarded document transitions."""

    TITLE = "TermWriter"
    ENABLE_COMMAND_PALETTE = True
    BINDINGS = APP_BINDINGS

    def __init__(
        self,
        workspace: Workspace,
        *,
        preview_debounce: float = 0.2,
        external_poll_interval: float = 2.0,
        recovery_debounce: float = 0.5,
        recovery_journal: RecoveryJournal | None = None,
        session_store: SessionStore | None = None,
        config: TermWriterConfig | None = None,
    ) -> None:
        self.config = config or TermWriterConfig(root=Path.home() / ".termwriter")
        self._config_root = config.root if config is not None else None
        default_css = Path(__file__).with_name("default.tcss")
        css_paths: list[str | PurePath] = [default_css]
        watch_user_css = config is not None and self.config.theme_path.is_file()
        if watch_user_css:
            css_paths.append(self.config.theme_path)
        super().__init__(css_path=css_paths, watch_css=watch_user_css)
        self.set_keymap(dict(self.config.keybindings))
        self.workspace = workspace
        self.document: Document | None = None
        self._open_documents: list[_OpenDocument] = []
        self._next_tab_id = 0
        self._quit_documents: list[Document] | None = None
        self.preview_debounce = preview_debounce
        self.external_poll_interval = external_poll_interval
        self.recovery_debounce = recovery_debounce
        self.recovery_journal = recovery_journal or RecoveryJournal()
        if session_store is None and recovery_journal is not None:
            session_store = SessionStore(recovery_journal.state_root.parent / "sessions")
        self.session_store = session_store or SessionStore()
        self._session_warning: str | None = None
        self._session_views: dict[Path, DocumentViewState] = {}
        self._session_active_path: Path | None = None
        self._recent_paths: list[Path] = []
        self._session_save_in_flight = False
        self._pending_session_state: SessionState | None = None
        self._exit_requested = False
        self.workspace_files: tuple[Path, ...] = ()
        self._preview_timer: Timer | None = None
        self._external_watch_timer: Timer | None = None
        self._recovery_timer: Timer | None = None
        self._recovery_revision = 0
        self._recovery_mutation_sequence = 0
        self._recovery_mutation_queue: list[_RecoveryMutation] = []
        self._recovery_mutation_in_flight: _RecoveryMutation | None = None
        self._recovery_delete_waiters: dict[Path, list[Callable[[], None]]] = {}
        self._known_recovery_fingerprints: dict[Path, str] = {}
        self._preview_revision = 0
        self._pending_transition: Callable[[], object] | None = None
        self._save_continuation: Callable[[], None] | None = None
        self._explorer_visible = True
        self._preview_visible = True
        self._narrow = False
        self._narrow_pane = "editor"
        self._editor_baseline_text = ""
        self._editor_baseline_source_text = ""
        self._pending_open_document: Document | None = None
        self._pending_recovery_entry: RecoveryEntry | None = None
        self._pending_recovery_record: RecoveryRecord | None = None
        self._pending_recovery_is_orphan = False
        self._orphan_recoveries: list[RecoveryEntry] = []
        self._scan_orphans_after_session_open = False
        self._mixed_reload_continuation: Callable[[], None] | None = None
        self._pending_open_location: tuple[Path, int, int] | None = None
        self._pending_focus_location: tuple[int, int] | None = None
        self._preview_heading_announcement: str | None = None
        self._document_generation = 0
        self._critical_io = False
        self._critical_document: Document | None = None
        self._critical_previous_read_only = False
        self._critical_previous_status: str | None = None
        self._critical_froze_editor = False
        self._critical_changed_status = False
        self._watch_probe_worker: Worker[None] | None = None

    def compose(self) -> ComposeResult:
        yield Static(f"TermWriter  ·  {self.workspace.root}", id="title-bar", markup=False)
        yield Tabs(id="document-tabs")
        with Horizontal(id="workspace"):
            yield FileExplorer(self.workspace)
            with Horizontal(id="workbench"):
                yield MarkdownEditor(
                    auto_continue_lists=self.config.editor.auto_continue_lists,
                    soft_wrap=self.config.editor.soft_wrap,
                    show_line_numbers=self.config.editor.show_line_numbers,
                )
                yield MarkdownPreview()
        yield TermWriterStatusBar()

    async def on_mount(self) -> None:
        await self._load_session_worker().wait()
        self._refresh_workspace_index()
        self._external_watch_timer = self.set_interval(
            self.external_poll_interval,
            self._check_external_in_background,
            name="external-file-watch",
        )
        self._narrow = self.size.width < 100
        self._apply_panel_visibility()
        if self._session_warning is not None:
            self.notify(
                escape(self._session_warning),
                severity="warning",
                title="Session state ignored",
            )
        initial_file = self.workspace.initial_file
        restoring_session = initial_file is None and self._session_active_path is not None
        if initial_file is None and self._session_active_path is not None:
            try:
                initial_file = self.workspace.validate_document_path(self._session_active_path)
            except WorkspaceNotFoundError as error:
                missing_path = self._session_active_path
                self._session_active_path = None
                self._forget_session_path(missing_path)
                self._persist_session()
                self.notify(
                    escape(str(error)),
                    severity="warning",
                    title="Previous session document unavailable",
                )
            except WorkspaceError as error:
                self.notify(
                    escape(str(error)),
                    severity="warning",
                    title="Previous session document unavailable",
                )
        if initial_file is not None:
            worker = self._open_file_now(initial_file)
            if worker is not None:
                await worker.wait()
            if restoring_session:
                if self._has_modal:
                    self._scan_orphans_after_session_open = True
                else:
                    await self._scan_orphan_recoveries().wait()
        else:
            self.explorer.directory_tree.focus()
            self._refresh_status()
            await self._scan_orphan_recoveries().wait()

    @work(group="session-load", thread=True, exit_on_error=False)
    def _load_session_worker(self) -> None:
        try:
            result = self.session_store.load(self.workspace.root)
        except Exception as error:
            result = SessionLoadResult(warning=f"Cannot read session state: {error}")
        self.call_from_thread(self._apply_session_result, result)

    def _apply_session_result(self, result: SessionLoadResult) -> None:
        """Install one bounded, content-free session result on the UI thread."""
        session = result.state
        self._session_warning = result.warning
        self._session_views = {
            view.path: view for view in (() if session is None else session.documents)
        }
        self._session_active_path = None if session is None else session.active_path
        self._recent_paths = [view.path for view in (() if session is None else session.documents)]
        if self._session_active_path is not None:
            self._mark_document_recent(self._session_active_path)

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
        screens = self.screen_stack
        return bool(screens and isinstance(screens[-1], ModalScreen))

    @property
    def editor(self) -> MarkdownEditor:
        return self.query_one(MarkdownEditor)

    @property
    def preview(self) -> MarkdownPreview:
        return self.query_one(MarkdownPreview)

    @property
    def document_tabs(self) -> Tabs:
        return self.query_one("#document-tabs", Tabs)

    @property
    def explorer(self) -> FileExplorer:
        return self.query_one(FileExplorer)

    def _document_ticket(self, document: Document) -> _DocumentTicket:
        return _DocumentTicket(
            document=document,
            generation=self._document_generation,
            path=document.path,
            snapshot=document.snapshot,
        )

    def _ticket_is_current(self, ticket: _DocumentTicket) -> bool:
        document = self.document
        return (
            document is ticket.document
            and self._document_generation == ticket.generation
            and document.path == ticket.path
            and document.snapshot == ticket.snapshot
        )

    def _accept_unchanged_snapshot(
        self,
        document: Document,
        snapshot: FileSnapshot,
    ) -> None:
        if snapshot != document.snapshot:
            self._document_generation += 1
        document.accept_unchanged_snapshot(snapshot)

    def _begin_critical_io(
        self,
        document: Document | None,
        *,
        freeze_editor: bool,
        status: str | None = None,
    ) -> bool:
        if self._critical_io:
            self.notify("A file operation is already in progress", severity="warning")
            return False
        self._critical_io = True
        self._critical_document = document
        self._critical_froze_editor = freeze_editor and document is not None
        self._critical_changed_status = status is not None and document is not None
        self._critical_previous_status = document.last_save_status if document is not None else None
        if document is not None:
            self._critical_previous_read_only = self.editor.read_only
            if self._critical_froze_editor:
                self.editor.read_only = True
            if status is not None:
                document.last_save_status = status
        self._refresh_status()
        return True

    def _finish_critical_io(self, *, restore_status: bool = True) -> None:
        document = self._critical_document
        if document is not None and self.document is document:
            if self._critical_froze_editor:
                self.editor.read_only = self._critical_previous_read_only
            if (
                restore_status
                and self._critical_changed_status
                and self._critical_previous_status is not None
            ):
                document.last_save_status = self._critical_previous_status
        self._critical_io = False
        self._critical_document = None
        self._critical_froze_editor = False
        self._critical_changed_status = False
        self._critical_previous_status = None
        self._refresh_status()

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

    def _remember_document_view(self, document: Document) -> None:
        """Cache one document's content-free view coordinates."""
        cursor = document.cursor
        self._session_views[document.path] = DocumentViewState(
            document.path,
            line=cursor.line,
            column=cursor.column,
            scroll_x=cursor.scroll_x,
            scroll_y=cursor.scroll_y,
        )
        self._mark_document_recent(document.path)

    def _mark_document_recent(self, path: Path) -> None:
        """Move one exact workspace path to the front of the MRU order."""
        recent_paths = [
            path,
            *(candidate for candidate in self._recent_paths if candidate != path),
        ]
        self._recent_paths = recent_paths[:MAX_SESSION_DOCUMENTS]
        for evicted in recent_paths[MAX_SESSION_DOCUMENTS:]:
            self._session_views.pop(evicted, None)

    def _forget_session_path(self, path: Path) -> None:
        self._recent_paths = [candidate for candidate in self._recent_paths if candidate != path]
        self._session_views.pop(path, None)

    def _recent_document_paths(self, *, prune_missing: bool = False) -> tuple[Path, ...]:
        """Return valid MRU entries and optionally forget confirmed missing paths."""
        available: list[Path] = []
        missing: list[Path] = []
        for path in self._recent_paths:
            try:
                safe_path = self.workspace.validate_document_path(path)
            except WorkspaceNotFoundError:
                if prune_missing:
                    missing.append(path)
                continue
            except WorkspaceError:
                continue
            if safe_path not in available:
                available.append(safe_path)
        for path in missing:
            self._forget_session_path(path)
        if missing:
            self._persist_session()
        return tuple(available)

    def _persist_session(self) -> None:
        """Queue best-effort view persistence; Markdown is never included."""
        document = self.document
        if document is not None:
            self._remember_document_view(document)
        state = SessionState(
            workspace_root=self.workspace.root,
            active_path=None if document is None else document.path,
            documents=tuple(
                self._session_views[path]
                for path in self._recent_paths
                if path in self._session_views
            ),
        )
        self._queue_session_save(state)

    def _queue_session_save(self, state: SessionState) -> None:
        """Serialize writes and retain only the newest waiting session snapshot."""
        if self._session_save_in_flight:
            self._pending_session_state = state
            return
        self._session_save_in_flight = True
        self._session_save_worker(state)

    @work(group="session-save", thread=True, exit_on_error=False)
    def _session_save_worker(self, state: SessionState) -> None:
        try:
            self.session_store.save(state)
            error_message = None
        except Exception as error:
            error_message = str(error)
        self.call_from_thread(self._handle_session_save_result, error_message)

    def _handle_session_save_result(self, error_message: str | None) -> None:
        self._session_save_in_flight = False
        if error_message is not None:
            self.notify(
                escape(error_message),
                severity="warning",
                title="Session position not saved",
            )
        pending = self._pending_session_state
        self._pending_session_state = None
        if pending is not None:
            self._queue_session_save(pending)
            return
        self._maybe_finish_exit()

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
        if not isinstance(event.widget, MarkdownPreview):
            self._preview_heading_announcement = None
        self._refresh_status()

    @on(MarkdownPreview.HeadingFocused)
    def preview_heading_focused(self, event: MarkdownPreview.HeadingFocused) -> None:
        event.stop()
        announcement = f"H{event.level} {event.position}/{event.total} · {event.label}"
        self._preview_heading_announcement = announcement
        self._refresh_status()

    def _open_document_for_path(self, path: Path) -> Document | None:
        for opened in self._open_documents:
            if paths_are_spelling_aliases(opened.document.path, path):
                return opened.document
        return None

    def _open_entry_for_document(self, document: Document) -> _OpenDocument | None:
        return next(
            (opened for opened in self._open_documents if opened.document is document),
            None,
        )

    def _tab_label(self, document: Document) -> str:
        relative = document.path.relative_to(self.workspace.root).as_posix()
        state = "!" if document.conflict else "●" if document.dirty else ""
        return f"{state} {relative}" if state else relative

    def _register_open_document(self, document: Document) -> bool:
        if self._open_entry_for_document(document) is not None:
            return False
        self._next_tab_id += 1
        opened = _OpenDocument(f"document-tab-{self._next_tab_id}", document)
        self._open_documents.append(opened)
        self.document_tabs.add_tab(Tab(self._tab_label(document), id=opened.tab_id))
        self.call_after_refresh(self._refresh_document_tabs)
        return True

    def _refresh_document_tabs(self) -> None:
        tabs = self.document_tabs
        tabs.display = len(self._open_documents) > 1
        active_id = ""
        for opened in self._open_documents:
            tab = tabs.get_tab(opened.tab_id)
            if tab is not None:
                tab.label = self._tab_label(opened.document)
            if opened.document is self.document:
                active_id = opened.tab_id
        if active_id and tabs.active != active_id:
            tabs.active = active_id

    @on(Tabs.TabActivated, "#document-tabs")
    def document_tab_activated(self, event: Tabs.TabActivated) -> None:
        event.stop()
        if not self.query(MarkdownEditor) or self.document_tabs.active != event.tab.id:
            return
        opened = next(
            (item for item in self._open_documents if item.tab_id == event.tab.id),
            None,
        )
        if opened is None or opened.document is self.document:
            return
        if self._critical_io or self._has_modal:
            self.notify("Wait for the current operation to finish", severity="warning")
            self.call_after_refresh(self._refresh_document_tabs)
            return
        self._activate_document(opened.document)

    def _request_open(self, path: Path) -> None:
        try:
            safe_path = self.workspace.validate_document_path(path)
        except WorkspaceError as error:
            self.notify(escape(str(error)), severity="error", title="Cannot open file")
            return
        if self.document is not None and safe_path == self.document.path:
            self.editor.focus()
            return
        opened = self._open_document_for_path(safe_path)
        if opened is not None:
            if self._critical_io:
                self.notify("Wait for the current file operation to finish", severity="warning")
                return
            self._activate_document(opened)
            return
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        self._open_file_now(safe_path)

    def _request_open_at(self, path: Path, line: int, column: int) -> None:
        """Open a validated search result through the normal transition guard."""
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        if self.document is not None and self._is_current_document(path):
            self._sync_editor_state()
            if self.document.dirty:
                self._focus_editor_at(line, column)
                return
            self._pending_focus_location = (line, column)
            ticket = self._document_ticket(self.document)
            if self._begin_critical_io(self.document, freeze_editor=False):
                self._probe_document_worker(ticket, _ProbePurpose.CURRENT_RESULT)
            return
        try:
            safe_path = self.workspace.validate_document_path(path)
        except WorkspaceError as error:
            self.notify(escape(str(error)), severity="error", title="Cannot open search result")
            return
        opened = self._open_document_for_path(safe_path)
        if opened is not None:
            self._activate_document(opened)
            self._focus_editor_at(line, column)
            return
        self._pending_open_location = (safe_path, line, column)
        self._open_file_now(safe_path)

    def _focus_editor_at(self, line: int, column: int) -> None:
        self._narrow_pane = "editor"
        self._apply_panel_visibility()
        self.editor.move_cursor((line, column), center=True)
        self.editor.focus()

    def _is_current_document(self, path: Path) -> bool:
        document = self.document
        if document is None:
            return False
        return paths_are_spelling_aliases(path, document.path)

    def _request_transition(self, continuation: Callable[[], object]) -> None:
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        self._sync_editor_state()
        document = self.document
        if document is not None and document.dirty:
            self._pending_transition = continuation
            self.push_screen(UnsavedChangesDialog(document.path), self._handle_unsaved_decision)
            return
        if document is not None:
            try:
                safe_path = self.workspace.validate_document_path(document.path)
            except WorkspaceError as error:
                self._pending_transition = continuation
                self._show_conflict(
                    ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error)),
                    after=self._complete_pending_transition,
                )
                return
            if safe_path != document.path:
                document.path = safe_path
                self._document_generation += 1
            self._pending_transition = continuation
            ticket = self._document_ticket(document)
            if self._begin_critical_io(document, freeze_editor=False):
                self._probe_document_worker(ticket, _ProbePurpose.TRANSITION)
            return
        continuation()

    def _handle_unsaved_decision(self, decision: UnsavedDecision | None) -> None:
        if decision is UnsavedDecision.SAVE:
            self._save_current(after=self._complete_pending_transition)
        elif decision is UnsavedDecision.DISCARD:
            if self.document is not None:
                self._clear_recovery(
                    self.document.path,
                    after=self._complete_pending_transition,
                )
            else:
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
        self._quit_documents = None
        self._pending_open_location = None
        self._pending_focus_location = None
        self._refresh_status()

    def _open_file_now(self, path: Path) -> Worker[None] | None:
        try:
            safe_path = self.workspace.validate_document_path(path)
        except WorkspaceError as error:
            self.notify(escape(str(error)), severity="error", title="Cannot open file")
            self._cancel_pending_open()
            return None

        current = self.document
        if not self._begin_critical_io(
            current,
            freeze_editor=current is not None,
            status="Opening…" if current is not None else None,
        ):
            return None
        return self._load_document_worker(None, safe_path, _LoadPurpose.OPEN, False)

    def _finish_open_loaded(self, safe_path: Path, loaded: LoadedFile) -> None:
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
            recovery_record = self.recovery_journal.record_for(document.path)
        except RecoveryError as error:
            self.notify(escape(str(error)), severity="error", title="Recovery draft unavailable")
            recovery_record = None
        recovery = None if recovery_record is None else recovery_record.entry
        self._pending_recovery_record = recovery_record
        if recovery_record is not None:
            self._known_recovery_fingerprints[document.path] = recovery_record.fingerprint
        if recovery is not None and recovery.text == document.text:
            self._pending_recovery_entry = None
            self._clear_recovery(document.path, after=self._continue_pending_open)
            return
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
        self._pending_recovery_record = None
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
            if is_orphan:
                self._clear_recovery(document.path, after=self._finish_discarded_orphan)
            else:
                self._clear_recovery(document.path, after=self._continue_pending_open)
        else:
            if is_orphan:
                self._orphan_recoveries.clear()
            self._cancel_pending_open()

    def _finish_discarded_orphan(self) -> None:
        self._cancel_pending_open()
        self.call_after_refresh(self._offer_next_orphan_recovery)

    def _scan_orphan_recoveries(self) -> Worker[None]:
        return self._scan_orphan_recoveries_worker()

    @work(group="recovery-inventory", exclusive=True, thread=True, exit_on_error=False)
    def _load_recovery_inventory_worker(self) -> None:
        worker = get_current_worker()
        try:
            records = self.recovery_journal.list_entries(
                self.workspace.root
            ) + self.recovery_journal.list_quarantined(self.workspace.root)
            error_message = None
        except Exception as error:
            records = ()
            error_message = str(error)
        if not worker.is_cancelled:
            self.call_from_thread(
                self._show_recovery_inventory,
                records,
                error_message,
            )

    def _show_recovery_inventory(
        self,
        records: tuple[RecoveryRecord, ...],
        error_message: str | None,
    ) -> None:
        if error_message is not None:
            self.notify(
                escape(error_message),
                severity="error",
                title="Recovery inventory unavailable",
            )
            return
        if self._has_modal or self._critical_io:
            return
        protected_paths = tuple(
            opened.document.path for opened in self._open_documents if opened.document.dirty
        )
        protected_journal_paths = frozenset(
            self.recovery_journal.path_for(path) for path in protected_paths
        )
        self.push_screen(
            RecoveryManagerDialog(
                records,
                self.workspace.root,
                protected_journal_paths=protected_journal_paths,
                retention_days=self.config.recovery.retention_days,
            ),
            self._handle_recovery_manager_request,
        )

    def _handle_recovery_manager_request(
        self,
        request: RecoveryManagerRequest | RecoveryRetentionRequest | None,
    ) -> None:
        if request is None:
            return
        if isinstance(request, RecoveryRetentionRequest):
            self.call_after_refresh(self._confirm_recovery_cleanup, request)
            return
        if request.action is RecoveryManagerAction.DELETE_QUARANTINED:
            self.call_after_refresh(self._confirm_quarantine_delete, request)
            return
        self._start_recovery_management(request)

    def _confirm_recovery_cleanup(self, request: RecoveryRetentionRequest) -> None:
        self.push_screen(
            RecoveryRetentionDialog(request),
            lambda confirmed: self._handle_recovery_cleanup_confirmation(request, confirmed),
        )

    def _handle_recovery_cleanup_confirmation(
        self,
        request: RecoveryRetentionRequest,
        confirmed: bool | None,
    ) -> None:
        if not confirmed:
            return
        self._sync_editor_state()
        if self._begin_critical_io(self.document, freeze_editor=False):
            self._cleanup_recovery_worker(request)

    @work(group="recovery-management", exclusive=True, thread=True, exit_on_error=False)
    def _cleanup_recovery_worker(self, request: RecoveryRetentionRequest) -> None:
        worker = get_current_worker()
        try:
            result = self.recovery_journal.cleanup_quarantined(
                before=request.cutoff,
                workspace_root=self.workspace.root,
                records=request.records,
            )
            outcome = _RecoveryCleanupWorkerResult(result=result)
        except Exception as error:
            outcome = _RecoveryCleanupWorkerResult(error=str(error))
        if not worker.is_cancelled:
            self.call_from_thread(self._handle_recovery_cleanup_result, outcome)

    def _handle_recovery_cleanup_result(self, outcome: _RecoveryCleanupWorkerResult) -> None:
        self._finish_critical_io()
        if outcome.error is not None:
            self.notify(
                escape(outcome.error),
                severity="error",
                title="Recovery retention failed",
            )
            return
        result = outcome.result
        if result is None:
            return
        message = f"Deleted {result.deleted_count} of {result.selected_count} expired recoveries"
        if result.failed_count:
            failures = [item for item in result.outcomes if not item.deleted]
            details = "\n".join(
                f"- {item.document_path}: {item.error or 'deletion failed'}" for item in failures
            )
            message += f"\n{details}"
            self.notify(
                escape(message),
                severity="warning",
                title=f"{result.failed_count} recovery deletion(s) failed",
            )
        else:
            self.notify(message)

    def _confirm_quarantine_delete(self, request: RecoveryManagerRequest) -> None:
        self.push_screen(
            RecoveryDeleteDialog(request.record),
            lambda confirmed: self._handle_quarantine_delete_confirmation(
                request,
                confirmed,
            ),
        )

    def _handle_quarantine_delete_confirmation(
        self,
        request: RecoveryManagerRequest,
        confirmed: bool | None,
    ) -> None:
        if confirmed:
            self._start_recovery_management(request)

    def _start_recovery_management(self, request: RecoveryManagerRequest) -> None:
        self._sync_editor_state()
        document = self.document
        protected_paths = tuple(
            opened.document.path for opened in self._open_documents if opened.document.dirty
        )
        if self._begin_critical_io(document, freeze_editor=True):
            self._manage_recovery_worker(request, protected_paths)

    @work(group="recovery-management", exclusive=True, thread=True, exit_on_error=False)
    def _manage_recovery_worker(
        self,
        request: RecoveryManagerRequest,
        protected_paths: tuple[Path, ...],
    ) -> None:
        worker = get_current_worker()
        try:
            protected_journal_paths = {
                self.recovery_journal.path_for(path) for path in protected_paths
            }
            if (
                request.action in {RecoveryManagerAction.RETARGET, RecoveryManagerAction.QUARANTINE}
                and request.record.journal_path in protected_journal_paths
            ):
                raise RecoveryError(
                    "Cannot move or archive an open dirty document's recovery draft"
                )
            if (
                request.action is RecoveryManagerAction.RESTORE_QUARANTINED
                and request.record.entry is not None
                and any(
                    paths_are_spelling_aliases(
                        request.record.entry.document_path,
                        protected_path,
                    )
                    for protected_path in protected_paths
                )
            ):
                raise RecoveryError(
                    "Cannot restore a quarantined draft onto an open dirty document"
                )
            if request.action is RecoveryManagerAction.RESTORE_QUARANTINED:
                entry = self.recovery_journal.restore_quarantined(request.record)
                result = _RecoveryManagementResult(
                    request.action,
                    entry=entry,
                    source_unavailable=self._recovery_source_is_unavailable(entry),
                )
            elif request.action is RecoveryManagerAction.DELETE_QUARANTINED:
                self.recovery_journal.delete_quarantined(request.record)
                result = _RecoveryManagementResult(
                    request.action,
                    quarantine_path=request.record.journal_path,
                )
            elif request.action is RecoveryManagerAction.EXPORT_QUARANTINED:
                target = self.workspace.validate_document_path(
                    Path(request.target or ""),
                    must_exist=False,
                )
                saved = self.recovery_journal.export_quarantined(
                    request.record,
                    destination=target,
                )
                result = _RecoveryManagementResult(
                    request.action,
                    target=target,
                    warning=saved.warning,
                )
            elif request.action is RecoveryManagerAction.RETARGET:
                target = self.workspace.validate_document_path(
                    Path(request.target or ""),
                    must_exist=False,
                )
                if any(
                    paths_are_spelling_aliases(target, protected_path)
                    for protected_path in protected_paths
                ):
                    raise RecoveryError(
                        "Cannot retarget a recovery draft onto an open dirty document"
                    )
                entry = self.recovery_journal.retarget(
                    request.record,
                    document_path=target,
                    workspace_root=self.workspace.root,
                )
                result = _RecoveryManagementResult(request.action, entry=entry)
            elif request.action is RecoveryManagerAction.QUARANTINE:
                quarantine_path = self.recovery_journal.quarantine(request.record)
                result = _RecoveryManagementResult(
                    request.action,
                    quarantine_path=quarantine_path,
                )
            else:
                selected = request.record.entry
                loaded_entry = (
                    None if selected is None else self.recovery_journal.load(selected.document_path)
                )
                if loaded_entry is None:
                    result = _RecoveryManagementResult(
                        request.action,
                        error="The selected recovery draft is no longer available.",
                    )
                else:
                    result = _RecoveryManagementResult(
                        request.action,
                        entry=loaded_entry,
                        source_unavailable=self._recovery_source_is_unavailable(loaded_entry),
                    )
        except (OSError, PersistenceError, RecoveryError, WorkspaceError) as error:
            result = _RecoveryManagementResult(request.action, error=str(error))
        except Exception as error:
            result = _RecoveryManagementResult(
                request.action,
                error=f"Unexpected recovery-management failure: {error}",
            )
        if not worker.is_cancelled:
            self.call_from_thread(self._handle_recovery_management_result, result)

    def _handle_recovery_management_result(
        self,
        result: _RecoveryManagementResult,
    ) -> None:
        self._finish_critical_io()
        if result.error is not None:
            self.notify(
                escape(result.error),
                severity="error",
                title="Recovery operation may be incomplete",
            )
            self.call_after_refresh(self._load_recovery_inventory_worker)
            return
        if result.action is RecoveryManagerAction.RETARGET and result.entry is not None:
            relative = result.entry.document_path.relative_to(self.workspace.root).as_posix()
            self.notify(f"Recovery draft now follows {escape(relative)}")
            return
        if result.action is RecoveryManagerAction.QUARANTINE:
            name = result.quarantine_path.name if result.quarantine_path is not None else "entry"
            self.notify(f"Archived recovery {escape(name)}")
            return
        if result.action is RecoveryManagerAction.DELETE_QUARANTINED:
            name = result.quarantine_path.name if result.quarantine_path is not None else "entry"
            self.notify(f"Permanently deleted quarantined recovery {escape(name)}")
            return
        if result.action is RecoveryManagerAction.EXPORT_QUARANTINED:
            if result.target is None:
                return
            relative = result.target.relative_to(self.workspace.root).as_posix()
            if result.warning is not None:
                self.notify(escape(result.warning), severity="warning")
            self.notify(f"Exported recovery copy to {escape(relative)}")
            self._refresh_workspace_index()
            return
        entry = result.entry
        if entry is None:
            return
        if result.action is RecoveryManagerAction.RESTORE_QUARANTINED:
            relative = entry.document_path.relative_to(self.workspace.root).as_posix()
            self.notify(f"Restored quarantined recovery for {escape(relative)}")
        if result.source_unavailable:
            self._request_transition(lambda: self._open_managed_orphan_recovery(entry))
        else:
            self._request_transition(lambda: self._open_file_now(entry.document_path))

    def _open_managed_orphan_recovery(self, entry: RecoveryEntry) -> None:
        self._orphan_recoveries.insert(0, entry)
        self._offer_next_orphan_recovery()

    def _finish_orphan_recovery_scan(self, result: _OrphanWorkerResult) -> None:
        scan = result.scan
        if scan.warnings:
            self.notify(
                f"Skipped {len(scan.warnings)} invalid recovery entry or entries",
                severity="warning",
                title="Recovery scan",
            )
        self._orphan_recoveries = list(result.unavailable)
        self._offer_next_orphan_recovery()

    def _scan_orphan_recoveries_now(self) -> _OrphanWorkerResult:
        result = self.recovery_journal.scan_workspace(self.workspace.root)
        unavailable = tuple(
            entry for entry in result.entries if self._recovery_source_is_unavailable(entry)
        )
        return _OrphanWorkerResult(result, unavailable)

    @work(group="recovery-scan", exclusive=True, thread=True, exit_on_error=False)
    def _scan_orphan_recoveries_worker(self) -> None:
        worker = get_current_worker()
        try:
            result = self._scan_orphan_recoveries_now()
            error_message = None
        except Exception as error:
            result = None
            error_message = str(error)
        if worker.is_cancelled:
            return
        if result is not None:
            self.call_from_thread(self._finish_orphan_recovery_scan, result)
        else:
            self.call_from_thread(
                self.notify,
                escape(error_message or "Recovery scan failed"),
                severity="warning",
                title="Recovery scan",
            )

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
        try:
            recovery_record = self.recovery_journal.record_for(recovery.document_path)
        except RecoveryError as error:
            self.notify(escape(str(error)), severity="warning", title="Recovery draft changed")
            recovery_record = None
        if recovery_record is not None:
            self._known_recovery_fingerprints[recovery.document_path] = recovery_record.fingerprint
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
        self._pending_recovery_record = recovery_record
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
        self._pending_recovery_record = None
        self._pending_recovery_is_orphan = False
        self._pending_open_location = None
        if self.document is not None:
            self.editor.focus()
            if self.document.dirty:
                self._schedule_recovery()
        self._continue_session_orphan_scan()
        self._refresh_status()

    def _install_document(self, document: Document) -> None:
        newly_opened = self._register_open_document(document)
        self._activate_document(document, newly_opened=newly_opened)

    def _flush_recovery_before_switch(self, document: Document) -> None:
        self._recovery_revision += 1
        if self._recovery_timer is not None:
            self._recovery_timer.stop()
            self._recovery_timer = None
        if document.dirty:
            self._queue_recovery_save(document)

    def _activate_document(
        self,
        document: Document,
        *,
        newly_opened: bool = False,
        flush_previous_recovery: bool = True,
    ) -> None:
        previous = self.document
        if previous is document:
            self.editor.focus()
            self._refresh_document_tabs()
            return
        if previous is not None:
            self._sync_editor_state()
            self._remember_document_view(previous)
            if flush_previous_recovery:
                self._flush_recovery_before_switch(previous)
        self.document = document
        self._document_generation += 1
        with self.editor.prevent(TextArea.Changed):
            self.editor.load_text(document.text)
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self.editor.read_only = document.read_only
        self.explorer.set_active(document.path)
        self._narrow_pane = "editor"
        self._apply_panel_visibility()
        self._schedule_preview(immediate=True)
        target = self._pending_open_location
        self._pending_open_location = None
        if target is not None and target[0] == document.path:
            self.editor.move_cursor((target[1], target[2]), center=True)
            line, column = self.editor.cursor_location
            document.update_cursor(
                line,
                column,
                scroll_x=float(self.editor.scroll_offset.x),
                scroll_y=float(self.editor.scroll_offset.y),
            )
        elif newly_opened:
            view = self._session_views.get(document.path)
            if view is not None:
                document.update_cursor(
                    view.line,
                    view.column,
                    scroll_x=view.scroll_x,
                    scroll_y=view.scroll_y,
                )
                self.editor.move_cursor((view.line, view.column))
                self.editor.scroll_to(
                    view.scroll_x,
                    view.scroll_y,
                    animate=False,
                    immediate=True,
                )
                line, column = self.editor.cursor_location
                document.update_cursor(
                    line,
                    column,
                    scroll_x=float(self.editor.scroll_offset.x),
                    scroll_y=float(self.editor.scroll_offset.y),
                )
            else:
                self.editor.scroll_to(0, 0, animate=False, immediate=True)
        else:
            cursor = document.cursor
            self.editor.move_cursor((cursor.line, cursor.column))
            self.editor.scroll_to(
                cursor.scroll_x,
                cursor.scroll_y,
                animate=False,
                immediate=True,
            )
            line, column = self.editor.cursor_location
            document.update_cursor(
                line,
                column,
                scroll_x=float(self.editor.scroll_offset.x),
                scroll_y=float(self.editor.scroll_offset.y),
            )
        self.editor.focus()
        self._persist_session()
        self._refresh_document_tabs()
        self._continue_session_orphan_scan()
        self._refresh_status()
        if not newly_opened:
            self.call_after_refresh(self._check_external_in_background)

    def _continue_session_orphan_scan(self) -> None:
        if self._scan_orphans_after_session_open:
            self._scan_orphans_after_session_open = False
            self.call_after_refresh(self._scan_orphan_recoveries)

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
        self._queue_recovery_save(document)

    def _queue_recovery_save(self, document: Document) -> None:
        document.recovery_saved = False
        self._recovery_mutation_sequence += 1
        mutation = _RecoveryMutation(
            sequence=self._recovery_mutation_sequence,
            kind=_RecoveryMutationKind.SAVE,
            path=document.path,
            document=document,
            text=document.text,
            encoding=document.encoding,
            base_snapshot=document.recovery_base_snapshot or document.snapshot,
        )
        for index in range(len(self._recovery_mutation_queue) - 1, -1, -1):
            queued = self._recovery_mutation_queue[index]
            if queued.path != mutation.path:
                continue
            if queued.kind is _RecoveryMutationKind.DELETE:
                break
            self._recovery_mutation_queue[index] = mutation
            self._start_recovery_mutation()
            return
        self._recovery_mutation_queue.append(mutation)
        self._start_recovery_mutation()

    def _clear_recovery(
        self,
        path: Path,
        *,
        after: Callable[[], None] | None = None,
    ) -> None:
        document = self.document
        if document is not None and document.path == path:
            self._recovery_revision += 1
            if self._recovery_timer is not None:
                self._recovery_timer.stop()
                self._recovery_timer = None
            document.recovery_saved = False
        if after is not None:
            self._recovery_delete_waiters.setdefault(path, []).append(after)
        self._recovery_mutation_queue = [
            mutation
            for mutation in self._recovery_mutation_queue
            if mutation.path != path or mutation.kind is not _RecoveryMutationKind.SAVE
        ]
        if (
            self._recovery_mutation_in_flight is not None
            and self._recovery_mutation_in_flight.path == path
            and self._recovery_mutation_in_flight.kind is _RecoveryMutationKind.DELETE
        ) or any(
            mutation.path == path and mutation.kind is _RecoveryMutationKind.DELETE
            for mutation in self._recovery_mutation_queue
        ):
            return
        self._recovery_mutation_sequence += 1
        self._recovery_mutation_queue.append(
            _RecoveryMutation(
                sequence=self._recovery_mutation_sequence,
                kind=_RecoveryMutationKind.DELETE,
                path=path,
            )
        )
        self._start_recovery_mutation()

    def _start_recovery_mutation(self) -> None:
        if self._recovery_mutation_in_flight is not None or not self._recovery_mutation_queue:
            return
        mutation = self._recovery_mutation_queue.pop(0)
        if mutation.kind is _RecoveryMutationKind.DELETE:
            mutation = replace(
                mutation,
                fingerprint=self._known_recovery_fingerprints.get(mutation.path),
            )
        self._recovery_mutation_in_flight = mutation
        self._recovery_mutation_worker(mutation)

    @work(group="recovery-mutation", thread=True, exit_on_error=False)
    def _recovery_mutation_worker(self, mutation: _RecoveryMutation) -> None:
        try:
            if mutation.kind is _RecoveryMutationKind.SAVE:
                assert mutation.text is not None
                assert mutation.encoding is not None
                assert mutation.base_snapshot is not None
                record = self.recovery_journal.publish(
                    document_path=mutation.path,
                    workspace_root=self.workspace.root,
                    text=mutation.text,
                    encoding=mutation.encoding,
                    base_snapshot=mutation.base_snapshot,
                )
            else:
                self.recovery_journal.delete_expected(
                    mutation.path,
                    fingerprint=mutation.fingerprint,
                )
                record = None
            result = _RecoveryMutationResult(record=record)
        except Exception as error:
            result = _RecoveryMutationResult(error=str(error))
        self.call_from_thread(self._handle_recovery_mutation_result, mutation, result)

    def _handle_recovery_mutation_result(
        self,
        mutation: _RecoveryMutation,
        result: _RecoveryMutationResult,
    ) -> None:
        if self._recovery_mutation_in_flight != mutation:
            return
        self._recovery_mutation_in_flight = None
        if mutation.kind is _RecoveryMutationKind.SAVE:
            document = mutation.document
            if result.record is not None:
                self._known_recovery_fingerprints[mutation.path] = result.record.fingerprint
            if document is not None:
                document.recovery_saved = bool(
                    result.record is not None
                    and document.path == mutation.path
                    and document.text == mutation.text
                    and document.encoding == mutation.encoding
                    and (document.recovery_base_snapshot or document.snapshot)
                    == mutation.base_snapshot
                    and document.dirty
                )
            if result.error is not None:
                self.notify(
                    escape(result.error),
                    severity="error",
                    title="Recovery draft failed",
                )
        else:
            if result.error is None:
                self._known_recovery_fingerprints.pop(mutation.path, None)
            else:
                self.notify(
                    escape(result.error),
                    severity="warning",
                    title="Recovery cleanup failed",
                )
            waiters = self._recovery_delete_waiters.pop(mutation.path, [])
            for continuation in waiters:
                continuation()
        self._start_recovery_mutation()
        self._refresh_status()
        self._maybe_finish_exit()

    async def _render_preview(self, revision: int) -> None:
        if revision != self._preview_revision or self.document is None:
            return
        self._preview_heading_announcement = None
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
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        self._sync_editor_state()
        document = self.document
        if document is None:
            self.notify("No Markdown file is open", severity="warning")
            return

        try:
            safe_path = self.workspace.validate_document_path(document.path)
        except WorkspaceError as error:
            self._show_conflict(
                ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error)),
                after=after,
            )
            return
        if safe_path != document.path:
            document.path = safe_path
            self._document_generation += 1

        self._save_continuation = after
        ticket = self._document_ticket(document)
        if document.recovery_conflict:
            purpose = _ProbePurpose.SAVE_RECOVERY
        elif not document.dirty:
            purpose = _ProbePurpose.SAVE_CLEAN
        else:
            if self._begin_critical_io(document, freeze_editor=True, status="Saving…"):
                self._save_document_worker(ticket, document.text, document.encoding)
            return
        if self._begin_critical_io(document, freeze_editor=False, status="Checking…"):
            self._probe_document_worker(ticket, purpose)

    @work(group="document-probe", exclusive=True, thread=True, exit_on_error=False)
    def _probe_document_worker(
        self,
        ticket: _DocumentTicket,
        purpose: _ProbePurpose,
    ) -> None:
        worker = get_current_worker()
        try:
            probe = probe_file(ticket.path)
        except Exception as error:
            probe = DiskProbe(ticket.path, None, str(error))
        if not worker.is_cancelled:
            self.call_from_thread(self._handle_probe_result, ticket, purpose, probe)

    def _handle_probe_result(
        self,
        ticket: _DocumentTicket,
        purpose: _ProbePurpose,
        probe: DiskProbe,
    ) -> None:
        if not self._ticket_is_current(ticket):
            if purpose is not _ProbePurpose.WATCH and self._critical_io:
                self._finish_critical_io()
                self._cancel_pending_transition()
                self.notify(
                    "Ignored a stale file check; the document state changed",
                    severity="warning",
                )
            return

        document = ticket.document
        self._sync_editor_state()
        if purpose is _ProbePurpose.WATCH:
            if (
                self._critical_io
                or self._has_modal
                or self._pending_transition is not None
                or self._save_continuation is not None
            ):
                return
            change = classify_external_change(
                ticket.snapshot,
                dirty=document.dirty,
                probe=probe,
            )
            if change.kind is ExternalChangeKind.UNCHANGED and change.snapshot is not None:
                self._accept_unchanged_snapshot(document, change.snapshot)
            elif change.kind is ExternalChangeKind.MODIFIED:
                self._reload_current_from_disk(automatic=True)
            else:
                self._mark_external_warning(change)
            self._refresh_status()
            return

        change = classify_external_change(
            ticket.snapshot,
            dirty=document.dirty,
            probe=probe,
        )

        if purpose is _ProbePurpose.TRANSITION:
            if document.dirty:
                continuation = self._pending_transition
                self._pending_transition = None
                self._finish_critical_io()
                if continuation is not None:
                    self._request_transition(continuation)
                return
            if change.kind is ExternalChangeKind.UNCHANGED and change.snapshot is not None:
                self._accept_unchanged_snapshot(document, change.snapshot)
            if change.kind in {ExternalChangeKind.DELETED, ExternalChangeKind.INACCESSIBLE}:
                self._finish_critical_io()
                self._show_conflict(change, after=self._complete_pending_transition)
                return
            self._finish_critical_io()
            self._complete_pending_transition()
            return

        if purpose is _ProbePurpose.CURRENT_RESULT:
            location = self._pending_focus_location
            self._pending_focus_location = None
            if change.kind is ExternalChangeKind.UNCHANGED and change.snapshot is not None:
                self._accept_unchanged_snapshot(document, change.snapshot)
                self._finish_critical_io()
            elif change.kind is ExternalChangeKind.MODIFIED:
                if location is not None:
                    self._save_continuation = lambda: self._focus_editor_at(*location)
                self._finish_critical_io()
                self._reload_current_from_disk(automatic=True, failure_dialog=True)
                return
            else:
                self._finish_critical_io()
                self._mark_external_warning(change)
            if location is not None:
                self._focus_editor_at(*location)
            return

        continuation = self._save_continuation
        if purpose is _ProbePurpose.SAVE_CLEAN and document.dirty:
            self._save_continuation = None
            self._finish_critical_io()
            self._save_current(after=continuation)
            return

        if purpose is _ProbePurpose.SAVE_RECOVERY:
            if change.kind is not ExternalChangeKind.INACCESSIBLE:
                change = ExternalChange(ExternalChangeKind.CONFLICT, change.snapshot)
            self._finish_critical_io()
            self._show_conflict(change, after=continuation)
            return

        if change.kind is ExternalChangeKind.UNCHANGED and change.snapshot is not None:
            self._accept_unchanged_snapshot(document, change.snapshot)
            document.last_save_status = "No changes"
            self._save_continuation = None
            self._finish_critical_io(restore_status=False)
            self._clear_recovery(document.path, after=continuation)
            return
        if change.kind is ExternalChangeKind.MODIFIED:
            self._finish_critical_io()
            self._reload_current_from_disk(automatic=True, failure_dialog=True)
            return
        self._finish_critical_io()
        self._show_conflict(change, after=continuation)

    @work(group="document-save", exclusive=True, thread=True, exit_on_error=False)
    def _save_document_worker(
        self,
        ticket: _DocumentTicket,
        text: str,
        encoding: str,
    ) -> None:
        worker = get_current_worker()
        try:
            saved = atomic_save(
                ticket.path,
                text,
                encoding=encoding,
                expected=ticket.snapshot,
            )
            outcome = _SaveWorkerResult(saved=saved)
        except ExternalModificationError as error:
            outcome = _SaveWorkerResult(external_snapshot=error.current)
        except (OSError, PersistenceError) as error:
            try:
                inaccessible = probe_file(ticket.path).snapshot is None
            except Exception:
                inaccessible = False
            outcome = _SaveWorkerResult(error=str(error), inaccessible=inaccessible)
        except Exception as error:
            outcome = _SaveWorkerResult(error=f"Unexpected save failure: {error}")
        if not worker.is_cancelled:
            self.call_from_thread(self._handle_save_worker_result, ticket, outcome)

    def _handle_save_worker_result(
        self,
        ticket: _DocumentTicket,
        outcome: _SaveWorkerResult,
    ) -> None:
        continuation = self._save_continuation
        if not self._ticket_is_current(ticket):
            self._finish_critical_io()
            self._cancel_pending_transition()
            self.notify(
                "Save finished, but its document baseline changed; verify the disk file",
                severity="error",
                title="Save state uncertain",
            )
            return

        document = ticket.document
        if outcome.external_snapshot is not None:
            change = classify_external_change(
                ticket.snapshot,
                dirty=document.dirty,
                probe=DiskProbe(ticket.path, outcome.external_snapshot),
            )
            self._finish_critical_io()
            self._show_conflict(change, after=continuation)
            return
        if outcome.inaccessible:
            self._finish_critical_io()
            self._show_conflict(
                ExternalChange(
                    ExternalChangeKind.INACCESSIBLE,
                    None,
                    outcome.error,
                ),
                after=continuation,
            )
            return
        if outcome.error is not None or outcome.saved is None:
            document.last_save_status = "Save failed"
            self._finish_critical_io(restore_status=False)
            self.notify(
                escape(outcome.error or "The save did not return a result"),
                severity="error",
                title="Save failed",
            )
            self._cancel_pending_transition()
            return

        timestamp = datetime.now().astimezone().strftime("Saved %H:%M:%S")
        document.mark_saved(outcome.saved.snapshot, timestamp)
        self._document_generation += 1
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self._save_continuation = None
        self._finish_critical_io(restore_status=False)
        if outcome.saved.warning:
            self.notify(outcome.saved.warning, severity="warning")
        else:
            self.notify(f"Saved {escape(document.path.name)}")
        self._clear_recovery(document.path, after=continuation)

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
        if self._critical_io:
            return
        document = self.document
        if document is None:
            event.dialog.dismiss(False)
            self._cancel_pending_transition()
            return
        if not event.value:
            event.dialog.show_error("Enter a Markdown filename.")
            return
        ticket = self._document_ticket(document)
        if self._begin_critical_io(document, freeze_editor=True, status="Saving copy…"):
            event.dialog.set_busy(True)
            self._save_as_worker(
                ticket,
                event.dialog,
                Path(event.value),
                document.text,
                document.encoding,
            )

    @work(group="save-as", exclusive=True, thread=True, exit_on_error=False)
    def _save_as_worker(
        self,
        ticket: _DocumentTicket,
        dialog: SaveAsDialog,
        requested_path: Path,
        text: str,
        encoding: str,
    ) -> None:
        worker = get_current_worker()
        try:
            target = self.workspace.validate_document_path(requested_path, must_exist=False)
            expected_target = snapshot_file(target)
            target = self.workspace.validate_document_path(target, must_exist=False)
            if expected_target.exists or target.exists() or target.is_symlink():
                outcome = _SaveAsWorkerResult(
                    error="That path already exists; choose a new filename."
                )
            else:
                saved = atomic_save(
                    target,
                    text,
                    encoding=encoding,
                    expected=expected_target,
                )
                outcome = _SaveAsWorkerResult(target=target, saved=saved)
        except (OSError, PersistenceError, WorkspaceError) as error:
            outcome = _SaveAsWorkerResult(error=str(error))
        except Exception as error:
            outcome = _SaveAsWorkerResult(error=f"Unexpected Save As failure: {error}")
        if not worker.is_cancelled:
            self.call_from_thread(self._handle_save_as_worker_result, ticket, dialog, outcome)

    def _handle_save_as_worker_result(
        self,
        ticket: _DocumentTicket,
        dialog: SaveAsDialog,
        outcome: _SaveAsWorkerResult,
    ) -> None:
        if not self._ticket_is_current(ticket):
            self._finish_critical_io()
            dialog.show_error("The active document changed before Save As completed.")
            return
        if outcome.error is not None or outcome.target is None or outcome.saved is None:
            self._finish_critical_io()
            dialog.show_error(outcome.error or "Save As did not return a result.")
            return

        document = ticket.document
        previous_path = document.path
        previous_view = self._session_views.pop(previous_path, None)
        self._recent_paths = [path for path in self._recent_paths if path != previous_path]
        self._clear_recovery(
            previous_path,
            after=lambda: self._finish_save_as_recovery_cleanup(dialog),
        )
        document.retarget(outcome.target)
        if previous_view is not None:
            self._session_views[outcome.target] = DocumentViewState(
                outcome.target,
                line=previous_view.line,
                column=previous_view.column,
                scroll_x=previous_view.scroll_x,
                scroll_y=previous_view.scroll_y,
            )
        timestamp = datetime.now().astimezone().strftime("Saved %H:%M:%S")
        document.mark_saved(outcome.saved.snapshot, timestamp)
        self._document_generation += 1
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self.explorer.set_active(outcome.target)
        self._refresh_workspace_index()
        self.explorer.directory_tree.reload()
        self._finish_critical_io(restore_status=False)
        self._persist_session()
        if outcome.saved.warning:
            self.notify(outcome.saved.warning, severity="warning")
        else:
            self.notify(f"Saved local version as {escape(outcome.target.name)}")

    def _finish_save_as_recovery_cleanup(self, dialog: SaveAsDialog) -> None:
        dialog.set_busy(False)
        dialog.dismiss(True)

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
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        document = self.document
        if document is None:
            self._cancel_pending_transition()
            return
        try:
            safe_path = self.workspace.validate_document_path(document.path)
        except WorkspaceError as error:
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

        if safe_path != document.path:
            document.path = safe_path
            self._document_generation += 1
        ticket = self._document_ticket(document)
        purpose = _LoadPurpose.RELOAD_AUTOMATIC if automatic else _LoadPurpose.RELOAD_MANUAL
        if self._begin_critical_io(document, freeze_editor=True, status="Reloading…"):
            self._load_document_worker(ticket, safe_path, purpose, failure_dialog)

    @work(group="document-load", exclusive=True, thread=True, exit_on_error=False)
    def _load_document_worker(
        self,
        ticket: _DocumentTicket | None,
        path: Path,
        purpose: _LoadPurpose,
        failure_dialog: bool,
    ) -> None:
        worker = get_current_worker()
        try:
            loaded = load_file(path)
            error_message = None
        except (OSError, PersistenceError) as error:
            loaded = None
            error_message = str(error)
        except Exception as error:
            loaded = None
            error_message = f"Unexpected load failure: {error}"
        if not worker.is_cancelled:
            self.call_from_thread(
                self._handle_load_worker_result,
                ticket,
                path,
                purpose,
                failure_dialog,
                loaded,
                error_message,
            )

    def _handle_load_worker_result(
        self,
        ticket: _DocumentTicket | None,
        path: Path,
        purpose: _LoadPurpose,
        failure_dialog: bool,
        loaded: LoadedFile | None,
        error_message: str | None,
    ) -> None:
        if purpose is _LoadPurpose.OPEN:
            self._finish_critical_io()
            if loaded is None:
                self.notify(
                    escape(error_message or "The file could not be loaded"),
                    severity="error",
                    title="Cannot open file",
                )
                self._cancel_pending_open()
                return
            self._finish_open_loaded(path, loaded)
            return

        if ticket is None or not self._ticket_is_current(ticket):
            self._finish_critical_io()
            self._cancel_pending_transition()
            self.notify(
                "Ignored a stale reload because the document baseline changed",
                severity="warning",
            )
            return

        automatic = purpose is _LoadPurpose.RELOAD_AUTOMATIC
        if loaded is None:
            self._finish_critical_io()
            error = ExternalChange(
                ExternalChangeKind.INACCESSIBLE,
                None,
                error_message or "The file could not be loaded",
            )
            if failure_dialog:
                continuation = self._save_continuation
                self._save_continuation = None
                self._show_conflict(error, after=continuation)
            elif automatic:
                self._mark_external_warning(error)
            else:
                self.notify(
                    escape(error.detail or "Reload failed"),
                    severity="error",
                    title="Reload failed",
                )
                self._cancel_pending_transition()
            return

        self._finish_critical_io()
        self._finish_reload_loaded(ticket.document, loaded, automatic=automatic)

    def _finish_reload_loaded(
        self,
        document: Document,
        loaded: LoadedFile,
        *,
        automatic: bool,
    ) -> None:

        previous_path = document.path
        document.replace_from_disk(loaded.text, loaded.snapshot, loaded.encoding)
        self._document_generation += 1
        with self.editor.prevent(TextArea.Changed):
            self.editor.load_text(document.text)
        self._editor_baseline_text = self.editor.text
        self._editor_baseline_source_text = document.text
        self._schedule_preview(immediate=True)
        if automatic:
            document.last_save_status = "Reloaded externally"
        self._refresh_status()
        if automatic:
            self.notify(f"Reloaded externally changed {escape(document.path.name)}")
        continuation = self._save_continuation
        self._save_continuation = None
        self._clear_recovery(
            previous_path,
            after=lambda: self._finish_reload_recovery_cleanup(document, continuation),
        )

    def _finish_reload_recovery_cleanup(
        self,
        document: Document,
        continuation: Callable[[], None] | None,
    ) -> None:
        if document.has_mixed_line_endings:
            document.read_only = True
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
        document.read_only = False
        self.editor.read_only = False
        if continuation is not None:
            continuation()

    def _handle_reloaded_mixed_line_ending_decision(self, accepted: bool | None) -> None:
        document = self.document
        if document is not None and accepted:
            document.read_only = False
            self.editor.read_only = False
            self.notify(
                f"Edits to {escape(document.path.name)} will normalize line endings to "
                f"{document.line_ending_label.removeprefix('MIXED→')}",
                severity="warning",
            )
        elif document is not None:
            document.read_only = True
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
            or self._critical_io
            or self._has_modal
            or self._pending_transition is not None
            or self._save_continuation is not None
            or (self._watch_probe_worker is not None and self._watch_probe_worker.is_running)
            or not self.query(MarkdownEditor)
        ):
            return
        self._sync_editor_state()
        try:
            safe_path = self.workspace.validate_document_path(document.path)
        except WorkspaceNotFoundError:
            safe_path = document.path
        except WorkspaceError as error:
            self._mark_external_warning(
                ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error))
            )
            return
        if safe_path != document.path:
            document.path = safe_path
            self._document_generation += 1
        self._watch_probe_worker = self._probe_document_worker(
            self._document_ticket(document),
            _ProbePurpose.WATCH,
        )

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
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        self._sync_editor_state()
        self._quit_documents = [opened.document for opened in self._open_documents]
        self._continue_quit()

    def _continue_quit(self) -> None:
        if self._quit_documents is None:
            return
        if not self._quit_documents:
            self._quit_documents = None
            self._exit_with_session()
            return
        document = self._quit_documents.pop(0)
        if document is not self.document:
            self._activate_document(document)
        self._request_transition(self._continue_quit)

    def _exit_with_session(self) -> None:
        self._sync_editor_state()
        if self.document is not None:
            self.editor.read_only = True
        self._exit_requested = True
        self._persist_session()
        self._maybe_finish_exit()

    def _maybe_finish_exit(self) -> None:
        if (
            self._exit_requested
            and not self._session_save_in_flight
            and self._pending_session_state is None
            and self._recovery_mutation_in_flight is None
            and not self._recovery_mutation_queue
        ):
            self.exit()

    async def action_quit(self) -> None:
        """Defensively route Textual's inherited quit action through the safety gate."""
        self.action_request_quit()

    def action_next_tab(self) -> None:
        self._cycle_tab(1)

    def action_previous_tab(self) -> None:
        self._cycle_tab(-1)

    def _cycle_tab(self, offset: int) -> None:
        if self._has_modal or self._critical_io or len(self._open_documents) < 2:
            return
        active_index = next(
            (
                index
                for index, opened in enumerate(self._open_documents)
                if opened.document is self.document
            ),
            0,
        )
        target = self._open_documents[(active_index + offset) % len(self._open_documents)]
        self._activate_document(target.document)

    def action_close_tab(self) -> None:
        if self._has_modal or self._critical_io or self.document is None:
            return
        document = self.document
        self._request_transition(lambda: self._close_document_now(document))

    def _close_document_now(self, document: Document) -> None:
        opened = self._open_entry_for_document(document)
        if opened is None:
            return
        index = self._open_documents.index(opened)
        self._open_documents.remove(opened)
        self.document_tabs.remove_tab(opened.tab_id)
        if self._open_documents:
            target = self._open_documents[min(index, len(self._open_documents) - 1)]
            self._activate_document(target.document, flush_previous_recovery=False)
        else:
            self._clear_active_document(flush_recovery=False)
        self.call_after_refresh(self._refresh_document_tabs)

    def _clear_active_document(self, *, flush_recovery: bool = True) -> None:
        document = self.document
        if document is not None:
            self._sync_editor_state()
            self._remember_document_view(document)
            if flush_recovery:
                self._flush_recovery_before_switch(document)
        self.document = None
        self._document_generation += 1
        self._preview_revision += 1
        if self._preview_timer is not None:
            self._preview_timer.stop()
            self._preview_timer = None
        with self.editor.prevent(TextArea.Changed):
            self.editor.load_text("")
        self._editor_baseline_text = ""
        self._editor_baseline_source_text = ""
        self.editor.read_only = True
        self.explorer.set_active(None)
        self.run_worker(
            self.preview.render_source("Select a Markdown file to begin."),
            group="empty-preview",
            exclusive=True,
            exit_on_error=False,
        )
        self._persist_session()
        self.explorer.directory_tree.focus()
        self._refresh_status()

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
        if self.preview.display:
            self.preview.focus()
        elif self.document is not None:
            self.editor.focus()

    def action_find_file(self) -> None:
        if self._has_modal:
            return
        self._refresh_workspace_index()
        self.push_screen(
            FileSearchDialog(self.workspace_files, self.workspace.root),
            self._handle_search_result,
        )

    def action_open_recent(self) -> None:
        if self._has_modal:
            return
        if self._critical_io:
            self.notify("Wait for the current file operation to finish", severity="warning")
            return
        self._sync_editor_state()
        paths = self._recent_document_paths(prune_missing=True)
        if not paths:
            self.notify("No recent Markdown documents are available", severity="warning")
            return
        active_path = None if self.document is None else self.document.path
        self.push_screen(
            RecentDocumentsDialog(paths, self.workspace.root, active_path),
            self._handle_recent_document,
        )

    def _handle_recent_document(self, path: Path | None) -> None:
        if path is not None:
            self._request_open(path)

    def _handle_search_result(self, path: Path | None) -> None:
        if path is not None:
            self._request_open(path)

    def action_search_text(self) -> None:
        if self._has_modal:
            return
        self._sync_editor_state()
        overrides = tuple(
            TextSearchOverride(
                opened.document.path,
                opened.document.text,
                prefer_disk=not (opened.document.dirty or opened.document.conflict),
            )
            for opened in self._open_documents
        )
        self.push_screen(
            TextSearchDialog(
                self.workspace,
                overrides=overrides,
            ),
            self._handle_text_search_result,
        )

    def action_manage_recovery(self) -> None:
        if self._has_modal or self._critical_io:
            return
        self._sync_editor_state()
        self._load_recovery_inventory_worker()

    def _handle_text_search_result(self, result: TextSearchMatch | None) -> None:
        if result is not None:
            self._request_open_at(result.path, result.line, result.column)

    def action_editor_undo(self) -> None:
        if not self._has_modal and self.document is not None and not self.editor.read_only:
            self.editor.undo()

    def action_editor_redo(self) -> None:
        if not self._has_modal and self.document is not None and not self.editor.read_only:
            self.editor.redo()

    def action_show_help(self) -> None:
        if not self._has_modal:
            self.push_screen(
                HelpDialog(
                    format_shortcut_help(
                        self.config.keybindings,
                        auto_continue_lists=self.config.editor.auto_continue_lists,
                    )
                )
            )

    def action_show_markdown_help(self) -> None:
        if not self._has_modal:
            self.push_screen(HelpDialog(MARKDOWN_SYNTAX_HELP, title="Markdown syntax"))

    def action_reload_config(self) -> None:
        if self._has_modal:
            return
        if self._config_root is None:
            self.notify(
                "No user configuration was loaded for this app instance", severity="warning"
            )
            return
        try:
            config = load_config(self._config_root)
        except ConfigError as error:
            self.notify(escape(str(error)), severity="error", title="Configuration not reloaded")
            return

        self.config = config
        self.set_keymap(dict(config.keybindings))
        self.editor.auto_continue_lists = config.editor.auto_continue_lists
        self.editor.soft_wrap = config.editor.soft_wrap
        self.editor.show_line_numbers = config.editor.show_line_numbers
        self._refresh_status()
        self.notify("Reloaded config.toml; existing theme.tcss files reload when saved")

    def get_system_commands(self, screen: Screen[object]) -> Iterable[SystemCommand]:
        """Expose only TermWriter actions in the searchable command palette."""
        del screen
        yield SystemCommand("Save document", "Save the open Markdown source", self.action_save)
        yield SystemCommand(
            "Find file", "Search Markdown paths in the workspace", self.action_find_file
        )
        yield SystemCommand(
            "Open recent document",
            "Switch to a recently used Markdown document",
            self.action_open_recent,
        )
        yield SystemCommand(
            "Next document tab",
            "Activate the next open Markdown buffer",
            self.action_next_tab,
        )
        yield SystemCommand(
            "Previous document tab",
            "Activate the previous open Markdown buffer",
            self.action_previous_tab,
        )
        yield SystemCommand(
            "Close document tab",
            "Close the active buffer with save protection",
            self.action_close_tab,
        )
        yield SystemCommand(
            "Search workspace text",
            "Find literal, fuzzy, whole-word, or regex matches in Markdown source",
            self.action_search_text,
        )
        yield SystemCommand(
            "Toggle file explorer",
            "Show or hide the workspace tree",
            self.action_toggle_explorer,
        )
        yield SystemCommand(
            "Toggle preview",
            "Show, hide, or switch to the rendered preview",
            self.action_toggle_preview,
        )
        yield SystemCommand("Undo", "Undo the last editor change", self.action_editor_undo)
        yield SystemCommand("Redo", "Redo the last undone editor change", self.action_editor_redo)
        yield SystemCommand(
            "Reload configuration",
            "Reload config.toml keybindings, editor, and retention options",
            self.action_reload_config,
        )
        yield SystemCommand(
            "Manage recovery drafts",
            "Restore, retarget, export, archive, or clean recovery entries",
            self.action_manage_recovery,
        )
        yield SystemCommand(
            "Shortcut help", "Show the effective keybindings", self.action_show_help
        )
        yield SystemCommand(
            "Markdown syntax help",
            "Show supported Markdown and nesting examples",
            self.action_show_markdown_help,
        )
        yield SystemCommand(
            "Quit safely", "Prompt before discarding changes", self.action_request_quit
        )

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
        if self.query("#document-tabs"):
            self._refresh_document_tabs()
        status_bars = self.query(TermWriterStatusBar)
        if not status_bars:
            return
        status_bars.first(TermWriterStatusBar).show_document(
            self.document,
            root=self.workspace.root,
            mode=self._focus_mode(),
            announcement=self._preview_heading_announcement,
        )
