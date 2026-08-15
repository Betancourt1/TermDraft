//! Ratatui event/update coordinator.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, IsTerminal as _, stdout};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
#[cfg(unix)]
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use tui_textarea::{CursorMove, TextArea};
use unicode_width::UnicodeWidthStr;

use crate::agent_bridge::{
    AgentAction, AgentCall, AgentResponse, AgentSession, AgentWorkspaceDocument,
    MAX_AGENT_RESPONSE_BYTES, MAX_AGENT_SOURCE_BYTES, ProposalChange, RangeEdit, json_size,
};
use crate::bindings::{Action as BindingAction, BindingScope};
use crate::checkpoint::{
    CaptureOutcome, Checkpoint, CheckpointError, CheckpointList, CheckpointReason, CheckpointStore,
    CheckpointWarning,
};
use crate::config::{self, Config, EditorConfig, StartupMode, StartupView};
use crate::continuation::{EnterAction, action_for};
use crate::coordinate_diagnostic::{CoordinateDiagnostic, diagnose_coordinate};
use crate::debug_trace::DebugTrace;
use crate::document::{
    Document, Encoding, LineEnding, MixedSource, RevisionMismatch, SourceRevision,
};
use crate::editor::{
    apply_editor_config, cursor_at_screen_position, inline_preview_editor, source_from_textarea,
    style_cursor, sync_inline_preview_cursor, textarea_from_source,
};
use crate::markdown::{
    PreviewLink, PreviewLinkTarget, RenderedMarkdown, render_markdown_document,
    render_markdown_with_source_lines,
};
use crate::path_filter::parse_path_filter;
use crate::persistence::{
    LoadedFile, SaveError, load_file, load_file_bounded, normalize_line_endings, save_atomic,
};
use time::{Duration as TimeDuration, OffsetDateTime};

#[cfg(test)]
use crate::recovery::RecoveryRecordStatus;
use crate::recovery::{RecoveryEntry, RecoveryJournal, RecoveryRecord};
use crate::search::{
    DEFAULT_FILE_RESULT_LIMIT, DocumentSearchMatch, MatchDirection, TextMatch, TextSearchMode,
    TextSearchOptions, TextSearchOverride, TextSearchRequest, cycle_document_match_index,
    find_document_matches, fuzzy_score, heading_outline, initial_document_match_index,
    location_to_offset, offset_to_location, replace_document_matches, search_files_with_filter,
    search_workspace_text,
};
use crate::semantic_blocks::{SemanticBlockMap, map_semantic_blocks};
use crate::session::{DocumentViewState, MAX_SESSION_DOCUMENTS, SessionState, SessionStore};
use crate::theme::Theme;
use crate::ui;
use crate::workspace::{
    IGNORED_DIRECTORIES, Workspace, WorkspaceEntry, WorkspaceScan, has_editable_suffix,
    paths_are_spelling_aliases,
};
use crate::workspace_entries::{
    copy_entry, create_file, create_folder, move_entry, move_to_trash, rename_entry,
};

pub const EXPLORER_DEFAULT_WIDTH: u16 = 34;
pub const EXPLORER_MIN_WIDTH: u16 = 20;
pub const EXPLORER_MAX_WIDTH: u16 = 48;
const HISTORY_DIFF_MAX_BYTES: usize = 512 * 1024;
const HISTORY_DIFF_MAX_SOURCE_LINES: usize = 2_000;
const HISTORY_DIFF_MAX_OUTPUT_LINES: usize = 400;
static NEXT_AGENT_PROPOSAL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Command,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorClipboardAction {
    Copy,
    Cut,
    Paste,
}

impl Mode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Command => "COMMAND",
            Self::Write => "WRITE",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMode {
    Inline,
    Split,
}

impl ViewMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inline => "HYBRID",
            Self::Split => "SPLIT",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Explorer,
    Editor,
    Preview,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UiRegions {
    pub tabs: Option<Rect>,
    pub overlay: Option<Rect>,
    pub workspace: Rect,
    pub explorer: Option<Rect>,
    pub explorer_list: Option<Rect>,
    pub explorer_divider: Option<Rect>,
    pub workbench: Rect,
    pub editor: Option<Rect>,
    pub preview: Option<Rect>,
    pub workbench_divider: Option<Rect>,
}

impl UiRegions {
    fn contains(area: Option<Rect>, column: u16, row: u16) -> bool {
        area.is_some_and(|area| {
            column >= area.x
                && column < area.x.saturating_add(area.width)
                && row >= area.y
                && row < area.y.saturating_add(area.height)
        })
    }

    fn contains_resize_handle(area: Option<Rect>, column: u16, row: u16) -> bool {
        area.is_some_and(|area| {
            let hit_area = Rect::new(
                area.x.saturating_sub(1),
                area.y,
                area.width.saturating_add(2),
                area.height,
            );
            Self::contains(Some(hit_area), column, row)
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MouseDragTarget {
    Explorer,
    Workbench,
    EditorSelection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmAction {
    Quit,
    CloseTab,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixedLineEndingContext {
    Open,
    Reload,
    Recovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictKind {
    Changed,
    Missing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RecoveryManagerFocus {
    #[default]
    Records,
    Target,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HistoryFocus {
    #[default]
    Checkpoints,
    Diff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryDiffKind {
    Context,
    Added,
    Removed,
    Notice,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryDiffLine {
    pub kind: HistoryDiffKind,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct AgentProposal {
    pub id: String,
    pub path: PathBuf,
    pub expected_revision: SourceRevision,
    pub origin: String,
    pub proposed_source: String,
    pub diff: Vec<HistoryDiffLine>,
    pub diff_truncated: bool,
}

#[derive(Clone, Debug)]
struct AgentOpenDocument {
    path: PathBuf,
    relative: PathBuf,
    source: String,
    revision: String,
    dirty: bool,
    conflict: bool,
}

#[derive(serde::Serialize)]
struct AgentWorkspaceDocumentRef<'a> {
    path: &'a str,
    source: &'a str,
    revision: &'a str,
    dirty: bool,
    open: bool,
    conflict: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryClearScope {
    Document,
    Workspace,
}

#[derive(Clone, Debug)]
struct PendingTransition {
    action: ConfirmAction,
    accepted_paths: Vec<PathBuf>,
    remaining_tabs: Vec<usize>,
    original_active_tab: Option<usize>,
}

enum DiskState {
    Current(LoadedFile),
    Missing,
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceTransactionError {
    Protected,
    Stale(RevisionMismatch),
}

#[derive(Clone, Debug)]
pub enum Overlay {
    Help {
        scroll: u16,
        max_scroll: u16,
    },
    MarkdownHelp {
        scroll: u16,
    },
    SemanticInspector {
        mapping: SemanticBlockMap,
        selected: usize,
    },
    SemanticReader {
        mapping: SemanticBlockMap,
        scroll: u16,
    },
    CoordinateInspector {
        diagnostic: CoordinateDiagnostic,
        screen_position: Option<(u16, u16)>,
    },
    Palette {
        input: TextInput,
        selected: usize,
    },
    FileFinder {
        query: TextInput,
        filter: TextInput,
        focus: FileFinderFocus,
        selected: usize,
        error: Option<String>,
    },
    RecentDocuments {
        paths: Vec<PathBuf>,
        selected: usize,
    },
    Find {
        query: TextInput,
        replacement: TextInput,
        case_sensitive: bool,
        focus: FindFocus,
        source: String,
        matches: Vec<DocumentSearchMatch>,
        selected: Option<usize>,
        anchor_offset: usize,
        read_only: bool,
    },
    WorkspaceSearch {
        query: TextInput,
        filter: TextInput,
        mode: TextSearchMode,
        case_sensitive: bool,
        focus: WorkspaceSearchFocus,
        results: Vec<TextMatch>,
        selected: usize,
        status: String,
    },
    SearchResults {
        results: Vec<TextMatch>,
        selected: usize,
    },
    Outline {
        items: Vec<(usize, usize, String)>,
        matches: Vec<(usize, usize, String)>,
        query: TextInput,
        selected: usize,
    },
    PathInput {
        action: PathAction,
        input: TextInput,
    },
    WorkspaceInput {
        action: WorkspaceInputAction,
        source: PathBuf,
        input: TextInput,
    },
    TrashConfirm {
        source: PathBuf,
        is_directory: bool,
    },
    Recovery {
        entry: Box<RecoveryEntry>,
    },
    RecoveryManager {
        records: Vec<RecoveryRecord>,
        selected: usize,
        focus: RecoveryManagerFocus,
        target: TextInput,
        protected_journals: Vec<PathBuf>,
        protected_documents: Vec<PathBuf>,
        retention_days: i64,
        status: String,
    },
    RecoveryDeleteConfirm {
        record: Box<RecoveryRecord>,
    },
    RecoveryCleanupConfirm {
        records: Vec<RecoveryRecord>,
        cutoff: OffsetDateTime,
        retention_days: i64,
    },
    LocalHistory {
        enabled: bool,
        path: PathBuf,
        checkpoints: Vec<Checkpoint>,
        selected: usize,
        focus: HistoryFocus,
        diff: Vec<HistoryDiffLine>,
        diff_scroll: usize,
        diff_truncated: bool,
        expected_revision: SourceRevision,
        status: String,
    },
    HistoryClearConfirm {
        scope: HistoryClearScope,
        path: PathBuf,
        checkpoint_ids: Vec<String>,
    },
    AgentSharing {
        root: PathBuf,
    },
    AgentProposal {
        proposal: AgentProposal,
        scroll: u16,
    },
    MixedLineEndings {
        tab_index: usize,
        previous_active: Option<usize>,
        context: MixedLineEndingContext,
        target: LineEnding,
    },
    Conflict {
        kind: ConflictKind,
        can_reload: bool,
        allow_continue: bool,
    },
    Confirm(ConfirmAction),
    Message(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FileFinderFocus {
    #[default]
    Query,
    Filter,
    Results,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FindFocus {
    #[default]
    Query,
    Replacement,
    Case,
    Previous,
    Next,
    Replace,
    ReplaceAll,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceSearchFocus {
    #[default]
    Query,
    Mode,
    Case,
    Filter,
    Results,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FindOperation {
    Previous,
    Next,
    Replace,
    ReplaceAll,
}

struct WorkspaceSearchCompletion {
    revision: u64,
    query: String,
    result: crate::search::TextSearchResult,
}

#[derive(Debug)]
struct WorkspaceScanCompletion {
    revision: u64,
    report: WorkspaceScan,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PreviewKey {
    path: PathBuf,
    revision: SourceRevision,
    selected_link: Option<usize>,
}

struct PreviewCache {
    key: PreviewKey,
    rendered: RenderedMarkdown,
}

struct PendingPreview {
    key: PreviewKey,
    source: String,
    ready_at: Instant,
}

struct PreviewCompletion {
    key: PreviewKey,
    rendered: RenderedMarkdown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum WorkspaceIndexState {
    #[default]
    Idle,
    Indexing,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextInput {
    pub value: String,
    pub cursor: usize,
}

impl TextInput {
    #[must_use]
    pub fn byte_cursor(&self) -> usize {
        self.value
            .char_indices()
            .nth(self.cursor)
            .map_or(self.value.len(), |(index, _)| index)
    }

    pub fn insert(&mut self, text: &str) {
        let text = text
            .chars()
            .filter(|character| !matches!(character, '\r' | '\n'))
            .collect::<String>();
        if text.is_empty() {
            return;
        }
        let byte = self.byte_cursor();
        self.value.insert_str(byte, &text);
        self.cursor += text.chars().count();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_cursor();
        self.cursor -= 1;
        let start = self.byte_cursor();
        self.value.replace_range(start..end, "");
    }

    pub fn delete(&mut self) {
        let start = self.byte_cursor();
        let Some((offset, character)) = self.value[start..].char_indices().next() else {
            return;
        };
        let end = start + offset + character.len_utf8();
        self.value.replace_range(start..end, "");
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.value.chars().count();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathAction {
    Create,
    SaveAs,
    SaveConflictAs,
    Duplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceInputAction {
    Create,
    Rename,
    Move,
}

impl WorkspaceInputAction {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Create => " Create file or folder ",
            Self::Rename => " Rename file or folder ",
            Self::Move => " Move file or folder ",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorkspaceClipboard {
    source: PathBuf,
    cut: bool,
}

impl PathAction {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Create => " Create file ",
            Self::SaveAs => " Save as ",
            Self::SaveConflictAs => " Save local version as ",
            Self::Duplicate => " Duplicate document ",
        }
    }

    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::SaveAs | Self::SaveConflictAs => "save",
            Self::Duplicate => "duplicate",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandAction {
    Save,
    SaveAs,
    Duplicate,
    Create,
    CopyEntry,
    CutEntry,
    PasteEntry,
    RenameEntry,
    MoveEntry,
    TrashEntry,
    CloseTab,
    Quit,
    FileFinder,
    RecentDocuments,
    NextTab,
    PreviousTab,
    WorkspaceSearch,
    Find,
    Outline,
    ToggleExplorer,
    TogglePreview,
    WriteMode,
    CommandMode,
    Undo,
    Redo,
    CreateCheckpoint,
    LocalHistory,
    AgentSharing,
    ReloadConfig,
    ChangeTheme,
    ManageRecovery,
    MarkdownHelp,
    InspectSemanticBlocks,
    ReadSemanticBlocks,
    InspectCursorCoordinates,
    Help,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandSpec {
    pub group: &'static str,
    pub label: &'static str,
    pub shortcut: &'static str,
    pub action: CommandAction,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        group: "DOCUMENT",
        label: "Save",
        shortcut: "w",
        action: CommandAction::Save,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Save as",
        shortcut: "W",
        action: CommandAction::SaveAs,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Duplicate",
        shortcut: "D",
        action: CommandAction::Duplicate,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Find file",
        shortcut: "f",
        action: CommandAction::FileFinder,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Recent documents",
        shortcut: "o",
        action: CommandAction::RecentDocuments,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Close tab",
        shortcut: "C",
        action: CommandAction::CloseTab,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Next tab",
        shortcut: "]",
        action: CommandAction::NextTab,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Previous tab",
        shortcut: "[",
        action: CommandAction::PreviousTab,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Search workspace",
        shortcut: "/",
        action: CommandAction::WorkspaceSearch,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Find and replace",
        shortcut: "s",
        action: CommandAction::Find,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Outline",
        shortcut: "S",
        action: CommandAction::Outline,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Explorer",
        shortcut: "e",
        action: CommandAction::ToggleExplorer,
    },
    CommandSpec {
        group: "FILES",
        label: "Create",
        shortcut: "a",
        action: CommandAction::Create,
    },
    CommandSpec {
        group: "FILES",
        label: "Copy",
        shortcut: "c",
        action: CommandAction::CopyEntry,
    },
    CommandSpec {
        group: "FILES",
        label: "Cut",
        shortcut: "x",
        action: CommandAction::CutEntry,
    },
    CommandSpec {
        group: "FILES",
        label: "Paste",
        shortcut: "p",
        action: CommandAction::PasteEntry,
    },
    CommandSpec {
        group: "FILES",
        label: "Rename",
        shortcut: "r",
        action: CommandAction::RenameEntry,
    },
    CommandSpec {
        group: "FILES",
        label: "Move",
        shortcut: "m",
        action: CommandAction::MoveEntry,
    },
    CommandSpec {
        group: "FILES",
        label: "Trash",
        shortcut: "d",
        action: CommandAction::TrashEntry,
    },
    CommandSpec {
        group: "MODE",
        label: "Write mode",
        shortcut: "i",
        action: CommandAction::WriteMode,
    },
    CommandSpec {
        group: "MODE",
        label: "Command mode",
        shortcut: "Esc",
        action: CommandAction::CommandMode,
    },
    CommandSpec {
        group: "EDIT",
        label: "Undo",
        shortcut: "u",
        action: CommandAction::Undo,
    },
    CommandSpec {
        group: "EDIT",
        label: "Redo",
        shortcut: "U",
        action: CommandAction::Redo,
    },
    CommandSpec {
        group: "EDIT",
        label: "Create checkpoint",
        shortcut: "",
        action: CommandAction::CreateCheckpoint,
    },
    CommandSpec {
        group: "EDIT",
        label: "Open Local History",
        shortcut: "",
        action: CommandAction::LocalHistory,
    },
    CommandSpec {
        group: "EDIT",
        label: "Agent sharing",
        shortcut: "",
        action: CommandAction::AgentSharing,
    },
    CommandSpec {
        group: "EDIT",
        label: "Reload config",
        shortcut: "R",
        action: CommandAction::ReloadConfig,
    },
    CommandSpec {
        group: "EDIT",
        label: "Inspect blocks",
        shortcut: "b",
        action: CommandAction::InspectSemanticBlocks,
    },
    CommandSpec {
        group: "EDIT",
        label: "Read blocks",
        shortcut: "B",
        action: CommandAction::ReadSemanticBlocks,
    },
    CommandSpec {
        group: "VIEW",
        label: "Preview",
        shortcut: "v",
        action: CommandAction::TogglePreview,
    },
    CommandSpec {
        group: "VIEW",
        label: "Change theme",
        shortcut: "t",
        action: CommandAction::ChangeTheme,
    },
    CommandSpec {
        group: "VIEW",
        label: "Recovery drafts",
        shortcut: "M",
        action: CommandAction::ManageRecovery,
    },
    CommandSpec {
        group: "VIEW",
        label: "Shortcut help",
        shortcut: "?",
        action: CommandAction::Help,
    },
    CommandSpec {
        group: "VIEW",
        label: "Markdown help",
        shortcut: "K",
        action: CommandAction::MarkdownHelp,
    },
    CommandSpec {
        group: "VIEW",
        label: "Cursor coordinates",
        shortcut: "I",
        action: CommandAction::InspectCursorCoordinates,
    },
    CommandSpec {
        group: "VIEW",
        label: "Quit",
        shortcut: "q",
        action: CommandAction::Quit,
    },
];

pub struct EditorTab {
    pub document: Document,
    pub editor: TextArea<'static>,
    pub inline_editor: TextArea<'static>,
    preview_cache: Option<PreviewCache>,
    pub(crate) editor_scroll_x: u16,
    pub(crate) editor_scroll_y: u16,
    pub(crate) preview_scroll_x: u16,
    pub(crate) preview_scroll_y: u16,
    pub(crate) pending_scroll_restore: bool,
    inline_source_lines: Vec<String>,
    inline_cursor: (usize, usize),
    pending_mixed_open: bool,
    undo_groups: Vec<usize>,
    redo_groups: Vec<usize>,
}

impl EditorTab {
    fn from_path(path: &Path, config: &EditorConfig) -> anyhow::Result<Self> {
        let loaded = load_file(path)?;
        Ok(Self::from_loaded(loaded, config))
    }

    fn from_loaded(loaded: LoadedFile, config: &EditorConfig) -> Self {
        let mut editor = textarea_from_source(&loaded.text);
        apply_editor_config(&mut editor, config);
        let inline_editor = inline_preview_editor(&editor);
        Self {
            document: loaded.into_document(),
            inline_source_lines: editor.lines().to_vec(),
            inline_cursor: editor.cursor(),
            editor,
            inline_editor,
            preview_cache: None,
            editor_scroll_x: 0,
            editor_scroll_y: 0,
            preview_scroll_x: 0,
            preview_scroll_y: 0,
            pending_scroll_restore: false,
            pending_mixed_open: false,
            undo_groups: Vec::new(),
            redo_groups: Vec::new(),
        }
    }

    fn from_unavailable_recovery(entry: &RecoveryEntry, config: &EditorConfig) -> Self {
        let line_ending = LineEnding::detect(&entry.text);
        let normalized = normalize_line_endings(&entry.text);
        let mixed_target = LineEnding::mixed_target(&entry.text);
        let mixed_source = mixed_target
            .map(|target| MixedSource::new(entry.text.clone(), normalized.clone(), target));
        let source = if mixed_source.is_some() {
            entry.text.clone()
        } else {
            normalized.clone()
        };
        let mut editor = textarea_from_source(&normalized);
        apply_editor_config(&mut editor, config);
        let inline_editor = inline_preview_editor(&editor);
        let source_revision = SourceRevision::initial(&source);
        Self {
            document: Document {
                path: entry.document_path.clone(),
                text: source.clone(),
                saved_text: source,
                encoding: entry.encoding,
                line_ending,
                mixed_source,
                snapshot: entry.baseline_snapshot(),
                conflict: true,
                recovery_conflict: true,
                source_revision,
            },
            inline_source_lines: editor.lines().to_vec(),
            inline_cursor: editor.cursor(),
            editor,
            inline_editor,
            preview_cache: None,
            editor_scroll_x: 0,
            editor_scroll_y: 0,
            preview_scroll_x: 0,
            preview_scroll_y: 0,
            pending_scroll_restore: false,
            pending_mixed_open: false,
            undo_groups: Vec::new(),
            redo_groups: Vec::new(),
        }
    }

    pub fn sync_document(&mut self) {
        self.document
            .update_from_editor(source_from_textarea(&self.editor));
    }

    pub fn refresh_inline_editor(&mut self) {
        let cursor = self.editor.cursor();
        if self.inline_source_lines.as_slice() != self.editor.lines() {
            self.inline_editor = inline_preview_editor(&self.editor);
            self.inline_source_lines = self.editor.lines().to_vec();
        } else if self.inline_cursor != cursor {
            sync_inline_preview_cursor(&mut self.inline_editor, &self.editor, self.inline_cursor.0);
        }
        self.sync_inline_selection();
        self.inline_cursor = cursor;
    }

    fn sync_inline_selection(&mut self) {
        self.inline_editor.cancel_selection();
        let Some((start, end)) = self.editor.selection_range() else {
            return;
        };
        let cursor = self.editor.cursor();
        let anchor = if cursor == start { end } else { start };
        self.inline_editor.move_cursor(CursorMove::Jump(
            u16::try_from(anchor.0).unwrap_or(u16::MAX),
            u16::try_from(anchor.1).unwrap_or(u16::MAX),
        ));
        self.inline_editor.start_selection();
        self.inline_editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
    }

    fn scroll_inline_editor(&mut self, mouse: MouseEvent) {
        self.refresh_inline_editor();
        self.inline_editor.input(mouse);
        let cursor = self.inline_editor.cursor();
        self.editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
    }

    fn place_cursor(&mut self, area: Rect, column: u16, row: u16, inline: bool) {
        let cursor = if inline {
            self.refresh_inline_editor();
            cursor_at_screen_position(&self.inline_editor, area, column, row)
        } else {
            cursor_at_screen_position(&self.editor, area, column, row)
        };
        let Some((cursor_row, cursor_column)) = cursor else {
            return;
        };
        let source_column = self.editor.lines()[cursor_row]
            .chars()
            .count()
            .min(cursor_column);
        self.editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor_row).unwrap_or(u16::MAX),
            u16::try_from(source_column).unwrap_or(u16::MAX),
        ));
        if inline {
            self.refresh_inline_editor();
        }
    }

    fn place_cursor_from_preview(&mut self, area: Rect, column: u16, row: u16, scroll: u16) {
        if area.is_empty()
            || column < area.x
            || column >= area.right()
            || row < area.y
            || row >= area.bottom()
        {
            return;
        }
        let (rendered, source_lines) = render_markdown_with_source_lines(&self.document.text);
        let target_row = usize::from(scroll) + usize::from(row.saturating_sub(area.y));
        let width = usize::from(area.width.max(1));
        let mut rendered_row = 0;
        let source_row = rendered.lines.iter().enumerate().find_map(|(index, line)| {
            let height = ui::preview_line_height(line, width);
            let contains_target = target_row < rendered_row + height;
            rendered_row += height;
            contains_target
                .then(|| source_lines.get(index).copied())
                .flatten()
        });
        let Some(source_row) = source_row.filter(|row| *row < self.editor.lines().len()) else {
            return;
        };
        let source_column = self.editor.lines()[source_row]
            .chars()
            .position(|character| !character.is_whitespace())
            .unwrap_or_default();
        self.editor.move_cursor(CursorMove::Jump(
            u16::try_from(source_row).unwrap_or(u16::MAX),
            u16::try_from(source_column).unwrap_or(u16::MAX),
        ));
        self.refresh_inline_editor();
    }

    fn record_edit(&mut self, history_items: usize) {
        if history_items == 0 {
            return;
        }
        self.undo_groups.push(history_items);
        self.redo_groups.clear();
    }

    fn cut_selection(&mut self) -> bool {
        if !self.editor.cut() {
            return false;
        }
        self.record_edit(1);
        self.sync_document();
        true
    }

    fn undo(&mut self) {
        let requested = self.undo_groups.pop().unwrap_or(1);
        let mut applied = 0;
        for _ in 0..requested {
            if !self.editor.undo() {
                break;
            }
            applied += 1;
        }
        if applied > 0 {
            self.redo_groups.push(applied);
            self.sync_document();
        }
    }

    fn redo(&mut self) {
        let requested = self.redo_groups.pop().unwrap_or(1);
        let mut applied = 0;
        for _ in 0..requested {
            if !self.editor.redo() {
                break;
            }
            applied += 1;
        }
        if applied > 0 {
            self.undo_groups.push(applied);
            self.sync_document();
        }
    }

    fn replace_range(&mut self, source_match: DocumentSearchMatch, replacement: &str) {
        let source = source_from_textarea(&self.editor);
        let start = offset_to_location(&source, source_match.start);
        let end = offset_to_location(&source, source_match.end);
        self.editor.move_cursor(CursorMove::Jump(
            u16::try_from(start.0).unwrap_or(u16::MAX),
            u16::try_from(start.1).unwrap_or(u16::MAX),
        ));
        self.editor.start_selection();
        self.editor.move_cursor(CursorMove::Jump(
            u16::try_from(end.0).unwrap_or(u16::MAX),
            u16::try_from(end.1).unwrap_or(u16::MAX),
        ));
        self.editor.insert_str(replacement);
        self.record_edit(1 + usize::from(!replacement.is_empty()));
        self.sync_document();
    }

    fn replace_source_transaction(
        &mut self,
        expected: SourceRevision,
        source: &str,
        allow_conflict: bool,
    ) -> Result<bool, SourceTransactionError> {
        if (!allow_conflict && (self.document.recovery_conflict || self.document.conflict))
            || !self.document.is_editable()
        {
            return Err(SourceTransactionError::Protected);
        }
        let current_source = source_from_textarea(&self.editor);
        let changed = self
            .document
            .update_from_editor_if_revision(expected, source.to_owned())
            .map_err(SourceTransactionError::Stale)?;
        if !changed {
            return Ok(false);
        }

        let cursor = self.editor.cursor();
        self.editor.select_all();
        self.editor.insert_str(source);
        self.record_edit(usize::from(!current_source.is_empty()) + usize::from(!source.is_empty()));
        self.editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
        self.refresh_inline_editor();
        Ok(true)
    }

    fn replace_all(&mut self, source: &str) -> Result<bool, SourceTransactionError> {
        let expected = self.document.source_revision;
        self.replace_source_transaction(expected, source, true)
    }
}

pub struct App {
    pub workspace: Workspace,
    pub config: Config,
    pub entries: Vec<WorkspaceEntry>,
    pub explorer_state: ListState,
    expanded_directories: HashSet<PathBuf>,
    pub tabs: Vec<EditorTab>,
    pub active_tab: Option<usize>,
    pub mode: Mode,
    pub theme: Theme,
    pub view_mode: ViewMode,
    pub focus: Focus,
    pub show_explorer: bool,
    pub preview_visible: bool,
    pub overlay: Option<Overlay>,
    pub status_message: Option<String>,
    pub preview_scroll: u16,
    pub preview_max_scroll: u16,
    pub preview_horizontal_scroll: u16,
    pub preview_horizontal_max_scroll: u16,
    pub preview_page: u16,
    pub preview_selected_link: Option<usize>,
    pub viewport_width: u16,
    pub explorer_width: u16,
    pub split_percent: u16,
    pub ui_regions: UiRegions,
    narrow_pane: Focus,
    mouse_drag_target: Option<MouseDragTarget>,
    last_explorer_click: Option<(usize, Instant)>,
    last_overlay_click: Option<(u16, Instant)>,
    workspace_clipboard: Option<WorkspaceClipboard>,
    session_store: Option<SessionStore>,
    last_session_state: Option<SessionState>,
    recovery_journal: Option<RecoveryJournal>,
    checkpoint_store: Option<CheckpointStore>,
    agent_session: Option<AgentSession>,
    pending_agent_proposal: Option<AgentProposal>,
    published_recovery: HashMap<PathBuf, (String, String)>,
    recent_paths: Vec<PathBuf>,
    session_views: HashMap<PathBuf, DocumentViewState>,
    pending_session_tabs: VecDeque<PathBuf>,
    restored_open_order: Vec<PathBuf>,
    inactive_probe_cursor: usize,
    workspace_scan_revision: Arc<AtomicU64>,
    workspace_scan_tx: Sender<WorkspaceScanCompletion>,
    workspace_scan_rx: Receiver<WorkspaceScanCompletion>,
    pub(crate) workspace_index_state: WorkspaceIndexState,
    pub workspace_scan_warnings: Vec<String>,
    workspace_search_revision: Arc<AtomicU64>,
    workspace_search_tx: Sender<WorkspaceSearchCompletion>,
    workspace_search_rx: Receiver<WorkspaceSearchCompletion>,
    pending_preview: Option<PendingPreview>,
    preview_inflight: HashSet<PreviewKey>,
    preview_tx: Sender<PreviewCompletion>,
    preview_rx: Receiver<PreviewCompletion>,
    preview_renders_started: u64,
    preview_synchronous_renders: u64,
    preview_stale_completions: u64,
    pending_transition: Option<PendingTransition>,
    should_quit: bool,
    pub(crate) debug_trace: Option<DebugTrace>,
}

impl App {
    /// Build an application and open an explicit file target when one was supplied.
    ///
    /// # Errors
    ///
    /// Returns an error when the initial file cannot be loaded safely.
    pub fn new(workspace: Workspace) -> anyhow::Result<Self> {
        Self::with_config(workspace, Config::default())
    }

    /// Build an application with the resolved `TermDraft` configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the initial file cannot be loaded safely.
    pub fn with_config(workspace: Workspace, config: Config) -> anyhow::Result<Self> {
        let session_store = SessionStore::platform_default().ok();
        let recovery_journal = RecoveryJournal::platform_default().ok();
        let checkpoint_store = CheckpointStore::platform_default().ok();
        Self::with_state_services_and_history(
            workspace,
            config,
            session_store,
            recovery_journal,
            checkpoint_store,
        )
    }

    pub(crate) fn enable_debug(&mut self, path: &Path) -> io::Result<()> {
        self.debug_trace = Some(DebugTrace::create(path, &self.workspace.root)?);
        Ok(())
    }

    fn debug_context(&self) -> String {
        format!(
            "mode={} focus={:?} overlay={}",
            self.mode.label(),
            self.focus,
            self.overlay.is_some()
        )
    }

    fn trace_event(&mut self, event: &Event) {
        let context = self.debug_context();
        let redact_text = self.mode == Mode::Write || self.overlay.is_some();
        if let Some(trace) = &mut self.debug_trace {
            trace.record_event(event, redact_text, &context);
        }
    }

    fn trace_command(&mut self, command: &str) {
        let context = self.debug_context();
        if let Some(trace) = &mut self.debug_trace {
            trace.record_command(command, &context);
        }
    }

    fn trace_runtime_error(&mut self, error: &str) {
        let context = self.debug_context();
        if let Some(trace) = &mut self.debug_trace {
            trace.record_error(error, &context);
        }
    }

    #[cfg(test)]
    fn with_state_services(
        workspace: Workspace,
        config: Config,
        session_store: Option<SessionStore>,
        recovery_journal: Option<RecoveryJournal>,
    ) -> anyhow::Result<Self> {
        Self::with_state_services_and_history(
            workspace,
            config,
            session_store,
            recovery_journal,
            None,
        )
    }

    fn with_state_services_and_history(
        workspace: Workspace,
        config: Config,
        session_store: Option<SessionStore>,
        recovery_journal: Option<RecoveryJournal>,
        checkpoint_store: Option<CheckpointStore>,
    ) -> anyhow::Result<Self> {
        let shallow_scan = workspace.scan_top_level();
        let entries = shallow_scan.entries;
        let selected = workspace.initial_file.as_ref().and_then(|initial| {
            entries
                .iter()
                .filter(|entry| entry.depth == 0)
                .position(|entry| entry.path == *initial)
        });
        let mut explorer_state = ListState::default();
        explorer_state.select(selected.or_else(|| (!entries.is_empty()).then_some(0)));

        let initial_file = workspace.initial_file.clone();
        let restore_open_tabs = initial_file.is_none();
        let startup_mode = config.editor.startup_mode;
        let theme = config.theme;
        let view_mode = match config.editor.view_mode {
            StartupView::Inline => ViewMode::Inline,
            StartupView::Split => ViewMode::Split,
        };
        let (workspace_scan_tx, workspace_scan_rx) = mpsc::channel();
        let (workspace_search_tx, workspace_search_rx) = mpsc::channel();
        let (preview_tx, preview_rx) = mpsc::channel();
        let mut app = Self {
            workspace,
            config,
            entries,
            explorer_state,
            expanded_directories: HashSet::new(),
            tabs: Vec::new(),
            active_tab: None,
            mode: Mode::Command,
            theme,
            view_mode,
            focus: Focus::Editor,
            show_explorer: true,
            preview_visible: true,
            overlay: None,
            status_message: None,
            preview_scroll: 0,
            preview_max_scroll: 0,
            preview_horizontal_scroll: 0,
            preview_horizontal_max_scroll: 0,
            preview_page: 1,
            preview_selected_link: None,
            viewport_width: 100,
            explorer_width: EXPLORER_DEFAULT_WIDTH,
            split_percent: 50,
            ui_regions: UiRegions::default(),
            narrow_pane: Focus::Editor,
            mouse_drag_target: None,
            last_explorer_click: None,
            last_overlay_click: None,
            workspace_clipboard: None,
            session_store,
            last_session_state: None,
            recovery_journal,
            checkpoint_store,
            agent_session: None,
            pending_agent_proposal: None,
            published_recovery: HashMap::new(),
            recent_paths: Vec::new(),
            session_views: HashMap::new(),
            pending_session_tabs: VecDeque::new(),
            restored_open_order: Vec::new(),
            inactive_probe_cursor: 0,
            workspace_scan_revision: Arc::new(AtomicU64::new(0)),
            workspace_scan_tx,
            workspace_scan_rx,
            workspace_index_state: WorkspaceIndexState::Idle,
            workspace_scan_warnings: shallow_scan.warnings,
            workspace_search_revision: Arc::new(AtomicU64::new(0)),
            workspace_search_tx,
            workspace_search_rx,
            pending_preview: None,
            preview_inflight: HashSet::new(),
            preview_tx,
            preview_rx,
            preview_renders_started: 0,
            preview_synchronous_renders: 0,
            preview_stale_completions: 0,
            pending_transition: None,
            should_quit: false,
            debug_trace: None,
        };
        if let Some(path) = initial_file {
            app.open_document(&path)?;
        }
        app.restore_session(restore_open_tabs);
        if app.tabs.is_empty() && app.entries.is_empty() {
            app.focus = Focus::Explorer;
            app.status_message = Some("Empty workspace · create or add a Markdown file".to_owned());
        } else if app.tabs.is_empty() {
            app.focus = Focus::Explorer;
        }
        if startup_mode == StartupMode::Write {
            app.set_mode(Mode::Write);
        }
        app.request_workspace_scan();
        Ok(app)
    }

    fn restore_session(&mut self, restore_open_tabs: bool) {
        let Some(store) = self.session_store.clone() else {
            return;
        };
        let loaded = store.load(&self.workspace.root);
        if let Some(warning) = loaded.warning {
            self.status_message = Some(warning);
        }
        let Some(state) = loaded.state else {
            return;
        };
        let stored_recent = state
            .documents
            .iter()
            .map(|view| view.path.clone())
            .collect::<Vec<_>>();
        let stored_views = state
            .documents
            .iter()
            .map(|view| (view.path.clone(), view.clone()))
            .collect::<HashMap<_, _>>();
        self.recent_paths = stored_recent;
        self.session_views = stored_views;
        self.restored_open_order.clone_from(&state.open_paths);
        if restore_open_tabs {
            let initial = state
                .active_path
                .as_ref()
                .filter(|path| state.open_paths.contains(path))
                .or_else(|| state.open_paths.first())
                .cloned();
            if let Some(path) = initial.as_ref() {
                let _ = self.open_document_with_prompts(path, false);
            }
            self.pending_session_tabs = state
                .open_paths
                .iter()
                .filter(|path| Some(*path) != initial.as_ref())
                .cloned()
                .collect();
            if !self.ensure_active_mixed_open_prompt(None)
                && let Some(path) = self.active_tab().map(|tab| tab.document.path.clone())
            {
                self.offer_recovery(&path);
            }
        }
        for index in 0..self.tabs.len() {
            let path = self.tabs[index].document.path.clone();
            if let Some(view) = state.view_for(&path) {
                Self::restore_tab_view(&mut self.tabs[index], view);
            }
        }
        self.restore_active_preview_scroll();
        if let Some(active) = self.active_tab().map(|tab| tab.document.path.clone()) {
            self.mark_recent(&active);
        }
    }

    fn restore_tab_view(tab: &mut EditorTab, view: &DocumentViewState) {
        tab.editor.move_cursor(CursorMove::Jump(
            u16::try_from(view.line).unwrap_or(u16::MAX),
            u16::try_from(view.column).unwrap_or(u16::MAX),
        ));
        tab.editor_scroll_x = scroll_coordinate(view.scroll_x);
        tab.editor_scroll_y = scroll_coordinate(view.scroll_y);
        tab.preview_scroll_x = scroll_coordinate(view.preview_scroll_x);
        tab.preview_scroll_y = scroll_coordinate(view.preview_scroll_y);
        tab.pending_scroll_restore = true;
    }

    fn materialize_next_session_tab(&mut self) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        let Some(path) = self.pending_session_tabs.pop_front() else {
            return false;
        };
        if self.tabs.iter().any(|tab| tab.document.path == path) {
            return true;
        }
        let active_path = self.active_tab().map(|tab| tab.document.path.clone());
        let result = self.open_document_with_prompts(&path, false);
        self.tabs.sort_by_key(|tab| {
            self.restored_open_order
                .iter()
                .position(|candidate| candidate == &tab.document.path)
                .unwrap_or(usize::MAX)
        });
        self.active_tab = active_path
            .as_ref()
            .and_then(|active| {
                self.tabs
                    .iter()
                    .position(|tab| tab.document.path == *active)
            })
            .or_else(|| (!self.tabs.is_empty()).then_some(0));
        self.restore_active_preview_scroll();
        if let Err(error) = result {
            self.status_message = Some(format!(
                "Skipped restored tab {} · {error}",
                self.workspace.relative(&path).display()
            ));
        }
        true
    }

    fn offer_recovery(&mut self, path: &Path) {
        if self.overlay.is_some() {
            return;
        }
        let Some(journal) = self.recovery_journal.clone() else {
            return;
        };
        match journal.load(path) {
            Ok(Some(entry)) if entry.workspace_root == self.workspace.root => {
                let disk_text = self
                    .tabs
                    .iter()
                    .find(|tab| tab.document.path == path)
                    .map(|tab| tab.document.saved_text.as_str());
                if disk_text == Some(entry.text.as_str()) {
                    let _ = journal.discard(path, Some(entry.fingerprint()));
                } else {
                    self.overlay = Some(Overlay::Recovery {
                        entry: Box::new(entry),
                    });
                }
            }
            Ok(_) => {}
            Err(error) => {
                self.status_message = Some(format!("Recovery journal ignored · {error}"));
            }
        }
    }

    fn restore_recovery_entry(&mut self, entry: &RecoveryEntry) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.document.path == entry.document_path)
        else {
            self.status_message = Some("Recovery target is not open".to_owned());
            return;
        };
        let conflict = !entry.baseline_matches(&self.tabs[index].document.snapshot);
        let line_ending = LineEnding::detect(&entry.text);
        let normalized = normalize_line_endings(&entry.text);
        let mixed_target = LineEnding::mixed_target(&entry.text);
        let cursor = self.tabs[index].editor.cursor();
        let mut editor = textarea_from_source(&normalized);
        apply_editor_config(&mut editor, &self.config.editor);
        style_cursor(&mut editor, self.mode);
        editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
        self.tabs[index].editor = editor;
        let recovered_source = if mixed_target.is_some() {
            entry.text.clone()
        } else {
            normalized.clone()
        };
        self.tabs[index].document.replace_source(recovered_source);
        self.tabs[index].document.encoding = entry.encoding;
        self.tabs[index].document.line_ending = line_ending;
        self.tabs[index].document.mixed_source =
            mixed_target.map(|target| MixedSource::new(entry.text.clone(), normalized, target));
        self.tabs[index].document.conflict = conflict;
        self.tabs[index].document.recovery_conflict = conflict;
        self.active_tab = Some(index);
        self.published_recovery.insert(
            entry.document_path.clone(),
            (entry.fingerprint().to_owned(), entry.text.clone()),
        );
        self.status_message = Some(if conflict {
            "Recovered draft · CONFLICT with current disk file".to_owned()
        } else {
            "Recovered unsaved draft".to_owned()
        });
        if let Some(target) = mixed_target {
            self.overlay = Some(Overlay::MixedLineEndings {
                tab_index: index,
                previous_active: Some(index),
                context: MixedLineEndingContext::Recovery,
                target,
            });
            self.enforce_active_read_only();
        }
    }

    fn discard_recovery_for(&mut self, path: &Path) {
        let Some(journal) = self.recovery_journal.clone() else {
            return;
        };
        let expected = self
            .published_recovery
            .get(path)
            .map(|(fingerprint, _)| fingerprint.clone())
            .or_else(|| {
                journal
                    .load(path)
                    .ok()
                    .flatten()
                    .map(|entry| entry.fingerprint().to_owned())
            });
        match journal.discard(path, expected.as_deref()) {
            Ok(()) => {
                self.published_recovery.remove(path);
            }
            Err(error) => {
                self.status_message = Some(format!("Recovery cleanup remains · {error}"));
            }
        }
    }

    fn persist_recovery(&mut self) {
        self.sync_active_document();
        let Some(journal) = self.recovery_journal.clone() else {
            return;
        };
        let dirty = self
            .tabs
            .iter()
            .filter(|tab| tab.document.is_dirty())
            .map(|tab| {
                (
                    tab.document.path.clone(),
                    tab.document.text.clone(),
                    tab.document.encoding,
                    tab.document.snapshot.clone(),
                )
            })
            .collect::<Vec<_>>();
        for (path, text, encoding, snapshot) in dirty {
            if self
                .published_recovery
                .get(&path)
                .is_some_and(|(_, published)| published == &text)
            {
                continue;
            }
            match journal.publish(&path, &self.workspace.root, &text, encoding, &snapshot) {
                Ok(entry) => {
                    self.published_recovery
                        .insert(path, (entry.fingerprint().to_owned(), text));
                }
                Err(error) => {
                    self.status_message = Some(format!("Recovery not saved · {error}"));
                }
            }
        }
    }

    fn open_recovery_manager(&mut self) {
        self.persist_recovery();
        self.show_recovery_manager(
            0,
            "Archive preserves bytes; quarantined drafts can be restored, exported, or deleted."
                .to_owned(),
        );
    }

    fn show_recovery_manager(&mut self, selected: usize, status: String) {
        let Some(journal) = self.recovery_journal.clone() else {
            self.status_message = Some("Recovery storage is unavailable".to_owned());
            return;
        };
        let records = match journal.inventory(Some(&self.workspace.root)) {
            Ok(records) => records,
            Err(error) => {
                self.status_message = Some(format!("Recovery inventory unavailable · {error}"));
                return;
            }
        };
        let protected_documents = self
            .tabs
            .iter()
            .filter(|tab| tab.document.is_dirty())
            .map(|tab| tab.document.path.clone())
            .collect::<Vec<_>>();
        let protected_journals = protected_documents
            .iter()
            .map(|path| journal.path_for(path))
            .collect::<Vec<_>>();
        self.overlay = Some(Overlay::RecoveryManager {
            selected: selected.min(records.len().saturating_sub(1)),
            records,
            focus: RecoveryManagerFocus::Records,
            target: TextInput::default(),
            protected_journals,
            protected_documents,
            retention_days: i64::from(self.config.recovery.retention_days),
            status,
        });
    }

    fn recovery_protection(&mut self) -> Option<(RecoveryJournal, Vec<PathBuf>, Vec<PathBuf>)> {
        self.sync_active_document();
        let journal = self.recovery_journal.clone()?;
        let documents = self
            .tabs
            .iter()
            .filter(|tab| tab.document.is_dirty())
            .map(|tab| tab.document.path.clone())
            .collect::<Vec<_>>();
        let journals = documents
            .iter()
            .map(|path| journal.path_for(path))
            .collect();
        Some((journal, documents, journals))
    }

    fn open_managed_recovery(&mut self, selected: usize, record: &RecoveryRecord) {
        let Some((journal, dirty_documents, dirty_journals)) = self.recovery_protection() else {
            return;
        };
        if record.quarantined {
            let Some(entry) = record.entry.as_ref() else {
                self.show_recovery_manager(
                    selected,
                    "Corrupt quarantine can only be deleted.".to_owned(),
                );
                return;
            };
            if dirty_documents
                .iter()
                .any(|path| paths_are_spelling_aliases(path, &entry.document_path))
            {
                self.show_recovery_manager(
                    selected,
                    "Cannot restore onto an open dirty document.".to_owned(),
                );
                return;
            }
            match journal.restore_quarantined(record) {
                Ok(entry) => self.open_recovery_entry(
                    selected,
                    entry,
                    "Restored quarantined recovery".to_owned(),
                ),
                Err(error) => {
                    self.show_recovery_manager(selected, format!("Restore failed · {error}"));
                }
            }
            return;
        }
        if dirty_journals.contains(&record.journal_path) {
            self.show_recovery_manager(
                selected,
                "This recovery belongs to an open dirty document.".to_owned(),
            );
            return;
        }
        let Some(entry) = record.entry.as_ref() else {
            self.show_recovery_manager(selected, "Corrupt recovery cannot be opened.".to_owned());
            return;
        };
        match journal.record_for(&entry.document_path) {
            Ok(Some(current)) if current.fingerprint == record.fingerprint => {
                if let Some(entry) = current.entry {
                    self.open_recovery_entry(selected, entry, "Opened recovery draft".to_owned());
                }
            }
            Ok(_) => self.show_recovery_manager(
                selected,
                "Recovery changed before it could be opened.".to_owned(),
            ),
            Err(error) => {
                self.show_recovery_manager(selected, format!("Recovery open failed · {error}"));
            }
        }
    }

    fn open_recovery_entry(&mut self, selected: usize, entry: RecoveryEntry, message: String) {
        match disk_state(&entry.document_path) {
            DiskState::Current(_) => match self.open_document(&entry.document_path) {
                Ok(()) => {
                    if matches!(
                        self.overlay,
                        Some(Overlay::MixedLineEndings {
                            context: MixedLineEndingContext::Open,
                            ..
                        })
                    ) {
                        return;
                    }
                    self.status_message = Some(message);
                    self.overlay = Some(Overlay::Recovery {
                        entry: Box::new(entry),
                    });
                }
                Err(error) => self.show_recovery_manager(
                    selected,
                    format!("Recovery draft cannot be opened · {error}"),
                ),
            },
            DiskState::Missing => {
                self.install_unavailable_recovery(&entry, ConflictKind::Missing);
            }
            DiskState::Unavailable(_) => {
                self.install_unavailable_recovery(&entry, ConflictKind::Unavailable);
            }
        }
    }

    fn install_unavailable_recovery(&mut self, entry: &RecoveryEntry, kind: ConflictKind) {
        let previous_active = self.active_tab;
        self.cache_active_view();
        let mut tab = EditorTab::from_unavailable_recovery(entry, &self.config.editor);
        style_cursor(&mut tab.editor, self.mode);
        let mixed_target = tab.document.mixed_line_ending_target();
        let index = self
            .tabs
            .iter()
            .position(|candidate| candidate.document.path == entry.document_path)
            .unwrap_or(self.tabs.len());
        if index == self.tabs.len() {
            self.tabs.push(tab);
        } else {
            self.tabs[index] = tab;
        }
        self.active_tab = Some(index);
        self.focus = Focus::Editor;
        self.preview_scroll = 0;
        self.preview_horizontal_scroll = 0;
        self.preview_selected_link = None;
        self.narrow_pane = Focus::Editor;
        self.mark_recent(&entry.document_path);
        self.published_recovery.insert(
            entry.document_path.clone(),
            (entry.fingerprint().to_owned(), entry.text.clone()),
        );
        if let Some(target) = mixed_target {
            self.overlay = Some(Overlay::MixedLineEndings {
                tab_index: index,
                previous_active,
                context: MixedLineEndingContext::Recovery,
                target,
            });
            self.status_message =
                Some("Recovered draft · original source unavailable · Save As required".to_owned());
            self.enforce_active_read_only();
        } else {
            self.show_conflict(index, kind);
        }
    }

    fn retarget_managed_recovery(
        &mut self,
        selected: usize,
        record: &RecoveryRecord,
        target: &str,
    ) {
        let Some((journal, dirty_documents, dirty_journals)) = self.recovery_protection() else {
            return;
        };
        if record.quarantined || record.entry.is_none() {
            self.show_recovery_manager(
                selected,
                "Only valid active recovery can be retargeted.".to_owned(),
            );
            return;
        }
        if dirty_journals.contains(&record.journal_path) {
            self.show_recovery_manager(
                selected,
                "Cannot move an open dirty document's recovery.".to_owned(),
            );
            return;
        }
        let target = match self.recovery_target_path(target, true) {
            Ok(target) => target,
            Err(error) => {
                self.show_recovery_manager(selected, format!("Retarget failed · {error}"));
                return;
            }
        };
        if dirty_documents.contains(&target) {
            self.show_recovery_manager(
                selected,
                "Cannot retarget onto an open dirty document.".to_owned(),
            );
            return;
        }
        match journal.retarget(record, &target, &self.workspace.root) {
            Ok(_) => self.show_recovery_manager(
                selected,
                format!(
                    "Recovery now follows {}",
                    self.workspace.relative(&target).display()
                ),
            ),
            Err(error) => {
                self.show_recovery_manager(selected, format!("Retarget failed · {error}"));
            }
        }
    }

    fn archive_managed_recovery(&mut self, selected: usize, record: &RecoveryRecord) {
        let Some((journal, _, dirty_journals)) = self.recovery_protection() else {
            return;
        };
        if record.quarantined {
            self.show_recovery_manager(selected, "Recovery is already archived.".to_owned());
            return;
        }
        if dirty_journals.contains(&record.journal_path) {
            self.show_recovery_manager(
                selected,
                "Cannot archive an open dirty document's recovery.".to_owned(),
            );
            return;
        }
        match journal.quarantine(record) {
            Ok(path) => self.show_recovery_manager(
                selected,
                format!(
                    "Archived recovery {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            ),
            Err(error) => self.show_recovery_manager(selected, format!("Archive failed · {error}")),
        }
    }

    fn export_managed_recovery(&mut self, selected: usize, record: &RecoveryRecord, target: &str) {
        let Some(journal) = self.recovery_journal.clone() else {
            return;
        };
        let target = match self.recovery_target_path(target, false) {
            Ok(target) => target,
            Err(error) => {
                self.show_recovery_manager(selected, format!("Export failed · {error}"));
                return;
            }
        };
        match journal.export_quarantined(record, &target) {
            Ok(_) => {
                self.refresh_entries(Some(&target));
                self.show_recovery_manager(
                    selected,
                    format!(
                        "Exported recovery copy to {}",
                        self.workspace.relative(&target).display()
                    ),
                );
            }
            Err(error) => self.show_recovery_manager(selected, format!("Export failed · {error}")),
        }
    }

    fn delete_managed_recovery(&mut self, record: &RecoveryRecord) {
        let Some(journal) = self.recovery_journal.clone() else {
            return;
        };
        match journal.delete_quarantined(record) {
            Ok(()) => self
                .show_recovery_manager(0, "Permanently deleted quarantined recovery.".to_owned()),
            Err(error) => self.show_recovery_manager(0, format!("Delete failed · {error}")),
        }
    }

    fn cleanup_managed_recovery(&mut self, records: &[RecoveryRecord], cutoff: OffsetDateTime) {
        let Some(journal) = self.recovery_journal.clone() else {
            return;
        };
        match journal.cleanup_quarantined(cutoff, Some(&self.workspace.root), Some(records)) {
            Ok(result) => {
                let mut status = format!(
                    "Deleted {} of {} expired recoveries",
                    result.deleted_count(),
                    result.selected_count()
                );
                if result.failed_count() > 0 {
                    let _ = write!(status, " · {} failed", result.failed_count());
                    for outcome in result.outcomes.iter().filter(|outcome| !outcome.deleted) {
                        let _ = write!(
                            status,
                            " · {}: {}",
                            self.workspace.relative(&outcome.document_path).display(),
                            outcome.error.as_deref().unwrap_or("deletion failed")
                        );
                    }
                }
                self.show_recovery_manager(0, status);
            }
            Err(error) => self.show_recovery_manager(0, format!("Cleanup failed · {error}")),
        }
    }

    fn recovery_target_path(&self, value: &str, allow_existing: bool) -> Result<PathBuf, String> {
        let relative = Path::new(value.trim());
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("enter a workspace-relative Markdown path".to_owned());
        }
        let candidate = self.workspace.root.join(relative);
        if allow_existing && candidate.exists() {
            return self
                .workspace
                .validate_document_path(&candidate)
                .map_err(|error| error.to_string());
        }
        self.workspace
            .new_document_path(relative)
            .map_err(|error| error.to_string())
    }

    fn current_session_state(&self) -> SessionState {
        let mut views = self.session_views.clone();
        for (index, tab) in self.tabs.iter().enumerate() {
            let (line, column) = tab.editor.cursor();
            let active = self.active_tab == Some(index);
            views.insert(
                tab.document.path.clone(),
                DocumentViewState {
                    path: tab.document.path.clone(),
                    line,
                    column,
                    scroll_x: f64::from(tab.editor_scroll_x),
                    scroll_y: f64::from(tab.editor_scroll_y),
                    preview_scroll_x: f64::from(if active {
                        self.preview_horizontal_scroll
                    } else {
                        tab.preview_scroll_x
                    }),
                    preview_scroll_y: f64::from(if active {
                        self.preview_scroll
                    } else {
                        tab.preview_scroll_y
                    }),
                },
            );
        }
        let loaded = self
            .tabs
            .iter()
            .map(|tab| tab.document.path.clone())
            .collect::<HashSet<_>>();
        let pending = self
            .pending_session_tabs
            .iter()
            .cloned()
            .collect::<HashSet<_>>();
        let mut open_paths = self
            .restored_open_order
            .iter()
            .filter(|path| loaded.contains(*path) || pending.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        for tab in &self.tabs {
            if !open_paths.contains(&tab.document.path) {
                open_paths.push(tab.document.path.clone());
            }
        }
        SessionState {
            workspace_root: self.workspace.root.clone(),
            active_path: self.active_tab().map(|tab| tab.document.path.clone()),
            documents: self
                .recent_paths
                .iter()
                .filter_map(|path| views.get(path).cloned())
                .collect(),
            open_paths,
        }
    }

    fn persist_session_if_changed(&mut self) {
        let Some(store) = self.session_store.clone() else {
            return;
        };
        let state = self.current_session_state();
        if self.last_session_state.as_ref() == Some(&state) {
            return;
        }
        match store.save(&state) {
            Ok(()) => self.last_session_state = Some(state),
            Err(error) => self.status_message = Some(format!("Session not saved · {error}")),
        }
    }

    fn flush_state(&mut self) {
        self.persist_recovery();
        self.persist_session_if_changed();
        self.agent_session = None;
        self.pending_agent_proposal = None;
    }

    #[must_use]
    pub fn active_tab(&self) -> Option<&EditorTab> {
        self.active_tab.and_then(|index| self.tabs.get(index))
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut EditorTab> {
        let index = self.active_tab?;
        self.tabs.get_mut(index)
    }

    pub fn sync_active_document(&mut self) {
        if let Some(tab) = self.active_tab_mut() {
            tab.sync_document();
        }
    }

    fn open_local_history(&mut self) {
        self.show_local_history(None, None);
    }

    fn show_local_history(&mut self, selected_id: Option<&str>, status: Option<String>) {
        self.sync_active_document();
        let Some(tab) = self.active_tab() else {
            self.status_message = Some("Open a Markdown document first".to_owned());
            return;
        };
        let path = tab.document.path.clone();
        let current = tab.document.text.clone();
        let expected_revision = tab.document.source_revision;
        let Some(store) = self.checkpoint_store.clone() else {
            self.overlay = Some(Overlay::Message(
                "Local History is unavailable on this system.".to_owned(),
            ));
            return;
        };
        let enabled = match store.is_enabled(&self.workspace.root) {
            Ok(enabled) => enabled,
            Err(error) => {
                self.overlay = Some(Overlay::Message(format!(
                    "Cannot open Local History · {error}"
                )));
                return;
            }
        };
        let listed = if enabled {
            match store.list(&self.workspace.root, &path) {
                Ok(listed) => listed,
                Err(error) => {
                    self.overlay = Some(Overlay::Message(format!(
                        "Cannot read Local History · {error}"
                    )));
                    return;
                }
            }
        } else {
            CheckpointList::default()
        };
        let selected = selected_id
            .and_then(|id| {
                listed
                    .checkpoints
                    .iter()
                    .position(|checkpoint| checkpoint.id == id)
            })
            .unwrap_or_default()
            .min(listed.checkpoints.len().saturating_sub(1));
        let (diff, diff_truncated) = listed.checkpoints.get(selected).map_or_else(
            || (Vec::new(), false),
            |checkpoint| build_history_diff(&checkpoint.source, &current),
        );
        let default_status = if !enabled {
            "Local History is disabled for this workspace.".to_owned()
        } else if listed.checkpoints.is_empty() {
            "No checkpoints for this document yet.".to_owned()
        } else {
            format!(
                "{} checkpoint{} · newest first",
                listed.checkpoints.len(),
                if listed.checkpoints.len() == 1 {
                    ""
                } else {
                    "s"
                }
            )
        };
        let status = append_history_warnings(status.unwrap_or(default_status), &listed.warnings);
        self.overlay = Some(Overlay::LocalHistory {
            enabled,
            path,
            checkpoints: listed.checkpoints,
            selected,
            focus: HistoryFocus::Checkpoints,
            diff,
            diff_scroll: 0,
            diff_truncated,
            expected_revision,
            status,
        });
    }

    fn create_manual_checkpoint(&mut self) {
        self.sync_active_document();
        let Some(tab) = self.active_tab() else {
            self.status_message = Some("Open a Markdown document first".to_owned());
            return;
        };
        let path = tab.document.path.clone();
        let source = tab.document.text.clone();
        let Some(store) = self.checkpoint_store.clone() else {
            self.overlay = Some(Overlay::Message(
                "Local History is unavailable on this system.".to_owned(),
            ));
            return;
        };
        match store.capture(
            &self.workspace.root,
            &path,
            &source,
            CheckpointReason::Manual,
        ) {
            Ok(CaptureOutcome::Stored {
                checkpoint,
                pruned,
                warnings,
            }) => {
                let mut status = "Checkpoint created".to_owned();
                if pruned > 0 {
                    let _ = write!(status, " · pruned {pruned} oldest");
                }
                self.show_local_history(
                    Some(&checkpoint.id),
                    Some(append_history_warnings(status, &warnings)),
                );
            }
            Ok(CaptureOutcome::Duplicate {
                checkpoint_id,
                warnings,
            }) => {
                self.show_local_history(
                    Some(&checkpoint_id),
                    Some(append_history_warnings(
                        "Already the newest checkpoint".to_owned(),
                        &warnings,
                    )),
                );
            }
            Ok(CaptureOutcome::Disabled) => self.show_local_history(
                None,
                Some("Enable Local History before creating a checkpoint.".to_owned()),
            ),
            Err(error) => {
                self.overlay = Some(Overlay::Message(format!(
                    "Checkpoint not created · {error}"
                )));
            }
        }
    }

    fn set_history_enabled(&mut self, enabled: bool) {
        let Some(store) = self.checkpoint_store.clone() else {
            self.overlay = Some(Overlay::Message(
                "Local History is unavailable on this system.".to_owned(),
            ));
            return;
        };
        match store.set_enabled(&self.workspace.root, enabled) {
            Ok(()) => self.show_local_history(
                None,
                Some(if enabled {
                    "Local History enabled for this workspace.".to_owned()
                } else {
                    "Local History disabled · existing checkpoints kept.".to_owned()
                }),
            ),
            Err(error) => {
                self.overlay = Some(Overlay::Message(format!(
                    "Local History setting not changed · {error}"
                )));
            }
        }
    }

    fn request_history_clear(&mut self, scope: HistoryClearScope, path: &Path) {
        let Some(store) = self.checkpoint_store.clone() else {
            self.overlay = Some(Overlay::Message(
                "Local History is unavailable on this system.".to_owned(),
            ));
            return;
        };
        let listed = match scope {
            HistoryClearScope::Document => store.list(&self.workspace.root, path),
            HistoryClearScope::Workspace => store.list_all(&self.workspace.root),
        };
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                self.show_local_history(
                    None,
                    Some(format!("Cannot prepare clear confirmation · {error}")),
                );
                return;
            }
        };
        if listed.checkpoints.is_empty() {
            self.show_local_history(
                None,
                Some(append_history_warnings(
                    "Nothing to clear.".to_owned(),
                    &listed.warnings,
                )),
            );
            return;
        }
        self.overlay = Some(Overlay::HistoryClearConfirm {
            scope,
            path: path.to_path_buf(),
            checkpoint_ids: sorted_checkpoint_ids(&listed.checkpoints),
        });
    }

    fn clear_history(&mut self, scope: HistoryClearScope, path: &Path, expected_ids: &[String]) {
        let Some(store) = self.checkpoint_store.clone() else {
            self.overlay = Some(Overlay::Message(
                "Local History is unavailable on this system.".to_owned(),
            ));
            return;
        };
        let mutation = match scope {
            HistoryClearScope::Document => {
                store.clear_document(&self.workspace.root, path, expected_ids)
            }
            HistoryClearScope::Workspace => store.clear_all(&self.workspace.root, expected_ids),
        };
        match mutation {
            Ok(mutation) => self.show_local_history(
                None,
                Some(append_history_warnings(
                    format!("Cleared {} checkpoint(s)", mutation.affected),
                    &mutation.warnings,
                )),
            ),
            Err(CheckpointError::Stale(_)) => self.show_local_history(
                None,
                Some("Clear canceled · Local History changed.".to_owned()),
            ),
            Err(error) => {
                self.show_local_history(None, Some(format!("Local History not cleared · {error}")));
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn restore_history_checkpoint(
        &mut self,
        path: &Path,
        checkpoint_id: &str,
        expected_revision: SourceRevision,
    ) {
        self.sync_active_document();
        let Some(index) = self.active_tab else {
            self.status_message = Some("Restore blocked · no document open".to_owned());
            return;
        };
        if self.tabs[index].document.path != path
            || self.tabs[index].document.source_revision != expected_revision
        {
            self.show_local_history(
                Some(checkpoint_id),
                Some("Restore blocked · source changed while history was open.".to_owned()),
            );
            return;
        }
        if self.tabs[index].document.recovery_conflict || !self.tabs[index].document.is_editable() {
            self.show_local_history(
                Some(checkpoint_id),
                Some("Restore blocked · resolve document protection first.".to_owned()),
            );
            return;
        }
        let Some(store) = self.checkpoint_store.clone() else {
            self.status_message = Some("Restore blocked · Local History unavailable".to_owned());
            return;
        };
        let listed = match store.list(&self.workspace.root, path) {
            Ok(listed) => listed,
            Err(error) => {
                self.show_local_history(
                    None,
                    Some(format!(
                        "Restore blocked · cannot verify checkpoint: {error}"
                    )),
                );
                return;
            }
        };
        let Some(checkpoint) = listed
            .checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == checkpoint_id)
            .cloned()
        else {
            self.show_local_history(
                None,
                Some(append_history_warnings(
                    "Restore blocked · checkpoint changed or is corrupt.".to_owned(),
                    &listed.warnings,
                )),
            );
            return;
        };
        let current = self.tabs[index].document.text.clone();
        if checkpoint.source == current {
            self.show_local_history(
                Some(checkpoint_id),
                Some("Already showing this checkpoint · no changes made.".to_owned()),
            );
            return;
        }

        let allow_missing_conflict = match disk_state(path) {
            DiskState::Current(loaded) => {
                let document = &self.tabs[index].document;
                let same_content = loaded.snapshot.sha256 == document.snapshot.sha256;
                let same_origin = loaded.snapshot.same_origin(&document.snapshot);
                let safe = loaded.snapshot == document.snapshot || (same_content && same_origin);
                if document.conflict || !safe {
                    self.tabs[index].document.conflict = true;
                    self.show_local_history(
                        Some(checkpoint_id),
                        Some(
                            "Restore blocked · file changed outside TermDraft; reload or Save As."
                                .to_owned(),
                        ),
                    );
                    return;
                }
                false
            }
            DiskState::Missing => true,
            DiskState::Unavailable(error) => {
                self.show_local_history(
                    Some(checkpoint_id),
                    Some(format!("Restore blocked · file unavailable: {error}")),
                );
                return;
            }
        };

        let capture = store.capture(
            &self.workspace.root,
            path,
            &current,
            CheckpointReason::BeforeRestore,
        );
        let capture_note = match capture {
            Ok(CaptureOutcome::Stored {
                pruned, warnings, ..
            }) => {
                let mut note = String::new();
                if pruned > 0 {
                    let _ = write!(note, "pruned {pruned} oldest");
                }
                append_history_warnings(note, &warnings)
            }
            Ok(CaptureOutcome::Duplicate { warnings, .. }) => {
                append_history_warnings(String::new(), &warnings)
            }
            Ok(CaptureOutcome::Disabled) => {
                self.show_local_history(
                    Some(checkpoint_id),
                    Some("Restore blocked · Local History is disabled.".to_owned()),
                );
                return;
            }
            Err(error) => {
                self.show_local_history(
                    Some(checkpoint_id),
                    Some(format!(
                        "Restore blocked · pre-restore checkpoint failed: {error}"
                    )),
                );
                return;
            }
        };
        match self.tabs[index].replace_source_transaction(
            expected_revision,
            &checkpoint.source,
            allow_missing_conflict,
        ) {
            Ok(true) => {
                if allow_missing_conflict {
                    self.tabs[index].document.conflict = true;
                }
                self.preview_selected_link = None;
                self.overlay = None;
                let message = if allow_missing_conflict {
                    "Restored checkpoint to buffer · original missing; use Save As".to_owned()
                } else {
                    "Restored checkpoint to buffer · disk unchanged until Save".to_owned()
                };
                self.status_message = Some(append_history_note(message, &capture_note));
            }
            Ok(false) => self.show_local_history(
                Some(checkpoint_id),
                Some("Already showing this checkpoint · no changes made.".to_owned()),
            ),
            Err(SourceTransactionError::Protected) => self.show_local_history(
                Some(checkpoint_id),
                Some("Restore blocked · document is protected.".to_owned()),
            ),
            Err(SourceTransactionError::Stale(_)) => self.show_local_history(
                Some(checkpoint_id),
                Some("Restore blocked · source revision changed.".to_owned()),
            ),
        }
    }

    fn capture_history_note(
        &self,
        index: usize,
        source: &str,
        reason: CheckpointReason,
    ) -> Option<String> {
        let store = self.checkpoint_store.as_ref()?;
        let path = &self.tabs[index].document.path;
        match store.capture(&self.workspace.root, path, source, reason) {
            Ok(CaptureOutcome::Stored {
                pruned, warnings, ..
            }) => {
                let mut note = String::new();
                if pruned > 0 {
                    let _ = write!(note, "Local History pruned {pruned} oldest");
                }
                let note = append_history_warnings(note, &warnings);
                (!note.is_empty()).then_some(note)
            }
            Ok(CaptureOutcome::Duplicate { warnings, .. }) => {
                let note = append_history_warnings(String::new(), &warnings);
                (!note.is_empty()).then_some(note)
            }
            Ok(CaptureOutcome::Disabled) => None,
            Err(error) => Some(format!("Local History unavailable: {error}")),
        }
    }

    fn open_agent_sharing(&mut self) {
        if let Some(proposal) = self.pending_agent_proposal.clone() {
            self.overlay = Some(Overlay::AgentProposal {
                proposal,
                scroll: 0,
            });
            return;
        }
        self.sync_active_document();
        if let Some(session) = &self.agent_session {
            self.overlay = Some(Overlay::AgentSharing {
                root: session.shared_root().to_path_buf(),
            });
            return;
        }
        let root = self.workspace.root.clone();
        match AgentSession::start(root.clone()) {
            Ok(session) => {
                self.agent_session = Some(session);
                self.status_message = Some(
                    "AGENT SHARED · current workspace is available to local agents".to_owned(),
                );
                self.overlay = Some(Overlay::AgentSharing { root });
            }
            Err(error) => {
                self.status_message = Some(format!("Agent sharing unavailable · {error}"));
            }
        }
    }

    fn revoke_agent_sharing(&mut self) {
        self.agent_session = None;
        self.pending_agent_proposal = None;
        self.status_message = Some("Agent sharing revoked · local endpoint removed".to_owned());
    }

    fn clear_agent_proposal_for_path(&mut self, path: &Path) {
        if self
            .pending_agent_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.path == path)
        {
            self.pending_agent_proposal = None;
        }
    }

    fn clear_agent_proposal_within(&mut self, path: &Path) {
        if self
            .pending_agent_proposal
            .as_ref()
            .is_some_and(|proposal| path_is_within(&proposal.path, path))
        {
            self.pending_agent_proposal = None;
        }
    }

    fn poll_agent_requests(&mut self) -> bool {
        let calls = self
            .agent_session
            .as_ref()
            .map_or_else(Vec::new, AgentSession::drain_calls);
        if calls.is_empty() {
            return false;
        }
        for call in calls {
            self.handle_agent_call(call);
        }
        true
    }

    fn handle_agent_call(&mut self, call: AgentCall) {
        let Some(shared_root) = self
            .agent_session
            .as_ref()
            .map(|session| session.shared_root().to_path_buf())
        else {
            call.respond(AgentResponse::error("agent sharing is no longer active"));
            return;
        };
        if shared_root != self.workspace.root {
            call.respond(AgentResponse::error(
                "the shared workspace is no longer active",
            ));
            return;
        }
        self.sync_active_document();
        if matches!(call.action, AgentAction::ReadWorkspace) {
            let workspace = self.workspace.clone();
            let open_documents =
                match self.agent_open_documents(MAX_AGENT_SOURCE_BYTES, MAX_AGENT_RESPONSE_BYTES) {
                    Ok(documents) => documents,
                    Err(response) => {
                        call.respond(response);
                        return;
                    }
                };
            thread::spawn(move || {
                let response = agent_workspace_response_with_limits(
                    &workspace,
                    open_documents,
                    MAX_AGENT_SOURCE_BYTES,
                    MAX_AGENT_RESPONSE_BYTES,
                    || call.is_cancelled(),
                );
                if !call.is_cancelled() {
                    call.respond(response);
                }
            });
            return;
        }
        let Some(index) = self.active_tab else {
            call.respond(AgentResponse::error(
                "no document is active in the shared workspace",
            ));
            return;
        };
        let active_path = self.tabs[index].document.path.clone();

        match call.action.clone() {
            AgentAction::Read => {
                let document = &self.tabs[index].document;
                call.respond(AgentResponse::Document {
                    path: self
                        .workspace
                        .relative(&document.path)
                        .display()
                        .to_string(),
                    source: document.text.clone(),
                    revision: revision_label(document.source_revision),
                    dirty: document.is_dirty(),
                });
            }
            AgentAction::Propose {
                expected_path,
                expected_revision,
                change,
                origin,
            } => {
                let response = self.receive_agent_proposal(
                    index,
                    &active_path,
                    &expected_path,
                    &expected_revision,
                    change,
                    origin.as_deref(),
                );
                call.respond(response);
            }
            AgentAction::ReadWorkspace => unreachable!("workspace reads return before this match"),
        }
    }

    fn agent_open_documents(
        &self,
        max_document_bytes: usize,
        max_response_bytes: usize,
    ) -> Result<Vec<AgentOpenDocument>, AgentResponse> {
        let empty_response = AgentResponse::Workspace {
            documents: Vec::new(),
            warnings: Vec::new(),
        };
        let mut encoded_size = json_size(&empty_response)
            .map_err(|_| AgentResponse::error("cannot measure the agent workspace response"))?;
        let response_limit = u64::try_from(max_response_bytes).unwrap_or(u64::MAX);
        if encoded_size >= response_limit {
            return Err(AgentResponse::error(
                "agent workspace response exceeds the size limit",
            ));
        }
        let mut candidates = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            let Ok(relative) = tab.document.path.strip_prefix(&self.workspace.root) else {
                continue;
            };
            let relative = relative.to_path_buf();
            if !agent_workspace_path_is_allowed(&relative)
                || !agent_open_path_is_shareable(&tab.document.path, &self.workspace.root)
            {
                continue;
            }
            if tab.document.text.len() > max_document_bytes {
                return Err(agent_document_limit_error(&relative));
            }
            let path = relative.display().to_string();
            let revision = revision_label(tab.document.source_revision);
            let preflight = AgentWorkspaceDocumentRef {
                path: &path,
                source: &tab.document.text,
                revision: &revision,
                dirty: tab.document.is_dirty(),
                open: true,
                conflict: tab.document.conflict || tab.document.recovery_conflict,
            };
            let item_size = json_size(&preflight)
                .map_err(|_| AgentResponse::error("cannot measure an agent workspace document"))?;
            reserve_agent_workspace_item(
                &mut encoded_size,
                item_size,
                candidates.len(),
                response_limit,
            )?;
            candidates.push((
                index,
                AgentOpenDocument {
                    path: tab.document.path.clone(),
                    relative,
                    source: String::new(),
                    revision,
                    dirty: tab.document.is_dirty(),
                    conflict: tab.document.conflict || tab.document.recovery_conflict,
                },
            ));
        }
        Ok(candidates
            .into_iter()
            .map(|(index, mut document)| {
                document.source.clone_from(&self.tabs[index].document.text);
                document
            })
            .collect())
    }

    fn receive_agent_proposal(
        &mut self,
        index: usize,
        target_path: &Path,
        expected_path: &str,
        expected_revision: &str,
        change: ProposalChange,
        origin: Option<&str>,
    ) -> AgentResponse {
        let document = &self.tabs[index].document;
        if document.conflict || document.recovery_conflict || !document.is_editable() {
            return AgentResponse::error("the active document is protected by a conflict");
        }
        if Path::new(expected_path) != self.workspace.relative(target_path) {
            return AgentResponse::error(
                "the proposal target is not the active document; read the intended document again",
            );
        }
        if expected_revision != revision_label(document.source_revision) {
            return AgentResponse::error("the proposal targets a stale source revision");
        }
        if self.pending_agent_proposal.is_some() {
            return AgentResponse::error("another agent proposal is awaiting review");
        }
        let proposed_source = match apply_proposal_change(&document.text, change) {
            Ok(source) => source,
            Err(error) => return AgentResponse::error(error),
        };
        if proposed_source.len() > MAX_AGENT_SOURCE_BYTES {
            return AgentResponse::error("the proposed source exceeds the 16 MiB limit");
        }
        if proposed_source == document.text {
            return AgentResponse::error("the proposal does not change the active source");
        }
        let (diff, diff_truncated) = build_history_diff(&document.text, &proposed_source);
        let proposal = AgentProposal {
            id: format!(
                "proposal-{}",
                NEXT_AGENT_PROPOSAL_ID.fetch_add(1, Ordering::Relaxed)
            ),
            path: target_path.to_path_buf(),
            expected_revision: document.source_revision,
            origin: sanitized_agent_origin(origin),
            proposed_source,
            diff,
            diff_truncated,
        };
        let proposal_id = proposal.id.clone();
        self.pending_agent_proposal = Some(proposal.clone());
        if self.overlay.is_none() {
            self.overlay = Some(Overlay::AgentProposal {
                proposal,
                scroll: 0,
            });
        } else {
            self.status_message =
                Some("Agent proposal ready · open Agent sharing to review".to_owned());
        }
        AgentResponse::PendingReview { proposal_id }
    }

    fn accept_agent_proposal(&mut self, proposal: &AgentProposal) {
        self.sync_active_document();
        if self
            .pending_agent_proposal
            .as_ref()
            .is_none_or(|pending| pending.id != proposal.id)
        {
            self.status_message = Some("Agent proposal is no longer pending".to_owned());
            return;
        }
        let Some(index) = self.active_tab else {
            self.status_message = Some("Agent proposal blocked · document is not open".to_owned());
            return;
        };
        if self.tabs[index].document.path != proposal.path {
            self.status_message =
                Some("Agent proposal blocked · shared document is not active".to_owned());
            return;
        }
        let document = &self.tabs[index].document;
        if document.conflict || document.recovery_conflict || !document.is_editable() {
            self.status_message = Some("Agent proposal blocked · document is protected".to_owned());
            return;
        }
        if document.source_revision != proposal.expected_revision {
            self.status_message =
                Some("Agent proposal blocked · source changed during review".to_owned());
            return;
        }
        let Some(recovery) = self.recovery_journal.clone() else {
            self.status_message =
                Some("Agent proposal blocked · Recovery storage is unavailable".to_owned());
            return;
        };
        let recovery_entry = match recovery.publish(
            &proposal.path,
            &self.workspace.root,
            &proposal.proposed_source,
            document.encoding,
            &document.snapshot,
        ) {
            Ok(entry) => entry,
            Err(error) => {
                self.status_message = Some(format!(
                    "Agent proposal blocked · Recovery not saved · {error}"
                ));
                return;
            }
        };
        self.published_recovery.insert(
            proposal.path.clone(),
            (
                recovery_entry.fingerprint().to_owned(),
                proposal.proposed_source.clone(),
            ),
        );
        let current = self.tabs[index].document.text.clone();
        let before_note =
            self.capture_history_note(index, &current, CheckpointReason::BeforeAgentEdit);
        match self.tabs[index].replace_source_transaction(
            proposal.expected_revision,
            &proposal.proposed_source,
            false,
        ) {
            Ok(true) => {
                let after_note = self.capture_history_note(
                    index,
                    &proposal.proposed_source,
                    CheckpointReason::AfterAgentEdit,
                );
                self.pending_agent_proposal = None;
                let mut status =
                    format!("Applied {} · buffer modified · not saved", proposal.origin);
                if let Some(note) = before_note.as_deref() {
                    status = append_history_note(status, note);
                }
                if let Some(note) = after_note.as_deref() {
                    status = append_history_note(status, note);
                }
                self.status_message = Some(status);
            }
            Ok(false) => {
                self.pending_agent_proposal = None;
                self.status_message = Some("Agent proposal made no changes".to_owned());
            }
            Err(SourceTransactionError::Protected) => {
                self.status_message =
                    Some("Agent proposal blocked · document is protected".to_owned());
            }
            Err(SourceTransactionError::Stale(_)) => {
                self.status_message =
                    Some("Agent proposal blocked · source changed during review".to_owned());
            }
        }
    }

    fn reject_agent_proposal(&mut self, proposal_id: &str) {
        if self
            .pending_agent_proposal
            .as_ref()
            .is_some_and(|proposal| proposal.id == proposal_id)
        {
            self.pending_agent_proposal = None;
            self.status_message = Some("Agent proposal rejected · buffer unchanged".to_owned());
        }
    }

    #[must_use]
    pub const fn workspace_is_shared(&self) -> bool {
        self.agent_session.is_some()
    }

    fn current_preview_key(&self) -> Option<PreviewKey> {
        let tab = self.active_tab()?;
        Some(PreviewKey {
            path: tab.document.path.clone(),
            revision: tab.document.source_revision,
            selected_link: self.preview_selected_link,
        })
    }

    fn schedule_preview_render(&mut self) {
        self.schedule_preview_render_at(Instant::now());
    }

    fn schedule_preview_render_at(&mut self, now: Instant) {
        if !self.preview_is_visible() {
            self.pending_preview = None;
            return;
        }
        self.sync_active_document();
        let Some(key) = self.current_preview_key() else {
            self.pending_preview = None;
            return;
        };
        let Some(cache_key) = self
            .active_tab()
            .and_then(|tab| tab.preview_cache.as_ref())
            .map(|cache| &cache.key)
        else {
            self.pending_preview = None;
            return;
        };
        if cache_key == &key
            || self
                .pending_preview
                .as_ref()
                .is_some_and(|pending| pending.key == key)
            || self.preview_inflight.contains(&key)
        {
            return;
        }
        let Some(source) = self.active_tab().map(|tab| tab.document.text.clone()) else {
            return;
        };
        self.pending_preview = Some(PendingPreview {
            key,
            source,
            ready_at: now + Duration::from_millis(50),
        });
    }

    fn start_due_preview_render(&mut self) {
        self.start_due_preview_render_at(Instant::now());
    }

    fn start_due_preview_render_at(&mut self, now: Instant) {
        let Some(pending) = self.pending_preview.take() else {
            return;
        };
        if pending.ready_at > now {
            self.pending_preview = Some(pending);
            return;
        }
        self.preview_inflight.insert(pending.key.clone());
        self.preview_renders_started += 1;
        let sender = self.preview_tx.clone();
        let _worker = thread::spawn(move || {
            let rendered = render_markdown_document(&pending.source, pending.key.selected_link);
            let _ = sender.send(PreviewCompletion {
                key: pending.key,
                rendered,
            });
        });
    }

    fn poll_preview_results(&mut self) -> bool {
        let completions = self.preview_rx.try_iter().collect::<Vec<_>>();
        let mut changed = false;
        for completion in completions {
            self.preview_inflight.remove(&completion.key);
            if self.current_preview_key().as_ref() != Some(&completion.key) {
                self.preview_stale_completions += 1;
                continue;
            }
            if let Some(tab) = self.active_tab_mut() {
                tab.preview_cache = Some(PreviewCache {
                    key: completion.key,
                    rendered: completion.rendered,
                });
                changed = true;
            }
        }
        changed
    }

    fn preview_poll_timeout(&self) -> Duration {
        let standard = if self.agent_session.is_some() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(250)
        };
        let worker_poll = if self.preview_inflight.is_empty() {
            standard
        } else {
            Duration::from_millis(16)
        };
        self.pending_preview
            .as_ref()
            .map_or(worker_poll, |pending| {
                pending
                    .ready_at
                    .saturating_duration_since(Instant::now())
                    .min(worker_poll)
            })
    }

    pub(crate) fn preview_for_draw(&mut self) -> Option<RenderedMarkdown> {
        self.sync_active_document();
        let key = self.current_preview_key()?;
        let active_index = self.active_tab?;
        if let Some(cache) = self.tabs[active_index].preview_cache.as_ref() {
            if cache.key == key {
                return Some(cache.rendered.clone());
            }
            if cache.key.path == key.path && cache.key.revision != key.revision {
                return Some(cache.rendered.clone());
            }
        }

        let source = self.tabs[active_index].document.text.clone();
        self.preview_synchronous_renders += 1;
        let rendered = render_markdown_document(&source, key.selected_link);
        self.tabs[active_index].preview_cache = Some(PreviewCache {
            key: key.clone(),
            rendered: rendered.clone(),
        });
        if self
            .pending_preview
            .as_ref()
            .is_some_and(|pending| pending.key == key)
        {
            self.pending_preview = None;
        }
        Some(rendered)
    }

    fn cache_active_view(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        self.tabs[index].preview_scroll_x = self.preview_horizontal_scroll;
        self.tabs[index].preview_scroll_y = self.preview_scroll;
        let tab = &self.tabs[index];
        let (line, column) = tab.editor.cursor();
        let view = DocumentViewState {
            path: tab.document.path.clone(),
            line,
            column,
            scroll_x: f64::from(tab.editor_scroll_x),
            scroll_y: f64::from(tab.editor_scroll_y),
            preview_scroll_x: f64::from(tab.preview_scroll_x),
            preview_scroll_y: f64::from(tab.preview_scroll_y),
        };
        self.session_views.insert(view.path.clone(), view);
    }

    fn restore_active_preview_scroll(&mut self) {
        let Some(index) = self.active_tab else {
            self.preview_scroll = 0;
            self.preview_horizontal_scroll = 0;
            return;
        };
        self.preview_scroll = self.tabs[index].preview_scroll_y;
        self.preview_horizontal_scroll = self.tabs[index].preview_scroll_x;
    }

    fn mark_recent(&mut self, path: &Path) {
        self.recent_paths.retain(|recent| recent != path);
        self.recent_paths.insert(0, path.to_path_buf());
        if self.recent_paths.len() > MAX_SESSION_DOCUMENTS
            && let Some(evicted) = self.recent_paths.pop()
            && !self.tabs.iter().any(|tab| tab.document.path == evicted)
        {
            self.session_views.remove(&evicted);
        }
    }

    fn retarget_recent(&mut self, source: &Path, target: &Path) {
        for recent in &mut self.recent_paths {
            if recent == source {
                *recent = target.to_path_buf();
            }
        }
        if let Some(view) = self.session_views.remove(source) {
            self.session_views.insert(
                target.to_path_buf(),
                DocumentViewState {
                    path: target.to_path_buf(),
                    ..view
                },
            );
        }
        self.mark_recent(target);
    }

    fn open_document(&mut self, path: &Path) -> anyhow::Result<()> {
        self.open_document_with_prompts(path, true)
    }

    fn open_document_with_prompts(
        &mut self,
        path: &Path,
        show_prompts: bool,
    ) -> anyhow::Result<()> {
        let path = self.workspace.validate_document_path(path)?;
        if let Some(index) = self.tabs.iter().position(|tab| tab.document.path == path) {
            let previous_active = self.active_tab;
            self.cache_active_view();
            self.active_tab = Some(index);
            self.focus = Focus::Editor;
            self.preview_scroll = 0;
            self.preview_horizontal_scroll = 0;
            self.preview_selected_link = None;
            self.narrow_pane = Focus::Editor;
            self.restore_active_preview_scroll();
            if show_prompts {
                self.mark_recent(&path);
                self.ensure_active_mixed_open_prompt(previous_active);
            }
            self.enforce_active_read_only();
            return Ok(());
        }
        let previous_active = self.active_tab;
        self.cache_active_view();
        let mut tab = EditorTab::from_path(&path, &self.config.editor)?;
        style_cursor(&mut tab.editor, self.mode);
        if let Some(view) = self.session_views.get(&path) {
            Self::restore_tab_view(&mut tab, view);
        }
        tab.pending_mixed_open = tab.document.mixed_line_ending_target().is_some();
        self.tabs.push(tab);
        self.active_tab = Some(self.tabs.len() - 1);
        self.focus = Focus::Editor;
        self.preview_scroll = 0;
        self.preview_horizontal_scroll = 0;
        self.preview_selected_link = None;
        self.narrow_pane = Focus::Editor;
        self.restore_active_preview_scroll();
        if show_prompts && !self.ensure_active_mixed_open_prompt(previous_active) {
            self.mark_recent(&path);
            self.offer_recovery(&path);
        }
        Ok(())
    }

    fn ensure_active_mixed_open_prompt(&mut self, previous_active: Option<usize>) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        let Some(tab_index) = self.active_tab else {
            return false;
        };
        let Some(target) = self.tabs.get(tab_index).and_then(|tab| {
            tab.pending_mixed_open
                .then(|| tab.document.mixed_line_ending_target())
                .flatten()
        }) else {
            return false;
        };
        let previous_active = previous_active
            .filter(|index| *index != tab_index && *index < self.tabs.len())
            .or_else(|| {
                (self.tabs.len() > 1).then(|| if tab_index == 0 { 1 } else { tab_index - 1 })
            });
        self.overlay = Some(Overlay::MixedLineEndings {
            tab_index,
            previous_active,
            context: MixedLineEndingContext::Open,
            target,
        });
        self.status_message = Some(format!(
            "Mixed line endings · edit will normalize to {}",
            line_ending_name(target)
        ));
        self.enforce_active_read_only();
        true
    }

    fn save_active(&mut self) {
        self.sync_active_document();
        let Some(index) = self.active_tab else {
            self.status_message = Some("No document to save".to_owned());
            return;
        };
        if !self.tabs[index].document.is_editable() {
            self.status_message =
                Some("Mixed line endings · choose whether to edit first".to_owned());
            return;
        }
        let path = self.tabs[index].document.path.clone();
        match disk_state(&path) {
            DiskState::Current(loaded) => self.save_against_loaded(index, loaded),
            DiskState::Missing => self.show_conflict(index, ConflictKind::Missing),
            DiskState::Unavailable(error) => {
                self.status_message = Some(format!("CONFLICT · file unavailable · {error}"));
                self.show_conflict(index, ConflictKind::Unavailable);
            }
        }
    }

    fn save_against_loaded(&mut self, index: usize, loaded: LoadedFile) {
        let baseline = self.tabs[index].document.snapshot.clone();
        let dirty = self.tabs[index].document.is_dirty();
        let same_content = loaded.snapshot.sha256 == baseline.sha256;
        let same_origin = loaded.snapshot.same_origin(&baseline);
        let safe_baseline = loaded.snapshot == baseline || (same_content && same_origin);

        if !dirty {
            if safe_baseline {
                self.tabs[index].document.snapshot = loaded.snapshot;
                self.tabs[index].document.conflict = false;
                self.status_message = Some("Already saved".to_owned());
            } else {
                let outgoing = self.tabs[index].document.text.clone();
                let history_note = self.capture_history_note(
                    index,
                    &outgoing,
                    CheckpointReason::BeforeExternalReload,
                );
                self.install_loaded(index, loaded, MixedLineEndingContext::Reload);
                self.status_message = Some(append_optional_history_note(
                    "Reloaded external changes".to_owned(),
                    history_note.as_deref(),
                ));
            }
            return;
        }

        if self.tabs[index].document.recovery_conflict || !safe_baseline {
            self.show_conflict(index, ConflictKind::Changed);
            return;
        }

        self.tabs[index].document.snapshot = loaded.snapshot;
        self.tabs[index].document.conflict = false;
        let document = &self.tabs[index].document;
        let result = save_atomic(
            &document.path,
            &document.text,
            document.encoding,
            document.line_ending,
            Some(&document.snapshot),
            false,
        );
        match result {
            Ok(snapshot) => {
                let path = self.tabs[index].document.path.clone();
                let previous_saved = self.tabs[index].document.saved_text.clone();
                let history_note = self.capture_history_note(
                    index,
                    &previous_saved,
                    CheckpointReason::PreviousSavedVersion,
                );
                self.tabs[index].document.mark_saved(snapshot);
                self.status_message = Some(append_optional_history_note(
                    "Saved".to_owned(),
                    history_note.as_deref(),
                ));
                self.discard_recovery_for(&path);
            }
            Err(SaveError::Conflict) => {
                let kind = conflict_kind_for_path(&self.tabs[index].document.path);
                self.show_conflict(index, kind);
            }
            Err(error) => self.status_message = Some(format!("Save failed · {error}")),
        }
    }

    fn install_loaded(
        &mut self,
        index: usize,
        loaded: LoadedFile,
        context: MixedLineEndingContext,
    ) {
        let cursor = self.tabs[index].editor.cursor();
        let editor_scroll_x = self.tabs[index].editor_scroll_x;
        let editor_scroll_y = self.tabs[index].editor_scroll_y;
        let preview_scroll_x = self.tabs[index].preview_scroll_x;
        let preview_scroll_y = self.tabs[index].preview_scroll_y;
        let mut tab = EditorTab::from_loaded(loaded, &self.config.editor);
        tab.document
            .continue_source_revision_from(&self.tabs[index].document);
        let mixed_target = tab.document.mixed_line_ending_target();
        style_cursor(&mut tab.editor, self.mode);
        tab.editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
        tab.editor_scroll_x = editor_scroll_x;
        tab.editor_scroll_y = editor_scroll_y;
        tab.preview_scroll_x = preview_scroll_x;
        tab.preview_scroll_y = preview_scroll_y;
        tab.pending_scroll_restore = true;
        self.tabs[index] = tab;
        if let Some(target) = mixed_target {
            self.overlay = Some(Overlay::MixedLineEndings {
                tab_index: index,
                previous_active: Some(index),
                context,
                target,
            });
            self.enforce_active_read_only();
        }
    }

    fn show_conflict(&mut self, index: usize, kind: ConflictKind) {
        self.tabs[index].document.conflict = true;
        let can_reload = kind == ConflictKind::Changed;
        let allow_continue = self.pending_transition.is_some()
            && !self.tabs[index].document.is_dirty()
            && !can_reload;
        self.overlay = Some(Overlay::Conflict {
            kind,
            can_reload,
            allow_continue,
        });
        self.status_message = Some(match kind {
            ConflictKind::Changed => {
                "CONFLICT · file changed outside TermDraft · use Save As".to_owned()
            }
            ConflictKind::Missing => "CONFLICT · source file is missing · use Save As".to_owned(),
            ConflictKind::Unavailable => {
                "CONFLICT · source file is unavailable · use Save As".to_owned()
            }
        });
    }

    fn set_mode(&mut self, mode: Mode) {
        if mode == Mode::Write
            && self
                .active_tab()
                .is_some_and(|tab| !tab.document.is_editable())
        {
            self.status_message = Some("This document is read-only".to_owned());
            return;
        }
        self.mode = mode;
        if mode == Mode::Write {
            self.focus = Focus::Editor;
            self.narrow_pane = Focus::Editor;
        }
        if let Some(tab) = self.active_tab_mut() {
            style_cursor(&mut tab.editor, mode);
        }
    }

    fn enforce_active_read_only(&mut self) {
        if self.mode != Mode::Write
            || self
                .active_tab()
                .is_none_or(|tab| tab.document.is_editable())
        {
            return;
        }
        self.mode = Mode::Command;
        if let Some(tab) = self.active_tab_mut() {
            style_cursor(&mut tab.editor, Mode::Command);
        }
        self.status_message = Some("Mixed line endings · read-only".to_owned());
    }

    fn accept_mixed_line_endings(
        &mut self,
        tab_index: usize,
        context: MixedLineEndingContext,
        target: LineEnding,
    ) {
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return;
        };
        if !tab.document.accept_mixed_line_endings() {
            return;
        }
        if context == MixedLineEndingContext::Open {
            tab.pending_mixed_open = false;
        }
        let path = tab.document.path.clone();
        self.status_message = Some(format!(
            "Mixed line endings · first edit will normalize to {}",
            line_ending_name(target)
        ));
        if context == MixedLineEndingContext::Open {
            self.mark_recent(&path);
            self.offer_recovery(&path);
        } else if context == MixedLineEndingContext::Reload && self.pending_transition.is_some() {
            self.continue_pending_transition();
        }
    }

    fn cancel_mixed_line_endings(
        &mut self,
        tab_index: usize,
        previous_active: Option<usize>,
        context: MixedLineEndingContext,
    ) {
        if context == MixedLineEndingContext::Open && tab_index < self.tabs.len() {
            self.tabs.remove(tab_index);
            self.active_tab = previous_active
                .and_then(|index| match index.cmp(&tab_index) {
                    std::cmp::Ordering::Less => Some(index),
                    std::cmp::Ordering::Equal => None,
                    std::cmp::Ordering::Greater => Some(index - 1),
                })
                .filter(|index| *index < self.tabs.len())
                .or_else(|| (!self.tabs.is_empty()).then_some(tab_index.min(self.tabs.len() - 1)));
            self.focus = if self.active_tab.is_some() {
                Focus::Editor
            } else {
                Focus::Explorer
            };
            self.status_message = Some("Cancelled opening mixed-line-ending file".to_owned());
            return;
        }
        self.active_tab = (tab_index < self.tabs.len()).then_some(tab_index);
        self.enforce_active_read_only();
        self.status_message = Some("Mixed line endings · kept read-only".to_owned());
        if context == MixedLineEndingContext::Reload && self.pending_transition.is_some() {
            self.continue_pending_transition();
        }
    }

    fn open_conflict_save_as(&mut self) {
        let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) else {
            return;
        };
        let suggestion = conflict_copy_path(&self.workspace, &path);
        self.overlay = Some(Overlay::PathInput {
            action: PathAction::SaveConflictAs,
            input: TextInput {
                cursor: suggestion.chars().count(),
                value: suggestion,
            },
        });
    }

    fn reload_conflict(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        let path = self.tabs[index].document.path.clone();
        match disk_state(&path) {
            DiskState::Current(loaded) => {
                self.install_loaded(index, loaded, MixedLineEndingContext::Reload);
                self.discard_recovery_for(&path);
                let awaiting_mixed_choice = matches!(
                    self.overlay,
                    Some(Overlay::MixedLineEndings {
                        context: MixedLineEndingContext::Reload,
                        ..
                    })
                );
                if self.pending_transition.is_some() && !awaiting_mixed_choice {
                    self.continue_pending_transition();
                } else if self.overlay.is_none() {
                    self.status_message = Some("Reloaded external version".to_owned());
                }
            }
            DiskState::Missing => self.show_conflict(index, ConflictKind::Missing),
            DiskState::Unavailable(error) => {
                self.status_message = Some(format!("CONFLICT · file unavailable · {error}"));
                self.show_conflict(index, ConflictKind::Unavailable);
            }
        }
    }

    fn request_quit(&mut self) {
        self.sync_active_document();
        let original_active_tab = self.active_tab;
        let remaining_tabs = original_active_tab
            .into_iter()
            .chain((0..self.tabs.len()).filter(|index| Some(*index) != original_active_tab))
            .collect();
        self.pending_transition = Some(PendingTransition {
            action: ConfirmAction::Quit,
            accepted_paths: Vec::new(),
            remaining_tabs,
            original_active_tab,
        });
        self.advance_quit_transition();
    }

    fn close_active(&mut self) {
        self.sync_active_document();
        let Some(index) = self.active_tab else {
            return;
        };
        self.pending_transition = Some(PendingTransition {
            action: ConfirmAction::CloseTab,
            accepted_paths: Vec::new(),
            remaining_tabs: Vec::new(),
            original_active_tab: None,
        });
        if self.tabs[index].document.is_dirty() {
            self.overlay = Some(Overlay::Confirm(ConfirmAction::CloseTab));
            return;
        }
        self.validate_pending_transition();
    }

    fn continue_pending_transition(&mut self) {
        let Some(action) = self
            .pending_transition
            .as_ref()
            .map(|pending| pending.action)
        else {
            return;
        };
        if self.active_tab().is_some_and(|tab| tab.document.is_dirty()) {
            self.save_active();
        }
        if self.active_tab().is_some_and(|tab| tab.document.is_dirty()) {
            if self.overlay.is_none() {
                self.overlay = Some(Overlay::Confirm(action));
            }
            return;
        }
        self.validate_pending_transition();
    }

    fn validate_pending_transition(&mut self) {
        let Some(action) = self
            .pending_transition
            .as_ref()
            .map(|pending| pending.action)
        else {
            return;
        };
        if action == ConfirmAction::Quit {
            self.advance_quit_transition();
            return;
        }
        let Some(index) = self.active_tab else {
            self.pending_transition = None;
            return;
        };
        if !self.validate_transition_tab(index) {
            return;
        }
        self.pending_transition = None;
        self.close_active_discarding();
    }

    fn advance_quit_transition(&mut self) {
        loop {
            let Some(pending) = self.pending_transition.as_ref() else {
                return;
            };
            if pending.action != ConfirmAction::Quit {
                return;
            }
            let Some(index) = pending.remaining_tabs.first().copied() else {
                let original_active_tab = pending
                    .original_active_tab
                    .filter(|index| *index < self.tabs.len());
                self.pending_transition = None;
                self.active_tab = original_active_tab;
                self.should_quit = true;
                return;
            };
            if index >= self.tabs.len() {
                if let Some(pending) = self.pending_transition.as_mut() {
                    pending.remaining_tabs.remove(0);
                }
                continue;
            }
            self.active_tab = Some(index);
            if self.tabs[index].document.is_dirty() {
                self.overlay = Some(Overlay::Confirm(ConfirmAction::Quit));
                return;
            }
            if !self.validate_transition_tab(index) {
                return;
            }
            if let Some(pending) = self.pending_transition.as_mut() {
                pending.remaining_tabs.remove(0);
            }
        }
    }

    fn validate_transition_tab(&mut self, index: usize) -> bool {
        let path = self.tabs[index].document.path.clone();
        if self.pending_transition.as_ref().is_some_and(|pending| {
            pending
                .accepted_paths
                .iter()
                .any(|accepted| accepted == &path)
        }) {
            return true;
        }
        match disk_state(&path) {
            DiskState::Current(loaded) => {
                let baseline = &self.tabs[index].document.snapshot;
                if loaded.snapshot == *baseline
                    || (loaded.snapshot.sha256 == baseline.sha256
                        && loaded.snapshot.same_origin(baseline))
                {
                    self.tabs[index].document.snapshot = loaded.snapshot;
                    if !self.tabs[index].document.recovery_conflict {
                        self.tabs[index].document.conflict = false;
                    }
                }
                true
            }
            DiskState::Missing => {
                self.active_tab = Some(index);
                self.show_conflict(index, ConflictKind::Missing);
                false
            }
            DiskState::Unavailable(error) => {
                self.active_tab = Some(index);
                self.status_message = Some(format!("CONFLICT · file unavailable · {error}"));
                self.show_conflict(index, ConflictKind::Unavailable);
                false
            }
        }
    }

    fn continue_without_conflict_copy(&mut self) {
        let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) else {
            return;
        };
        if let Some(pending) = self.pending_transition.as_mut()
            && !pending.accepted_paths.contains(&path)
        {
            pending.accepted_paths.push(path);
        }
        self.validate_pending_transition();
    }

    fn discard_pending_changes(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::CloseTab => {
                if let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) {
                    self.discard_recovery_for(&path);
                }
                self.pending_transition = None;
                self.close_active_discarding();
            }
            ConfirmAction::Quit => {
                let Some(index) = self.active_tab else {
                    self.pending_transition = None;
                    return;
                };
                let path = self.tabs[index].document.path.clone();
                let text = self.tabs[index].document.text.clone();
                self.discard_recovery_for(&path);
                self.tabs[index].document.saved_text = text;
                self.tabs[index].document.conflict = false;
                self.tabs[index].document.recovery_conflict = false;
                if let Some(pending) = self.pending_transition.as_mut()
                    && !pending.accepted_paths.contains(&path)
                {
                    pending.accepted_paths.push(path);
                }
                self.advance_quit_transition();
            }
        }
    }

    fn close_active_discarding(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        let closed_path = self.tabs[index].document.path.clone();
        self.cache_active_view();
        self.tabs.remove(index);
        self.clear_agent_proposal_for_path(&closed_path);
        self.active_tab = if self.tabs.is_empty() {
            None
        } else {
            Some(index.min(self.tabs.len() - 1))
        };
        if let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) {
            self.mark_recent(&path);
        }
    }

    fn switch_tab(&mut self, direction: isize) {
        self.sync_active_document();
        let Some(current) = self.active_tab else {
            return;
        };
        if self.tabs.len() < 2 {
            return;
        }
        let len = isize::try_from(self.tabs.len()).unwrap_or(isize::MAX);
        let next = (isize::try_from(current).unwrap_or_default() + direction).rem_euclid(len);
        if let Ok(next) = usize::try_from(next) {
            self.activate_tab(next);
        }
    }

    fn activate_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || self.active_tab == Some(index) {
            return;
        }
        self.sync_active_document();
        let previous = self.active_tab;
        self.cache_active_view();
        self.active_tab = Some(index);
        if let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) {
            self.mark_recent(&path);
        }
        self.preview_scroll = 0;
        self.preview_horizontal_scroll = 0;
        self.preview_selected_link = None;
        self.narrow_pane = Focus::Editor;
        self.restore_active_preview_scroll();
        self.ensure_active_mixed_open_prompt(previous);
        self.enforce_active_read_only();
    }

    fn move_editor(&mut self, movement: CursorMove) {
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(movement);
        }
    }

    fn execute_binding_action(&mut self, action: BindingAction) {
        match action {
            BindingAction::Save => self.execute_command(CommandAction::Save),
            BindingAction::SaveAs => self.execute_command(CommandAction::SaveAs),
            BindingAction::Quit => self.execute_command(CommandAction::Quit),
            BindingAction::ToggleExplorer => {
                self.execute_command(CommandAction::ToggleExplorer);
            }
            BindingAction::FindFile => self.execute_command(CommandAction::FileFinder),
            BindingAction::RecentDocuments => {
                self.execute_command(CommandAction::RecentDocuments);
            }
            BindingAction::NextTab => self.execute_command(CommandAction::NextTab),
            BindingAction::PreviousTab => self.execute_command(CommandAction::PreviousTab),
            BindingAction::CloseTab => self.execute_command(CommandAction::CloseTab),
            BindingAction::FindReplace => self.execute_command(CommandAction::Find),
            BindingAction::SearchText => self.execute_command(CommandAction::WorkspaceSearch),
            BindingAction::DocumentOutline => self.execute_command(CommandAction::Outline),
            BindingAction::TogglePreview => self.execute_command(CommandAction::TogglePreview),
            BindingAction::PreviewNextHeading => {
                self.trace_command("PreviewNextHeading");
                self.focus_preview_heading(1);
            }
            BindingAction::PreviewPreviousHeading => {
                self.trace_command("PreviewPreviousHeading");
                self.focus_preview_heading(-1);
            }
            BindingAction::Undo => self.execute_command(CommandAction::Undo),
            BindingAction::Redo => self.execute_command(CommandAction::Redo),
            BindingAction::ShowHelp => self.execute_command(CommandAction::Help),
            BindingAction::CommandPalette => {
                self.trace_command("CommandPalette");
                self.overlay = Some(Overlay::Palette {
                    input: TextInput::default(),
                    selected: 0,
                });
            }
            BindingAction::EnterWriteMode => self.execute_command(CommandAction::WriteMode),
            BindingAction::ChangeTheme => self.execute_command(CommandAction::ChangeTheme),
            BindingAction::DuplicateDocument => self.execute_command(CommandAction::Duplicate),
            BindingAction::ReloadConfig => self.execute_command(CommandAction::ReloadConfig),
            BindingAction::ManageRecovery => self.execute_command(CommandAction::ManageRecovery),
            BindingAction::MarkdownHelp => self.execute_command(CommandAction::MarkdownHelp),
            BindingAction::InspectSemanticBlocks => {
                self.execute_command(CommandAction::InspectSemanticBlocks);
            }
            BindingAction::ReadSemanticBlocks => {
                self.execute_command(CommandAction::ReadSemanticBlocks);
            }
            BindingAction::InspectCursorCoordinates => {
                self.execute_command(CommandAction::InspectCursorCoordinates);
            }
            BindingAction::CursorLeft => {
                self.trace_command("CursorLeft");
                self.move_editor(CursorMove::Back);
            }
            BindingAction::CursorDown => {
                self.trace_command("CursorDown");
                self.move_editor(CursorMove::Down);
            }
            BindingAction::CursorUp => {
                self.trace_command("CursorUp");
                self.move_editor(CursorMove::Up);
            }
            BindingAction::CursorRight => {
                self.trace_command("CursorRight");
                self.move_editor(CursorMove::Forward);
            }
            BindingAction::LineStart => {
                self.trace_command("LineStart");
                self.move_editor(CursorMove::Head);
            }
            BindingAction::LineEnd => {
                self.trace_command("LineEnd");
                self.move_editor(CursorMove::End);
            }
            BindingAction::DocumentStart => {
                self.trace_command("DocumentStart");
                self.move_editor(CursorMove::Top);
            }
            BindingAction::DocumentEnd => {
                self.trace_command("DocumentEnd");
                self.move_editor(CursorMove::Bottom);
            }
        }
    }

    fn focus_preview_heading(&mut self, direction: isize) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let headings = heading_outline(&tab.document.text);
        if headings.is_empty() {
            self.status_message = Some("No headings in this document".to_owned());
            return;
        }
        let current = headings
            .iter()
            .enumerate()
            .filter(|(_, (line, _, _))| *line <= usize::from(self.preview_scroll))
            .map(|(index, _)| index)
            .next_back();
        let selected = if direction < 0 {
            current.unwrap_or(headings.len()).saturating_sub(1)
        } else {
            current.map_or(0, |index| (index + 1).min(headings.len() - 1))
        };
        let (line, level, title) = &headings[selected];
        self.preview_selected_link = None;
        self.preview_scroll = u16::try_from(*line)
            .unwrap_or(u16::MAX)
            .min(self.preview_max_scroll);
        self.status_message = Some(format!(
            "H{level} {}/{} · {title}",
            selected + 1,
            headings.len()
        ));
    }

    fn reload_config(&mut self) {
        if self.config.root.as_os_str().is_empty() {
            self.status_message = Some("No user configuration was loaded".to_owned());
            return;
        }
        match config::load(self.config.root.clone()) {
            Ok(config) => {
                for tab in &mut self.tabs {
                    apply_editor_config(&mut tab.editor, &config.editor);
                    apply_editor_config(&mut tab.inline_editor, &config.editor);
                    style_cursor(&mut tab.editor, self.mode);
                    style_cursor(&mut tab.inline_editor, self.mode);
                }
                self.view_mode = match config.editor.view_mode {
                    StartupView::Inline => ViewMode::Inline,
                    StartupView::Split => ViewMode::Split,
                };
                if self.view_mode == ViewMode::Inline && self.narrow_pane == Focus::Preview {
                    self.narrow_pane = Focus::Editor;
                    self.focus = Focus::Editor;
                }
                self.theme = config.theme;
                self.config = config;
                self.status_message = Some("Reloaded config.toml".to_owned());
            }
            Err(error) => {
                self.status_message = Some(format!("Configuration not reloaded · {error}"));
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn execute_command(&mut self, action: CommandAction) {
        self.trace_command(&format!("{action:?}"));
        self.overlay = None;
        match action {
            CommandAction::Save => self.save_active(),
            CommandAction::SaveAs => self.open_path_input(PathAction::SaveAs),
            CommandAction::Duplicate => self.open_path_input(PathAction::Duplicate),
            CommandAction::Create => self.open_workspace_input(WorkspaceInputAction::Create),
            CommandAction::CopyEntry => self.set_workspace_clipboard(false),
            CommandAction::CutEntry => self.set_workspace_clipboard(true),
            CommandAction::PasteEntry => self.paste_workspace_entry(),
            CommandAction::RenameEntry => self.open_workspace_input(WorkspaceInputAction::Rename),
            CommandAction::MoveEntry => self.open_workspace_input(WorkspaceInputAction::Move),
            CommandAction::TrashEntry => self.request_trash_entry(),
            CommandAction::CloseTab => self.close_active(),
            CommandAction::Quit => self.request_quit(),
            CommandAction::FileFinder => {
                let _ = self.poll_external_state();
                self.overlay = Some(Overlay::FileFinder {
                    query: TextInput::default(),
                    filter: TextInput::default(),
                    focus: FileFinderFocus::Query,
                    selected: 0,
                    error: None,
                });
            }
            CommandAction::RecentDocuments => self.open_recent_documents(),
            CommandAction::NextTab => self.switch_tab(1),
            CommandAction::PreviousTab => self.switch_tab(-1),
            CommandAction::WorkspaceSearch => {
                self.sync_active_document();
                self.workspace_search_revision
                    .fetch_add(1, Ordering::Relaxed);
                self.overlay = Some(Overlay::WorkspaceSearch {
                    query: TextInput::default(),
                    filter: TextInput::default(),
                    mode: TextSearchMode::Literal,
                    case_sensitive: false,
                    focus: WorkspaceSearchFocus::Query,
                    results: Vec::new(),
                    selected: 0,
                    status: "Enter a query to search Markdown source.".to_owned(),
                });
            }
            CommandAction::Find => self.open_document_find(),
            CommandAction::Outline => self.open_outline(),
            CommandAction::ToggleExplorer => {
                self.show_explorer = !self.show_explorer;
                self.focus = if self.show_explorer {
                    Focus::Explorer
                } else {
                    Focus::Editor
                };
            }
            CommandAction::TogglePreview => self.toggle_preview(),
            CommandAction::WriteMode => self.set_mode(Mode::Write),
            CommandAction::CommandMode => self.set_mode(Mode::Command),
            CommandAction::Undo => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.undo();
                }
            }
            CommandAction::Redo => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.redo();
                }
            }
            CommandAction::CreateCheckpoint => self.create_manual_checkpoint(),
            CommandAction::LocalHistory => self.open_local_history(),
            CommandAction::AgentSharing => self.open_agent_sharing(),
            CommandAction::ReloadConfig => self.reload_config(),
            CommandAction::ChangeTheme => {
                self.theme = self.theme.next();
                let mode = if self.theme.is_light() {
                    "light"
                } else {
                    "dark"
                };
                self.status_message = Some(match self.config.save_theme_choice(self.theme) {
                    Ok(()) => format!("Theme · {} ({mode})", self.theme.name()),
                    Err(error) => {
                        format!(
                            "Theme · {} ({mode}) · not saved: {error}",
                            self.theme.name()
                        )
                    }
                });
            }
            CommandAction::ManageRecovery => self.open_recovery_manager(),
            CommandAction::MarkdownHelp => {
                self.overlay = Some(Overlay::MarkdownHelp { scroll: 0 });
            }
            CommandAction::InspectSemanticBlocks => self.open_semantic_inspector(),
            CommandAction::ReadSemanticBlocks => self.open_semantic_reader(),
            CommandAction::InspectCursorCoordinates => self.open_coordinate_inspector(),
            CommandAction::Help => {
                self.overlay = Some(Overlay::Help {
                    scroll: 0,
                    max_scroll: 0,
                });
            }
        }
    }

    fn open_semantic_inspector(&mut self) {
        self.sync_active_document();
        let Some(source) = self.active_tab().map(|tab| tab.document.text.clone()) else {
            self.status_message = Some("Open a Markdown document first".to_owned());
            return;
        };
        self.overlay = Some(Overlay::SemanticInspector {
            mapping: map_semantic_blocks(&source),
            selected: 0,
        });
    }

    fn open_semantic_reader(&mut self) {
        self.sync_active_document();
        let Some(source) = self.active_tab().map(|tab| tab.document.text.clone()) else {
            self.status_message = Some("Open a Markdown document first".to_owned());
            return;
        };
        self.overlay = Some(Overlay::SemanticReader {
            mapping: map_semantic_blocks(&source),
            scroll: 0,
        });
    }

    fn open_coordinate_inspector(&mut self) {
        self.sync_active_document();
        let Some(tab) = self.active_tab() else {
            self.status_message = Some("Open a Markdown document first".to_owned());
            return;
        };
        let source = tab.document.text.clone();
        let cursor = tab.editor.cursor();
        let line_count = tab.editor.lines().len();
        let tab_width = usize::from(tab.editor.tab_length());
        let screen_position = tab
            .editor
            .rendered_cursor_position()
            .map(|position| (position.y, position.x));
        let wrap_width = self.editor_wrap_width(line_count);
        match diagnose_coordinate(&source, cursor, wrap_width, tab_width) {
            Ok(diagnostic) => {
                self.overlay = Some(Overlay::CoordinateInspector {
                    diagnostic,
                    screen_position,
                });
            }
            Err(error) => {
                self.status_message = Some(format!("Cannot inspect cursor coordinates · {error}"));
            }
        }
    }

    fn editor_wrap_width(&self, line_count: usize) -> usize {
        if !self.config.editor.soft_wrap {
            return 0;
        }
        let editor_width = self
            .ui_regions
            .editor
            .map_or(self.viewport_width, |area| area.width)
            .min(108);
        let line_number_width = if self.config.editor.show_line_numbers {
            line_count.max(1).ilog10() as usize + 3
        } else {
            0
        };
        usize::from(editor_width)
            .saturating_sub(line_number_width)
            .max(1)
    }

    fn open_outline(&mut self) {
        self.sync_active_document();
        let Some(tab) = self.active_tab() else {
            self.status_message = Some("No document open".to_owned());
            return;
        };
        let items = heading_outline(&tab.document.text);
        if items.is_empty() {
            self.status_message = Some("No headings in this document".to_owned());
        } else {
            self.overlay = Some(Overlay::Outline {
                matches: items.clone(),
                items,
                query: TextInput::default(),
                selected: 0,
            });
        }
    }

    fn jump_to_outline_source(&mut self, line: usize) {
        self.narrow_pane = Focus::Editor;
        self.focus = Focus::Editor;
        self.move_editor(CursorMove::Jump(u16::try_from(line).unwrap_or(u16::MAX), 0));
    }

    fn show_outline_in_preview(&mut self, line: usize, level: usize, title: &str) {
        let Some(source) = self.active_tab().map(|tab| tab.document.text.clone()) else {
            return;
        };
        let rendered = render_markdown_document(&source, None);
        let width = self.preview_content_width();
        let rendered_line = rendered
            .source_lines
            .iter()
            .position(|source_line| *source_line == line)
            .unwrap_or_default();
        let visual_row = rendered
            .text
            .lines
            .iter()
            .take(rendered_line)
            .map(|preview_line| ui::preview_line_height(preview_line, width))
            .sum::<usize>();
        self.preview_visible = true;
        self.narrow_pane = Focus::Preview;
        self.focus = Focus::Preview;
        self.preview_selected_link = None;
        self.preview_scroll = u16::try_from(visual_row).unwrap_or(u16::MAX);
        self.status_message = Some(format!("H{level} · {title}"));
    }

    fn open_document_find(&mut self) {
        self.sync_active_document();
        let Some(tab) = self.active_tab() else {
            self.status_message = Some("Open a Markdown document first".to_owned());
            return;
        };
        let source = tab.document.text.clone();
        let anchor_offset = location_to_offset(&source, tab.editor.cursor());
        let read_only = !tab.document.is_editable();
        self.overlay = Some(Overlay::Find {
            query: TextInput::default(),
            replacement: TextInput::default(),
            case_sensitive: false,
            focus: FindFocus::Query,
            source,
            matches: Vec::new(),
            selected: None,
            anchor_offset,
            read_only,
        });
    }

    fn select_document_match(&mut self, source_match: DocumentSearchMatch) {
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        let source = source_from_textarea(&tab.editor);
        let start = offset_to_location(&source, source_match.start);
        let end = offset_to_location(&source, source_match.end);
        tab.editor.move_cursor(CursorMove::Jump(
            u16::try_from(start.0).unwrap_or(u16::MAX),
            u16::try_from(start.1).unwrap_or(u16::MAX),
        ));
        tab.editor.start_selection();
        tab.editor.move_cursor(CursorMove::Jump(
            u16::try_from(end.0).unwrap_or(u16::MAX),
            u16::try_from(end.1).unwrap_or(u16::MAX),
        ));
    }

    fn replace_document_match(
        &mut self,
        expected_source: &str,
        source_match: DocumentSearchMatch,
        replacement: &str,
    ) -> (String, usize) {
        self.sync_active_document();
        let Some(tab) = self.active_tab_mut() else {
            return (expected_source.to_owned(), 0);
        };
        if tab.document.text != expected_source || !tab.document.is_editable() {
            let anchor = location_to_offset(&tab.document.text, tab.editor.cursor());
            return (tab.document.text.clone(), anchor);
        }
        tab.replace_range(source_match, replacement);
        let anchor = source_match.start + replacement.chars().count();
        let updated = tab.document.text.clone();
        self.status_message = Some("Replaced one match".to_owned());
        (updated, anchor)
    }

    fn replace_all_document_matches(
        &mut self,
        expected_source: &str,
        matches: &[DocumentSearchMatch],
        replacement: &str,
    ) -> (String, usize) {
        self.sync_active_document();
        let Some(tab) = self.active_tab_mut() else {
            return (expected_source.to_owned(), 0);
        };
        if tab.document.text != expected_source || !tab.document.is_editable() {
            let anchor = location_to_offset(&tab.document.text, tab.editor.cursor());
            return (tab.document.text.clone(), anchor);
        }
        let Ok(replaced) = replace_document_matches(expected_source, matches, replacement) else {
            self.status_message = Some("Replace all failed · invalid match ranges".to_owned());
            return (expected_source.to_owned(), 0);
        };
        let changed = replaced != expected_source;
        let result = changed.then(|| tab.replace_all(&replaced));
        let updated = tab.document.text.clone();
        match result {
            Some(Ok(true)) => {
                self.status_message = Some(format!("Replaced {} matches", matches.len()));
            }
            Some(Err(SourceTransactionError::Protected)) => {
                self.status_message =
                    Some("Replace all blocked · document is protected".to_owned());
            }
            Some(Err(SourceTransactionError::Stale(_))) => {
                self.status_message = Some("Replace all canceled · source changed".to_owned());
            }
            Some(Ok(false)) | None => {}
        }
        (updated, 0)
    }

    #[must_use]
    pub fn is_narrow(&self) -> bool {
        self.viewport_width < 100
    }

    #[must_use]
    pub fn editor_is_visible(&self) -> bool {
        !(self.preview_visible
            && (self.view_mode == ViewMode::Inline || self.is_narrow())
            && self.narrow_pane == Focus::Preview)
    }

    #[must_use]
    pub fn preview_is_visible(&self) -> bool {
        if !self.preview_visible {
            return false;
        }
        if self.view_mode == ViewMode::Inline || self.is_narrow() {
            self.narrow_pane == Focus::Preview
        } else {
            true
        }
    }

    pub fn update_viewport_width(&mut self, width: u16) {
        let was_narrow = self.is_narrow();
        self.viewport_width = width;
        if self.is_narrow() && !was_narrow {
            self.narrow_pane = Focus::Editor;
            if self.focus == Focus::Preview {
                self.focus = Focus::Editor;
            }
        }
    }

    fn toggle_preview(&mut self) {
        if self.active_tab().is_none() {
            self.status_message = Some("No document open".to_owned());
            return;
        }
        let opening_preview = !self.preview_is_visible();
        let editor_line = self
            .active_tab()
            .map(|tab| u16::try_from(tab.editor.cursor().0).unwrap_or(u16::MAX));
        if self.view_mode == ViewMode::Inline || self.is_narrow() {
            self.preview_visible = true;
            self.narrow_pane = if self.narrow_pane == Focus::Preview {
                Focus::Editor
            } else {
                Focus::Preview
            };
        } else {
            self.preview_visible = !self.preview_visible;
        }
        if opening_preview && let Some(line) = editor_line {
            self.preview_scroll = line;
        }
        self.focus = if self.preview_is_visible() {
            Focus::Preview
        } else {
            Focus::Editor
        };
    }

    fn scroll_preview_by(&mut self, amount: i32) {
        let next = i32::from(self.preview_scroll).saturating_add(amount);
        self.preview_scroll = u16::try_from(next.clamp(0, i32::from(self.preview_max_scroll)))
            .unwrap_or(self.preview_max_scroll);
    }

    fn scroll_preview_horizontally_by(&mut self, amount: i32) {
        let next = i32::from(self.preview_horizontal_scroll).saturating_add(amount);
        self.preview_horizontal_scroll =
            u16::try_from(next.clamp(0, i32::from(self.preview_horizontal_max_scroll)))
                .unwrap_or(self.preview_horizontal_max_scroll);
    }

    fn handle_preview_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return false;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.trace_command("PreviewUp");
                self.scroll_preview_by(-1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.trace_command("PreviewDown");
                self.scroll_preview_by(1);
            }
            KeyCode::PageUp => {
                self.trace_command("PreviewPageUp");
                self.scroll_preview_by(-i32::from(self.preview_page));
            }
            KeyCode::PageDown => {
                self.trace_command("PreviewPageDown");
                self.scroll_preview_by(i32::from(self.preview_page));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.trace_command("PreviewStart");
                self.preview_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.trace_command("PreviewEnd");
                self.preview_scroll = self.preview_max_scroll;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.trace_command("PreviewLeft");
                self.scroll_preview_horizontally_by(-1);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.trace_command("PreviewRight");
                self.scroll_preview_horizontally_by(1);
            }
            KeyCode::Char('0') => {
                self.trace_command("PreviewLeftEdge");
                self.preview_horizontal_scroll = 0;
            }
            KeyCode::Char('$') => {
                self.trace_command("PreviewRightEdge");
                self.preview_horizontal_scroll = self.preview_horizontal_max_scroll;
            }
            KeyCode::Tab => {
                self.trace_command("PreviewNextLink");
                self.select_preview_link(1);
            }
            KeyCode::BackTab => {
                self.trace_command("PreviewPreviousLink");
                self.select_preview_link(-1);
            }
            KeyCode::Enter => {
                self.trace_command("ActivatePreviewLink");
                self.activate_preview_link();
            }
            _ => return false,
        }
        true
    }

    fn select_preview_link(&mut self, direction: isize) {
        let Some(source) = self.active_tab().map(|tab| tab.document.text.clone()) else {
            return;
        };
        let rendered = render_markdown_document(&source, None);
        if rendered.links.is_empty() {
            self.preview_selected_link = None;
            self.status_message = Some("No links in this preview".to_owned());
            return;
        }
        let count = rendered.links.len();
        let selected = match (self.preview_selected_link, direction < 0) {
            (Some(0) | None, true) => count - 1,
            (Some(index), true) => index.saturating_sub(1),
            (Some(index), false) => (index + 1) % count,
            (None, false) => 0,
        };
        self.preview_selected_link = Some(selected);
        self.reveal_preview_link(&rendered, selected);
        self.status_message = Some(format!("Preview link {}/{}", selected + 1, count));
    }

    fn activate_preview_link(&mut self) {
        let Some(selected) = self.preview_selected_link else {
            return;
        };
        let Some(source) = self.active_tab().map(|tab| tab.document.text.clone()) else {
            return;
        };
        let rendered = render_markdown_document(&source, None);
        let Some(link) = rendered.links.get(selected) else {
            self.preview_selected_link = None;
            return;
        };
        match &link.target {
            PreviewLinkTarget::External(_) => {
                self.status_message =
                    Some("External links are intentionally inert in preview".to_owned());
            }
            PreviewLinkTarget::FootnoteDefinition(label) => {
                if let Some(index) = rendered.links.iter().position(|candidate| {
                    candidate.target == PreviewLinkTarget::FootnoteBackReference(label.clone())
                }) {
                    self.preview_selected_link = Some(index);
                    self.reveal_preview_link(&rendered, index);
                    self.status_message = Some(format!("Footnote {label}"));
                }
            }
            PreviewLinkTarget::FootnoteBackReference(label) => {
                if let Some(index) = rendered.links.iter().rposition(|candidate| {
                    candidate.target == PreviewLinkTarget::FootnoteDefinition(label.clone())
                }) {
                    self.preview_selected_link = Some(index);
                    self.reveal_preview_link(&rendered, index);
                    self.status_message = Some(format!("Back to footnote {label} reference"));
                }
            }
        }
    }

    fn reveal_preview_link(&mut self, rendered: &RenderedMarkdown, index: usize) {
        let Some(link) = rendered.links.get(index) else {
            return;
        };
        let width = self.preview_content_width();
        let visual_row = preview_link_visual_row(rendered, link, width);
        let page = usize::from(self.preview_page.max(1));
        let current = usize::from(self.preview_scroll);
        let next = if visual_row < current {
            visual_row
        } else if visual_row >= current + page {
            visual_row.saturating_sub(page - 1)
        } else {
            current
        };
        self.preview_scroll = u16::try_from(next)
            .unwrap_or(u16::MAX)
            .min(self.preview_max_scroll);
    }

    fn preview_content_width(&self) -> usize {
        self.ui_regions
            .preview
            .map(ui::preview_content_area)
            .map_or_else(
                || usize::from(self.viewport_width.max(1)),
                |area| usize::from(area.width.max(1)),
            )
    }

    fn preview_link_at_position(&self, area: Rect, column: u16, row: u16) -> Option<usize> {
        if area.is_empty()
            || column < area.x
            || column >= area.right()
            || row < area.y
            || row >= area.bottom()
        {
            return None;
        }
        let source = &self.active_tab()?.document.text;
        let rendered = render_markdown_document(source, None);
        let width = usize::from(area.width.max(1));
        let target_row = usize::from(self.preview_scroll) + usize::from(row.saturating_sub(area.y));
        let mut visual_row = 0;
        for (rendered_line, line) in rendered.text.lines.iter().enumerate() {
            let height = ui::preview_line_height(line, width);
            if target_row < visual_row + height {
                let wrapped_row = target_row.saturating_sub(visual_row);
                let target_column =
                    wrapped_row * width + usize::from(column.saturating_sub(area.x));
                return rendered.links.iter().position(|link| {
                    link.rendered_line == rendered_line
                        && target_column >= link.start_column
                        && target_column < link.end_column
                });
            }
            visual_row += height;
        }
        None
    }

    fn open_recent_documents(&mut self) {
        self.cache_active_view();
        let previous_len = self.recent_paths.len();
        self.recent_paths
            .retain(|path| match fs::symlink_metadata(path) {
                Ok(metadata) => metadata.is_file() && !metadata.file_type().is_symlink(),
                Err(error) => error.kind() != io::ErrorKind::NotFound,
            });
        if self.recent_paths.len() != previous_len {
            self.session_views
                .retain(|path, _| self.recent_paths.contains(path));
            self.persist_session_if_changed();
        }
        if self.recent_paths.is_empty() {
            self.status_message = Some("No recent Markdown documents are available".to_owned());
            return;
        }
        self.overlay = Some(Overlay::RecentDocuments {
            paths: self.recent_paths.clone(),
            selected: 0,
        });
    }

    fn selected_workspace_path(&self, fallback_to_document: bool) -> PathBuf {
        self.selected_explorer_entry()
            .map(|entry| entry.path.clone())
            .or_else(|| {
                fallback_to_document
                    .then(|| self.active_tab().map(|tab| tab.document.path.clone()))
                    .flatten()
            })
            .unwrap_or_else(|| self.workspace.root.clone())
    }

    fn open_workspace_input(&mut self, action: WorkspaceInputAction) {
        self.sync_active_document();
        let selected = self.selected_workspace_path(true);
        let (source, suggested) = match action {
            WorkspaceInputAction::Create => {
                let parent = if selected.is_dir() {
                    selected
                } else {
                    selected
                        .parent()
                        .unwrap_or(&self.workspace.root)
                        .to_path_buf()
                };
                (parent, String::new())
            }
            WorkspaceInputAction::Rename => {
                if selected == self.workspace.root {
                    self.status_message = Some("Select a file or folder first".to_owned());
                    return;
                }
                let name = selected
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                (selected, name)
            }
            WorkspaceInputAction::Move => {
                if selected == self.workspace.root {
                    self.status_message = Some("Select a file or folder first".to_owned());
                    return;
                }
                let relative = self.workspace.relative(&selected).display().to_string();
                (selected, relative)
            }
        };
        let mut input = TextInput::default();
        input.insert(&suggested);
        self.overlay = Some(Overlay::WorkspaceInput {
            action,
            source,
            input,
        });
    }

    fn set_workspace_clipboard(&mut self, cut: bool) {
        let source = self.selected_workspace_path(false);
        if source == self.workspace.root {
            self.status_message = Some("Select a file or folder first".to_owned());
            return;
        }
        self.workspace_clipboard = Some(WorkspaceClipboard {
            source: source.clone(),
            cut,
        });
        let verb = if cut { "Cut" } else { "Copied" };
        self.status_message = Some(format!(
            "{verb} {} · select a destination and press p",
            self.workspace.relative(&source).display()
        ));
    }

    fn paste_workspace_entry(&mut self) {
        let Some(clipboard) = self.workspace_clipboard.clone() else {
            self.status_message = Some("Copy or cut a file or folder first".to_owned());
            return;
        };
        if !clipboard.source.exists() {
            self.workspace_clipboard = None;
            self.status_message = Some("Cannot paste · source no longer exists".to_owned());
            return;
        }
        let selected = self.selected_workspace_path(false);
        let parent = if selected.is_dir() {
            selected
        } else {
            selected
                .parent()
                .unwrap_or(&self.workspace.root)
                .to_path_buf()
        };
        let Some(name) = clipboard.source.file_name() else {
            self.status_message = Some("Cannot paste the workspace root".to_owned());
            return;
        };
        let target = parent.join(name);
        self.sync_active_document();
        if clipboard.cut {
            if let Err(error) = self.check_path_change(&clipboard.source, &target) {
                self.status_message = Some(error);
                return;
            }
            match move_entry(&self.workspace, &clipboard.source, &target) {
                Ok(target) => {
                    let history_note = self.retarget_workspace_paths(&clipboard.source, &target);
                    self.workspace_clipboard = None;
                    self.status_message = Some(append_optional_history_note(
                        format!("Moved to {}", self.workspace.relative(&target).display()),
                        history_note.as_deref(),
                    ));
                    self.refresh_entries(Some(&target));
                }
                Err(error) => self.status_message = Some(format!("Cannot paste · {error}")),
            }
        } else {
            match copy_entry(&self.workspace, &clipboard.source, &target) {
                Ok(target) => {
                    self.status_message = Some(format!(
                        "Copied to {}",
                        self.workspace.relative(&target).display()
                    ));
                    self.refresh_entries(Some(&target));
                }
                Err(error) => self.status_message = Some(format!("Cannot paste · {error}")),
            }
        }
    }

    fn apply_workspace_input(
        &mut self,
        action: WorkspaceInputAction,
        source: &Path,
        value: &str,
    ) -> bool {
        let value = value.trim();
        if value.is_empty() {
            self.status_message = Some("Enter a file or folder path".to_owned());
            return false;
        }
        match action {
            WorkspaceInputAction::Create => {
                let folder = value.ends_with('/');
                let relative = created_entry_path(value.trim_end_matches('/'), folder);
                if relative.as_os_str().is_empty() {
                    self.status_message = Some("Enter a file or folder path".to_owned());
                    return false;
                }
                let result = if folder {
                    create_folder(&self.workspace, source, &relative)
                } else {
                    create_file(&self.workspace, source, &relative)
                };
                match result {
                    Ok(target) => {
                        self.refresh_entries(Some(&target));
                        if !folder && has_editable_suffix(&target) {
                            if let Err(error) = self.open_document(&target) {
                                self.status_message =
                                    Some(format!("Created but cannot open · {error}"));
                                return true;
                            }
                        } else {
                            self.focus = Focus::Explorer;
                        }
                        self.status_message = Some(if folder {
                            format!(
                                "Created folder {}",
                                self.workspace.relative(&target).display()
                            )
                        } else {
                            format!("Created {}", self.workspace.relative(&target).display())
                        });
                        true
                    }
                    Err(error) => {
                        self.status_message = Some(format!("Cannot create · {error}"));
                        false
                    }
                }
            }
            WorkspaceInputAction::Rename => {
                let Some(parent) = source.parent() else {
                    self.status_message = Some("Cannot rename the workspace root".to_owned());
                    return false;
                };
                let target = parent.join(value);
                if let Err(error) = self.check_path_change(source, &target) {
                    self.status_message = Some(error);
                    return false;
                }
                match rename_entry(&self.workspace, source, Path::new(value).as_os_str()) {
                    Ok(target) => {
                        let history_note = self.retarget_workspace_paths(source, &target);
                        self.refresh_entries(Some(&target));
                        self.status_message = Some(append_optional_history_note(
                            format!("Renamed to {}", self.workspace.relative(&target).display()),
                            history_note.as_deref(),
                        ));
                        true
                    }
                    Err(error) => {
                        self.status_message = Some(format!("Cannot rename · {error}"));
                        false
                    }
                }
            }
            WorkspaceInputAction::Move => {
                let requested = PathBuf::from(value);
                let target = if requested.is_absolute() {
                    requested
                } else {
                    self.workspace.root.join(requested)
                };
                if let Err(error) = self.check_path_change(source, &target) {
                    self.status_message = Some(error);
                    return false;
                }
                match move_entry(&self.workspace, source, &target) {
                    Ok(target) => {
                        let history_note = self.retarget_workspace_paths(source, &target);
                        self.refresh_entries(Some(&target));
                        self.status_message = Some(append_optional_history_note(
                            format!("Moved to {}", self.workspace.relative(&target).display()),
                            history_note.as_deref(),
                        ));
                        true
                    }
                    Err(error) => {
                        self.status_message = Some(format!("Cannot move · {error}"));
                        false
                    }
                }
            }
        }
    }

    fn check_path_change(&self, source: &Path, target: &Path) -> Result<(), String> {
        let affected = self
            .tabs
            .iter()
            .filter(|tab| path_is_within(&tab.document.path, source))
            .collect::<Vec<_>>();
        if affected.iter().any(|tab| tab.document.is_dirty()) {
            return Err(
                "Save or close open documents inside this path before changing it".to_owned(),
            );
        }
        for tab in &affected {
            let loaded = load_file(&tab.document.path).map_err(|error| {
                format!("Cannot verify {} · {error}", tab.document.path.display())
            })?;
            if loaded.snapshot.sha256 != tab.document.snapshot.sha256
                || !loaded.snapshot.same_origin(&tab.document.snapshot)
            {
                return Err(format!(
                    "{} changed on disk; reload or close it before changing its path",
                    tab.document.path.display()
                ));
            }
        }
        let moved_paths = affected
            .iter()
            .map(|tab| retargeted_path(&tab.document.path, source, target))
            .collect::<Vec<_>>();
        if self.tabs.iter().any(|tab| {
            !path_is_within(&tab.document.path, source) && moved_paths.contains(&tab.document.path)
        }) {
            return Err("The destination is already reserved by an open document".to_owned());
        }
        Ok(())
    }

    fn retarget_workspace_paths(&mut self, source: &Path, target: &Path) -> Option<String> {
        self.clear_agent_proposal_within(source);
        let previous_paths = self
            .tabs
            .iter()
            .filter(|tab| path_is_within(&tab.document.path, source))
            .map(|tab| tab.document.path.clone())
            .collect::<Vec<_>>();
        for previous in &previous_paths {
            self.discard_recovery_for(previous);
        }
        for tab in &mut self.tabs {
            if !path_is_within(&tab.document.path, source) {
                continue;
            }
            let moved = retargeted_path(&tab.document.path, source, target);
            match load_file(&moved) {
                Ok(loaded)
                    if loaded.text == tab.document.saved_text
                        && loaded.snapshot.same_origin(&tab.document.snapshot) =>
                {
                    tab.document.path = moved;
                    tab.document.snapshot = loaded.snapshot;
                    tab.document.conflict = false;
                }
                _ => {
                    tab.document.path = moved;
                    tab.document.conflict = true;
                }
            }
        }
        let mut retargeted_recent = Vec::with_capacity(self.recent_paths.len());
        for path in &self.recent_paths {
            let path = retargeted_path(path, source, target);
            if !retargeted_recent.contains(&path) {
                retargeted_recent.push(path);
            }
        }
        self.recent_paths = retargeted_recent;
        self.session_views = std::mem::take(&mut self.session_views)
            .into_values()
            .map(|mut view| {
                view.path = retargeted_path(&view.path, source, target);
                (view.path.clone(), view)
            })
            .collect();
        if let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) {
            self.mark_recent(&path);
        }
        self.persist_session_if_changed();
        self.checkpoint_store.as_ref().and_then(|store| {
            match store.retarget_paths(&self.workspace.root, source, target) {
                Ok(mutation) => {
                    let note = append_history_warnings(String::new(), &mutation.warnings);
                    (!note.is_empty()).then_some(note)
                }
                Err(error) => Some(format!("Local History path not updated: {error}")),
            }
        })
    }

    fn request_trash_entry(&mut self) {
        self.sync_active_document();
        let source = self.selected_workspace_path(true);
        if source == self.workspace.root {
            self.status_message = Some("Select a file or folder first".to_owned());
            return;
        }
        let open_count = self
            .tabs
            .iter()
            .filter(|tab| path_is_within(&tab.document.path, &source))
            .count();
        if open_count > 0 {
            self.status_message = Some(format!(
                "Close {open_count} open document(s) before removing this path"
            ));
            return;
        }
        let is_directory = source.is_dir();
        self.overlay = Some(Overlay::TrashConfirm {
            source,
            is_directory,
        });
    }

    fn trash_workspace_entry(&mut self, source: &Path) -> bool {
        let cached = self
            .recent_paths
            .iter()
            .filter(|path| path_is_within(path, source))
            .cloned()
            .collect::<Vec<_>>();
        match move_to_trash(&self.workspace, source) {
            Ok(removed) => {
                for path in cached {
                    self.discard_recovery_for(&path);
                }
                self.recent_paths
                    .retain(|path| !path_is_within(path, &removed));
                self.session_views
                    .retain(|path, _| !path_is_within(path, &removed));
                self.refresh_entries(removed.parent());
                self.persist_session_if_changed();
                self.status_message = Some(format!(
                    "Moved {} to Trash",
                    self.workspace.relative(&removed).display()
                ));
                true
            }
            Err(error) => {
                self.status_message = Some(format!("Cannot move to Trash · {error}"));
                false
            }
        }
    }

    fn open_path_input(&mut self, action: PathAction) {
        if action != PathAction::Create && self.active_tab().is_none() {
            self.status_message = Some("No document open".to_owned());
            return;
        }
        self.overlay = Some(Overlay::PathInput {
            action,
            input: TextInput::default(),
        });
    }

    fn apply_path_action(&mut self, action: PathAction, value: &str) -> bool {
        let value = value.trim();
        if value.is_empty() {
            self.status_message = Some(format!("Cannot {} an empty path", action.verb()));
            return false;
        }
        let target = match self.workspace.new_document_path(Path::new(value)) {
            Ok(target) => target,
            Err(error) => {
                self.status_message = Some(format!("Cannot {} · {error}", action.verb()));
                return false;
            }
        };

        if action == PathAction::Create {
            return match save_atomic(&target, "", Encoding::Utf8, LineEnding::Lf, None, true) {
                Ok(_) => {
                    self.finish_new_document(&target, "Created");
                    true
                }
                Err(error) => {
                    self.status_message = Some(format!("Create failed · {error}"));
                    false
                }
            };
        }

        self.sync_active_document();
        let Some(active_index) = self.active_tab else {
            return false;
        };
        if self
            .tabs
            .iter()
            .enumerate()
            .any(|(index, tab)| index != active_index && tab.document.path == target)
        {
            self.status_message = Some("Save failed · path is reserved by an open tab".to_owned());
            return false;
        }
        if action == PathAction::SaveConflictAs && self.tabs[active_index].document.path == target {
            self.status_message =
                Some("Save failed · the original path will not be recreated".to_owned());
            return false;
        }
        let tab = &self.tabs[active_index];
        let text = tab.document.text.clone();
        let encoding = tab.document.encoding;
        let line_ending = tab.document.line_ending;
        match save_atomic(&target, &text, encoding, line_ending, None, true) {
            Ok(snapshot) if matches!(action, PathAction::SaveAs | PathAction::SaveConflictAs) => {
                let previous = self.active_tab().map(|tab| tab.document.path.clone());
                if let Some(previous) = previous.as_deref() {
                    self.clear_agent_proposal_for_path(previous);
                }
                if let Some(tab) = self.active_tab_mut() {
                    tab.document.path.clone_from(&target);
                    tab.document.mark_saved(snapshot);
                }
                if let Some(previous) = previous.as_deref() {
                    self.retarget_recent(previous, &target);
                }
                self.refresh_entries(Some(&target));
                let label = if action == PathAction::SaveConflictAs {
                    "Saved local version as"
                } else {
                    "Saved as"
                };
                self.status_message = Some(format!(
                    "{label} {}",
                    self.workspace.relative(&target).display()
                ));
                if let Some(previous) = previous {
                    self.discard_recovery_for(&previous);
                }
                if action == PathAction::SaveConflictAs && self.pending_transition.is_some() {
                    self.continue_pending_transition();
                }
                true
            }
            Ok(_) => {
                self.refresh_entries(Some(&target));
                self.status_message = Some(format!(
                    "Duplicated as {}",
                    self.workspace.relative(&target).display()
                ));
                true
            }
            Err(error) => {
                self.status_message = Some(format!("{} failed · {error}", action.verb()));
                false
            }
        }
    }

    fn finish_new_document(&mut self, target: &Path, verb: &str) {
        self.refresh_entries(Some(target));
        match self.open_document(target) {
            Ok(()) => {
                self.status_message = Some(format!(
                    "{verb} {}",
                    self.workspace.relative(target).display()
                ));
            }
            Err(error) => self.status_message = Some(format!("Open failed · {error}")),
        }
    }

    fn refresh_entries(&mut self, selected_path: Option<&Path>) {
        self.workspace_scan_revision.fetch_add(1, Ordering::Relaxed);
        self.workspace_index_state = WorkspaceIndexState::Idle;
        let report = self.workspace.scan_report();
        self.entries = report.entries;
        self.workspace_scan_warnings = report.warnings;
        self.retain_existing_expanded_directories();
        if let Some(path) = selected_path {
            self.expand_ancestors(path);
        }
        let selected = selected_path
            .and_then(|path| self.visible_position(path))
            .or_else(|| (!self.visible_entry_indices().is_empty()).then_some(0));
        self.explorer_state.select(selected);
    }

    fn request_workspace_scan(&mut self) {
        if self.workspace_index_state == WorkspaceIndexState::Indexing {
            return;
        }
        let revision = self.workspace_scan_revision.fetch_add(1, Ordering::Relaxed) + 1;
        let workspace = self.workspace.clone();
        let sender = self.workspace_scan_tx.clone();
        self.workspace_index_state = WorkspaceIndexState::Indexing;
        let _worker = thread::spawn(move || {
            let report = workspace.scan_report();
            let _ = sender.send(WorkspaceScanCompletion { revision, report });
        });
    }

    fn poll_workspace_scan_results(&mut self) -> bool {
        let completions = self.workspace_scan_rx.try_iter().collect::<Vec<_>>();
        let mut changed = false;
        for completion in completions {
            if completion.revision != self.workspace_scan_revision.load(Ordering::Relaxed) {
                continue;
            }
            self.workspace_index_state = WorkspaceIndexState::Idle;
            let selected_path = self
                .selected_explorer_entry()
                .map(|entry| entry.path.clone());
            if self.entries != completion.report.entries {
                self.entries = completion.report.entries;
                self.retain_existing_expanded_directories();
                let selection = selected_path
                    .as_ref()
                    .and_then(|path| self.visible_position(path))
                    .or_else(|| (!self.visible_entry_indices().is_empty()).then_some(0));
                self.explorer_state.select(selection);
                changed = true;
            }
            if self.workspace_scan_warnings != completion.report.warnings {
                self.workspace_scan_warnings = completion.report.warnings;
                changed = true;
            }
            if let Some(first) = self.workspace_scan_warnings.first() {
                self.status_message = Some(format!(
                    "Files indexed with {} warning{} · {first}",
                    self.workspace_scan_warnings.len(),
                    if self.workspace_scan_warnings.len() == 1 {
                        ""
                    } else {
                        "s"
                    }
                ));
            }
        }
        changed
    }

    #[must_use]
    pub const fn workspace_is_indexing(&self) -> bool {
        matches!(self.workspace_index_state, WorkspaceIndexState::Indexing)
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }
        if key.code == KeyCode::Esc {
            self.execute_command(CommandAction::CommandMode);
            return;
        }
        if self.handle_global_key(key) {
            return;
        }
        if self.focus == Focus::Explorer
            && (self.handle_explorer_key(key) || self.mode == Mode::Write)
        {
            return;
        }
        if self.focus == Focus::Preview {
            if let Some(action) = self
                .config
                .keybindings
                .action_for(BindingScope::Preview, key)
            {
                self.execute_binding_action(action);
                return;
            }
            if self.handle_preview_key(key) || self.mode == Mode::Write {
                return;
            }
        }
        match self.mode {
            Mode::Write => self.handle_write_key(key),
            Mode::Command => self.handle_command_key(key),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        let Some(action) = self
            .config
            .keybindings
            .action_for(BindingScope::Global, key)
        else {
            return false;
        };
        self.execute_binding_action(action);
        true
    }

    fn handle_write_key(&mut self, key: KeyEvent) {
        if self.focus == Focus::Editor
            && let Some(action) = editor_clipboard_action(key)
        {
            match action {
                EditorClipboardAction::Copy => {
                    self.trace_command("CopySelection");
                    self.copy_editor_selection();
                }
                EditorClipboardAction::Cut => {
                    self.trace_command("CutSelection");
                    self.cut_editor_selection();
                }
                EditorClipboardAction::Paste => {
                    self.trace_command("PasteClipboard");
                    self.paste_system_clipboard();
                }
            }
            return;
        }
        if self.focus == Focus::Editor
            && let Some(action) = self
                .config
                .keybindings
                .action_for(BindingScope::Editor, key)
        {
            self.execute_binding_action(action);
            return;
        }
        if self
            .active_tab()
            .is_none_or(|tab| !tab.document.is_editable())
        {
            self.enforce_active_read_only();
            return;
        }
        let auto_continue = self.config.editor.auto_continue_lists
            && key.code == KeyCode::Enter
            && key.modifiers == KeyModifiers::NONE;
        let modified = self.active_tab_mut().is_some_and(|tab| {
            let selecting = tab.editor.is_selecting();
            let mut history_items = 1;
            if auto_continue && !tab.editor.is_selecting() {
                let (row, column) = tab.editor.cursor();
                let modified = match action_for(tab.editor.lines(), row, column) {
                    EnterAction::Continue(marker) => {
                        tab.editor.insert_str(format!("\n{marker}"));
                        true
                    }
                    EnterAction::EndMarker(characters) => {
                        tab.editor.delete_str(characters);
                        tab.editor.insert_newline();
                        history_items = 2;
                        true
                    }
                    EnterAction::Plain => tab.editor.input(key),
                };
                if modified {
                    tab.record_edit(history_items);
                }
                modified
            } else {
                let modified = tab.editor.input(key);
                if modified {
                    if selecting && input_replaces_selection(key) {
                        history_items = 2;
                    }
                    tab.record_edit(history_items);
                }
                modified
            }
        });
        if modified {
            self.sync_active_document();
            self.status_message = None;
        }
    }

    fn copy_editor_selection(&mut self) {
        let Some(text) = self.active_tab().and_then(selected_text) else {
            self.status_message = Some("Select text to copy".to_owned());
            return;
        };
        match write_system_clipboard(&text) {
            Ok(()) => self.status_message = Some("Copied selection".to_owned()),
            Err(error) => self.status_message = Some(format!("Copy failed · {error}")),
        }
    }

    fn cut_editor_selection(&mut self) {
        if self
            .active_tab()
            .is_none_or(|tab| !tab.document.is_editable())
        {
            self.enforce_active_read_only();
            return;
        }
        let Some(text) = self.active_tab().and_then(selected_text) else {
            self.status_message = Some("Select text to cut".to_owned());
            return;
        };
        match write_system_clipboard(&text) {
            Ok(()) => {
                if self.active_tab_mut().is_some_and(EditorTab::cut_selection) {
                    self.status_message = Some("Cut selection".to_owned());
                }
            }
            Err(error) => self.status_message = Some(format!("Cut failed · {error}")),
        }
    }

    fn paste_system_clipboard(&mut self) {
        match read_system_clipboard() {
            Ok(text) => self.paste_into_document(&text),
            Err(error) => self.status_message = Some(format!("Paste failed · {error}")),
        }
    }

    fn paste_into_document(&mut self, text: &str) {
        if self.mode != Mode::Write {
            return;
        }
        let Some(tab) = self
            .active_tab_mut()
            .filter(|tab| tab.document.is_editable())
        else {
            self.enforce_active_read_only();
            return;
        };
        let selecting = tab.editor.is_selecting();
        tab.editor.insert_str(text);
        tab.record_edit(1 + usize::from(selecting && !text.is_empty()));
        tab.sync_document();
        self.status_message = None;
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        if self.focus == Focus::Editor
            && let Some(action) = self
                .config
                .keybindings
                .action_for(BindingScope::Editor, key)
        {
            self.execute_binding_action(action);
            return;
        }
        if let Some(action) = self
            .config
            .keybindings
            .action_for(BindingScope::Command, key)
        {
            if self.focus == Focus::Editor || !is_navigation_action(action) {
                self.execute_binding_action(action);
            }
        } else {
            match key.code {
                KeyCode::Left
                    if self.focus == Focus::Editor && key.modifiers == KeyModifiers::NONE =>
                {
                    self.trace_command("CursorLeft");
                    self.move_editor(CursorMove::Back);
                }
                KeyCode::Down
                    if self.focus == Focus::Editor && key.modifiers == KeyModifiers::NONE =>
                {
                    self.trace_command("CursorDown");
                    self.move_editor(CursorMove::Down);
                }
                KeyCode::Up
                    if self.focus == Focus::Editor && key.modifiers == KeyModifiers::NONE =>
                {
                    self.trace_command("CursorUp");
                    self.move_editor(CursorMove::Up);
                }
                KeyCode::Right
                    if self.focus == Focus::Editor && key.modifiers == KeyModifiers::NONE =>
                {
                    self.trace_command("CursorRight");
                    self.move_editor(CursorMove::Forward);
                }
                KeyCode::Tab if self.show_explorer => {
                    self.trace_command("ToggleFocus");
                    self.focus = if self.focus == Focus::Explorer {
                        Focus::Editor
                    } else {
                        Focus::Explorer
                    };
                }
                _ => {}
            }
        }
    }

    fn handle_explorer_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers == KeyModifiers::SHIFT {
            match key.code {
                KeyCode::Left => {
                    self.trace_command("NarrowExplorer");
                    self.resize_explorer_by(-2);
                    return true;
                }
                KeyCode::Right => {
                    self.trace_command("WidenExplorer");
                    self.resize_explorer_by(2);
                    return true;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.trace_command("ExplorerUp");
                self.move_explorer(-1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.trace_command("ExplorerDown");
                self.move_explorer(1);
            }
            KeyCode::Enter => {
                self.trace_command("OpenSelectedEntry");
                self.open_selected_entry();
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.trace_command("ExpandOrOpenSelectedEntry");
                self.expand_or_open_selected_entry();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.trace_command("CollapseSelectedEntry");
                self.collapse_selected_entry();
            }
            KeyCode::Char('a') => self.execute_command(CommandAction::Create),
            KeyCode::Char('c') => self.execute_command(CommandAction::CopyEntry),
            KeyCode::Char('x') => self.execute_command(CommandAction::CutEntry),
            KeyCode::Char('p') => self.execute_command(CommandAction::PasteEntry),
            KeyCode::Char('r') => self.execute_command(CommandAction::RenameEntry),
            KeyCode::Char('m') => self.execute_command(CommandAction::MoveEntry),
            KeyCode::Char('d') => self.execute_command(CommandAction::TrashEntry),
            _ => return false,
        }
        true
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.overlay.is_some() {
            self.handle_overlay_mouse(mouse);
            return;
        }
        let column = mouse.column;
        let row = mouse.row;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse_drag_target = None;
                if UiRegions::contains(self.ui_regions.tabs, column, row) {
                    if let Some(index) = self.tab_at_position(column, row) {
                        self.activate_tab(index);
                    }
                    return;
                }
                if UiRegions::contains_resize_handle(self.ui_regions.explorer_divider, column, row)
                {
                    self.mouse_drag_target = Some(MouseDragTarget::Explorer);
                    return;
                }
                if UiRegions::contains(self.ui_regions.workbench_divider, column, row) {
                    self.mouse_drag_target = Some(MouseDragTarget::Workbench);
                    return;
                }
                if UiRegions::contains(self.ui_regions.explorer_list, column, row) {
                    self.focus = Focus::Explorer;
                    let list = self.ui_regions.explorer_list.unwrap_or_default();
                    let index = self
                        .explorer_state
                        .offset()
                        .saturating_add(usize::from(row.saturating_sub(list.y)));
                    if index < self.visible_entry_indices().len() {
                        self.explorer_state.select(Some(index));
                        let now = Instant::now();
                        let double_click =
                            self.last_explorer_click.is_some_and(|(previous, instant)| {
                                previous == index
                                    && now.duration_since(instant) <= Duration::from_millis(400)
                            });
                        self.last_explorer_click = Some((index, now));
                        if double_click {
                            self.open_selected_entry();
                        }
                    }
                } else if UiRegions::contains(self.ui_regions.preview, column, row) {
                    self.handle_preview_click(column, row);
                } else if UiRegions::contains(self.ui_regions.editor, column, row) {
                    self.focus = Focus::Editor;
                    let inline = self.view_mode == ViewMode::Inline;
                    let select = inline && self.mode == Mode::Write;
                    if let Some(area) = self.ui_regions.editor.map(ui::editor_area)
                        && let Some(tab) = self.active_tab_mut()
                    {
                        tab.editor.cancel_selection();
                        tab.place_cursor(area, column, row, inline);
                        if select {
                            tab.editor.start_selection();
                            tab.refresh_inline_editor();
                            self.mouse_drag_target = Some(MouseDragTarget::EditorSelection);
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.handle_mouse_drag(column, row),
            MouseEventKind::Up(MouseButton::Left) => self.finish_mouse_drag(),
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let direction = if mouse.kind == MouseEventKind::ScrollUp {
                    -2
                } else {
                    2
                };
                if UiRegions::contains(self.ui_regions.preview, column, row) {
                    self.scroll_preview_by(direction);
                } else if UiRegions::contains(self.ui_regions.explorer, column, row) {
                    self.move_explorer(direction as isize);
                } else if UiRegions::contains(self.ui_regions.editor, column, row) {
                    let inline = self.view_mode == ViewMode::Inline;
                    if let Some(tab) = self.active_tab_mut() {
                        if inline {
                            tab.scroll_inline_editor(mouse);
                        } else {
                            tab.editor.input(mouse);
                        }
                    }
                }
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                if UiRegions::contains(self.ui_regions.preview, column, row) =>
            {
                let direction = if mouse.kind == MouseEventKind::ScrollLeft {
                    -2
                } else {
                    2
                };
                self.scroll_preview_horizontally_by(direction);
            }
            _ => {}
        }
    }

    fn tab_at_position(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.ui_regions.tabs?;
        if !UiRegions::contains(Some(area), column, row) {
            return None;
        }
        let mut x = area.x;
        for (index, tab) in self.tabs.iter().enumerate() {
            if index > 0 {
                x = x.saturating_add(3);
            }
            let name = tab
                .document
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let dirty = if tab.document.is_dirty() { " ●" } else { "" };
            let conflict = if tab.document.conflict { " !" } else { "" };
            let shared = if self.workspace_is_shared() {
                " ↔"
            } else {
                ""
            };
            let label = format!(" {name}{dirty}{conflict}{shared} ");
            let width = u16::try_from(UnicodeWidthStr::width(label.as_str())).unwrap_or(u16::MAX);
            if column >= x && column < x.saturating_add(width) {
                return Some(index);
            }
            x = x.saturating_add(width);
        }
        None
    }

    fn handle_overlay_mouse(&mut self, mouse: MouseEvent) {
        let Some(area) = self.ui_regions.overlay else {
            return;
        };
        if !UiRegions::contains(Some(area), mouse.column, mouse.row) {
            return;
        }
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) && let Some(Overlay::LocalHistory {
            enabled: true,
            focus,
            ..
        }) = &mut self.overlay
        {
            let position = Position::new(mouse.column, mouse.row);
            if ui::history_list_area(area).contains(position) {
                *focus = HistoryFocus::Checkpoints;
            } else if ui::history_diff_area(area).contains(position) {
                *focus = HistoryFocus::Diff;
            }
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.handle_overlay_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
            }
            MouseEventKind::ScrollDown => {
                self.handle_overlay_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_overlay_click(area, mouse.column, mouse.row);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_overlay_click(&mut self, area: Rect, column: u16, row: u16) {
        let inner = ui::overlay_inner_area(area);
        let now = Instant::now();
        let double_click = self.last_overlay_click.is_some_and(|(previous, instant)| {
            previous == row && now.duration_since(instant) <= Duration::from_millis(400)
        });
        self.last_overlay_click = Some((row, now));

        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        let mut activation = None;
        match &mut overlay {
            Overlay::Palette { input, selected } => {
                let candidates = command_candidates(&input.value);
                if let Some(index) =
                    command_at_overlay_position(column, row, inner, &candidates, *selected)
                {
                    *selected = index;
                    if double_click {
                        activation = Some(KeyCode::Enter);
                    }
                }
            }
            Overlay::RecentDocuments { paths, selected } => {
                if let Some(index) = overlay_list_index(
                    row,
                    inner.y.saturating_add(2),
                    inner.height.saturating_sub(3),
                    paths.len(),
                    *selected,
                ) {
                    *selected = index;
                    activation = double_click.then_some(KeyCode::Enter);
                }
            }
            Overlay::SearchResults { results, selected } => {
                if let Some(index) = overlay_list_index(
                    row,
                    inner.y.saturating_add(2),
                    inner.height.saturating_sub(3),
                    results.len(),
                    *selected,
                ) {
                    *selected = index;
                    activation = double_click.then_some(KeyCode::Enter);
                }
            }
            Overlay::Outline {
                matches, selected, ..
            } => {
                if let Some(index) = overlay_list_index(
                    row,
                    inner.y.saturating_add(3),
                    inner.height.saturating_sub(4),
                    matches.len(),
                    *selected,
                ) {
                    *selected = index;
                    activation = double_click.then_some(KeyCode::Enter);
                }
            }
            Overlay::SemanticInspector { mapping, selected } => {
                let count = mapping.segments().len();
                if let Some(index) = overlay_list_index(
                    row,
                    inner.y.saturating_add(2),
                    inner.height.saturating_sub(13),
                    count,
                    *selected,
                ) {
                    *selected = index;
                    activation = double_click.then_some(KeyCode::Enter);
                }
            }
            Overlay::FileFinder {
                focus,
                selected,
                query,
                filter,
                ..
            } => {
                if row < inner.y.saturating_add(2) {
                    *focus = FileFinderFocus::Query;
                } else if row < inner.y.saturating_add(4) {
                    *focus = FileFinderFocus::Filter;
                } else {
                    let count = self
                        .filtered_file_candidates(&query.value, &filter.value)
                        .map_or(0, |candidates| candidates.len());
                    if let Some(index) = overlay_list_index(
                        row,
                        inner.y.saturating_add(5),
                        inner.height.saturating_sub(6),
                        count,
                        *selected,
                    ) {
                        *focus = FileFinderFocus::Results;
                        *selected = index;
                        activation = double_click.then_some(KeyCode::Enter);
                    }
                }
            }
            Overlay::Find {
                focus, read_only, ..
            } => {
                let offset = row.saturating_sub(inner.y);
                *focus = match offset {
                    0..=1 => FindFocus::Query,
                    2..=3 if !*read_only => FindFocus::Replacement,
                    4 => FindFocus::Case,
                    6 => find_action_at(column.saturating_sub(inner.x), *read_only),
                    _ => *focus,
                };
                if offset == 4 || offset == 6 {
                    activation = Some(KeyCode::Enter);
                }
            }
            Overlay::WorkspaceSearch {
                focus,
                results,
                selected,
                ..
            } => {
                let offset = row.saturating_sub(inner.y);
                match offset {
                    0..=1 => *focus = WorkspaceSearchFocus::Query,
                    2 => {
                        *focus = if column < inner.x.saturating_add(16) {
                            WorkspaceSearchFocus::Mode
                        } else {
                            WorkspaceSearchFocus::Case
                        };
                        activation = Some(KeyCode::Enter);
                    }
                    3..=4 => *focus = WorkspaceSearchFocus::Filter,
                    _ => {
                        if let Some(index) = overlay_list_index(
                            row,
                            inner.y.saturating_add(6),
                            inner.height.saturating_sub(7),
                            results.len(),
                            *selected,
                        ) {
                            *focus = WorkspaceSearchFocus::Results;
                            *selected = index;
                            activation = double_click.then_some(KeyCode::Enter);
                        }
                    }
                }
            }
            Overlay::RecoveryManager {
                records,
                selected,
                focus,
                retention_days,
                ..
            } => {
                let records_height = inner.height.saturating_sub(13);
                if let Some(index) = overlay_list_index(
                    row,
                    inner.y.saturating_add(2),
                    records_height,
                    records.len(),
                    *selected,
                ) {
                    *focus = RecoveryManagerFocus::Records;
                    *selected = index;
                    activation = double_click.then_some(KeyCode::Enter);
                } else if row >= inner.bottom().saturating_sub(7)
                    && row < inner.bottom().saturating_sub(5)
                {
                    *focus = RecoveryManagerFocus::Target;
                } else if row >= inner.bottom().saturating_sub(5)
                    && row < inner.bottom().saturating_sub(3)
                    && let Some(code) = recovery_action_at(
                        records.get(*selected),
                        column.saturating_sub(inner.x),
                        *retention_days,
                    )
                {
                    *focus = RecoveryManagerFocus::Records;
                    activation = Some(code);
                }
            }
            Overlay::LocalHistory {
                enabled,
                checkpoints,
                selected,
                focus,
                diff,
                diff_scroll,
                diff_truncated,
                ..
            } => {
                if *enabled {
                    let list_area = ui::history_list_area(area);
                    let diff_area = ui::history_diff_area(area);
                    if list_area.contains(Position::new(column, row)) {
                        let start =
                            ui::history_list_start(checkpoints.len(), *selected, list_area.height);
                        let index = start + usize::from(row.saturating_sub(list_area.y));
                        if index < checkpoints.len() {
                            *focus = HistoryFocus::Checkpoints;
                            *selected = index;
                            let current = self
                                .active_tab()
                                .map_or("", |tab| tab.document.text.as_str());
                            let (updated, truncated) =
                                build_history_diff(&checkpoints[index].source, current);
                            *diff = updated;
                            *diff_scroll = 0;
                            *diff_truncated = truncated;
                        }
                    } else if diff_area.contains(Position::new(column, row)) {
                        *focus = HistoryFocus::Diff;
                    } else if row == inner.bottom().saturating_sub(2) {
                        activation = match column.saturating_sub(inner.x) {
                            0..=11 => Some(KeyCode::Char('c')),
                            15..=23 => Some(KeyCode::Char('r')),
                            27..=34 => Some(KeyCode::Tab),
                            38..=46 => Some(KeyCode::Char('e')),
                            _ => None,
                        };
                    } else if row == inner.bottom().saturating_sub(1) {
                        activation = match column.saturating_sub(inner.x) {
                            0..=15 => Some(KeyCode::Char('d')),
                            19..=35 => Some(KeyCode::Char('D')),
                            39..=47 => Some(KeyCode::Esc),
                            _ => None,
                        };
                    }
                } else if row == inner.bottom().saturating_sub(2) {
                    activation = match column.saturating_sub(inner.x) {
                        0..=27 => Some(KeyCode::Char('e')),
                        33..=42 => Some(KeyCode::Esc),
                        _ => None,
                    };
                }
            }
            Overlay::HistoryClearConfirm { scope, .. }
                if row == inner.bottom().saturating_sub(3) =>
            {
                let relative_column = column.saturating_sub(inner.x);
                activation = match scope {
                    HistoryClearScope::Document if relative_column <= 24 => {
                        Some(KeyCode::Char('d'))
                    }
                    HistoryClearScope::Workspace if relative_column <= 25 => {
                        Some(KeyCode::Char('D'))
                    }
                    HistoryClearScope::Document if (30..=58).contains(&relative_column) => {
                        Some(KeyCode::Enter)
                    }
                    HistoryClearScope::Workspace if (31..=59).contains(&relative_column) => {
                        Some(KeyCode::Enter)
                    }
                    _ => None,
                };
            }
            Overlay::PathInput { .. }
            | Overlay::WorkspaceInput { .. }
            | Overlay::Help { .. }
            | Overlay::MarkdownHelp { .. }
            | Overlay::SemanticReader { .. }
            | Overlay::CoordinateInspector { .. }
                if row >= inner.bottom().saturating_sub(1) =>
            {
                activation = Some(KeyCode::Enter);
            }
            Overlay::Message(_) => activation = Some(KeyCode::Enter),
            Overlay::AgentSharing { .. }
                if row == inner.bottom().saturating_sub(1)
                    && column >= inner.x
                    && column < inner.right() =>
            {
                activation = match column.saturating_sub(inner.x) {
                    0..=12 => Some(KeyCode::Char('r')),
                    18..=44 => Some(KeyCode::Esc),
                    _ => None,
                };
            }
            Overlay::AgentProposal { .. }
                if row == inner.bottom().saturating_sub(1)
                    && column >= inner.x
                    && column < inner.right() =>
            {
                activation = match column.saturating_sub(inner.x) {
                    0..=20 => Some(KeyCode::Char('a')),
                    26..=34 => Some(KeyCode::Char('r')),
                    55..=71 => Some(KeyCode::Esc),
                    _ => None,
                };
            }
            Overlay::TrashConfirm { .. } if row >= inner.y.saturating_add(3) => {
                activation = (column < inner.x.saturating_add(22)).then_some(KeyCode::Char('y'));
            }
            Overlay::Recovery { .. } if row >= inner.y.saturating_add(3) => {
                activation = if column < inner.x.saturating_add(13) {
                    Some(KeyCode::Char('r'))
                } else if column < inner.x.saturating_add(28) {
                    Some(KeyCode::Char('d'))
                } else {
                    Some(KeyCode::Esc)
                };
            }
            Overlay::RecoveryDeleteConfirm { .. } | Overlay::RecoveryCleanupConfirm { .. }
                if row >= inner.bottom().saturating_sub(2) =>
            {
                activation = (column < inner.x.saturating_add(24)).then_some(KeyCode::Char('d'));
            }
            Overlay::MixedLineEndings { .. } if row >= inner.y.saturating_add(3) => {
                activation = if column < inner.x.saturating_add(30) {
                    Some(KeyCode::Enter)
                } else {
                    Some(KeyCode::Esc)
                };
            }
            Overlay::Conflict {
                can_reload,
                allow_continue,
                ..
            } if row >= inner.y.saturating_add(3) => {
                activation = conflict_action_at(
                    column.saturating_sub(inner.x),
                    *can_reload,
                    *allow_continue,
                );
            }
            Overlay::Confirm(_) if row >= inner.y.saturating_add(2) => {
                activation = match column.saturating_sub(inner.x) {
                    0..=10 => Some(KeyCode::Char('y')),
                    11..=27 => Some(KeyCode::Char('n')),
                    _ => Some(KeyCode::Esc),
                };
            }
            _ => {}
        }
        self.overlay = Some(overlay);
        if let Some(code) = activation {
            self.handle_overlay_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
    }

    fn handle_mouse_drag(&mut self, column: u16, row: u16) {
        match self.mouse_drag_target {
            Some(MouseDragTarget::Explorer) => self.resize_explorer(column),
            Some(MouseDragTarget::Workbench) => self.resize_workbench(column),
            Some(MouseDragTarget::EditorSelection) => {
                if let Some(area) = self.ui_regions.editor.map(ui::editor_area)
                    && let Some(tab) = self.active_tab_mut()
                {
                    let column = column.clamp(area.x, area.right().saturating_sub(1));
                    let row = row.clamp(area.y, area.bottom().saturating_sub(1));
                    tab.place_cursor(area, column, row, true);
                }
            }
            None => {}
        }
    }

    fn handle_preview_click(&mut self, column: u16, row: u16) {
        self.focus = Focus::Preview;
        let Some(area) = self.ui_regions.preview.map(ui::preview_content_area) else {
            return;
        };
        if let Some(index) = self.preview_link_at_position(area, column, row) {
            self.preview_selected_link = Some(index);
            self.activate_preview_link();
        } else {
            self.preview_selected_link = None;
            let scroll = self.preview_scroll;
            if let Some(tab) = self.active_tab_mut() {
                tab.place_cursor_from_preview(area, column, row, scroll);
            }
        }
    }

    fn finish_mouse_drag(&mut self) {
        if self.mouse_drag_target == Some(MouseDragTarget::EditorSelection)
            && let Some(tab) = self.active_tab_mut()
        {
            if tab
                .editor
                .selection_range()
                .is_some_and(|(start, end)| start == end)
            {
                tab.editor.cancel_selection();
            }
            tab.refresh_inline_editor();
        }
        self.mouse_drag_target = None;
    }

    fn resize_explorer(&mut self, column: u16) {
        let workspace = self.ui_regions.workspace;
        let maximum = workspace
            .width
            .saturating_sub(EXPLORER_MIN_WIDTH)
            .min(EXPLORER_MAX_WIDTH);
        let minimum = EXPLORER_MIN_WIDTH.min(maximum);
        let requested = column.saturating_sub(workspace.x);
        self.explorer_width = requested.clamp(minimum, maximum);
        self.status_message = Some(format!("Files width · {} columns", self.explorer_width));
    }

    fn resize_explorer_by(&mut self, columns: i16) {
        let workspace_width = self.ui_regions.workspace.width.max(self.viewport_width);
        let maximum = workspace_width
            .saturating_sub(EXPLORER_MIN_WIDTH)
            .min(EXPLORER_MAX_WIDTH);
        let minimum = EXPLORER_MIN_WIDTH.min(maximum);
        self.explorer_width = self
            .explorer_width
            .saturating_add_signed(columns)
            .clamp(minimum, maximum);
        self.status_message = Some(format!("Files width · {} columns", self.explorer_width));
    }

    fn resize_workbench(&mut self, column: u16) {
        let workbench = self.ui_regions.workbench;
        let available = workbench.width.saturating_sub(1);
        if available < 40 {
            return;
        }
        let editor_width = column
            .saturating_sub(workbench.x)
            .clamp(20, available.saturating_sub(20));
        self.split_percent =
            u16::try_from(u32::from(editor_width) * 100 / u32::from(available.max(1)))
                .unwrap_or(50)
                .clamp(1, 99);
    }

    fn move_explorer(&mut self, direction: isize) {
        let visible_count = self.visible_entry_indices().len();
        if visible_count == 0 {
            return;
        }
        let current = self.explorer_state.selected().unwrap_or_default();
        let maximum = visible_count - 1;
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(maximum)
        };
        self.explorer_state.select(Some(next));
    }

    fn open_selected_entry(&mut self) {
        let Some(entry) = self.selected_explorer_entry().cloned() else {
            return;
        };
        if entry.is_dir {
            let expanded = if self.expanded_directories.remove(&entry.relative) {
                false
            } else {
                self.expanded_directories.insert(entry.relative.clone());
                true
            };
            self.status_message = Some(format!(
                "{} · {}",
                entry.relative.display(),
                if expanded { "expanded" } else { "collapsed" }
            ));
            return;
        }
        let path = entry.path.clone();
        if let Err(error) = self.open_document(&path) {
            self.status_message = Some(format!("Open failed · {error}"));
        }
    }

    fn expand_or_open_selected_entry(&mut self) {
        let Some(entry) = self.selected_explorer_entry().cloned() else {
            return;
        };
        if entry.is_dir {
            self.expanded_directories.insert(entry.relative.clone());
            self.status_message = Some(format!("{} · expanded", entry.relative.display()));
        } else {
            self.open_selected_entry();
        }
    }

    fn collapse_selected_entry(&mut self) {
        let Some(entry) = self.selected_explorer_entry().cloned() else {
            self.focus = Focus::Editor;
            return;
        };
        if entry.is_dir && self.expanded_directories.remove(&entry.relative) {
            self.status_message = Some(format!("{} · collapsed", entry.relative.display()));
            return;
        }
        let Some(parent) = entry
            .relative
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        else {
            self.focus = Focus::Editor;
            return;
        };
        let parent = self.workspace.root.join(parent);
        if let Some(index) = self.visible_position(&parent) {
            self.explorer_state.select(Some(index));
        }
    }

    pub(crate) fn visible_entry_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| self.entry_is_visible(entry).then_some(index))
            .collect()
    }

    pub(crate) fn directory_is_expanded(&self, entry: &WorkspaceEntry) -> bool {
        entry.is_dir && self.expanded_directories.contains(&entry.relative)
    }

    fn entry_is_visible(&self, entry: &WorkspaceEntry) -> bool {
        entry
            .relative
            .ancestors()
            .skip(1)
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .all(|ancestor| self.expanded_directories.contains(ancestor))
    }

    fn selected_explorer_entry(&self) -> Option<&WorkspaceEntry> {
        let visible_index = self.explorer_state.selected()?;
        let entry_index = *self.visible_entry_indices().get(visible_index)?;
        self.entries.get(entry_index)
    }

    fn visible_position(&self, path: &Path) -> Option<usize> {
        self.visible_entry_indices()
            .into_iter()
            .position(|index| self.entries[index].path == path)
    }

    fn expand_ancestors(&mut self, path: &Path) {
        let relative = self.workspace.relative(path);
        self.expanded_directories.extend(
            relative
                .ancestors()
                .skip(1)
                .filter(|ancestor| !ancestor.as_os_str().is_empty())
                .map(Path::to_path_buf),
        );
    }

    fn retain_existing_expanded_directories(&mut self) {
        self.expanded_directories.retain(|relative| {
            self.entries
                .iter()
                .any(|entry| entry.is_dir && entry.relative == *relative)
        });
    }

    #[allow(clippy::if_not_else, clippy::too_many_lines)]
    fn handle_overlay_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            let overlay = self.overlay.take();
            match overlay {
                Some(Overlay::MixedLineEndings {
                    tab_index,
                    previous_active,
                    context,
                    ..
                }) => self.cancel_mixed_line_endings(tab_index, previous_active, context),
                Some(Overlay::PathInput {
                    action: PathAction::SaveConflictAs,
                    ..
                }) => {
                    if let Some(index) = self.active_tab {
                        let kind = conflict_kind_for_path(&self.tabs[index].document.path);
                        self.show_conflict(index, kind);
                    }
                }
                Some(Overlay::Confirm(_) | Overlay::Conflict { .. }) => {
                    self.pending_transition = None;
                }
                _ => {}
            }
            return;
        }
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        let keep = match &mut overlay {
            Overlay::Help { scroll, max_scroll } => match key.code {
                KeyCode::Enter | KeyCode::F(1) => false,
                code => {
                    update_scroll(scroll, code, usize::from(*max_scroll) + 1);
                    true
                }
            },
            Overlay::CoordinateInspector { .. } | Overlay::Message(_) => key.code != KeyCode::Enter,
            Overlay::AgentSharing { .. } => {
                if key.code == KeyCode::Char('r') {
                    self.revoke_agent_sharing();
                    false
                } else {
                    true
                }
            }
            Overlay::AgentProposal { proposal, scroll } => match key.code {
                KeyCode::Char('a') => {
                    self.accept_agent_proposal(proposal);
                    false
                }
                KeyCode::Char('r') => {
                    self.reject_agent_proposal(&proposal.id);
                    false
                }
                code => {
                    update_scroll(scroll, code, proposal.diff.len());
                    true
                }
            },
            Overlay::MarkdownHelp { scroll } => match key.code {
                KeyCode::Enter | KeyCode::F(1) => false,
                code => {
                    update_scroll(
                        scroll,
                        code,
                        crate::markdown_help::MARKDOWN_SYNTAX_HELP.lines().count(),
                    );
                    true
                }
            },
            Overlay::SemanticInspector { mapping, selected } => {
                let count = mapping.segments().len();
                match key.code {
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        true
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(count.saturating_sub(1));
                        true
                    }
                    KeyCode::PageUp => {
                        *selected = selected.saturating_sub(10);
                        true
                    }
                    KeyCode::PageDown => {
                        *selected = (*selected + 10).min(count.saturating_sub(1));
                        true
                    }
                    KeyCode::Home => {
                        *selected = 0;
                        true
                    }
                    KeyCode::End => {
                        *selected = count.saturating_sub(1);
                        true
                    }
                    KeyCode::Enter => {
                        let line = mapping
                            .segments()
                            .get(*selected)
                            .map(|segment| segment.start_line);
                        if let Some(line) = line {
                            self.move_editor(CursorMove::Jump(
                                u16::try_from(line).unwrap_or(u16::MAX),
                                0,
                            ));
                            self.focus = Focus::Editor;
                            self.narrow_pane = Focus::Editor;
                        }
                        false
                    }
                    _ => true,
                }
            }
            Overlay::SemanticReader { mapping, scroll } => match key.code {
                KeyCode::Enter => false,
                code => {
                    update_scroll(scroll, code, semantic_reader_line_count(mapping));
                    true
                }
            },
            Overlay::MixedLineEndings {
                tab_index,
                context,
                target,
                ..
            } => match key.code {
                KeyCode::Enter | KeyCode::Char('e') => {
                    self.accept_mixed_line_endings(*tab_index, *context, *target);
                    false
                }
                _ => true,
            },
            Overlay::Conflict {
                can_reload,
                allow_continue,
                ..
            } => match key.code {
                KeyCode::Char('s') => {
                    self.open_conflict_save_as();
                    false
                }
                KeyCode::Char('r') if *can_reload => {
                    self.reload_conflict();
                    false
                }
                KeyCode::Char('n') if *allow_continue => {
                    self.continue_without_conflict_copy();
                    false
                }
                _ => true,
            },
            Overlay::Confirm(action) => match key.code {
                KeyCode::Char('y') => {
                    self.continue_pending_transition();
                    false
                }
                KeyCode::Char('n') => {
                    self.discard_pending_changes(*action);
                    false
                }
                _ => true,
            },
            Overlay::Palette { input, selected } => {
                let candidates = command_candidates(&input.value);
                match key.code {
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        true
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(candidates.len().saturating_sub(1));
                        true
                    }
                    KeyCode::Enter => {
                        if let Some(command) = candidates.get(*selected) {
                            self.execute_command(command.action);
                        }
                        false
                    }
                    _ if edit_text_input(input, key) => {
                        *selected = 0;
                        true
                    }
                    _ => true,
                }
            }
            Overlay::FileFinder {
                query,
                filter,
                focus,
                selected,
                error,
            } => {
                let candidates = self
                    .filtered_file_candidates(&query.value, &filter.value)
                    .unwrap_or_default();
                match key.code {
                    KeyCode::Tab => {
                        *focus = match *focus {
                            FileFinderFocus::Query => FileFinderFocus::Filter,
                            FileFinderFocus::Filter if !candidates.is_empty() => {
                                FileFinderFocus::Results
                            }
                            FileFinderFocus::Filter | FileFinderFocus::Results => {
                                FileFinderFocus::Query
                            }
                        };
                        true
                    }
                    KeyCode::BackTab => {
                        *focus = match *focus {
                            FileFinderFocus::Query if !candidates.is_empty() => {
                                FileFinderFocus::Results
                            }
                            FileFinderFocus::Query | FileFinderFocus::Results => {
                                FileFinderFocus::Filter
                            }
                            FileFinderFocus::Filter => FileFinderFocus::Query,
                        };
                        true
                    }
                    KeyCode::Up => {
                        if *focus == FileFinderFocus::Results {
                            *selected = selected.saturating_sub(1);
                        }
                        true
                    }
                    KeyCode::Down => {
                        if !candidates.is_empty() {
                            if *focus == FileFinderFocus::Results {
                                *selected = (*selected + 1).min(candidates.len().saturating_sub(1));
                            } else {
                                *focus = FileFinderFocus::Results;
                            }
                        }
                        true
                    }
                    KeyCode::Enter => {
                        let index = if *focus == FileFinderFocus::Results {
                            candidates.get(*selected)
                        } else {
                            candidates.first()
                        };
                        if let Some(index) = index {
                            let path = self.entries[*index].path.clone();
                            if let Err(error) = self.open_document(&path) {
                                self.status_message = Some(format!("Open failed · {error}"));
                            }
                        }
                        false
                    }
                    _ if *focus == FileFinderFocus::Query && edit_text_input(query, key) => {
                        *selected = 0;
                        *error = self
                            .filtered_file_candidates(&query.value, &filter.value)
                            .err();
                        true
                    }
                    _ if *focus == FileFinderFocus::Filter && edit_text_input(filter, key) => {
                        *selected = 0;
                        *error = self
                            .filtered_file_candidates(&query.value, &filter.value)
                            .err();
                        true
                    }
                    _ => true,
                }
            }
            Overlay::RecentDocuments { paths, selected } => match key.code {
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    true
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(paths.len().saturating_sub(1));
                    true
                }
                KeyCode::Enter => {
                    if let Some(path) = paths.get(*selected).cloned()
                        && let Err(error) = self.open_document(&path)
                    {
                        self.status_message =
                            Some(format!("Cannot open recent document · {error}"));
                    }
                    false
                }
                _ => true,
            },
            Overlay::Find {
                query,
                replacement,
                case_sensitive,
                focus,
                source,
                matches,
                selected,
                anchor_offset,
                read_only,
            } => {
                let mut operation = if key.code == KeyCode::F(3) {
                    Some(if key.modifiers.contains(KeyModifiers::SHIFT) {
                        FindOperation::Previous
                    } else {
                        FindOperation::Next
                    })
                } else {
                    None
                };
                let mut refresh = false;
                match key.code {
                    KeyCode::Tab => {
                        *focus = cycle_find_focus(*focus, *read_only, false);
                    }
                    KeyCode::BackTab => {
                        *focus = cycle_find_focus(*focus, *read_only, true);
                    }
                    KeyCode::Char(' ') if *focus == FindFocus::Case => {
                        *case_sensitive = !*case_sensitive;
                        refresh = true;
                    }
                    KeyCode::Enter => match *focus {
                        FindFocus::Query | FindFocus::Next => {
                            operation = Some(FindOperation::Next);
                        }
                        FindFocus::Replacement | FindFocus::Replace if !*read_only => {
                            operation = Some(FindOperation::Replace);
                        }
                        FindFocus::Case => {
                            *case_sensitive = !*case_sensitive;
                            refresh = true;
                        }
                        FindFocus::Previous => operation = Some(FindOperation::Previous),
                        FindFocus::ReplaceAll if !*read_only => {
                            operation = Some(FindOperation::ReplaceAll);
                        }
                        _ => {}
                    },
                    _ if *focus == FindFocus::Query && edit_text_input(query, key) => {
                        refresh = true;
                    }
                    _ if *focus == FindFocus::Replacement && !*read_only => {
                        let _ = edit_text_input(replacement, key);
                    }
                    _ => {}
                }
                if refresh {
                    *matches = find_document_matches(source, &query.value, *case_sensitive);
                    *selected = initial_document_match_index(matches, *anchor_offset);
                }
                match operation {
                    Some(FindOperation::Previous) => {
                        *selected = cycle_document_match_index(
                            matches.len(),
                            *selected,
                            MatchDirection::Previous,
                        );
                    }
                    Some(FindOperation::Next) => {
                        *selected = cycle_document_match_index(
                            matches.len(),
                            *selected,
                            MatchDirection::Next,
                        );
                    }
                    Some(FindOperation::Replace) => {
                        if let Some(source_match) = (*selected).and_then(|index| matches.get(index))
                        {
                            let (new_source, new_anchor) = self.replace_document_match(
                                source,
                                *source_match,
                                &replacement.value,
                            );
                            *source = new_source;
                            *anchor_offset = new_anchor;
                            *matches = find_document_matches(source, &query.value, *case_sensitive);
                            *selected = initial_document_match_index(matches, *anchor_offset);
                        }
                    }
                    Some(FindOperation::ReplaceAll) if !matches.is_empty() => {
                        let (new_source, new_anchor) =
                            self.replace_all_document_matches(source, matches, &replacement.value);
                        *source = new_source;
                        *anchor_offset = new_anchor;
                        *matches = find_document_matches(source, &query.value, *case_sensitive);
                        *selected = initial_document_match_index(matches, *anchor_offset);
                    }
                    Some(FindOperation::ReplaceAll) | None => {}
                }
                if let Some(source_match) =
                    (*selected).and_then(|index| matches.get(index)).copied()
                {
                    self.select_document_match(source_match);
                }
                true
            }
            Overlay::WorkspaceSearch {
                query,
                filter,
                mode,
                case_sensitive,
                focus,
                results,
                selected,
                status,
            } => {
                let mut submit = false;
                match key.code {
                    KeyCode::Tab => {
                        *focus = cycle_workspace_search_focus(*focus, !results.is_empty(), false);
                    }
                    KeyCode::BackTab => {
                        *focus = cycle_workspace_search_focus(*focus, !results.is_empty(), true);
                    }
                    KeyCode::Up if *focus == WorkspaceSearchFocus::Results => {
                        *selected = selected.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        if !results.is_empty() {
                            if *focus == WorkspaceSearchFocus::Results {
                                *selected = (*selected + 1).min(results.len().saturating_sub(1));
                            } else {
                                *focus = WorkspaceSearchFocus::Results;
                            }
                        }
                    }
                    KeyCode::Left | KeyCode::Right if *focus == WorkspaceSearchFocus::Mode => {
                        *mode = cycle_text_search_mode(*mode, key.code == KeyCode::Left);
                    }
                    KeyCode::Char(' ') if *focus == WorkspaceSearchFocus::Mode => {
                        *mode = cycle_text_search_mode(*mode, false);
                    }
                    KeyCode::Char(' ') if *focus == WorkspaceSearchFocus::Case => {
                        *case_sensitive = !*case_sensitive;
                    }
                    KeyCode::Enter => match *focus {
                        WorkspaceSearchFocus::Query => submit = true,
                        WorkspaceSearchFocus::Mode => {
                            *mode = cycle_text_search_mode(*mode, false);
                        }
                        WorkspaceSearchFocus::Case => {
                            *case_sensitive = !*case_sensitive;
                        }
                        WorkspaceSearchFocus::Results => {
                            if let Some(result) = results.get(*selected).cloned() {
                                self.open_search_result(&result);
                                return;
                            }
                        }
                        WorkspaceSearchFocus::Filter => {}
                    },
                    _ if *focus == WorkspaceSearchFocus::Query => {
                        let _ = edit_text_input(query, key);
                    }
                    _ if *focus == WorkspaceSearchFocus::Filter => {
                        let _ = edit_text_input(filter, key);
                    }
                    _ => {}
                }
                if submit {
                    if query.value.is_empty() {
                        results.clear();
                        "Enter a non-empty query.".clone_into(status);
                    } else {
                        "Searching…".clone_into(status);
                        results.clear();
                        *selected = 0;
                        self.start_workspace_search(
                            &query.value,
                            *mode,
                            *case_sensitive,
                            &filter.value,
                        );
                    }
                }
                true
            }
            Overlay::PathInput { action, input } => match key.code {
                KeyCode::Enter => !self.apply_path_action(*action, &input.value),
                _ if edit_text_input(input, key) => true,
                _ => true,
            },
            Overlay::WorkspaceInput {
                action,
                source,
                input,
            } => match key.code {
                KeyCode::Enter => !self.apply_workspace_input(*action, source, &input.value),
                _ if edit_text_input(input, key) => true,
                _ => true,
            },
            Overlay::TrashConfirm { source, .. } => match key.code {
                KeyCode::Char('y') => {
                    self.trash_workspace_entry(source);
                    false
                }
                _ => true,
            },
            Overlay::Recovery { entry } => match key.code {
                KeyCode::Char('r') => {
                    self.restore_recovery_entry(entry);
                    false
                }
                KeyCode::Char('d') => {
                    let path = entry.document_path.clone();
                    self.published_recovery.insert(
                        path.clone(),
                        (entry.fingerprint().to_owned(), entry.text.clone()),
                    );
                    self.discard_recovery_for(&path);
                    self.status_message = Some("Using the saved disk version".to_owned());
                    false
                }
                _ => true,
            },
            Overlay::RecoveryManager {
                records,
                selected,
                focus,
                target,
                protected_journals,
                retention_days,
                status,
                ..
            } => {
                let record = records.get(*selected).cloned();
                match key.code {
                    KeyCode::Tab | KeyCode::BackTab => {
                        *focus = if *focus == RecoveryManagerFocus::Records {
                            RecoveryManagerFocus::Target
                        } else {
                            RecoveryManagerFocus::Records
                        };
                        true
                    }
                    KeyCode::Up if *focus == RecoveryManagerFocus::Records => {
                        *selected = selected.saturating_sub(1);
                        true
                    }
                    KeyCode::Down if *focus == RecoveryManagerFocus::Records => {
                        *selected = (*selected + 1).min(records.len().saturating_sub(1));
                        true
                    }
                    KeyCode::Home if *focus == RecoveryManagerFocus::Records => {
                        *selected = 0;
                        true
                    }
                    KeyCode::End if *focus == RecoveryManagerFocus::Records => {
                        *selected = records.len().saturating_sub(1);
                        true
                    }
                    KeyCode::Enter if *focus == RecoveryManagerFocus::Target => {
                        if let Some(record) = record {
                            if record.quarantined {
                                self.export_managed_recovery(*selected, &record, &target.value);
                            } else {
                                self.retarget_managed_recovery(*selected, &record, &target.value);
                            }
                        }
                        false
                    }
                    KeyCode::Enter | KeyCode::Char('o')
                        if *focus == RecoveryManagerFocus::Records =>
                    {
                        if let Some(record) = record {
                            self.open_managed_recovery(*selected, &record);
                            false
                        } else {
                            true
                        }
                    }
                    KeyCode::Char('r') if *focus == RecoveryManagerFocus::Records => {
                        if let Some(record) = record {
                            if record.quarantined {
                                self.overlay = Some(Overlay::RecoveryDeleteConfirm {
                                    record: Box::new(record),
                                });
                                false
                            } else if record.entry.is_none() {
                                "Corrupt active recovery can only be archived.".clone_into(status);
                                true
                            } else if protected_journals.contains(&record.journal_path) {
                                "This recovery belongs to an open dirty document."
                                    .clone_into(status);
                                true
                            } else if target.value.trim().is_empty() {
                                *focus = RecoveryManagerFocus::Target;
                                "Enter a new workspace-relative Markdown path.".clone_into(status);
                                true
                            } else {
                                self.retarget_managed_recovery(*selected, &record, &target.value);
                                false
                            }
                        } else {
                            true
                        }
                    }
                    KeyCode::Char('a') if *focus == RecoveryManagerFocus::Records => {
                        if let Some(record) = record {
                            if record.quarantined {
                                if record.entry.is_none() {
                                    "Corrupt quarantine can only be deleted.".clone_into(status);
                                    true
                                } else if target.value.trim().is_empty() {
                                    *focus = RecoveryManagerFocus::Target;
                                    "Enter a workspace-relative path for the exported copy."
                                        .clone_into(status);
                                    true
                                } else {
                                    self.export_managed_recovery(*selected, &record, &target.value);
                                    false
                                }
                            } else {
                                self.archive_managed_recovery(*selected, &record);
                                false
                            }
                        } else {
                            true
                        }
                    }
                    KeyCode::Char('x') if *focus == RecoveryManagerFocus::Records => {
                        let cutoff =
                            OffsetDateTime::now_utc() - TimeDuration::days(*retention_days);
                        let expired = expired_recovery_records(records, cutoff);
                        if expired.is_empty() {
                            "No expired quarantined recoveries.".clone_into(status);
                            true
                        } else {
                            self.overlay = Some(Overlay::RecoveryCleanupConfirm {
                                records: expired,
                                cutoff,
                                retention_days: *retention_days,
                            });
                            false
                        }
                    }
                    _ if *focus == RecoveryManagerFocus::Target && edit_text_input(target, key) => {
                        true
                    }
                    _ => true,
                }
            }
            Overlay::LocalHistory {
                enabled,
                path,
                checkpoints,
                selected,
                focus,
                diff,
                diff_scroll,
                diff_truncated,
                expected_revision,
                ..
            } => {
                if !*enabled {
                    if key.code == KeyCode::Char('e') {
                        self.set_history_enabled(true);
                        false
                    } else {
                        true
                    }
                } else {
                    let previous_selected = *selected;
                    let mut keep = true;
                    match key.code {
                        KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                            *focus = if *focus == HistoryFocus::Checkpoints {
                                HistoryFocus::Diff
                            } else {
                                HistoryFocus::Checkpoints
                            };
                        }
                        KeyCode::Up if *focus == HistoryFocus::Checkpoints => {
                            *selected = selected.saturating_sub(1);
                        }
                        KeyCode::Down if *focus == HistoryFocus::Checkpoints => {
                            *selected = (*selected + 1).min(checkpoints.len().saturating_sub(1));
                        }
                        KeyCode::PageUp if *focus == HistoryFocus::Checkpoints => {
                            *selected = selected.saturating_sub(10);
                        }
                        KeyCode::PageDown if *focus == HistoryFocus::Checkpoints => {
                            *selected = (*selected + 10).min(checkpoints.len().saturating_sub(1));
                        }
                        KeyCode::Home if *focus == HistoryFocus::Checkpoints => {
                            *selected = 0;
                        }
                        KeyCode::End if *focus == HistoryFocus::Checkpoints => {
                            *selected = checkpoints.len().saturating_sub(1);
                        }
                        KeyCode::Up if *focus == HistoryFocus::Diff => {
                            *diff_scroll = diff_scroll.saturating_sub(1);
                        }
                        KeyCode::Down if *focus == HistoryFocus::Diff => {
                            *diff_scroll = (*diff_scroll + 1).min(diff.len().saturating_sub(1));
                        }
                        KeyCode::PageUp if *focus == HistoryFocus::Diff => {
                            *diff_scroll = diff_scroll.saturating_sub(10);
                        }
                        KeyCode::PageDown if *focus == HistoryFocus::Diff => {
                            *diff_scroll = (*diff_scroll + 10).min(diff.len().saturating_sub(1));
                        }
                        KeyCode::Home if *focus == HistoryFocus::Diff => {
                            *diff_scroll = 0;
                        }
                        KeyCode::End if *focus == HistoryFocus::Diff => {
                            *diff_scroll = diff.len().saturating_sub(1);
                        }
                        KeyCode::Enter if !checkpoints.is_empty() => {
                            *focus = HistoryFocus::Diff;
                        }
                        KeyCode::Char('c') => {
                            self.create_manual_checkpoint();
                            keep = false;
                        }
                        KeyCode::Char('r') => {
                            if let Some(checkpoint) = checkpoints.get(*selected) {
                                self.restore_history_checkpoint(
                                    path,
                                    &checkpoint.id,
                                    *expected_revision,
                                );
                                keep = false;
                            }
                        }
                        KeyCode::Char('d') => {
                            self.request_history_clear(HistoryClearScope::Document, path);
                            keep = false;
                        }
                        KeyCode::Char('D') => {
                            self.request_history_clear(HistoryClearScope::Workspace, path);
                            keep = false;
                        }
                        KeyCode::Char('e') => {
                            self.set_history_enabled(false);
                            keep = false;
                        }
                        _ => {}
                    }
                    if keep && *selected != previous_selected {
                        let current = self
                            .active_tab()
                            .map_or("", |tab| tab.document.text.as_str());
                        let (updated, truncated) = checkpoints.get(*selected).map_or_else(
                            || (Vec::new(), false),
                            |checkpoint| build_history_diff(&checkpoint.source, current),
                        );
                        *diff = updated;
                        *diff_truncated = truncated;
                        *diff_scroll = 0;
                    }
                    keep
                }
            }
            Overlay::HistoryClearConfirm {
                scope,
                path,
                checkpoint_ids,
            } => {
                let confirm = match scope {
                    HistoryClearScope::Document => KeyCode::Char('d'),
                    HistoryClearScope::Workspace => KeyCode::Char('D'),
                };
                if key.code == confirm {
                    self.clear_history(*scope, path, checkpoint_ids);
                    false
                } else if key.code == KeyCode::Enter {
                    self.show_local_history(None, Some("Clear canceled.".to_owned()));
                    false
                } else {
                    true
                }
            }
            Overlay::RecoveryDeleteConfirm { record } => match key.code {
                KeyCode::Char('d') => {
                    self.delete_managed_recovery(record);
                    false
                }
                KeyCode::Enter => false,
                _ => true,
            },
            Overlay::RecoveryCleanupConfirm {
                records, cutoff, ..
            } => match key.code {
                KeyCode::Char('d') => {
                    self.cleanup_managed_recovery(records, *cutoff);
                    false
                }
                KeyCode::Enter => false,
                _ => true,
            },
            Overlay::SearchResults { results, selected } => match key.code {
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    true
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(results.len().saturating_sub(1));
                    true
                }
                KeyCode::Enter => {
                    if let Some(result) = results.get(*selected).cloned() {
                        self.open_search_result(&result);
                    }
                    false
                }
                _ => true,
            },
            Overlay::Outline {
                items,
                matches,
                query,
                selected,
            } => match key.code {
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    true
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(matches.len().saturating_sub(1));
                    true
                }
                KeyCode::Enter if key.modifiers == KeyModifiers::CONTROL => {
                    if let Some((line, level, title)) = matches.get(*selected) {
                        self.show_outline_in_preview(*line, *level, title);
                    }
                    false
                }
                KeyCode::Enter => {
                    if let Some((line, _, _)) = matches.get(*selected) {
                        self.jump_to_outline_source(*line);
                    }
                    false
                }
                _ if edit_text_input(query, key) => {
                    *matches = filter_outline_items(items, &query.value);
                    *selected = 0;
                    true
                }
                _ => true,
            },
        };
        if keep && self.overlay.is_none() {
            self.overlay = Some(overlay);
        }
    }

    fn paste_into_overlay(&mut self, text: &str) -> bool {
        let Some(overlay) = self.overlay.as_mut() else {
            return false;
        };
        let mut selected_match = None;
        let handled = match overlay {
            Overlay::Palette { input, selected } => {
                input.insert(text);
                *selected = 0;
                true
            }
            Overlay::FileFinder {
                query,
                filter,
                focus,
                selected,
                error,
            } => {
                match focus {
                    FileFinderFocus::Query => query.insert(text),
                    FileFinderFocus::Filter => filter.insert(text),
                    FileFinderFocus::Results => return false,
                }
                *selected = 0;
                *error = parse_path_filter(Some(&filter.value))
                    .err()
                    .map(|filter_error| filter_error.to_string());
                true
            }
            Overlay::Find {
                query,
                replacement,
                case_sensitive,
                focus,
                source,
                matches,
                selected,
                anchor_offset,
                read_only,
            } => {
                match focus {
                    FindFocus::Query => {
                        query.insert(text);
                        *matches = find_document_matches(source, &query.value, *case_sensitive);
                        *selected = initial_document_match_index(matches, *anchor_offset);
                        selected_match = (*selected).and_then(|index| matches.get(index)).copied();
                    }
                    FindFocus::Replacement if !*read_only => replacement.insert(text),
                    _ => return false,
                }
                true
            }
            Overlay::WorkspaceSearch {
                query,
                filter,
                focus,
                ..
            } => {
                match focus {
                    WorkspaceSearchFocus::Query => query.insert(text),
                    WorkspaceSearchFocus::Filter => filter.insert(text),
                    _ => return false,
                }
                true
            }
            Overlay::PathInput { input, .. } | Overlay::WorkspaceInput { input, .. } => {
                input.insert(text);
                true
            }
            Overlay::RecoveryManager { focus, target, .. }
                if *focus == RecoveryManagerFocus::Target =>
            {
                target.insert(text);
                true
            }
            Overlay::Outline {
                items,
                matches,
                query,
                selected,
            } => {
                query.insert(text);
                *matches = filter_outline_items(items, &query.value);
                *selected = 0;
                true
            }
            _ => false,
        };
        if let Some(source_match) = selected_match {
            self.select_document_match(source_match);
        }
        handled
    }

    #[must_use]
    pub fn file_candidates(&self, query: &str) -> Vec<usize> {
        self.filtered_file_candidates(query, "").unwrap_or_default()
    }

    /// Return official-ranked file candidates after applying an optional path filter.
    ///
    /// # Errors
    ///
    /// Returns the path-filter validation message when the expression is invalid.
    pub fn filtered_file_candidates(
        &self,
        query: &str,
        filter: &str,
    ) -> Result<Vec<usize>, String> {
        let path_filter = parse_path_filter(Some(filter)).map_err(|error| error.to_string())?;
        let matches = search_files_with_filter(
            query,
            &self.entries,
            DEFAULT_FILE_RESULT_LIMIT,
            path_filter.as_ref(),
        );
        Ok(matches
            .into_iter()
            .filter_map(|entry| self.entries.iter().position(|candidate| candidate == entry))
            .collect())
    }

    fn start_workspace_search(
        &mut self,
        query: &str,
        mode: TextSearchMode,
        case_sensitive: bool,
        file_filter: &str,
    ) {
        if query.is_empty() {
            return;
        }
        self.sync_active_document();
        let overrides = self
            .tabs
            .iter()
            .map(|tab| TextSearchOverride {
                path: tab.document.path.clone(),
                text: tab.document.text.clone(),
                prefer_disk: tab.document.text == tab.document.saved_text && !tab.document.conflict,
            })
            .collect::<Vec<_>>();
        let query = query.to_owned();
        let workspace = self.workspace.clone();
        let options = TextSearchOptions {
            mode,
            file_filter: (!file_filter.trim().is_empty()).then(|| file_filter.trim().to_owned()),
            case_sensitive,
        };
        let revision = self
            .workspace_search_revision
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let current_revision = Arc::clone(&self.workspace_search_revision);
        let sender = self.workspace_search_tx.clone();
        let _worker = thread::spawn(move || {
            let cancelled = || current_revision.load(Ordering::Relaxed) != revision;
            let scan = workspace.scan_report();
            let files = scan
                .entries
                .into_iter()
                .filter(|entry| !entry.is_dir)
                .map(|entry| entry.path)
                .collect::<Vec<_>>();
            let mut request = TextSearchRequest::new(&files, &query);
            request.root = Some(&workspace.root);
            request.overrides = &overrides;
            request.options = options;
            request.should_cancel = Some(&cancelled);
            let mut result = search_workspace_text(&request);
            result.warnings.splice(0..0, scan.warnings);
            if !cancelled() {
                let _ = sender.send(WorkspaceSearchCompletion {
                    revision,
                    query,
                    result,
                });
            }
        });
    }

    fn poll_workspace_search_results(&mut self) -> bool {
        let completions = self.workspace_search_rx.try_iter().collect::<Vec<_>>();
        let mut changed = false;
        for completion in completions {
            if completion.revision != self.workspace_search_revision.load(Ordering::Relaxed) {
                continue;
            }
            let Some(Overlay::WorkspaceSearch {
                query,
                filter,
                mode,
                focus,
                results,
                selected,
                status,
                ..
            }) = self.overlay.as_mut()
            else {
                continue;
            };
            if query.value != completion.query {
                continue;
            }
            if let Some(search_error) = completion.result.error {
                results.clear();
                *status = format!("Search failed: {search_error}");
            } else {
                *results = completion.result.matches;
                *selected = 0;
                *status = workspace_search_status(
                    results.len(),
                    *mode,
                    &filter.value,
                    completion.result.warnings.len(),
                );
                if !results.is_empty() {
                    *focus = WorkspaceSearchFocus::Results;
                }
            }
            changed = true;
        }
        changed
    }

    fn open_search_result(&mut self, result: &TextMatch) {
        if let Err(error) = self.open_document(&result.path) {
            self.status_message = Some(format!("Open failed · {error}"));
            return;
        }
        let clean = self.active_tab().is_some_and(|tab| {
            tab.document.text == tab.document.saved_text && !tab.document.conflict
        });
        if clean {
            let _ = self.poll_active_document();
        }
        self.move_editor(CursorMove::Jump(
            u16::try_from(result.line).unwrap_or(u16::MAX),
            u16::try_from(result.column).unwrap_or(u16::MAX),
        ));
    }

    fn poll_external_state(&mut self) -> bool {
        if self.overlay.is_some() {
            return false;
        }
        self.sync_active_document();
        let mut changed = self.poll_active_document();
        changed |= self.poll_next_inactive_document();
        self.request_workspace_scan();
        changed
    }

    fn poll_active_document(&mut self) -> bool {
        let Some(index) = self.active_tab else {
            return false;
        };
        self.poll_document(index, true)
    }

    fn poll_next_inactive_document(&mut self) -> bool {
        let count = self.tabs.len();
        if count < 2 {
            return false;
        }
        for _ in 0..count {
            let index = self.inactive_probe_cursor % count;
            self.inactive_probe_cursor = (index + 1) % count;
            if Some(index) != self.active_tab {
                return self.poll_document(index, false);
            }
        }
        false
    }

    fn poll_document(&mut self, index: usize, active: bool) -> bool {
        let path = self.tabs[index].document.path.clone();
        let baseline = self.tabs[index].document.snapshot.clone();
        let relative = self.workspace.relative(&path);
        match load_file(&path) {
            Ok(loaded) if loaded.snapshot == baseline => {
                if self.tabs[index].document.conflict
                    && !self.tabs[index].document.recovery_conflict
                {
                    self.tabs[index].document.conflict = false;
                    self.status_message = Some("External conflict cleared".to_owned());
                    true
                } else {
                    false
                }
            }
            Ok(loaded) => {
                let same_content = loaded.snapshot.sha256 == baseline.sha256;
                let same_origin = loaded.snapshot.same_origin(&baseline);
                let dirty = self.tabs[index].document.is_dirty();
                if dirty
                    && (self.tabs[index].document.recovery_conflict
                        || !same_content
                        || !same_origin)
                {
                    if !self.tabs[index].document.conflict {
                        self.tabs[index].document.conflict = true;
                        self.status_message = Some(if active {
                            "CONFLICT · file changed outside TermDraft".to_owned()
                        } else {
                            format!(
                                "CONFLICT · {} changed outside TermDraft",
                                relative.display()
                            )
                        });
                        return true;
                    }
                    return false;
                }
                if same_content && same_origin {
                    self.tabs[index].document.snapshot = loaded.snapshot;
                    let cleared = self.tabs[index].document.conflict
                        && !self.tabs[index].document.recovery_conflict;
                    if cleared {
                        self.tabs[index].document.conflict = false;
                        self.status_message = Some("External conflict cleared".to_owned());
                    }
                    return cleared;
                }

                let mixed_target = loaded.mixed_line_ending_target();
                let outgoing = self.tabs[index].document.text.clone();
                let history_note = self.capture_history_note(
                    index,
                    &outgoing,
                    CheckpointReason::BeforeExternalReload,
                );
                if active {
                    self.install_loaded(index, loaded, MixedLineEndingContext::Reload);
                } else {
                    self.install_inactive_loaded(index, loaded);
                }
                let status = mixed_target.map_or_else(
                    || {
                        if active {
                            "Reloaded external changes".to_owned()
                        } else {
                            format!("Reloaded external changes in {}", relative.display())
                        }
                    },
                    |target| {
                        format!(
                            "{} has mixed line endings · target {}",
                            if active {
                                "External file".to_owned()
                            } else {
                                relative.display().to_string()
                            },
                            line_ending_name(target)
                        )
                    },
                );
                self.status_message = Some(append_optional_history_note(
                    status,
                    history_note.as_deref(),
                ));
                true
            }
            Err(error) => {
                if self.tabs[index].document.conflict {
                    false
                } else {
                    self.tabs[index].document.conflict = true;
                    self.status_message = Some(if active {
                        format!("CONFLICT · file unavailable · {error}")
                    } else {
                        format!("CONFLICT · {} unavailable · {error}", relative.display())
                    });
                    true
                }
            }
        }
    }

    fn install_inactive_loaded(&mut self, index: usize, loaded: LoadedFile) {
        let cursor = self.tabs[index].editor.cursor();
        let editor_scroll_x = self.tabs[index].editor_scroll_x;
        let editor_scroll_y = self.tabs[index].editor_scroll_y;
        let preview_scroll_x = self.tabs[index].preview_scroll_x;
        let preview_scroll_y = self.tabs[index].preview_scroll_y;
        let mut tab = EditorTab::from_loaded(loaded, &self.config.editor);
        tab.document
            .continue_source_revision_from(&self.tabs[index].document);
        tab.pending_mixed_open = tab.document.mixed_line_ending_target().is_some();
        style_cursor(&mut tab.editor, self.mode);
        tab.editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
        tab.editor_scroll_x = editor_scroll_x;
        tab.editor_scroll_y = editor_scroll_y;
        tab.preview_scroll_x = preview_scroll_x;
        tab.preview_scroll_y = preview_scroll_y;
        tab.pending_scroll_restore = true;
        self.tabs[index] = tab;
    }
}

fn build_history_diff(checkpoint: &str, current: &str) -> (Vec<HistoryDiffLine>, bool) {
    if checkpoint == current {
        return (
            vec![HistoryDiffLine {
                kind: HistoryDiffKind::Notice,
                text: "No changes from the current source.".to_owned(),
            }],
            false,
        );
    }
    let (checkpoint, checkpoint_truncated) = bounded_history_diff_source(checkpoint);
    let (current, current_truncated) = bounded_history_diff_source(current);
    let diff_lines = diff::lines(&checkpoint, &current);
    let is_changed = diff_lines
        .iter()
        .map(|change| !matches!(change, diff::Result::Both(_, _)))
        .collect::<Vec<_>>();
    if !is_changed.iter().any(|changed| *changed) {
        return (
            vec![HistoryDiffLine {
                kind: HistoryDiffKind::Notice,
                text: "No changes in the compared prefix.".to_owned(),
            }],
            checkpoint_truncated || current_truncated,
        );
    }

    let mut lines = Vec::new();
    let mut truncated = checkpoint_truncated || current_truncated;
    let mut omitted = 0;
    for (index, change) in diff_lines.into_iter().enumerate() {
        let context_start = index.saturating_sub(3);
        let context_end = (index + 4).min(is_changed.len());
        let include = is_changed[index]
            || is_changed[context_start..context_end]
                .iter()
                .any(|line| *line);
        if !include {
            omitted += 1;
            continue;
        }
        if omitted > 0 && lines.len() + 1 < HISTORY_DIFF_MAX_OUTPUT_LINES {
            lines.push(HistoryDiffLine {
                kind: HistoryDiffKind::Notice,
                text: format!("… {omitted} unchanged lines …"),
            });
            omitted = 0;
        }
        if lines.len() == HISTORY_DIFF_MAX_OUTPUT_LINES {
            truncated = true;
            break;
        }
        let (kind, text) = match change {
            diff::Result::Left(line) => (HistoryDiffKind::Removed, line),
            diff::Result::Right(line) => (HistoryDiffKind::Added, line),
            diff::Result::Both(line, _) => (HistoryDiffKind::Context, line),
        };
        lines.push(HistoryDiffLine {
            kind,
            text: text.to_owned(),
        });
    }
    if omitted > 0 && lines.len() < HISTORY_DIFF_MAX_OUTPUT_LINES {
        lines.push(HistoryDiffLine {
            kind: HistoryDiffKind::Notice,
            text: format!("… {omitted} unchanged lines …"),
        });
    }
    (lines, truncated)
}

struct AgentWorkspaceCollector {
    documents: Vec<AgentWorkspaceDocument>,
    warnings: Vec<String>,
    encoded_size: u64,
    response_limit: u64,
    document_limit: usize,
}

impl AgentWorkspaceCollector {
    fn new(document_limit: usize, response_limit: usize) -> Result<Self, AgentResponse> {
        let empty_response = AgentResponse::Workspace {
            documents: Vec::new(),
            warnings: Vec::new(),
        };
        let encoded_size = json_size(&empty_response)
            .map_err(|_| AgentResponse::error("cannot measure the agent workspace response"))?;
        let response_limit = u64::try_from(response_limit).unwrap_or(u64::MAX);
        if encoded_size >= response_limit {
            return Err(AgentResponse::error(
                "agent workspace response exceeds the size limit",
            ));
        }
        Ok(Self {
            documents: Vec::new(),
            warnings: Vec::new(),
            encoded_size,
            response_limit,
            document_limit,
        })
    }

    fn add_warning(&mut self, warning: String) -> Result<(), AgentResponse> {
        let item_size = json_size(&warning)
            .map_err(|_| AgentResponse::error("cannot measure an agent workspace warning"))?;
        self.reserve(item_size, self.warnings.len())?;
        self.warnings.push(warning);
        Ok(())
    }

    fn add_open_document(&mut self, open: AgentOpenDocument) -> Result<(), AgentResponse> {
        if open.source.len() > self.document_limit {
            return Err(agent_document_limit_error(&open.relative));
        }
        self.add_document(AgentWorkspaceDocument {
            path: open.relative.display().to_string(),
            source: open.source,
            revision: open.revision,
            dirty: open.dirty,
            open: true,
            conflict: open.conflict,
        })
    }

    fn add_disk_document(&mut self, entry: &WorkspaceEntry) -> Result<(), AgentResponse> {
        match fs::symlink_metadata(&entry.path) {
            Ok(metadata)
                if metadata.len() > u64::try_from(self.document_limit).unwrap_or(u64::MAX) =>
            {
                return Err(agent_document_limit_error(&entry.relative));
            }
            Ok(_) => {}
            Err(error) => {
                return self
                    .add_warning(format!("Cannot read {}: {error}", entry.relative.display()));
            }
        }
        match load_file_bounded(&entry.path, self.document_limit) {
            Ok(loaded) => {
                let document = loaded.into_document();
                self.add_document(AgentWorkspaceDocument {
                    path: entry.relative.display().to_string(),
                    source: document.text,
                    revision: revision_label(document.source_revision),
                    dirty: false,
                    open: false,
                    conflict: false,
                })
            }
            Err(error) if error.to_string().contains("read limit") => {
                Err(agent_document_limit_error(&entry.relative))
            }
            Err(error) => {
                self.add_warning(format!("Cannot read {}: {error}", entry.relative.display()))
            }
        }
    }

    fn add_document(&mut self, document: AgentWorkspaceDocument) -> Result<(), AgentResponse> {
        let item_size = json_size(&document)
            .map_err(|_| AgentResponse::error("cannot measure an agent workspace document"))?;
        self.reserve(item_size, self.documents.len())?;
        self.documents.push(document);
        Ok(())
    }

    fn reserve(&mut self, item_size: u64, item_count: usize) -> Result<(), AgentResponse> {
        reserve_agent_workspace_item(
            &mut self.encoded_size,
            item_size,
            item_count,
            self.response_limit,
        )
    }

    fn finish(mut self) -> AgentResponse {
        self.documents.sort_by(|left, right| {
            left.path
                .to_lowercase()
                .cmp(&right.path.to_lowercase())
                .then_with(|| left.path.cmp(&right.path))
        });
        AgentResponse::Workspace {
            documents: self.documents,
            warnings: self.warnings,
        }
    }
}

fn agent_workspace_response_with_limits(
    workspace: &Workspace,
    open_documents: Vec<AgentOpenDocument>,
    max_document_bytes: usize,
    max_response_bytes: usize,
    cancelled: impl Fn() -> bool,
) -> AgentResponse {
    let mut collector = match AgentWorkspaceCollector::new(max_document_bytes, max_response_bytes) {
        Ok(collector) => collector,
        Err(response) => return response,
    };
    let scan = workspace.scan_report_until(&cancelled);
    if cancelled() {
        return AgentResponse::error("agent workspace read was cancelled");
    }
    for warning in scan.warnings {
        if let Err(response) = collector.add_warning(warning) {
            return response;
        }
    }

    let mut open_by_path = open_documents
        .into_iter()
        .map(|document| (document.path.clone(), document))
        .collect::<HashMap<_, _>>();
    for entry in scan.entries.into_iter().filter(|entry| !entry.is_dir) {
        if cancelled() {
            return AgentResponse::error("agent workspace read was cancelled");
        }
        let result = if let Some(open) = open_by_path.remove(&entry.path) {
            collector.add_open_document(open)
        } else {
            collector.add_disk_document(&entry)
        };
        if let Err(response) = result {
            return response;
        }
    }

    let mut remaining_open = open_by_path.into_values().collect::<Vec<_>>();
    remaining_open.sort_by(|left, right| {
        left.relative
            .to_string_lossy()
            .to_lowercase()
            .cmp(&right.relative.to_string_lossy().to_lowercase())
    });
    for open in remaining_open {
        if cancelled() {
            return AgentResponse::error("agent workspace read was cancelled");
        }
        if let Err(response) = collector.add_open_document(open) {
            return response;
        }
    }
    collector.finish()
}

fn reserve_agent_workspace_item(
    encoded_size: &mut u64,
    item_size: u64,
    item_count: usize,
    response_limit: u64,
) -> Result<(), AgentResponse> {
    let separator = u64::from(item_count > 0);
    let next_size = encoded_size
        .checked_add(separator)
        .and_then(|size| size.checked_add(item_size))
        .filter(|size| *size < response_limit)
        .ok_or_else(|| AgentResponse::error("agent workspace response exceeds the size limit"))?;
    *encoded_size = next_size;
    Ok(())
}

fn agent_document_limit_error(path: &Path) -> AgentResponse {
    AgentResponse::error(format!(
        "agent workspace document {} exceeds the per-document size limit",
        path.display()
    ))
}

fn agent_workspace_path_is_allowed(relative: &Path) -> bool {
    !relative.as_os_str().is_empty()
        && has_editable_suffix(relative)
        && relative.components().all(|component| match component {
            std::path::Component::Normal(name) => name
                .to_str()
                .is_none_or(|name| !IGNORED_DIRECTORIES.contains(&name)),
            _ => false,
        })
}

fn agent_open_path_is_shareable(path: &Path, workspace_root: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => false,
        Ok(_) => path
            .canonicalize()
            .is_ok_and(|resolved| resolved.starts_with(workspace_root)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn revision_label(revision: SourceRevision) -> String {
    let mut digest = String::with_capacity(revision.sha256.len() * 2);
    for byte in revision.sha256 {
        let _ = write!(digest, "{byte:02x}");
    }
    format!("{}:{digest}", revision.generation)
}

fn sanitized_agent_origin(origin: Option<&str>) -> String {
    let origin = origin
        .unwrap_or("local agent")
        .chars()
        .filter(|character| !character.is_control())
        .take(80)
        .collect::<String>();
    let origin = origin.trim();
    if origin.is_empty() {
        "local agent".to_owned()
    } else {
        origin.to_owned()
    }
}

fn apply_proposal_change(source: &str, change: ProposalChange) -> Result<String, String> {
    match change {
        ProposalChange::ReplaceSource { source } => Ok(source),
        ProposalChange::ReplaceRanges { mut edits } => {
            if edits.len() > 10_000 {
                return Err("the proposal contains too many range edits".to_owned());
            }
            edits.sort_by_key(|edit| (edit.start, edit.end));
            validate_range_edits(source, &edits)?;
            let mut proposed = source.to_owned();
            for edit in edits.into_iter().rev() {
                proposed.replace_range(edit.start..edit.end, &edit.replacement);
            }
            Ok(proposed)
        }
    }
}

fn validate_range_edits(source: &str, edits: &[RangeEdit]) -> Result<(), String> {
    let mut previous_end = 0;
    for edit in edits {
        if edit.start > edit.end || edit.end > source.len() {
            return Err("an agent range is outside the active source".to_owned());
        }
        if !source.is_char_boundary(edit.start) || !source.is_char_boundary(edit.end) {
            return Err("agent ranges must use UTF-8 byte boundaries".to_owned());
        }
        if edit.start < previous_end {
            return Err("agent ranges must not overlap".to_owned());
        }
        previous_end = edit.end;
    }
    Ok(())
}

fn bounded_history_diff_source(source: &str) -> (String, bool) {
    let mut output = String::new();
    for (index, line) in source.split('\n').enumerate() {
        let separator_bytes = usize::from(index > 0);
        if index == HISTORY_DIFF_MAX_SOURCE_LINES
            || output
                .len()
                .saturating_add(separator_bytes)
                .saturating_add(line.len())
                > HISTORY_DIFF_MAX_BYTES
        {
            return (output, true);
        }
        if index > 0 {
            output.push('\n');
        }
        output.push_str(line);
    }
    (output, false)
}

fn sorted_checkpoint_ids(checkpoints: &[Checkpoint]) -> Vec<String> {
    let mut ids = checkpoints
        .iter()
        .map(|checkpoint| checkpoint.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

fn append_history_warnings(mut message: String, warnings: &[CheckpointWarning]) -> String {
    if let Some(warning) = warnings.first() {
        let prefix = if message.is_empty() {
            "Local History warning".to_owned()
        } else {
            format!("{message} · Local History warning")
        };
        message = format!(
            "{prefix}: {}{}",
            warning.message,
            if warnings.len() > 1 {
                format!(" (+{} more)", warnings.len() - 1)
            } else {
                String::new()
            }
        );
    }
    message
}

fn append_history_note(message: String, note: &str) -> String {
    if note.is_empty() {
        message
    } else {
        format!("{message} · {note}")
    }
}

fn append_optional_history_note(message: String, note: Option<&str>) -> String {
    match note {
        Some(note) => append_history_note(message, note),
        None => message,
    }
}

fn expired_recovery_records(
    records: &[RecoveryRecord],
    cutoff: OffsetDateTime,
) -> Vec<RecoveryRecord> {
    records
        .iter()
        .filter(|record| {
            record.quarantined
                && record
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.updated_at < cutoff)
        })
        .cloned()
        .collect()
}

fn disk_state(path: &Path) -> DiskState {
    match load_file(path) {
        Ok(loaded) => DiskState::Current(loaded),
        Err(error) => match fs::symlink_metadata(path) {
            Err(metadata_error) if metadata_error.kind() == io::ErrorKind::NotFound => {
                DiskState::Missing
            }
            _ => DiskState::Unavailable(error.to_string()),
        },
    }
}

fn conflict_kind_for_path(path: &Path) -> ConflictKind {
    match disk_state(path) {
        DiskState::Current(_) => ConflictKind::Changed,
        DiskState::Missing => ConflictKind::Missing,
        DiskState::Unavailable(_) => ConflictKind::Unavailable,
    }
}

const fn line_ending_name(line_ending: LineEnding) -> &'static str {
    match line_ending {
        LineEnding::Crlf => "CRLF",
        LineEnding::Cr => "CR",
        LineEnding::None | LineEnding::Lf | LineEnding::Mixed => "LF",
    }
}

fn created_entry_path(value: &str, folder: bool) -> PathBuf {
    let mut path = PathBuf::from(value);
    if !folder && path.extension().is_none_or(std::ffi::OsStr::is_empty) {
        path.set_extension("md");
    }
    path
}

fn conflict_copy_path(workspace: &Workspace, source: &Path) -> String {
    let stem = source.file_stem().unwrap_or_default().to_string_lossy();
    let extension = source
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let file_name = format!("{stem}-local{extension}");
    workspace
        .relative(source)
        .parent()
        .map_or_else(
            || PathBuf::from(&file_name),
            |parent| parent.join(&file_name),
        )
        .to_string_lossy()
        .into_owned()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn scroll_coordinate(value: f64) -> u16 {
    value.clamp(0.0, f64::from(u16::MAX)).round() as u16
}

fn update_scroll(scroll: &mut u16, code: KeyCode, line_count: usize) {
    let maximum = u16::try_from(line_count.saturating_sub(1)).unwrap_or(u16::MAX);
    *scroll = match code {
        KeyCode::Up => scroll.saturating_sub(1),
        KeyCode::Down => scroll.saturating_add(1).min(maximum),
        KeyCode::PageUp => scroll.saturating_sub(10),
        KeyCode::PageDown => scroll.saturating_add(10).min(maximum),
        KeyCode::Home => 0,
        KeyCode::End => maximum,
        _ => *scroll,
    };
}

fn semantic_reader_line_count(mapping: &SemanticBlockMap) -> usize {
    mapping
        .reader_segments()
        .map(|(segment, _)| {
            segment
                .source
                .split('\n')
                .map(|line| line.chars().count().max(1).div_ceil(68))
                .sum::<usize>()
                + 2
        })
        .sum::<usize>()
        .max(1)
}

fn overlay_list_index(
    row: u16,
    start: u16,
    height: u16,
    count: usize,
    selected: usize,
) -> Option<usize> {
    if count == 0 || row < start || row >= start.saturating_add(height) {
        return None;
    }
    let height = usize::from(height);
    let offset = if selected >= height {
        selected.saturating_add(1).saturating_sub(height)
    } else {
        0
    };
    let index = offset.saturating_add(usize::from(row.saturating_sub(start)));
    (index < count).then_some(index)
}

fn command_at_overlay_position(
    column: u16,
    row: u16,
    inner: Rect,
    commands: &[CommandSpec],
    selected: usize,
) -> Option<usize> {
    let results = Rect::new(
        inner.x,
        inner.y.saturating_add(2),
        inner.width,
        inner.height.saturating_sub(4),
    );
    if !UiRegions::contains(Some(results), column, row) {
        return None;
    }
    if results.width < 58 || results.height < 25 {
        return overlay_list_index(row, results.y, results.height, commands.len(), selected);
    }

    let relative_row = row.saturating_sub(results.y);
    let (groups, group_row) = match relative_row {
        0..=6 => (["DOCUMENT", "NAVIGATE"], relative_row),
        8..=15 => (["FILES", "MODE"], relative_row - 8),
        17.. => (["EDIT", "VIEW"], relative_row - 17),
        _ => return None,
    };
    if group_row == 0 {
        return None;
    }
    let left_width = results.width.saturating_sub(2) / 2;
    let group = if column < results.x.saturating_add(left_width) {
        groups[0]
    } else if column >= results.x.saturating_add(left_width).saturating_add(2) {
        groups[1]
    } else {
        return None;
    };
    commands
        .iter()
        .enumerate()
        .filter(|(_, command)| command.group == group)
        .nth(usize::from(group_row.saturating_sub(1)))
        .map(|(index, _)| index)
}

fn recovery_action_at(
    record: Option<&RecoveryRecord>,
    column: u16,
    retention_days: i64,
) -> Option<KeyCode> {
    let mut actions = Vec::new();
    if record.is_some_and(|record| record.quarantined) {
        if record.is_some_and(|record| record.entry.is_some()) {
            actions.push((KeyCode::Char('o'), "[o] Restore".to_owned()));
        }
        actions.push((KeyCode::Char('r'), "[r] Delete forever".to_owned()));
        if record.is_some_and(|record| record.entry.is_some()) {
            actions.push((KeyCode::Char('a'), "[a] Export copy".to_owned()));
        }
    } else {
        actions.extend([
            (KeyCode::Char('o'), "[o] Open draft".to_owned()),
            (KeyCode::Char('r'), "[r] Retarget".to_owned()),
            (KeyCode::Char('a'), "[a] Archive".to_owned()),
        ]);
    }
    actions.push((
        KeyCode::Char('x'),
        format!("[x] Delete >{retention_days}d (0)"),
    ));
    action_at(column, &actions, 2)
}

fn conflict_action_at(column: u16, can_reload: bool, allow_continue: bool) -> Option<KeyCode> {
    let mut actions = vec![(KeyCode::Char('s'), "s  Save local as…".to_owned())];
    if can_reload {
        actions.push((KeyCode::Char('r'), "r  Reload external".to_owned()));
    }
    if allow_continue {
        actions.push((KeyCode::Char('n'), "n  Continue without copy".to_owned()));
    }
    actions.push((KeyCode::Esc, "Esc  Cancel".to_owned()));
    action_at(column, &actions, 5)
}

fn action_at(column: u16, actions: &[(KeyCode, String)], gap: u16) -> Option<KeyCode> {
    let mut x = 0_u16;
    for (code, label) in actions {
        let width = u16::try_from(UnicodeWidthStr::width(label.as_str())).unwrap_or(u16::MAX);
        if column >= x && column < x.saturating_add(width) {
            return Some(*code);
        }
        x = x.saturating_add(width).saturating_add(gap);
    }
    None
}

fn find_action_at(column: u16, read_only: bool) -> FindFocus {
    match column {
        0..=9 => FindFocus::Previous,
        10..=16 => FindFocus::Next,
        17..=26 if !read_only => FindFocus::Replace,
        _ if !read_only => FindFocus::ReplaceAll,
        _ => FindFocus::Next,
    }
}

fn edit_text_input(input: &mut TextInput, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Left => input.move_left(),
        KeyCode::Right => input.move_right(),
        KeyCode::Home => input.move_home(),
        KeyCode::End => input.move_end(),
        KeyCode::Char(character)
            if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(character, 'a' | 'A') =>
        {
            input.move_home();
        }
        KeyCode::Char(character)
            if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(character, 'e' | 'E') =>
        {
            input.move_end();
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            input.insert(&character.to_string());
        }
        _ => return false,
    }
    true
}

fn selected_text(tab: &EditorTab) -> Option<String> {
    let (start, end) = tab.editor.selection_range()?;
    let source = source_from_textarea(&tab.editor);
    let start = location_to_offset(&source, start);
    let end = location_to_offset(&source, end);
    (start < end).then(|| source.chars().skip(start).take(end - start).collect())
}

fn preview_link_visual_row(rendered: &RenderedMarkdown, link: &PreviewLink, width: usize) -> usize {
    let preceding = rendered
        .text
        .lines
        .iter()
        .take(link.rendered_line)
        .map(|line| ui::preview_line_height(line, width))
        .sum::<usize>();
    preceding + link.start_column / width.max(1)
}

fn filter_outline_items(
    items: &[(usize, usize, String)],
    query: &str,
) -> Vec<(usize, usize, String)> {
    let query = query.trim().to_lowercase();
    items
        .iter()
        .filter(|(_, _, title)| query.is_empty() || title.to_lowercase().contains(&query))
        .cloned()
        .collect()
}

fn editor_clipboard_action(key: KeyEvent) -> Option<EditorClipboardAction> {
    if !editor_clipboard_modifier(key.modifiers) {
        return None;
    }
    match key.code {
        KeyCode::Char('c' | 'C') => Some(EditorClipboardAction::Copy),
        KeyCode::Char('x' | 'X') => Some(EditorClipboardAction::Cut),
        KeyCode::Char('v' | 'V') => Some(EditorClipboardAction::Paste),
        _ => None,
    }
}

fn editor_clipboard_modifier(modifiers: KeyModifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers == KeyModifiers::SUPER
    }
    #[cfg(target_os = "linux")]
    {
        modifiers == KeyModifiers::CONTROL
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = modifiers;
        false
    }
}

#[cfg(target_os = "macos")]
fn write_system_clipboard(text: &str) -> io::Result<()> {
    let mut child = ProcessCommand::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("pbcopy stdin is unavailable"))?;
    std::io::Write::write_all(&mut stdin, text.as_bytes())?;
    drop(stdin);
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("pbcopy exited unsuccessfully"))
    }
}

#[cfg(target_os = "linux")]
fn write_system_clipboard(text: &str) -> io::Result<()> {
    let mut last_error = None;
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match write_linux_clipboard("wl-copy", &[], text) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    if std::env::var_os("DISPLAY").is_some() {
        match write_linux_clipboard("xclip", &["-selection", "clipboard", "-in"], text) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux clipboard requires a Wayland or X11 session",
        )
    }))
}

#[cfg(target_os = "macos")]
fn read_system_clipboard() -> io::Result<String> {
    let output = ProcessCommand::new("pbpaste").output()?;
    if !output.status.success() {
        return Err(io::Error::other("pbpaste exited unsuccessfully"));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

#[cfg(target_os = "linux")]
fn read_system_clipboard() -> io::Result<String> {
    let mut last_error = None;
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match read_linux_clipboard("wl-paste", &["--no-newline"]) {
            Ok(text) => return Ok(text),
            Err(error) => last_error = Some(error),
        }
    }
    if std::env::var_os("DISPLAY").is_some() {
        match read_linux_clipboard("xclip", &["-selection", "clipboard", "-out"]) {
            Ok(text) => return Ok(text),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux clipboard requires a Wayland or X11 session",
        )
    }))
}

#[cfg(target_os = "linux")]
fn write_linux_clipboard(program: &str, arguments: &[&str], text: &str) -> io::Result<()> {
    let mut child = ProcessCommand::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other(format!("{program} stdin is unavailable")))?;
    std::io::Write::write_all(&mut stdin, text.as_bytes())?;
    drop(stdin);
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{program} exited unsuccessfully")))
    }
}

#[cfg(target_os = "linux")]
fn read_linux_clipboard(program: &str, arguments: &[&str]) -> io::Result<String> {
    let output = ProcessCommand::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("{program} exited unsuccessfully")));
    }
    String::from_utf8(output.stdout).map_err(io::Error::other)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn write_system_clipboard(_text: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "system clipboard integration is available on Linux and macOS",
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_system_clipboard() -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "system clipboard integration is available on Linux and macOS",
    ))
}

const fn input_replaces_selection(key: KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab
    )
}

fn cycle_find_focus(current: FindFocus, read_only: bool, reverse: bool) -> FindFocus {
    const EDITABLE: &[FindFocus] = &[
        FindFocus::Query,
        FindFocus::Replacement,
        FindFocus::Case,
        FindFocus::Previous,
        FindFocus::Next,
        FindFocus::Replace,
        FindFocus::ReplaceAll,
    ];
    const READ_ONLY: &[FindFocus] = &[
        FindFocus::Query,
        FindFocus::Case,
        FindFocus::Previous,
        FindFocus::Next,
    ];
    let fields = if read_only { READ_ONLY } else { EDITABLE };
    let current = fields
        .iter()
        .position(|field| *field == current)
        .unwrap_or(0);
    let next = if reverse {
        (current + fields.len() - 1) % fields.len()
    } else {
        (current + 1) % fields.len()
    };
    fields[next]
}

fn cycle_workspace_search_focus(
    current: WorkspaceSearchFocus,
    has_results: bool,
    reverse: bool,
) -> WorkspaceSearchFocus {
    const WITH_RESULTS: &[WorkspaceSearchFocus] = &[
        WorkspaceSearchFocus::Query,
        WorkspaceSearchFocus::Mode,
        WorkspaceSearchFocus::Case,
        WorkspaceSearchFocus::Filter,
        WorkspaceSearchFocus::Results,
    ];
    const WITHOUT_RESULTS: &[WorkspaceSearchFocus] = &[
        WorkspaceSearchFocus::Query,
        WorkspaceSearchFocus::Mode,
        WorkspaceSearchFocus::Case,
        WorkspaceSearchFocus::Filter,
    ];
    let fields = if has_results {
        WITH_RESULTS
    } else {
        WITHOUT_RESULTS
    };
    let current = fields
        .iter()
        .position(|field| *field == current)
        .unwrap_or(0);
    let next = if reverse {
        (current + fields.len() - 1) % fields.len()
    } else {
        (current + 1) % fields.len()
    };
    fields[next]
}

fn cycle_text_search_mode(current: TextSearchMode, reverse: bool) -> TextSearchMode {
    const MODES: &[TextSearchMode] = &[
        TextSearchMode::Literal,
        TextSearchMode::Fuzzy,
        TextSearchMode::WholeWord,
        TextSearchMode::Regex,
    ];
    let current = MODES.iter().position(|mode| *mode == current).unwrap_or(0);
    let next = if reverse {
        (current + MODES.len() - 1) % MODES.len()
    } else {
        (current + 1) % MODES.len()
    };
    MODES[next]
}

#[must_use]
pub const fn text_search_mode_label(mode: TextSearchMode) -> &'static str {
    match mode {
        TextSearchMode::Literal => "literal",
        TextSearchMode::Fuzzy => "fuzzy",
        TextSearchMode::WholeWord => "whole word",
        TextSearchMode::Regex => "regular expression",
    }
}

fn workspace_search_status(
    matches: usize,
    mode: TextSearchMode,
    filter: &str,
    warnings: usize,
) -> String {
    let noun = if matches == 1 { "match" } else { "matches" };
    let mut status = format!("{matches} {noun} · {}", text_search_mode_label(mode));
    if !filter.trim().is_empty() {
        status.push_str(" · ");
        status.push_str(filter.trim());
    }
    if warnings > 0 {
        let noun = if warnings == 1 { "warning" } else { "warnings" };
        let _ = write!(status, " · {warnings} {noun}");
    }
    status
}

const fn is_navigation_action(action: BindingAction) -> bool {
    matches!(
        action,
        BindingAction::CursorLeft
            | BindingAction::CursorDown
            | BindingAction::CursorUp
            | BindingAction::CursorRight
            | BindingAction::LineStart
            | BindingAction::LineEnd
            | BindingAction::DocumentStart
            | BindingAction::DocumentEnd
    )
}

fn path_is_within(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

fn retargeted_path(path: &Path, source: &Path, target: &Path) -> PathBuf {
    path.strip_prefix(source)
        .map_or_else(|_| path.to_path_buf(), |relative| target.join(relative))
}

#[must_use]
pub fn command_candidates(query: &str) -> Vec<CommandSpec> {
    COMMANDS
        .iter()
        .copied()
        .filter(|command| {
            fuzzy_score(query, &format!("{} {}", command.group, command.label)).is_some()
        })
        .collect()
}

/// Run the full-screen Rust application and always restore terminal state.
///
/// # Errors
///
/// Returns an error when initial loading, terminal drawing, or event input fails.
pub fn run(workspace: Workspace) -> anyhow::Result<()> {
    run_with_config(workspace, Config::default())
}

/// Run the full-screen application with a resolved configuration.
///
/// # Errors
///
/// Returns an error when initial loading, terminal drawing, or event input fails.
pub fn run_with_config(workspace: Workspace, config: Config) -> anyhow::Result<()> {
    run_with_options(workspace, config, None)
}

/// Run the application with a visible input/command trace and return its log path.
///
/// # Errors
///
/// Returns an error when the trace cannot be created or the application cannot run.
pub fn run_with_debug(workspace: Workspace, config: Config) -> anyhow::Result<PathBuf> {
    let path = DebugTrace::default_path();
    run_with_options(workspace, config, Some(&path))
        .with_context(|| format!("debug log: {}", path.display()))?;
    Ok(path)
}

#[allow(clippy::too_many_lines)]
fn run_with_options(
    workspace: Workspace,
    config: Config,
    debug_path: Option<&Path>,
) -> anyhow::Result<()> {
    let mut app = App::with_config(workspace, config)?;
    if let Some(path) = debug_path {
        app.enable_debug(path)
            .with_context(|| format!("create debug log {}", path.display()))?;
    }
    let shutdown_requested = install_shutdown_handlers()?;
    ratatui::run(|terminal| -> anyhow::Result<()> {
        let mut terminal_extras = match TerminalExtrasGuard::enable(app.theme) {
            Ok(guard) => guard,
            Err(error) => {
                app.trace_runtime_error(&format!("enable terminal features: {error}"));
                return Err(error.into());
            }
        };
        let result = (|| {
            let mut rendered_mode = app.mode;
            let mut rendered_theme = app.theme;
            let mut needs_draw = true;
            let mut next_disk_poll = Instant::now() + Duration::from_secs(2);
            let mut next_recovery_flush = Instant::now() + Duration::from_millis(500);
            while !app.should_quit {
                if observe_shutdown_request(&mut app, &shutdown_requested) {
                    break;
                }
                app.schedule_preview_render();
                app.start_due_preview_render();
                if needs_draw {
                    if let Err(error) = terminal.draw(|frame| ui::draw(frame, &mut app)) {
                        if is_terminal_disconnect_error(&error) {
                            app.trace_runtime_error(&format!(
                                "terminal disconnected during draw: {error}"
                            ));
                            break;
                        }
                        app.trace_runtime_error(&format!("draw terminal: {error}"));
                        return Err(error.into());
                    }
                    needs_draw = false;
                }
                let poll_timeout = app.preview_poll_timeout();
                let poll_start = Instant::now();
                let received_event = match event::poll(poll_timeout) {
                    Ok(true) => match event::read() {
                        Ok(event) => {
                            app.trace_event(&event);
                            match event {
                                Event::Key(key)
                                    if matches!(
                                        key.kind,
                                        KeyEventKind::Press | KeyEventKind::Repeat
                                    ) =>
                                {
                                    app.handle_key(key);
                                }
                                Event::Paste(text) => {
                                    if !app.paste_into_overlay(&text) {
                                        app.paste_into_document(&text);
                                    }
                                }
                                Event::Mouse(mouse) => app.handle_mouse(mouse),
                                _ => {}
                            }
                            true
                        }
                        Err(_error) if shutdown_requested.load(Ordering::Relaxed) => {
                            app.should_quit = true;
                            false
                        }
                        Err(error) if is_terminal_disconnect_error(&error) => {
                            app.trace_runtime_error(&format!(
                                "terminal disconnected during read: {error}"
                            ));
                            app.should_quit = true;
                            false
                        }
                        Err(error) => {
                            app.trace_runtime_error(&format!("read terminal event: {error}"));
                            return Err(error.into());
                        }
                    },
                    Ok(false) => {
                        if poll_timeout >= Duration::from_millis(10)
                            && poll_start.elapsed() < Duration::from_millis(1)
                            && !io::stdin().is_terminal()
                        {
                            app.trace_runtime_error(
                                "terminal disconnected: stdin is no longer a terminal",
                            );
                            app.should_quit = true;
                        }
                        false
                    }
                    Err(_error) if shutdown_requested.load(Ordering::Relaxed) => {
                        app.should_quit = true;
                        false
                    }
                    Err(error) if is_terminal_disconnect_error(&error) => {
                        app.trace_runtime_error(&format!(
                            "terminal disconnected during poll: {error}"
                        ));
                        app.should_quit = true;
                        false
                    }
                    Err(error) => {
                        app.trace_runtime_error(&format!("poll terminal event: {error}"));
                        return Err(error.into());
                    }
                };
                if app.should_quit {
                    break;
                }
                needs_draw |= received_event && !app.should_quit;
                needs_draw |= app.poll_agent_requests();
                needs_draw |= app.poll_preview_results();
                needs_draw |= app.poll_workspace_search_results();
                needs_draw |= app.poll_workspace_scan_results();
                needs_draw |= app.materialize_next_session_tab();
                if rendered_mode != app.mode {
                    let shape = if app.mode == Mode::Write {
                        SetCursorStyle::BlinkingBar
                    } else {
                        SetCursorStyle::SteadyBlock
                    };
                    if let Err(error) = execute!(stdout(), shape) {
                        if is_terminal_disconnect_error(&error) {
                            app.trace_runtime_error(&format!(
                                "terminal disconnected during cursor mode update: {error}"
                            ));
                            break;
                        }
                        return Err(error.into());
                    }
                    rendered_mode = app.mode;
                }
                if rendered_theme != app.theme {
                    if let Err(error) = set_terminal_cursor_color(&mut stdout(), app.theme) {
                        if is_terminal_disconnect_error(&error) {
                            app.trace_runtime_error(&format!(
                                "terminal disconnected during cursor color update: {error}"
                            ));
                            break;
                        }
                        return Err(error.into());
                    }
                    rendered_theme = app.theme;
                }
                if Instant::now() >= next_disk_poll {
                    if !io::stdin().is_terminal() {
                        app.trace_runtime_error(
                            "terminal disconnected: stdin lost terminal status",
                        );
                        break;
                    }
                    needs_draw |= app.poll_external_state();
                    app.persist_session_if_changed();
                    next_disk_poll = Instant::now() + Duration::from_secs(2);
                }
                if Instant::now() >= next_recovery_flush {
                    app.persist_recovery();
                    next_recovery_flush = Instant::now() + Duration::from_millis(500);
                }
            }
            app.flush_state();
            Ok(())
        })();
        terminal_extras.restore();
        result
    })
}

fn is_terminal_disconnect_error(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe | io::ErrorKind::NotConnected
    ) {
        return true;
    }
    #[cfg(unix)]
    if let Some(raw) = error.raw_os_error() {
        return raw == libc::EIO || raw == libc::EPIPE || raw == libc::EBADF || raw == libc::ENOTTY;
    }
    false
}

fn observe_shutdown_request(app: &mut App, requested: &AtomicBool) -> bool {
    if !requested.load(Ordering::Relaxed) {
        return false;
    }
    app.trace_command("SignalShutdown");
    app.should_quit = true;
    true
}

fn install_shutdown_handlers() -> anyhow::Result<Arc<AtomicBool>> {
    let requested = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        signal_hook::flag::register(SIGTERM, Arc::clone(&requested))
            .context("install SIGTERM shutdown handler")?;
        signal_hook::flag::register(SIGHUP, Arc::clone(&requested))
            .context("install SIGHUP shutdown handler")?;
        signal_hook::flag::register(SIGINT, Arc::clone(&requested))
            .context("install SIGINT shutdown handler")?;
    }
    Ok(requested)
}

struct TerminalExtrasGuard {
    active: bool,
}

impl TerminalExtrasGuard {
    fn enable(theme: Theme) -> io::Result<Self> {
        execute!(
            stdout(),
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        let guard = Self { active: true };
        if let Err(error) = set_terminal_cursor_color(&mut stdout(), theme)
            .and_then(|()| execute!(stdout(), SetCursorStyle::SteadyBlock))
        {
            drop(guard);
            return Err(error);
        }
        Ok(guard)
    }

    fn restore(&mut self) {
        if self.active {
            let _ = execute!(
                stdout(),
                DisableMouseCapture,
                PopKeyboardEnhancementFlags,
                SetCursorStyle::DefaultUserShape
            );
            let _ = reset_terminal_cursor_color(&mut stdout());
            self.active = false;
        }
    }
}

impl Drop for TerminalExtrasGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(
                stdout(),
                DisableMouseCapture,
                PopKeyboardEnhancementFlags,
                SetCursorStyle::DefaultUserShape
            );
            let _ = reset_terminal_cursor_color(&mut stdout());
        }
    }
}

fn set_terminal_cursor_color(output: &mut impl io::Write, theme: Theme) -> io::Result<()> {
    if let Some((red, green, blue)) = theme.terminal_cursor_color() {
        write!(output, "\x1b]12;#{red:02x}{green:02x}{blue:02x}\x1b\\")?;
        output.flush()
    } else {
        reset_terminal_cursor_color(output)
    }
}

fn reset_terminal_cursor_color(output: &mut impl io::Write) -> io::Result<()> {
    output.write_all(b"\x1b]112\x1b\\")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn session_view(path: PathBuf, line: usize, column: usize) -> DocumentViewState {
        DocumentViewState {
            path,
            line,
            column,
            scroll_x: 0.0,
            scroll_y: 0.0,
            preview_scroll_x: 0.0,
            preview_scroll_y: 0.0,
        }
    }

    #[test]
    fn terminal_cursor_color_follows_light_themes_and_resets_for_dark_themes() {
        let mut output = Vec::new();

        set_terminal_cursor_color(&mut output, Theme::Paper).unwrap();
        set_terminal_cursor_color(&mut output, Theme::Linen).unwrap();
        set_terminal_cursor_color(&mut output, Theme::Mist).unwrap();
        set_terminal_cursor_color(&mut output, Theme::Midnight).unwrap();

        assert_eq!(
            output,
            b"\x1b]12;#174d46\x1b\\\x1b]12;#75342e\x1b\\\x1b]12;#164f63\x1b\\\x1b]112\x1b\\"
        );
    }

    #[test]
    fn terminal_disconnect_errors_are_detected() {
        assert!(is_terminal_disconnect_error(&io::Error::from(
            io::ErrorKind::UnexpectedEof
        )));
        assert!(is_terminal_disconnect_error(&io::Error::from(
            io::ErrorKind::BrokenPipe
        )));
        assert!(is_terminal_disconnect_error(&io::Error::from(
            io::ErrorKind::NotConnected
        )));
        #[cfg(unix)]
        {
            assert!(is_terminal_disconnect_error(&io::Error::from_raw_os_error(
                libc::EIO
            )));
            assert!(is_terminal_disconnect_error(&io::Error::from_raw_os_error(
                libc::EPIPE
            )));
            assert!(is_terminal_disconnect_error(&io::Error::from_raw_os_error(
                libc::EBADF
            )));
            assert!(is_terminal_disconnect_error(&io::Error::from_raw_os_error(
                libc::ENOTTY
            )));
        }
        assert!(!is_terminal_disconnect_error(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
        assert!(!is_terminal_disconnect_error(&io::Error::from(
            io::ErrorKind::NotFound
        )));
    }

    #[test]
    fn shutdown_request_observes_signal_flag_and_requests_quit() {
        let mut app = App::new(Workspace::from_target(Path::new(".")).unwrap()).unwrap();
        let flag = AtomicBool::new(false);
        assert!(!observe_shutdown_request(&mut app, &flag));
        assert!(!app.should_quit);

        flag.store(true, Ordering::Relaxed);
        assert!(observe_shutdown_request(&mut app, &flag));
        assert!(app.should_quit);
    }

    fn execute_palette_action(app: &mut App, action: CommandAction) {
        let selected = command_candidates("")
            .iter()
            .position(|command| command.action == action)
            .expect("palette action must be registered");
        app.overlay = Some(Overlay::Palette {
            input: TextInput::default(),
            selected,
        });
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    fn app_with_history(workspace: Workspace, store: CheckpointStore) -> App {
        App::with_state_services_and_history(workspace, Config::default(), None, None, Some(store))
            .unwrap()
    }

    fn finish_workspace_search(app: &mut App) {
        for _ in 0..200 {
            let _ = app.poll_workspace_search_results();
            if matches!(
                &app.overlay,
                Some(Overlay::WorkspaceSearch { status, .. }) if status != "Searching…"
            ) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("workspace search did not finish");
    }

    fn finish_workspace_scan(app: &mut App) {
        for _ in 0..200 {
            let _ = app.poll_workspace_scan_results();
            if !app.workspace_is_indexing() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("workspace scan did not finish");
    }

    #[test]
    fn command_palette_preserves_order_with_the_native_theme_command() {
        let expected = [
            ("DOCUMENT", "Save", "w", CommandAction::Save),
            ("DOCUMENT", "Save as", "W", CommandAction::SaveAs),
            ("DOCUMENT", "Duplicate", "D", CommandAction::Duplicate),
            ("DOCUMENT", "Find file", "f", CommandAction::FileFinder),
            (
                "DOCUMENT",
                "Recent documents",
                "o",
                CommandAction::RecentDocuments,
            ),
            ("DOCUMENT", "Close tab", "C", CommandAction::CloseTab),
            ("NAVIGATE", "Next tab", "]", CommandAction::NextTab),
            ("NAVIGATE", "Previous tab", "[", CommandAction::PreviousTab),
            (
                "NAVIGATE",
                "Search workspace",
                "/",
                CommandAction::WorkspaceSearch,
            ),
            ("NAVIGATE", "Find and replace", "s", CommandAction::Find),
            ("NAVIGATE", "Outline", "S", CommandAction::Outline),
            ("NAVIGATE", "Explorer", "e", CommandAction::ToggleExplorer),
            ("FILES", "Create", "a", CommandAction::Create),
            ("FILES", "Copy", "c", CommandAction::CopyEntry),
            ("FILES", "Cut", "x", CommandAction::CutEntry),
            ("FILES", "Paste", "p", CommandAction::PasteEntry),
            ("FILES", "Rename", "r", CommandAction::RenameEntry),
            ("FILES", "Move", "m", CommandAction::MoveEntry),
            ("FILES", "Trash", "d", CommandAction::TrashEntry),
            ("MODE", "Write mode", "i", CommandAction::WriteMode),
            ("MODE", "Command mode", "Esc", CommandAction::CommandMode),
            ("EDIT", "Undo", "u", CommandAction::Undo),
            ("EDIT", "Redo", "U", CommandAction::Redo),
            (
                "EDIT",
                "Create checkpoint",
                "",
                CommandAction::CreateCheckpoint,
            ),
            (
                "EDIT",
                "Open Local History",
                "",
                CommandAction::LocalHistory,
            ),
            ("EDIT", "Agent sharing", "", CommandAction::AgentSharing),
            ("EDIT", "Reload config", "R", CommandAction::ReloadConfig),
            (
                "EDIT",
                "Inspect blocks",
                "b",
                CommandAction::InspectSemanticBlocks,
            ),
            (
                "EDIT",
                "Read blocks",
                "B",
                CommandAction::ReadSemanticBlocks,
            ),
            ("VIEW", "Preview", "v", CommandAction::TogglePreview),
            ("VIEW", "Change theme", "t", CommandAction::ChangeTheme),
            (
                "VIEW",
                "Recovery drafts",
                "M",
                CommandAction::ManageRecovery,
            ),
            ("VIEW", "Shortcut help", "?", CommandAction::Help),
            ("VIEW", "Markdown help", "K", CommandAction::MarkdownHelp),
            (
                "VIEW",
                "Cursor coordinates",
                "I",
                CommandAction::InspectCursorCoordinates,
            ),
            ("VIEW", "Quit", "q", CommandAction::Quit),
        ];
        let commands = command_candidates("")
            .iter()
            .map(|command| {
                (
                    command.group,
                    command.label,
                    command.shortcut,
                    command.action,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(commands, expected);
    }

    #[test]
    fn theme_command_cycles_all_six_built_in_themes() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::new(workspace).unwrap();

        for expected in Theme::ALL {
            app.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
            assert_eq!(app.theme, expected);
            assert_eq!(
                app.status_message.as_deref(),
                Some(format!(
                    "Theme · {} ({})",
                    expected.name(),
                    if expected.is_light() { "light" } else { "dark" }
                ))
                .as_deref()
            );
        }
    }

    #[test]
    fn selected_theme_is_restored_by_the_next_app_session() {
        let directory = tempfile::tempdir().unwrap();
        let workspace_path = directory.path().join("workspace");
        let config_root = directory.path().join("config");
        fs::create_dir(&workspace_path).unwrap();
        let workspace = Workspace::from_target(&workspace_path).unwrap();
        let config = config::load(config_root.clone()).unwrap();
        let mut app = App::with_state_services(workspace, config, None, None).unwrap();

        execute_palette_action(&mut app, CommandAction::ChangeTheme);
        assert_eq!(app.theme, Theme::Paper);

        let workspace = Workspace::from_target(&workspace_path).unwrap();
        let config = config::load(config_root).unwrap();
        let restored = App::with_state_services(workspace, config, None, None).unwrap();
        assert_eq!(restored.theme, Theme::Paper);
    }

    #[test]
    fn palette_tab_navigation_wraps_both_directions() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        let third = directory.path().join("third.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        fs::write(&third, "third").unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        let third = third.canonicalize().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();
        app.open_document(&first).unwrap();
        app.open_document(&second).unwrap();
        app.open_document(&third).unwrap();
        app.active_tab = Some(1);

        execute_palette_action(&mut app, CommandAction::NextTab);
        assert_eq!(app.active_tab().unwrap().document.path, third);
        assert!(app.overlay.is_none());
        execute_palette_action(&mut app, CommandAction::NextTab);
        assert_eq!(app.active_tab().unwrap().document.path, first);
        execute_palette_action(&mut app, CommandAction::PreviousTab);
        assert_eq!(app.active_tab().unwrap().document.path, third);
    }

    #[test]
    fn palette_reload_config_is_atomic_for_valid_and_invalid_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let config_root = directory.path().join("config");
        fs::create_dir(&config_root).unwrap();
        fs::write(&path, "note").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.root.clone_from(&config_root);
        let mut app = App::with_state_services(workspace, config, None, None).unwrap();

        fs::write(
            config_root.join(config::CONFIG_FILE_NAME),
            r#"[editor]
soft_wrap = false
show_line_numbers = false
view_mode = "split"

[recovery]
retention_days = 45

[keybindings]
command_manage_recovery = "Z"
"#,
        )
        .unwrap();
        execute_palette_action(&mut app, CommandAction::ReloadConfig);

        assert!(!app.config.editor.soft_wrap);
        assert!(!app.config.editor.show_line_numbers);
        assert_eq!(app.config.editor.view_mode, StartupView::Split);
        assert_eq!(app.config.recovery.retention_days, 45);
        assert_eq!(
            app.config
                .keybindings
                .binding("command_manage_recovery")
                .unwrap()
                .text,
            "Z"
        );
        assert_eq!(app.view_mode, ViewMode::Split);
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|message| message.starts_with("Reloaded config.toml"))
        );

        let accepted_editor = app.config.editor.clone();
        let accepted_recovery = app.config.recovery.clone();
        fs::write(
            config_root.join(config::CONFIG_FILE_NAME),
            "[editor]\nview_mode = \"broken\"\n",
        )
        .unwrap();
        execute_palette_action(&mut app, CommandAction::ReloadConfig);

        assert_eq!(app.config.editor, accepted_editor);
        assert_eq!(app.config.recovery, accepted_recovery);
        assert_eq!(
            app.config
                .keybindings
                .binding("command_manage_recovery")
                .unwrap()
                .text,
            "Z"
        );
        assert_eq!(app.view_mode, ViewMode::Split);
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|message| message.starts_with("Configuration not reloaded"))
        );
    }

    #[test]
    fn markdown_help_opens_without_a_document_and_scrolls() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('K'), KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::MarkdownHelp { scroll: 0 })
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::MarkdownHelp { scroll: 1 })
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.overlay.is_none());

        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        assert!(app.overlay.is_none());
        assert_eq!(
            app.status_message.as_deref(),
            Some("Open a Markdown document first")
        );
    }

    #[test]
    fn shortcut_help_scrolls_within_the_rendered_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();

        app.execute_command(CommandAction::Help);
        let Some(Overlay::Help { max_scroll, .. }) = &mut app.overlay else {
            panic!("shortcut help should be open");
        };
        *max_scroll = 6;

        app.handle_overlay_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Some(Overlay::Help { scroll: 6, .. })));
        app.handle_overlay_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Some(Overlay::Help { scroll: 6, .. })));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert!(matches!(app.overlay, Some(Overlay::Help { scroll: 0, .. })));
        app.handle_overlay_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(app.overlay.is_none());
    }

    #[test]
    fn semantic_and_coordinate_overlays_are_read_only_and_navigable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let source = "# Heading\n\n- item\n\né🙂\n";
        fs::write(&path, source).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        let Some(Overlay::SemanticInspector { mapping, selected }) = &app.overlay else {
            panic!("semantic inspector did not open");
        };
        assert_eq!(*selected, 0);
        assert_eq!(mapping.segments()[0].kind.label(), "heading");
        app.handle_overlay_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.overlay.is_none());
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (1, 0));
        assert_eq!(app.focus, Focus::Editor);

        app.handle_key(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE));
        let Some(Overlay::SemanticReader { mapping, scroll }) = &app.overlay else {
            panic!("semantic reader did not open");
        };
        assert_eq!(*scroll, 0);
        assert!(mapping.reader_segments().all(|(segment, _)| {
            segment.kind != crate::semantic_blocks::SemanticBlockKind::Separator
        }));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::SemanticReader { scroll: 1, .. })
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        app.move_editor(CursorMove::Jump(4, 1));
        app.handle_key(KeyEvent::new(KeyCode::Char('I'), KeyModifiers::NONE));
        let Some(Overlay::CoordinateInspector { diagnostic, .. }) = &app.overlay else {
            panic!("coordinate inspector did not open");
        };
        assert_eq!((diagnostic.logical_line, diagnostic.logical_column), (4, 1));
        assert!(diagnostic.utf8_byte_offset > diagnostic.source_offset);
        assert!(diagnostic.grapheme_boundary);

        let tab = app.active_tab().unwrap();
        assert_eq!(tab.document.text, source);
        assert_eq!(tab.document.saved_text, source);
        assert!(!tab.document.is_dirty());
    }

    #[test]
    fn popup_input_edits_unicode_at_the_character_cursor() {
        let mut input = TextInput::default();
        input.insert("café");
        input.move_left();
        input.insert(" noir");
        assert_eq!(input.value, "caf noiré");

        input.backspace();
        assert_eq!(input.value, "caf noié");
        input.delete();
        assert_eq!(input.value, "caf noi");

        input.move_home();
        input.delete();
        input.move_end();
        input.backspace();
        assert_eq!(input.value, "af no");
    }

    #[test]
    fn paste_targets_the_open_popup_instead_of_the_document() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "source").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.set_mode(Mode::Write);
        app.open_document_find();

        assert!(app.paste_into_overlay("café\nneedle"));

        let Some(Overlay::Find { query, .. }) = &app.overlay else {
            panic!("find popup closed unexpectedly");
        };
        assert_eq!(query.value, "caféneedle");
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "source"
        );
    }

    #[test]
    fn file_finder_filters_and_opens_the_first_official_ranked_match() {
        let directory = tempfile::tempdir().unwrap();
        let docs = directory.path().join("docs");
        fs::create_dir(&docs).unwrap();
        let selected = docs.join("selected.md");
        fs::write(&selected, "selected").unwrap();
        fs::write(directory.path().join("excluded.md"), "excluded").unwrap();
        let selected = fs::canonicalize(selected).unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();
        finish_workspace_scan(&mut app);

        app.execute_command(CommandAction::FileFinder);
        let Some(Overlay::FileFinder {
            query,
            filter,
            focus,
            ..
        }) = app.overlay.as_mut()
        else {
            panic!("file finder did not open");
        };
        query.insert("sel");
        filter.insert("docs/**");
        *focus = FileFinderFocus::Query;
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_tab().unwrap().document.path, selected);

        app.execute_command(CommandAction::FileFinder);
        let Some(Overlay::FileFinder { filter, focus, .. }) = app.overlay.as_mut() else {
            panic!("file finder did not reopen");
        };
        filter.insert("*.md,");
        *focus = FileFinderFocus::Filter;
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char(','), KeyModifiers::NONE));
        let Some(Overlay::FileFinder { error, .. }) = &app.overlay else {
            panic!("file finder closed on invalid filter");
        };
        assert!(
            error
                .as_deref()
                .is_some_and(|error| error.contains("empty patterns"))
        );
    }

    #[test]
    fn document_find_wraps_replaces_all_and_undoes_as_one_action() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "one ONE one").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();
        app.move_editor(CursorMove::Jump(0, 4));
        app.execute_command(CommandAction::Find);
        for character in "one".chars() {
            app.handle_overlay_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }

        let Some(Overlay::Find {
            matches, selected, ..
        }) = &app.overlay
        else {
            panic!("find dialog closed while typing");
        };
        assert_eq!(matches.len(), 3);
        assert_eq!(*selected, Some(1));

        app.handle_overlay_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
        let Some(Overlay::Find { selected, .. }) = &app.overlay else {
            panic!("find dialog closed on F3");
        };
        assert_eq!(*selected, Some(2));
        app.handle_overlay_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT));
        let Some(Overlay::Find { selected, .. }) = &app.overlay else {
            panic!("find dialog closed on Shift+F3");
        };
        assert_eq!(*selected, Some(1));

        let Some(Overlay::Find {
            replacement, focus, ..
        }) = app.overlay.as_mut()
        else {
            panic!("find dialog unavailable");
        };
        replacement.insert("x");
        *focus = FindFocus::ReplaceAll;
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.text, "x x x");

        app.handle_overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "one ONE one");
    }

    #[test]
    fn replace_all_preserves_conflict_buffer_editing() {
        for recovery_conflict in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("note.md");
            fs::write(&path, "one one").unwrap();
            let workspace = Workspace::from_target(&path).unwrap();
            let mut app =
                App::with_state_services(workspace, Config::default(), None, None).unwrap();
            let document = &mut app.active_tab_mut().unwrap().document;
            document.conflict = !recovery_conflict;
            document.recovery_conflict = recovery_conflict;

            let source = app.active_tab().unwrap().document.text.clone();
            let matches = find_document_matches(&source, "one", false);
            let (updated, _) = app.replace_all_document_matches(&source, &matches, "x");

            assert_eq!(updated, "x x");
            assert_eq!(app.active_tab().unwrap().document.text, "x x");
            assert_eq!(app.status_message.as_deref(), Some("Replaced 2 matches"));
            assert_eq!(
                app.active_tab().unwrap().document.conflict,
                !recovery_conflict
            );
            assert_eq!(
                app.active_tab().unwrap().document.recovery_conflict,
                recovery_conflict
            );

            app.execute_command(CommandAction::Undo);
            assert_eq!(app.active_tab().unwrap().document.text, "one one");
        }
    }

    #[test]
    fn whole_source_transaction_groups_empty_boundaries_exactly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "prior").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();

        app.active_tab_mut().unwrap().replace_all("").unwrap();
        app.active_tab_mut()
            .unwrap()
            .replace_all("restored")
            .unwrap();
        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "");
        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "prior");

        let empty_path = directory.path().join("empty.md");
        fs::write(&empty_path, "").unwrap();
        app.open_document(&empty_path).unwrap();
        app.active_tab_mut()
            .unwrap()
            .replace_all("current")
            .unwrap();
        app.active_tab_mut().unwrap().replace_all("").unwrap();
        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "current");
        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "");
    }

    #[test]
    fn workspace_search_uses_dirty_tabs_and_reloads_clean_disk_results() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "base").unwrap();
        fs::write(&second, "target needle").unwrap();
        let first = fs::canonicalize(first).unwrap();
        let second = fs::canonicalize(second).unwrap();
        let workspace = Workspace::from_target(&first).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();
        app.active_tab_mut()
            .unwrap()
            .editor
            .insert_str("dirty needle ");
        app.sync_active_document();

        app.execute_command(CommandAction::WorkspaceSearch);
        assert!(app.paste_into_overlay("needle"));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        finish_workspace_search(&mut app);
        let Some(Overlay::WorkspaceSearch {
            results,
            focus,
            selected,
            ..
        }) = app.overlay.as_mut()
        else {
            panic!("workspace search closed after submission");
        };
        assert_eq!(results.len(), 2);
        assert!(
            results.iter().any(|result| {
                result.path == first && result.preview.starts_with("dirty needle")
            })
        );
        *selected = results
            .iter()
            .position(|result| result.path == second)
            .unwrap();
        *focus = WorkspaceSearchFocus::Results;
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.path, second);
        assert_eq!(app.tabs[0].document.text, "dirty needle base");

        fs::write(&second, "new disk needle").unwrap();
        app.execute_command(CommandAction::WorkspaceSearch);
        assert!(app.paste_into_overlay("new disk needle"));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        finish_workspace_search(&mut app);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.text, "new disk needle");

        let fresh = app.workspace.root.join("fresh.md");
        fs::write(&fresh, "brand new search target").unwrap();
        assert!(!app.entries.iter().any(|entry| entry.path == fresh));
        app.execute_command(CommandAction::WorkspaceSearch);
        assert!(app.paste_into_overlay("brand new search target"));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        finish_workspace_search(&mut app);
        let Some(Overlay::WorkspaceSearch { results, .. }) = &app.overlay else {
            panic!("workspace search closed after fresh-file submission");
        };
        assert!(results.iter().any(|result| result.path == fresh));
    }

    #[test]
    fn invalid_path_keeps_the_popup_open_for_correction() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "source").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        let mut input = TextInput::default();
        input.insert("not-supported.png");
        app.overlay = Some(Overlay::PathInput {
            action: PathAction::Create,
            input,
        });

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(app.overlay, Some(Overlay::PathInput { .. })));
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|message| message.contains("unsupported"))
        );
    }

    #[test]
    fn opening_same_file_reuses_tab() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "hello").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&path).unwrap();
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn write_mode_edits_and_atomic_save_reaches_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "a").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut()
            .unwrap()
            .editor
            .move_cursor(CursorMove::End);

        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));

        assert_eq!(fs::read_to_string(path).unwrap(), "ab");
        assert!(!app.active_tab().unwrap().document.is_dirty());
    }

    #[test]
    fn quit_with_dirty_document_opens_guard() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "a").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_char('x');
        app.sync_active_document();

        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));

        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(ConfirmAction::Quit))
        ));
        assert!(!app.should_quit);
    }

    #[test]
    fn dirty_guard_never_discards_on_enter_and_y_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "disk").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("local ");
        app.sync_active_document();
        app.request_quit();

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(ConfirmAction::Quit))
        ));
        assert!(!app.should_quit);
        assert_eq!(fs::read_to_string(&path).unwrap(), "disk");

        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(app.should_quit);
        assert_eq!(fs::read_to_string(&path).unwrap(), "local disk");
    }

    #[test]
    fn quit_saves_one_dirty_tab_then_cancel_preserves_the_next() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        let workspace = Workspace::from_target(&first).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        app.sync_active_document();
        app.open_document(&second).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("saved ");

        app.request_quit();
        assert_eq!(app.active_tab().unwrap().document.path, second);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(ConfirmAction::Quit))
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));

        assert_eq!(fs::read_to_string(&second).unwrap(), "saved second");
        assert_eq!(app.active_tab().unwrap().document.path, first);
        assert!(app.active_tab().unwrap().document.is_dirty());
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(ConfirmAction::Quit))
        ));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(!app.should_quit);
        assert!(app.pending_transition.is_none());
        assert_eq!(app.active_tab().unwrap().document.path, first);
        assert!(app.active_tab().unwrap().document.is_dirty());
        assert_eq!(fs::read_to_string(first).unwrap(), "first");
    }

    #[test]
    fn quit_can_save_one_dirty_tab_and_discard_another() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        let workspace = Workspace::from_target(&first).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut()
            .unwrap()
            .editor
            .insert_str("discarded ");
        app.sync_active_document();
        app.open_document(&second).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("saved ");

        app.request_quit();
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.path, first);
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

        assert!(app.should_quit);
        assert!(app.pending_transition.is_none());
        assert_eq!(app.active_tab().unwrap().document.path, second);
        assert_eq!(fs::read_to_string(first).unwrap(), "first");
        assert_eq!(fs::read_to_string(second).unwrap(), "saved second");
    }

    #[test]
    fn mixed_line_endings_cannot_inherit_write_mode() {
        let directory = tempfile::tempdir().unwrap();
        let normal = directory.path().join("normal.md");
        let mixed = directory.path().join("mixed.md");
        fs::write(&normal, "normal").unwrap();
        fs::write(&mixed, b"one\r\ntwo\n").unwrap();
        let workspace = Workspace::from_target(&normal).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.set_mode(Mode::Write);

        app.open_document(&mixed).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::Command);
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "one\ntwo\n"
        );
    }

    #[test]
    fn configured_write_mode_survives_an_empty_directory_launch() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut config = Config::default();
        config.editor.startup_mode = StartupMode::Write;

        let app = App::with_config(workspace, config).unwrap();

        assert_eq!(app.mode, Mode::Write);
    }

    #[test]
    fn configured_startup_and_markdown_continuation_reach_the_editor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "- item").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.editor.startup_mode = StartupMode::Write;
        config.editor.view_mode = StartupView::Split;
        config.editor.show_line_numbers = false;
        let mut app = App::with_config(workspace, config).unwrap();
        app.active_tab_mut()
            .unwrap()
            .editor
            .move_cursor(CursorMove::End);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.mode, Mode::Write);
        assert_eq!(app.view_mode, ViewMode::Split);
        assert_eq!(app.active_tab().unwrap().editor.line_number_style(), None);
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "- item\n- "
        );
    }

    #[test]
    fn modified_enter_bypasses_markdown_continuation() {
        for modifiers in [
            KeyModifiers::SHIFT,
            KeyModifiers::SUPER,
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("note.md");
            fs::write(&path, "- item").unwrap();
            let workspace = Workspace::from_target(&path).unwrap();
            let mut config = Config::default();
            config.editor.startup_mode = StartupMode::Write;
            let mut app = App::with_config(workspace, config).unwrap();
            app.active_tab_mut()
                .unwrap()
                .editor
                .move_cursor(CursorMove::End);

            app.handle_key(KeyEvent::new(KeyCode::Enter, modifiers));

            assert_eq!(
                source_from_textarea(&app.active_tab().unwrap().editor),
                "- item\n",
                "{modifiers:?}+Enter should reach the editor without list continuation"
            );
        }
    }

    #[test]
    fn preview_toggle_matches_official_wide_and_narrow_layouts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# Preview\n\nbody").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.update_viewport_width(120);

        assert!(app.editor_is_visible());
        assert!(!app.preview_is_visible());
        app.toggle_preview();
        assert!(!app.editor_is_visible());
        assert!(app.preview_is_visible());
        assert_eq!(app.focus, Focus::Preview);
        app.toggle_preview();
        assert!(app.editor_is_visible());
        assert_eq!(app.focus, Focus::Editor);

        app.view_mode = ViewMode::Split;
        assert!(app.editor_is_visible());
        assert!(app.preview_is_visible());
        app.toggle_preview();
        assert!(app.editor_is_visible());
        assert!(!app.preview_is_visible());
        app.toggle_preview();
        assert!(app.editor_is_visible());
        assert!(app.preview_is_visible());
        assert_eq!(app.focus, Focus::Preview);

        app.update_viewport_width(99);
        assert!(app.editor_is_visible());
        assert!(!app.preview_is_visible());
        assert_eq!(app.focus, Focus::Editor);
        app.toggle_preview();
        assert!(!app.editor_is_visible());
        assert!(app.preview_is_visible());
        assert_eq!(app.focus, Focus::Preview);
    }

    #[test]
    fn opening_preview_tracks_the_editor_cursor_line() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let source = (0..40)
            .map(|line| format!("Line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, source).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();

        app.active_tab_mut()
            .unwrap()
            .editor
            .move_cursor(CursorMove::Jump(18, 0));
        app.toggle_preview();
        assert_eq!(app.preview_scroll, 18);

        app.toggle_preview();
        app.active_tab_mut()
            .unwrap()
            .editor
            .move_cursor(CursorMove::Jump(7, 0));
        app.toggle_preview();
        assert_eq!(app.preview_scroll, 7);
    }

    #[test]
    fn focused_preview_navigation_clamps_to_rendered_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "preview").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.focus = Focus::Preview;
        app.preview_max_scroll = 30;
        app.preview_page = 10;

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.preview_scroll, 11);
        app.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.preview_scroll, 30);
        app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(app.preview_scroll, 20);
        app.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.preview_scroll, 0);
    }

    #[test]
    fn focused_preview_scrolls_wide_tables_horizontally() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "preview").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.focus = Focus::Preview;
        app.preview_horizontal_max_scroll = 30;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(app.preview_horizontal_scroll, 2);
        app.handle_key(KeyEvent::new(KeyCode::Char('$'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.preview_horizontal_scroll, 30);
        app.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert_eq!(app.preview_horizontal_scroll, 29);
        app.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.preview_horizontal_scroll, 0);
    }

    #[test]
    fn preview_keyboard_selects_links_and_navigates_footnotes_internally() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let filler = (0..20)
            .map(|index| format!("Paragraph {index}."))
            .collect::<Vec<_>>()
            .join("\n\n");
        fs::write(
            &path,
            format!(
                "Start[^note] and [external](https://example.com).\n\n{filler}\n\n[^note]: Detail."
            ),
        )
        .unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.focus = Focus::Preview;
        app.preview_max_scroll = 100;
        app.preview_page = 5;
        app.ui_regions.preview = Some(Rect::new(0, 0, 60, 10));

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.preview_selected_link, Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.preview_selected_link, Some(2));
        assert!(app.preview_scroll > 0);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.preview_selected_link, Some(0));
        assert_eq!(app.preview_scroll, 0);

        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.preview_selected_link, Some(1));
        assert_eq!(
            app.status_message.as_deref(),
            Some("External links are intentionally inert in preview")
        );
    }

    #[test]
    fn preview_mouse_click_activates_a_rendered_footnote_link() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "Read[^note].\n\n[^note]: Detail.").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.ui_regions.preview = Some(Rect::new(0, 0, 60, 10));
        app.preview_max_scroll = 10;
        app.preview_page = 5;
        let content = ui::preview_content_area(app.ui_regions.preview.unwrap());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: content.x + 4,
            row: content.y,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.focus, Focus::Preview);
        assert_eq!(app.preview_selected_link, Some(1));
        assert_eq!(app.status_message.as_deref(), Some("Footnote note"));
    }

    #[test]
    fn outline_filters_unicode_labels_and_jumps_to_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# Overview\n\n## Résumé Details\n\n### Finish\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();

        app.execute_command(CommandAction::Outline);
        assert!(app.paste_into_overlay("RÉSUMÉ"));
        let Some(Overlay::Outline {
            matches, selected, ..
        }) = &app.overlay
        else {
            panic!("outline should be open");
        };
        assert_eq!(*selected, 0);
        assert_eq!(matches, &[(2, 2, "Résumé Details".to_owned())]);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.overlay.is_none());
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (2, 0));
    }

    #[test]
    fn outline_can_reveal_a_filtered_heading_in_narrow_preview() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let filler = (0..12)
            .map(|index| format!("Paragraph {index}."))
            .collect::<Vec<_>>()
            .join("\n\n");
        fs::write(&path, format!("# Overview\n\n{filler}\n\n## Details\n")).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.viewport_width = 70;
        app.ui_regions.preview = Some(Rect::new(0, 0, 70, 12));

        app.execute_command(CommandAction::Outline);
        assert!(app.paste_into_overlay("details"));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));

        assert!(app.overlay.is_none());
        assert_eq!(app.focus, Focus::Preview);
        assert!(app.preview_is_visible());
        assert!(!app.editor_is_visible());
        assert!(app.preview_scroll > 0);
        assert_eq!(app.status_message.as_deref(), Some("H2 · Details"));
    }

    #[test]
    fn mouse_focus_scroll_resize_and_double_click_reach_the_workbench() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.ui_regions = UiRegions {
            workspace: Rect::new(0, 2, 120, 20),
            explorer: Some(Rect::new(0, 2, 34, 20)),
            explorer_list: Some(Rect::new(0, 3, 34, 19)),
            explorer_divider: Some(Rect::new(34, 2, 1, 20)),
            workbench: Rect::new(35, 2, 85, 20),
            editor: Some(Rect::new(35, 2, 42, 20)),
            preview: Some(Rect::new(78, 2, 42, 20)),
            workbench_divider: Some(Rect::new(77, 2, 1, 20)),
            ..UiRegions::default()
        };
        let mouse = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 4));
        assert_eq!(app.focus, Focus::Explorer);
        assert_eq!(app.explorer_state.selected(), Some(1));
        assert!(app.active_tab().is_none());
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 4));
        assert_eq!(
            app.active_tab().unwrap().document.path,
            second.canonicalize().unwrap()
        );

        app.preview_max_scroll = 10;
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 90, 10));
        assert_eq!(app.preview_scroll, 2);
        app.preview_horizontal_max_scroll = 10;
        app.handle_mouse(mouse(MouseEventKind::ScrollRight, 90, 10));
        assert_eq!(app.preview_horizontal_scroll, 2);
        app.handle_mouse(mouse(MouseEventKind::ScrollLeft, 90, 10));
        assert_eq!(app.preview_horizontal_scroll, 0);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 90, 10));
        assert_eq!(app.focus, Focus::Preview);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 50, 10));
        assert_eq!(app.focus, Focus::Editor);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 33, 10));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 47, 10));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 47, 10));
        assert_eq!(app.explorer_width, 47);
        assert_eq!(
            app.status_message.as_deref(),
            Some("Files width · 47 columns")
        );

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 77, 10));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 80, 10));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 80, 10));
        assert!(app.split_percent > 50);
    }

    #[test]
    fn mouse_click_activates_a_document_tab() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        let workspace = Workspace::from_target(&first).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&second).unwrap();
        assert_eq!(app.active_tab, Some(1));

        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let tabs = app.ui_regions.tabs.unwrap();
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tabs.x.saturating_add(2),
            row: tabs.y,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.active_tab, Some(0));
        assert_eq!(
            app.active_tab().unwrap().document.path.file_name(),
            first.file_name()
        );

        app.agent_session = Some(AgentSession::start(app.workspace.root.clone()).unwrap());
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let tabs = app.ui_regions.tabs.unwrap();
        let first_label = format!(
            " {} ↔ ",
            first.file_name().unwrap_or_default().to_string_lossy()
        );
        let second_label = format!(
            " {} ↔ ",
            second.file_name().unwrap_or_default().to_string_lossy()
        );
        let second_right_edge = tabs
            .x
            .saturating_add(u16::try_from(UnicodeWidthStr::width(first_label.as_str())).unwrap())
            .saturating_add(3)
            .saturating_add(u16::try_from(UnicodeWidthStr::width(second_label.as_str())).unwrap())
            .saturating_sub(1);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: second_right_edge,
            row: tabs.y,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.active_tab, Some(1));
    }

    #[test]
    fn mouse_selects_and_opens_an_outline_row() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# First\n\n## Second\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.execute_command(CommandAction::Outline);

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let inner = ui::overlay_inner_area(app.ui_regions.overlay.unwrap());
        let row = inner.y.saturating_add(4);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x.saturating_add(2),
            row,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(click);
        let Some(Overlay::Outline { selected, .. }) = &app.overlay else {
            panic!("outline should stay open after one click");
        };
        assert_eq!(*selected, 1);

        app.handle_mouse(click);
        assert!(app.overlay.is_none());
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (2, 0));
    }

    #[test]
    fn mouse_selects_and_opens_a_palette_command() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "draft").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.overlay = Some(Overlay::Palette {
            input: TextInput::default(),
            selected: 0,
        });

        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let inner = ui::overlay_inner_area(app.ui_regions.overlay.unwrap());
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: inner.x.saturating_add(2),
            row: inner.y.saturating_add(3),
            modifiers: KeyModifiers::NONE,
        };
        app.handle_mouse(click);
        let Some(Overlay::Palette { selected, .. }) = &app.overlay else {
            panic!("palette should stay open after one click");
        };
        assert_eq!(*selected, 1);

        app.handle_mouse(click);
        assert!(matches!(
            app.overlay,
            Some(Overlay::PathInput {
                action: PathAction::SaveAs,
                ..
            })
        ));
    }

    #[test]
    fn mouse_click_places_the_source_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "alpha\nbravo\ncharlie").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.view_mode = ViewMode::Split;
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let editor = ui::editor_area(app.ui_regions.editor.unwrap());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: editor.x.saturating_add(6),
            row: editor.y.saturating_add(1),
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (1, 3));
    }

    #[test]
    fn mouse_click_places_the_hybrid_cursor_on_the_visible_line() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# First\n\n**Second**").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.view_mode = ViewMode::Inline;
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let editor = ui::editor_area(app.ui_regions.editor.unwrap());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: editor.x.saturating_add(5),
            row: editor.y.saturating_add(2),
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.active_tab().unwrap().editor.cursor().0, 2);
    }

    #[test]
    fn mouse_click_places_the_hybrid_cursor_low_in_the_viewport() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let source = (0..20)
            .map(|line| format!("Line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, source).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.view_mode = ViewMode::Inline;
        let mut terminal = Terminal::new(TestBackend::new(100, 18)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let editor = ui::editor_area(app.ui_regions.editor.unwrap());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: editor.x.saturating_add(8),
            row: editor.y.saturating_add(10),
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.active_tab().unwrap().editor.cursor().0, 10);
    }

    #[test]
    fn mouse_click_places_the_hybrid_cursor_after_wrapped_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let source = "# TermDraft Markdown gallery\n\nOpen this file in TermDraft. With the default Inline configuration, `v` switches between the\nsource editor and rendered preview. With `editor.view_mode = \"split\"` in a wide terminal, `v`\nshows or hides the preview beside the source editor:\n\n```bash\ncargo run --release --locked -- docs/markdown-gallery.md\n```\n\nIf `termdraft` is installed, use `termdraft docs/markdown-gallery.md` instead.";
        fs::write(&path, source).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.view_mode = ViewMode::Inline;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let editor = ui::editor_area(app.ui_regions.editor.unwrap());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: editor.x.saturating_add(15),
            row: editor.y.saturating_add(16),
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.active_tab().unwrap().editor.cursor().0, 10);
    }

    #[test]
    fn mouse_click_on_preview_relocates_the_source_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# First\n\nSecond paragraph\n\nThird").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.view_mode = ViewMode::Split;
        let mut terminal = Terminal::new(TestBackend::new(120, 18)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let preview = ui::preview_content_area(app.ui_regions.preview.unwrap());

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: preview.x.saturating_add(3),
            row: preview.y.saturating_add(2),
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.focus, Focus::Preview);
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (2, 0));
    }

    #[test]
    fn hybrid_write_mode_drag_selects_source_and_keeps_the_selection_visible() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "alpha bravo\n**charlie**").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.editor.startup_mode = StartupMode::Write;
        let mut app = App::with_config(workspace, config).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let cursor = app
            .active_tab()
            .unwrap()
            .inline_editor
            .rendered_cursor_position()
            .unwrap();
        let mouse = |kind, column| MouseEvent {
            kind,
            column,
            row: cursor.y,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), cursor.x));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), cursor.x + 5));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), cursor.x + 5));

        let tab = app.active_tab().unwrap();
        assert_eq!(selected_text(tab).as_deref(), Some("alpha"));
        assert_eq!(
            tab.inline_editor.selection_range(),
            tab.editor.selection_range()
        );
    }

    #[test]
    fn hybrid_write_mode_drag_selects_across_rendered_lines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "alpha bravo\n**charlie**\nthird").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.editor.startup_mode = StartupMode::Write;
        let mut app = App::with_config(workspace, config).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let cursor = app
            .active_tab()
            .unwrap()
            .inline_editor
            .rendered_cursor_position()
            .unwrap();
        let mouse = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            cursor.x,
            cursor.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            cursor.x.saturating_add(3),
            cursor.y.saturating_add(1),
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            cursor.x.saturating_add(3),
            cursor.y.saturating_add(1),
        ));

        let tab = app.active_tab().unwrap();
        assert_eq!(selected_text(tab).as_deref(), Some("alpha bravo\n**c"));
        assert_eq!(
            tab.inline_editor.selection_range(),
            tab.editor.selection_range()
        );
    }

    #[test]
    fn hybrid_write_mode_paste_and_command_undo_share_grouped_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "alpha bravo").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.editor.startup_mode = StartupMode::Write;
        let mut app = App::with_config(workspace, config).unwrap();
        let tab = app.active_tab_mut().unwrap();
        tab.editor.start_selection();
        tab.editor.move_cursor(CursorMove::Jump(0, 5));

        app.paste_into_document("café");
        assert_eq!(app.active_tab().unwrap().document.text, "café bravo");
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER));

        assert_eq!(app.active_tab().unwrap().document.text, "alpha bravo");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn editor_clipboard_shortcuts_use_the_platform_modifier() {
        #[cfg(target_os = "linux")]
        let (expected, rejected) = (KeyModifiers::CONTROL, KeyModifiers::SUPER);
        #[cfg(target_os = "macos")]
        let (expected, rejected) = (KeyModifiers::SUPER, KeyModifiers::CONTROL);

        for (code, action) in [
            (KeyCode::Char('c'), EditorClipboardAction::Copy),
            (KeyCode::Char('x'), EditorClipboardAction::Cut),
            (KeyCode::Char('v'), EditorClipboardAction::Paste),
        ] {
            assert_eq!(
                editor_clipboard_action(KeyEvent::new(code, expected)),
                Some(action)
            );
            assert_eq!(editor_clipboard_action(KeyEvent::new(code, rejected)), None);
            assert_eq!(
                editor_clipboard_action(KeyEvent::new(code, expected | KeyModifiers::SHIFT)),
                None
            );
        }
    }

    #[test]
    fn hybrid_write_mode_cut_is_one_undo_group() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "alpha bravo").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.editor.startup_mode = StartupMode::Write;
        let mut app = App::with_config(workspace, config).unwrap();
        let tab = app.active_tab_mut().unwrap();
        tab.editor.start_selection();
        tab.editor.move_cursor(CursorMove::Jump(0, 5));

        assert!(tab.cut_selection());
        assert_eq!(tab.document.text, " bravo");
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::SUPER));

        assert_eq!(app.active_tab().unwrap().document.text, "alpha bravo");
    }

    #[test]
    fn hybrid_mouse_scroll_survives_redraws() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let source = std::iter::once("TOP_MARKER".to_owned())
            .chain((1..40).map(|line| format!("# Line {line:02}")))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, source).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let mouse = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let editor = app.ui_regions.editor.unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            editor.x.saturating_add(2),
            editor.y.saturating_add(2),
        ));

        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let after_scroll = terminal.backend().buffer().clone();
        let screen = after_scroll
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert_eq!(app.active_tab().unwrap().editor.cursor().0, 1);
        assert!(!screen.contains("TOP_MARKER"));
        assert!(screen.contains("# Line 01"));

        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert_eq!(terminal.backend().buffer(), &after_scroll);

        app.handle_mouse(mouse(
            MouseEventKind::ScrollUp,
            editor.x.saturating_add(2),
            editor.y.saturating_add(2),
        ));
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let after_scroll_up = terminal.backend().buffer().clone();
        let screen = after_scroll_up
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert_eq!(app.active_tab().unwrap().editor.cursor().0, 1);
        assert!(screen.contains("TOP_MARKER"));
        assert!(screen.contains("# Line 01"));

        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        assert_eq!(terminal.backend().buffer(), &after_scroll_up);
    }

    #[test]
    fn explorer_width_can_be_controlled_from_the_keyboard() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::new(workspace).unwrap();
        finish_workspace_scan(&mut app);
        app.focus = Focus::Explorer;
        app.update_viewport_width(120);

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.explorer_width, EXPLORER_DEFAULT_WIDTH + 2);
        assert_eq!(
            app.status_message.as_deref(),
            Some("Files width · 36 columns")
        );

        for _ in 0..20 {
            app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        }
        assert_eq!(app.explorer_width, EXPLORER_MIN_WIDTH);
    }

    #[test]
    fn explorer_starts_collapsed_and_folders_can_be_toggled() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("notes/2026")).unwrap();
        fs::write(directory.path().join("notes/2026/draft.md"), "draft").unwrap();
        fs::write(directory.path().join("readme.md"), "readme").unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::new(workspace).unwrap();
        finish_workspace_scan(&mut app);
        app.focus = Focus::Explorer;
        let visible_paths = |app: &App| {
            app.visible_entry_indices()
                .into_iter()
                .map(|index| app.entries[index].relative.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            visible_paths(&app),
            [PathBuf::from("notes"), PathBuf::from("readme.md")]
        );
        assert!(!app.file_candidates("draft").is_empty());

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            visible_paths(&app),
            [
                PathBuf::from("notes"),
                PathBuf::from("notes/2026"),
                PathBuf::from("readme.md"),
            ]
        );

        app.explorer_state.select(Some(1));
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(visible_paths(&app).contains(&PathBuf::from("notes/2026/draft.md")));
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(!visible_paths(&app).contains(&PathBuf::from("notes/2026/draft.md")));

        app.explorer_state.select(Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            visible_paths(&app),
            [PathBuf::from("notes"), PathBuf::from("readme.md")]
        );
        assert_eq!(app.status_message.as_deref(), Some("notes · collapsed"));
    }

    #[test]
    fn effective_keymap_drives_exact_global_and_command_shortcuts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "disk").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let config = Config {
            keybindings: crate::bindings::Keymap::resolve(
                &[("command_save".to_owned(), "z".to_owned())]
                    .into_iter()
                    .collect(),
            )
            .unwrap(),
            ..Config::default()
        };
        let mut app = App::with_config(workspace, config).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("local ");

        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert_eq!(fs::read_to_string(&path).unwrap(), "disk");
        app.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
        assert_eq!(fs::read_to_string(&path).unwrap(), "local disk");

        app.handle_key(KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert!(matches!(app.overlay, Some(Overlay::WorkspaceSearch { .. })));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(matches!(app.overlay, Some(Overlay::Find { .. })));
    }

    #[test]
    fn command_mode_unbound_arrows_navigate_and_bound_arrows_take_priority() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "alpha\nbeta\ncharlie").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut()
            .unwrap()
            .editor
            .move_cursor(CursorMove::Jump(1, 2));

        for (key, expected) in [
            (KeyCode::Up, (0, 2)),
            (KeyCode::Left, (0, 1)),
            (KeyCode::Down, (1, 1)),
            (KeyCode::Right, (1, 2)),
        ] {
            app.handle_key(KeyEvent::new(key, KeyModifiers::NONE));
            assert_eq!(app.active_tab().unwrap().editor.cursor(), expected);
        }

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (1, 2));

        app.config.keybindings = crate::bindings::Keymap::resolve(
            &[("command_cursor_left".to_owned(), "right".to_owned())]
                .into_iter()
                .collect(),
        )
        .unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        let tab = app.active_tab().unwrap();
        assert_eq!(tab.editor.cursor(), (1, 1));
        assert_eq!(source_from_textarea(&tab.editor), "alpha\nbeta\ncharlie");
        assert!(!tab.document.is_dirty());
    }

    #[test]
    fn extensionless_file_creation_defaults_to_markdown_and_is_indexed() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let root = workspace.root.clone();
        let mut app = App::new(workspace).unwrap();

        assert!(app.apply_workspace_input(WorkspaceInputAction::Create, &root, "daily-note"));

        let markdown = root.join("daily-note.md");
        assert!(markdown.is_file());
        assert!(!root.join("daily-note").exists());
        assert_eq!(app.active_tab().unwrap().document.path, markdown);
        assert!(app.entries.iter().any(|entry| entry.path == markdown));
        assert_eq!(app.status_message.as_deref(), Some("Created daily-note.md"));

        assert!(app.apply_workspace_input(WorkspaceInputAction::Create, &root, "plain.txt"));
        assert!(root.join("plain.txt").is_file());
        assert_eq!(
            app.active_tab().unwrap().document.path,
            root.join("plain.txt")
        );

        assert!(app.apply_workspace_input(WorkspaceInputAction::Create, &root, "archive/"));
        assert!(root.join("archive").is_dir());
        assert!(!root.join("archive.md").exists());
    }

    #[test]
    fn files_workflow_creates_copies_moves_and_retargets_clean_tabs() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("a")).unwrap();
        fs::create_dir(directory.path().join("b")).unwrap();
        let note = directory.path().join("a/note.md");
        fs::write(&note, "note").unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let root = workspace.root.clone();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&note).unwrap();

        assert!(app.apply_workspace_input(WorkspaceInputAction::Create, &root, "drafts/"));
        assert!(app.apply_workspace_input(WorkspaceInputAction::Create, &root, "drafts/new.md"));
        assert_eq!(
            app.active_tab().unwrap().document.path,
            root.join("drafts/new.md")
        );
        app.close_active_discarding();

        let note = root.join("a/note.md");
        app.expand_ancestors(&note);
        let note_index = app.visible_position(&note).unwrap();
        app.explorer_state.select(Some(note_index));
        app.set_workspace_clipboard(false);
        let b = root.join("b");
        let b_index = app.visible_position(&b).unwrap();
        app.explorer_state.select(Some(b_index));
        app.paste_workspace_entry();
        assert_eq!(fs::read_to_string(root.join("b/note.md")).unwrap(), "note");
        assert!(app.workspace_clipboard.is_some());

        app.open_document(&note).unwrap();
        let a = root.join("a");
        let a_index = app.visible_position(&a).unwrap();
        app.explorer_state.select(Some(a_index));
        app.set_workspace_clipboard(true);
        let b_index = app.visible_position(&b).unwrap();
        app.explorer_state.select(Some(b_index));
        app.paste_workspace_entry();

        let moved_folder = root.join("b/a");
        let moved_note = moved_folder.join("note.md");
        assert!(!a.exists());
        assert_eq!(app.active_tab().unwrap().document.path, moved_note);
        assert!(app.workspace_clipboard.is_none());
        assert!(app.recent_paths.contains(&moved_note));

        app.active_tab_mut().unwrap().editor.insert_str("local ");
        app.sync_active_document();
        assert!(!app.apply_workspace_input(WorkspaceInputAction::Rename, &moved_folder, "renamed"));
        assert!(moved_folder.exists());

        app.save_active();
        assert!(app.apply_workspace_input(WorkspaceInputAction::Rename, &moved_folder, "renamed"));
        let renamed = root.join("b/renamed");
        assert_eq!(
            app.active_tab().unwrap().document.path,
            renamed.join("note.md")
        );

        let renamed_index = app.visible_position(&renamed).unwrap();
        app.explorer_state.select(Some(renamed_index));
        app.request_trash_entry();
        assert!(app.overlay.is_none());
        app.close_active_discarding();
        app.request_trash_entry();
        assert!(matches!(app.overlay, Some(Overlay::TrashConfirm { .. })));
    }

    #[test]
    fn create_and_save_as_use_no_clobber_workspace_paths() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.md");
        fs::write(&original, "draft").unwrap();
        let workspace = Workspace::from_target(&original).unwrap();
        let mut app = App::new(workspace).unwrap();

        app.apply_path_action(PathAction::Create, "new.md");
        assert_eq!(
            fs::read_to_string(directory.path().join("new.md")).unwrap(),
            ""
        );
        assert_eq!(app.tabs.len(), 2);

        app.active_tab_mut().unwrap().editor.insert_str("created");
        app.apply_path_action(PathAction::SaveAs, "retargeted.md");
        assert_eq!(
            fs::read_to_string(directory.path().join("retargeted.md")).unwrap(),
            "created"
        );
        assert_eq!(
            app.active_tab().unwrap().document.path,
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join("retargeted.md")
        );

        app.apply_path_action(PathAction::Create, "retargeted.md");
        assert_eq!(
            fs::read_to_string(directory.path().join("retargeted.md")).unwrap(),
            "created"
        );
        assert!(
            app.status_message
                .as_deref()
                .unwrap()
                .contains("already exists")
        );
    }

    #[test]
    fn duplicate_keeps_the_dirty_original_active() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.md");
        fs::write(&original, "draft").unwrap();
        let workspace = Workspace::from_target(&original).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str(" changed");

        app.apply_path_action(PathAction::Duplicate, "copy.md");

        assert_eq!(
            fs::read_to_string(directory.path().join("copy.md")).unwrap(),
            " changeddraft"
        );
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.active_tab().unwrap().document.path,
            original.canonicalize().unwrap()
        );
        assert!(app.tabs[0].document.is_dirty());
        assert!(
            app.entries
                .iter()
                .any(|entry| entry.relative == Path::new("copy.md"))
        );
    }

    #[test]
    fn clean_external_change_reloads_but_dirty_change_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "first").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        let opened_revision = app.active_tab().unwrap().document.source_revision;

        fs::write(&path, "external").unwrap();
        assert!(app.poll_external_state());
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "external"
        );
        assert!(!app.active_tab().unwrap().document.conflict);
        assert_eq!(
            app.active_tab()
                .unwrap()
                .document
                .source_revision
                .generation,
            opened_revision.generation + 1
        );
        assert_ne!(
            app.active_tab().unwrap().document.source_revision.sha256,
            opened_revision.sha256
        );

        app.active_tab_mut().unwrap().editor.insert_str("local ");
        fs::write(&path, "second external").unwrap();
        assert!(app.poll_external_state());
        assert!(app.active_tab().unwrap().document.conflict);
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "local external"
        );
    }

    #[test]
    fn external_poll_rotates_through_inactive_tabs_without_activating_them() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let workspace = Workspace::from_target(&first).unwrap();
        let mut app = App::new(workspace).unwrap();
        let first = app.workspace.root.join("first.md");
        let second = app.workspace.root.join("second.md");
        app.open_document(&second).unwrap();

        fs::write(&first, "external first").unwrap();
        assert!(app.poll_external_state());
        assert_eq!(app.active_tab().unwrap().document.path, second);
        let first_tab = app
            .tabs
            .iter()
            .find(|tab| tab.document.path == first)
            .unwrap();
        assert_eq!(first_tab.document.text, "external first");
        assert!(!first_tab.document.conflict);

        app.activate_tab(0);
        app.active_tab_mut().unwrap().editor.insert_str("local ");
        app.sync_active_document();
        app.activate_tab(1);
        fs::write(&first, "another external").unwrap();
        assert!(app.poll_external_state());

        let first_tab = app
            .tabs
            .iter()
            .find(|tab| tab.document.path == first)
            .unwrap();
        assert_eq!(first_tab.document.text, "local external first");
        assert!(first_tab.document.conflict);
        assert_eq!(app.active_tab().unwrap().document.path, second);
    }

    #[test]
    fn external_poll_refreshes_the_workspace_tree() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("one.md");
        fs::write(&path, "one").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        finish_workspace_scan(&mut app);

        fs::write(directory.path().join("two.md"), "two").unwrap();

        let _ = app.poll_external_state();
        finish_workspace_scan(&mut app);
        assert!(
            app.entries
                .iter()
                .any(|entry| entry.relative == Path::new("two.md"))
        );
    }

    #[test]
    fn workspace_index_starts_shallow_and_finishes_in_the_background() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        fs::create_dir(root.join("notes")).unwrap();
        fs::write(root.join("notes/nested.md"), "nested").unwrap();
        let workspace = Workspace::from_target(&root).unwrap();

        let mut app = App::new(workspace).unwrap();

        assert!(app.workspace_is_indexing());
        assert!(
            app.entries
                .iter()
                .any(|entry| entry.relative == Path::new("notes"))
        );
        assert!(
            !app.entries
                .iter()
                .any(|entry| entry.relative == Path::new("notes/nested.md"))
        );
        finish_workspace_scan(&mut app);
        assert!(
            app.entries
                .iter()
                .any(|entry| entry.relative == Path::new("notes/nested.md"))
        );
    }

    #[test]
    fn directory_launch_restores_content_free_tabs_and_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.md");
        let second = root.join("second.md");
        fs::write(&first, "first\nline").unwrap();
        fs::write(&second, "second\nline").unwrap();
        let store = SessionStore::new(root.join("state"));
        store
            .save(&SessionState {
                workspace_root: root.clone(),
                active_path: Some(second.clone()),
                documents: vec![
                    session_view(first.clone(), 1, 2),
                    DocumentViewState {
                        scroll_y: 1.0,
                        preview_scroll_x: 2.0,
                        preview_scroll_y: 3.0,
                        ..session_view(second.clone(), 1, 3)
                    },
                ],
                open_paths: vec![first.clone(), second.clone()],
            })
            .unwrap();
        let workspace = Workspace::from_target(&root).unwrap();

        let mut app =
            App::with_state_services(workspace, Config::default(), Some(store), None).unwrap();

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab().unwrap().document.path, second);
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (1, 3));
        assert_eq!(app.active_tab().unwrap().editor_scroll_y, 1);
        assert_eq!(app.preview_horizontal_scroll, 2);
        assert_eq!(app.preview_scroll, 3);

        assert!(app.materialize_next_session_tab());
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[0].document.path, first);
        assert_eq!(app.active_tab().unwrap().document.path, second);
    }

    #[test]
    fn session_restore_prompts_only_the_active_mixed_tab() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mixed = root.join("mixed.md");
        let normal = root.join("normal.md");
        fs::write(&mixed, b"one\r\ntwo\n").unwrap();
        fs::write(&normal, "normal").unwrap();
        let store = SessionStore::new(root.join("state"));
        store
            .save(&SessionState {
                workspace_root: root.clone(),
                active_path: Some(normal.clone()),
                documents: vec![
                    session_view(mixed.clone(), 0, 0),
                    session_view(normal.clone(), 0, 0),
                ],
                open_paths: vec![mixed.clone(), normal.clone()],
            })
            .unwrap();
        let workspace = Workspace::from_target(&root).unwrap();

        let mut app =
            App::with_state_services(workspace, Config::default(), Some(store), None).unwrap();

        assert_eq!(app.active_tab().unwrap().document.path, normal);
        assert!(app.overlay.is_none());
        app.open_document(&mixed).unwrap();
        assert!(matches!(
            app.overlay,
            Some(Overlay::MixedLineEndings {
                context: MixedLineEndingContext::Open,
                ..
            })
        ));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab().unwrap().document.path, normal);
    }

    #[test]
    fn each_restored_mixed_tab_prompts_when_activated() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.md");
        let second = root.join("second.md");
        fs::write(&first, b"one\r\ntwo\n").unwrap();
        fs::write(&second, b"three\nfour\r\n").unwrap();
        let store = SessionStore::new(root.join("state"));
        store
            .save(&SessionState {
                workspace_root: root.clone(),
                active_path: Some(first.clone()),
                documents: vec![
                    session_view(first.clone(), 0, 0),
                    session_view(second.clone(), 0, 0),
                ],
                open_paths: vec![first.clone(), second.clone()],
            })
            .unwrap();
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), Some(store), None).unwrap();

        assert_eq!(app.active_tab().unwrap().document.path, first);
        assert!(matches!(
            app.overlay,
            Some(Overlay::MixedLineEndings { .. })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.tabs[0].document.is_editable());

        assert!(app.materialize_next_session_tab());
        app.switch_tab(1);
        assert_eq!(app.active_tab().unwrap().document.path, second);
        assert!(matches!(
            app.overlay,
            Some(Overlay::MixedLineEndings { .. })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(app.tabs.iter().all(|tab| tab.document.is_editable()));
    }

    #[test]
    fn recent_order_is_persisted_independently_from_open_tab_order() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.md");
        let second = root.join("second.md");
        let third = root.join("third.md");
        for path in [&first, &second, &third] {
            fs::write(path, path.file_stem().unwrap().as_encoded_bytes()).unwrap();
        }
        let store = SessionStore::new(root.join("state"));
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), Some(store.clone()), None)
                .unwrap();

        app.open_document(&first).unwrap();
        app.open_document(&second).unwrap();
        app.open_document(&third).unwrap();
        app.persist_session_if_changed();

        let state = store.load(&root).state.unwrap();
        assert_eq!(
            state.open_paths,
            vec![first.clone(), second.clone(), third.clone()]
        );
        assert_eq!(
            state
                .documents
                .iter()
                .map(|view| view.path.clone())
                .collect::<Vec<_>>(),
            vec![third, second, first]
        );
    }

    #[test]
    fn recent_picker_prunes_missing_paths_but_keeps_closed_documents() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let current = root.join("current.md");
        let closed = root.join("closed.md");
        let missing = root.join("missing.md");
        fs::write(&current, "current").unwrap();
        fs::write(&closed, "closed").unwrap();
        let store = SessionStore::new(root.join("state"));
        store
            .save(&SessionState {
                workspace_root: root.clone(),
                active_path: Some(current.clone()),
                documents: vec![
                    session_view(missing, 0, 0),
                    session_view(closed.clone(), 0, 0),
                    session_view(current.clone(), 0, 0),
                ],
                open_paths: vec![current],
            })
            .unwrap();
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), Some(store), None).unwrap();

        app.open_recent_documents();

        let Some(Overlay::RecentDocuments { paths, .. }) = &app.overlay else {
            panic!("recent documents did not open");
        };
        assert_eq!(
            paths,
            &vec![
                app.active_tab().unwrap().document.path.clone(),
                closed.clone()
            ]
        );
        assert!(!app.tabs.iter().any(|tab| tab.document.path == closed));
    }

    #[test]
    fn session_persistence_never_contains_markdown_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "private source").unwrap();
        let store = SessionStore::new(root.join("state"));
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), Some(store.clone()), None)
                .unwrap();

        app.persist_session_if_changed();
        let bytes = fs::read(store.path_for(&root)).unwrap();

        assert!(!String::from_utf8_lossy(&bytes).contains("private source"));
        assert_eq!(store.load(&root).state.unwrap().open_paths, vec![path]);
    }

    #[test]
    fn recovery_restore_is_dirty_and_successful_save_removes_the_journal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved").unwrap();
        let loaded = load_file(&path).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(
                &path,
                &root,
                "unsaved draft",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                .unwrap();

        assert!(matches!(app.overlay, Some(Overlay::Recovery { .. })));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.text, "unsaved draft");
        assert!(app.active_tab().unwrap().document.is_dirty());
        assert!(!app.active_tab().unwrap().document.conflict);

        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert_eq!(fs::read_to_string(&path).unwrap(), "unsaved draft");
        assert!(!journal.path_for(&path).exists());
    }

    #[test]
    fn recovery_from_a_changed_baseline_cannot_overwrite_disk() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "baseline").unwrap();
        let loaded = load_file(&path).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(
                &path,
                &root,
                "recovered draft",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();
        fs::write(&path, "new disk").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal)).unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));

        assert!(app.active_tab().unwrap().document.conflict);
        assert_eq!(fs::read_to_string(path).unwrap(), "new disk");
        assert!(app.status_message.as_deref().unwrap().contains("Save As"));
    }

    #[test]
    fn dirty_source_is_journaled_and_explicit_quit_discard_removes_it() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved").unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                .unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");

        app.persist_recovery();

        assert_eq!(journal.load(&path).unwrap().unwrap().text, "draft saved");
        app.request_quit();
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        app.persist_recovery();
        assert!(!journal.path_for(&path).exists());
        assert_eq!(fs::read_to_string(path).unwrap(), "saved");
    }

    #[test]
    fn cooperative_shutdown_flushes_dirty_recovery_and_session_state() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved").unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let store = SessionStore::new(root.join("sessions"));
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(
            workspace,
            Config::default(),
            Some(store.clone()),
            Some(journal.clone()),
        )
        .unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        let requested = AtomicBool::new(true);

        assert!(observe_shutdown_request(&mut app, &requested));
        assert!(app.should_quit);
        app.flush_state();

        assert_eq!(journal.load(&path).unwrap().unwrap().text, "draft saved");
        let session = store.load(&root).state.unwrap();
        assert_eq!(session.active_path, Some(path.clone()));
        assert_eq!(session.open_paths, vec![path]);
    }

    #[test]
    fn mixed_open_requires_consent_and_untouched_save_preserves_exact_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mixed.md");
        let original = b"first\nsecond\r\nthird\r";
        fs::write(&path, original).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();

        assert!(matches!(
            app.overlay,
            Some(Overlay::MixedLineEndings {
                context: MixedLineEndingContext::Open,
                target: LineEnding::Crlf,
                ..
            })
        ));
        assert!(!app.active_tab().unwrap().document.is_editable());

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.active_tab().unwrap().document.is_editable());
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));

        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!app.active_tab().unwrap().document.is_dirty());
        assert_eq!(
            app.active_tab().unwrap().document.line_ending,
            LineEnding::Mixed
        );
    }

    #[test]
    fn cancelling_mixed_open_restores_previous_tab_and_first_edit_normalizes() {
        let directory = tempfile::tempdir().unwrap();
        let normal = directory.path().join("normal.md");
        let mixed = directory.path().join("mixed.md");
        fs::write(&normal, "normal").unwrap();
        fs::write(&mixed, b"one\r\ntwo\n").unwrap();
        let workspace = Workspace::from_target(&normal).unwrap();
        let mut app = App::new(workspace).unwrap();

        app.open_document(&mixed).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(
            app.active_tab().unwrap().document.path,
            normal.canonicalize().unwrap()
        );
        assert_eq!(fs::read(&mixed).unwrap(), b"one\r\ntwo\n");

        app.open_document(&mixed).unwrap();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));

        assert_eq!(fs::read(&mixed).unwrap(), b"xone\r\ntwo\r\n");
        assert_eq!(
            app.active_tab().unwrap().document.line_ending,
            LineEnding::Crlf
        );
    }

    #[test]
    fn conflict_reload_waits_for_mixed_choice_before_closing() {
        for choice in [KeyCode::Enter, KeyCode::Esc] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("note.md");
            fs::write(&path, "disk").unwrap();
            let workspace = Workspace::from_target(&path).unwrap();
            let mut app = App::new(workspace).unwrap();
            app.active_tab_mut().unwrap().editor.insert_str("local ");
            fs::write(&path, b"external\r\nversion\n").unwrap();

            app.close_active();
            app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
            assert!(matches!(
                app.overlay,
                Some(Overlay::Conflict {
                    kind: ConflictKind::Changed,
                    ..
                })
            ));
            app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

            assert!(matches!(
                app.overlay,
                Some(Overlay::MixedLineEndings {
                    context: MixedLineEndingContext::Reload,
                    ..
                })
            ));
            assert_eq!(app.tabs.len(), 1);
            assert!(app.pending_transition.is_some());

            app.handle_key(KeyEvent::new(choice, KeyModifiers::NONE));

            assert!(app.tabs.is_empty());
            assert!(app.pending_transition.is_none());
            assert_eq!(fs::read(&path).unwrap(), b"external\r\nversion\n");
        }
    }

    #[test]
    fn quit_continues_to_the_next_dirty_tab_after_mixed_conflict_reload() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.md");
        let second = directory.path().join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let first = first.canonicalize().unwrap();
        let second = second.canonicalize().unwrap();
        let workspace = Workspace::from_target(&first).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        app.sync_active_document();
        app.open_document(&second).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("local ");
        fs::write(&second, b"external\r\nversion\n").unwrap();

        app.request_quit();
        app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::MixedLineEndings {
                context: MixedLineEndingContext::Reload,
                ..
            })
        ));

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(app.active_tab().unwrap().document.path, first);
        assert!(app.active_tab().unwrap().document.is_dirty());
        assert!(matches!(
            app.overlay,
            Some(Overlay::Confirm(ConfirmAction::Quit))
        ));
        assert_eq!(fs::read(&second).unwrap(), b"external\r\nversion\n");

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.should_quit);
        assert!(app.pending_transition.is_none());
    }

    #[test]
    fn conflict_save_as_preserves_external_and_retargets_local_draft() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "disk").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("local ");
        fs::write(&path, "external").unwrap();

        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::Conflict {
                kind: ConflictKind::Changed,
                can_reload: true,
                ..
            })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::PathInput {
                action: PathAction::SaveConflictAs,
                input,
            }) if input.value == "note-local.md"
        ));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let local = directory.path().join("note-local.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
        assert_eq!(fs::read_to_string(&local).unwrap(), "local disk");
        assert_eq!(
            app.active_tab().unwrap().document.path,
            local.canonicalize().unwrap()
        );
        assert!(!app.active_tab().unwrap().document.conflict);
        assert!(!app.active_tab().unwrap().document.is_dirty());
    }

    #[test]
    fn reverted_conflict_clears_and_allows_guarded_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "disk").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("local ");

        fs::write(&path, "external").unwrap();
        assert!(app.poll_external_state());
        assert!(app.active_tab().unwrap().document.conflict);
        fs::write(&path, "disk").unwrap();
        assert!(app.poll_external_state());
        assert!(!app.active_tab().unwrap().document.conflict);

        app.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert_eq!(fs::read_to_string(path).unwrap(), "local disk");
    }

    #[test]
    fn clean_missing_close_and_quit_require_explicit_continue() {
        let directory = tempfile::tempdir().unwrap();
        let close_path = directory.path().join("close.md");
        fs::write(&close_path, "clean").unwrap();
        let workspace = Workspace::from_target(&close_path).unwrap();
        let mut app = App::new(workspace).unwrap();
        fs::remove_file(&close_path).unwrap();

        app.close_active();
        assert!(matches!(
            app.overlay,
            Some(Overlay::Conflict {
                kind: ConflictKind::Missing,
                can_reload: false,
                allow_continue: true,
            })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.tabs.len(), 1);
        app.close_active();
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(app.tabs.is_empty());

        let quit_path = directory.path().join("quit.md");
        fs::write(&quit_path, "clean").unwrap();
        let workspace = Workspace::from_target(&quit_path).unwrap();
        let mut app = App::new(workspace).unwrap();
        fs::remove_file(&quit_path).unwrap();
        app.request_quit();
        assert!(matches!(
            app.overlay,
            Some(Overlay::Conflict {
                kind: ConflictKind::Missing,
                allow_continue: true,
                ..
            })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(app.should_quit);
    }

    #[test]
    fn recovery_manager_is_identical_from_binding_and_palette() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.md");
        let second = root.join("second.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let workspace = Workspace::from_target(&root).unwrap();
        let mut config = Config::default();
        config.recovery.retention_days = 45;
        let mut app =
            App::with_state_services(workspace, config, None, Some(journal.clone())).unwrap();

        app.open_document(&first).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        app.sync_active_document();
        app.open_document(&second).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        app.execute_binding_action(BindingAction::ManageRecovery);
        let binding_overlay = app.overlay.take().unwrap();
        execute_palette_action(&mut app, CommandAction::ManageRecovery);

        let Overlay::RecoveryManager {
            records: binding_records,
            selected: binding_selected,
            focus: binding_focus,
            target: binding_target,
            protected_journals: binding_journals,
            protected_documents: binding_documents,
            retention_days: binding_retention,
            status: binding_status,
        } = binding_overlay
        else {
            panic!("recovery binding did not open the manager");
        };
        let Some(Overlay::RecoveryManager {
            records,
            selected,
            focus,
            target,
            protected_journals,
            protected_documents,
            retention_days,
            status,
        }) = &app.overlay
        else {
            panic!("recovery palette action did not open the manager");
        };
        assert_eq!(records, &binding_records);
        assert_eq!(*selected, binding_selected);
        assert_eq!(*focus, binding_focus);
        assert_eq!(target, &binding_target);
        assert_eq!(protected_journals, &binding_journals);
        assert_eq!(protected_documents, &binding_documents);
        assert_eq!(*retention_days, binding_retention);
        assert_eq!(status, &binding_status);
        assert_eq!(records.len(), 2);
        assert!(protected_documents.contains(&first));
        assert!(protected_documents.contains(&second));
        assert!(protected_journals.contains(&journal.path_for(&first)));
        assert!(protected_journals.contains(&journal.path_for(&second)));
        assert_eq!(*retention_days, 45);
    }

    #[test]
    fn recovery_manager_rechecks_dirty_state_before_archiving() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved").unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                .unwrap();
        app.open_document(&path).unwrap();
        let loaded = load_file(&path).unwrap();
        journal
            .publish(
                &path,
                &root,
                "recovery draft",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();

        app.open_recovery_manager();
        app.active_tab_mut().unwrap().editor.insert_str("new ");
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        assert!(journal.path_for(&path).exists());
        assert!(journal.list_quarantined(Some(&root)).unwrap().is_empty());
        assert!(matches!(
            &app.overlay,
            Some(Overlay::RecoveryManager { status, .. })
                if status.contains("open dirty document")
        ));
    }

    #[test]
    fn recovery_manager_retargets_archives_exports_and_restores() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let original = root.join("original.md");
        let renamed = root.join("renamed.md");
        fs::write(&original, "saved").unwrap();
        let loaded = load_file(&original).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                .unwrap();
        journal
            .publish(
                &original,
                &root,
                "unsaved draft",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();
        fs::rename(&original, &renamed).unwrap();

        app.open_recovery_manager();
        assert!(matches!(
            &app.overlay,
            Some(Overlay::RecoveryManager { records, .. })
                if records[0].status == RecoveryRecordStatus::Missing
        ));
        if let Some(Overlay::RecoveryManager { focus, target, .. }) = &mut app.overlay {
            *focus = RecoveryManagerFocus::Target;
            target.value = "renamed.md".to_owned();
            target.cursor = target.value.chars().count();
        }
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!journal.path_for(&original).exists());
        assert_eq!(
            journal.load(&renamed).unwrap().unwrap().text,
            "unsaved draft"
        );
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(!journal.path_for(&renamed).exists());
        assert_eq!(journal.list_quarantined(Some(&root)).unwrap().len(), 1);

        if let Some(Overlay::RecoveryManager { focus, target, .. }) = &mut app.overlay {
            *focus = RecoveryManagerFocus::Target;
            target.value = "exported.md".to_owned();
            target.cursor = target.value.chars().count();
        }
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            fs::read_to_string(root.join("exported.md")).unwrap(),
            "unsaved draft"
        );
        assert_eq!(journal.list_quarantined(Some(&root)).unwrap().len(), 1);

        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(journal.list_quarantined(Some(&root)).unwrap().is_empty());
        assert!(journal.path_for(&renamed).exists());
        assert!(matches!(app.overlay, Some(Overlay::Recovery { .. })));
    }

    #[test]
    fn recovery_manager_opens_unavailable_drafts_only_through_save_as() {
        for orphaned in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let original = root.join("unavailable.md");
            fs::write(&original, "saved").unwrap();
            let loaded = load_file(&original).unwrap();
            let journal = RecoveryJournal::new(root.join("recovery"));
            journal
                .publish(
                    &original,
                    &root,
                    "unsaved draft",
                    loaded.encoding,
                    &loaded.snapshot,
                )
                .unwrap();
            fs::remove_file(&original).unwrap();
            if orphaned {
                fs::create_dir(&original).unwrap();
            }
            let workspace = Workspace::from_target(&root).unwrap();
            let mut app =
                App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                    .unwrap();

            app.open_recovery_manager();
            app.handle_overlay_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

            let tab = app.active_tab().unwrap();
            assert_eq!(tab.document.path, original);
            assert_eq!(tab.document.text, "unsaved draft");
            assert!(tab.document.is_dirty());
            assert!(tab.document.conflict);
            assert!(tab.document.recovery_conflict);
            assert!(matches!(
                app.overlay,
                Some(Overlay::Conflict {
                    kind: ConflictKind::Missing | ConflictKind::Unavailable,
                    can_reload: false,
                    ..
                })
            ));

            app.handle_overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
            let Some(Overlay::PathInput { input, .. }) = &mut app.overlay else {
                panic!("unavailable recovery did not require Save As");
            };
            input.value = "rescued.md".to_owned();
            input.cursor = input.value.chars().count();
            app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

            assert_eq!(
                fs::read_to_string(root.join("rescued.md")).unwrap(),
                "unsaved draft"
            );
            assert!(!journal.path_for(&original).exists());
            assert_eq!(
                app.active_tab().unwrap().document.path,
                root.join("rescued.md")
            );
            assert!(!app.active_tab().unwrap().document.is_dirty());
            if orphaned {
                assert!(original.is_dir());
            } else {
                assert!(!original.exists());
            }
        }
    }

    #[test]
    fn recovery_manager_defers_active_and_restored_drafts_until_mixed_consent() {
        for quarantined in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let path = root.join("mixed.md");
            fs::write(&path, b"one\r\ntwo\n").unwrap();
            let loaded = load_file(&path).unwrap();
            let journal = RecoveryJournal::new(root.join("recovery"));
            journal
                .publish(
                    &path,
                    &root,
                    "recovered draft\n",
                    loaded.encoding,
                    &loaded.snapshot,
                )
                .unwrap();
            if quarantined {
                let record = journal.list_entries(Some(&root)).unwrap().pop().unwrap();
                journal.quarantine(&record).unwrap();
            }
            let workspace = Workspace::from_target(&root).unwrap();
            let mut app =
                App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                    .unwrap();

            app.open_recovery_manager();
            app.handle_overlay_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));

            assert!(matches!(
                app.overlay,
                Some(Overlay::MixedLineEndings {
                    context: MixedLineEndingContext::Open,
                    ..
                })
            ));
            assert!(!app.active_tab().unwrap().document.is_editable());

            app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

            assert!(app.active_tab().unwrap().document.is_editable());
            assert!(matches!(
                &app.overlay,
                Some(Overlay::Recovery { entry }) if entry.text == "recovered draft\n"
            ));
        }
    }

    #[test]
    fn recovery_manager_protects_dirty_documents_reached_through_spelling_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("Case.md");
        let alias = root.join("case.md");
        fs::write(&path, "saved").unwrap();
        if alias.canonicalize().ok() != path.canonicalize().ok() {
            return;
        }
        let loaded = load_file(&alias).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(
                &alias,
                &root,
                "archived draft",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();
        let active = journal.list_entries(Some(&root)).unwrap().pop().unwrap();
        let archived_path = journal.quarantine(&active).unwrap();
        let archived = journal
            .list_quarantined(Some(&root))
            .unwrap()
            .pop()
            .unwrap();
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal)).unwrap();
        app.open_document(&path).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("dirty ");

        app.open_managed_recovery(0, &archived);

        assert!(archived_path.exists());
        assert!(matches!(
            &app.overlay,
            Some(Overlay::RecoveryManager { status, .. })
                if status.contains("open dirty document")
        ));
    }

    #[test]
    fn corrupt_quarantine_requires_an_explicit_delete_key() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let recovery_root = root.join("recovery");
        let quarantine_root = recovery_root.join("quarantine");
        fs::create_dir_all(&quarantine_root).unwrap();
        let corrupt = quarantine_root.join(format!("{}.json", "e".repeat(64)));
        fs::write(&corrupt, b"{not valid JSON}\n").unwrap();
        let journal = RecoveryJournal::new(recovery_root);
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                .unwrap();

        app.open_recovery_manager();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(matches!(
            app.overlay,
            Some(Overlay::RecoveryDeleteConfirm { .. })
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(corrupt.exists());
        assert!(app.overlay.is_none());

        app.open_recovery_manager();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(!corrupt.exists());
        assert!(journal.list_quarantined(Some(&root)).unwrap().is_empty());
    }

    #[test]
    fn expired_cleanup_is_cancel_safe_and_rechecks_exact_records() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("old.md");
        fs::write(&path, "saved").unwrap();
        let loaded = load_file(&path).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(journal.clone()))
                .unwrap();
        journal
            .publish(
                &path,
                &root,
                "old recovery",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();
        let active_path = journal.path_for(&path);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&active_path).unwrap()).unwrap();
        value["updated_at"] = serde_json::Value::String("2000-01-01T00:00:00Z".to_owned());
        fs::write(&active_path, serde_json::to_vec(&value).unwrap()).unwrap();
        let listed = journal.list_entries(Some(&root)).unwrap().pop().unwrap();
        let archived_path = journal.quarantine(&listed).unwrap();

        app.open_recovery_manager();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(matches!(
            &app.overlay,
            Some(Overlay::RecoveryCleanupConfirm { records, .. })
                if records.len() == 1 && records[0].journal_path == archived_path
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(archived_path.exists());

        app.open_recovery_manager();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let mut changed = fs::read(&archived_path).unwrap();
        changed.push(b'\n');
        fs::write(&archived_path, changed).unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(archived_path.exists());
        assert!(matches!(
            &app.overlay,
            Some(Overlay::RecoveryManager { status, .. })
                if status.contains("1 failed")
                    && status.contains("old.md")
                    && status.contains("changed after it was listed")
        ));

        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(!archived_path.exists());
        assert!(journal.list_quarantined(Some(&root)).unwrap().is_empty());
    }

    #[test]
    fn rapid_preview_changes_coalesce_into_the_latest_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# First\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();
        app.view_mode = ViewMode::Split;
        app.viewport_width = 160;

        let initial_key = app.current_preview_key().unwrap();
        let initial_revision = initial_key.revision;
        let initial_source = app.active_tab().unwrap().document.text.clone();
        app.active_tab_mut().unwrap().preview_cache = Some(PreviewCache {
            key: initial_key,
            rendered: render_markdown_document(&initial_source, None),
        });
        let started = Instant::now();

        app.active_tab_mut().unwrap().editor.insert_str(" second");
        app.sync_active_document();
        app.schedule_preview_render_at(started);
        let first_pending_revision = app.pending_preview.as_ref().unwrap().key.revision;

        app.active_tab_mut().unwrap().editor.insert_str(" third");
        app.sync_active_document();
        app.schedule_preview_render_at(started + Duration::from_millis(10));
        let pending = app.pending_preview.as_ref().unwrap();
        assert!(pending.key.revision.generation > first_pending_revision.generation);
        assert!(pending.source.ends_with(" second third# First\n"));
        assert_eq!(pending.ready_at, started + Duration::from_millis(60));
        assert!(app.preview_for_draw().is_some());
        assert_eq!(
            app.active_tab()
                .unwrap()
                .preview_cache
                .as_ref()
                .unwrap()
                .key
                .revision,
            initial_revision
        );

        app.start_due_preview_render_at(started + Duration::from_millis(59));
        assert_eq!(app.preview_renders_started, 0);
        app.start_due_preview_render_at(started + Duration::from_millis(60));
        assert_eq!(app.preview_renders_started, 1);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !app.preview_inflight.is_empty() {
            assert!(Instant::now() < deadline, "preview worker did not finish");
            std::thread::yield_now();
            app.poll_preview_results();
        }
        assert_eq!(
            app.active_tab()
                .unwrap()
                .preview_cache
                .as_ref()
                .unwrap()
                .key,
            app.current_preview_key().unwrap()
        );
    }

    #[test]
    fn stale_preview_completion_never_replaces_newer_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# First\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();
        app.view_mode = ViewMode::Split;
        app.viewport_width = 160;

        let stale_key = app.current_preview_key().unwrap();
        let stale_source = app.active_tab().unwrap().document.text.clone();
        app.active_tab_mut().unwrap().editor.insert_str("newer ");
        app.sync_active_document();
        app.preview_tx
            .send(PreviewCompletion {
                key: stale_key,
                rendered: render_markdown_document(&stale_source, None),
            })
            .unwrap();

        assert!(!app.poll_preview_results());
        assert!(app.active_tab().unwrap().preview_cache.is_none());
        assert_eq!(app.preview_stale_completions, 1);
        assert!(
            app.active_tab()
                .unwrap()
                .document
                .text
                .starts_with("newer ")
        );
    }

    #[test]
    fn unchanged_preview_draws_reuse_the_cached_parse() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# First\n\n[Example](https://example.com).\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();

        assert!(app.preview_for_draw().is_some());
        app.preview_selected_link = Some(0);
        assert!(app.preview_for_draw().is_some());
        assert!(app.preview_for_draw().is_some());
        assert_eq!(app.preview_synchronous_renders, 2);
        assert_eq!(
            app.active_tab()
                .unwrap()
                .preview_cache
                .as_ref()
                .unwrap()
                .key,
            app.current_preview_key().unwrap()
        );
        assert_eq!(
            app.active_tab()
                .unwrap()
                .preview_cache
                .as_ref()
                .unwrap()
                .key
                .revision,
            app.active_tab().unwrap().document.source_revision
        );
    }

    #[test]
    fn local_history_opt_in_restore_keeps_disk_and_is_one_undo_group() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "saved\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        let mut app = app_with_history(workspace, store.clone());

        execute_palette_action(&mut app, CommandAction::LocalHistory);
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory { enabled: false, .. })
        ));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert!(store.is_enabled(&app.workspace.root).unwrap());

        app.handle_overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.active_tab_mut()
            .unwrap()
            .replace_all("checkpoint\n")
            .unwrap();
        execute_palette_action(&mut app, CommandAction::CreateCheckpoint);
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory {
                ref checkpoints,
                ..
            }) if checkpoints.len() == 1
                && checkpoints[0].reason == CheckpointReason::Manual
        ));

        app.handle_overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.active_tab_mut()
            .unwrap()
            .replace_all("current\n")
            .unwrap();
        execute_palette_action(&mut app, CommandAction::LocalHistory);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

        let document = &app.active_tab().unwrap().document;
        assert_eq!(document.text, "checkpoint\n");
        assert_eq!(document.saved_text, "saved\n");
        assert!(document.is_dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");
        assert_eq!(
            app.status_message.as_deref(),
            Some("Restored checkpoint to buffer · disk unchanged until Save")
        );

        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "current\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");
        app.execute_command(CommandAction::Redo);
        assert_eq!(app.active_tab().unwrap().document.text, "checkpoint\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");

        let checkpoints = store.list(&app.workspace.root, &path).unwrap().checkpoints;
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].reason, CheckpointReason::BeforeRestore);
        assert_eq!(checkpoints[0].source, "current\n");
    }

    #[test]
    fn agent_proposal_acceptance_is_reviewed_dirty_recoverable_and_one_undo_group() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let history = CheckpointStore::new(state.path().join("history"));
        history.set_enabled(&root, true).unwrap();
        let recovery = RecoveryJournal::new(state.path().join("recovery"));
        let mut app = App::with_state_services_and_history(
            workspace,
            Config::default(),
            None,
            Some(recovery.clone()),
            Some(history.clone()),
        )
        .unwrap();
        let revision = revision_label(app.active_tab().unwrap().document.source_revision);

        let response = app.receive_agent_proposal(
            0,
            &path,
            "note.md",
            &revision,
            ProposalChange::ReplaceSource {
                source: "agent draft\n".to_owned(),
            },
            Some("test-agent"),
        );

        assert!(matches!(response, AgentResponse::PendingReview { .. }));
        assert!(matches!(app.overlay, Some(Overlay::AgentProposal { .. })));
        assert_eq!(app.active_tab().unwrap().document.text, "saved\n");
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        let document = &app.active_tab().unwrap().document;
        assert_eq!(document.text, "agent draft\n");
        assert!(document.is_dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");
        assert_eq!(recovery.load(&path).unwrap().unwrap().text, "agent draft\n");
        assert!(app.pending_agent_proposal.is_none());
        assert!(app.status_message.as_deref().unwrap().contains("not saved"));

        let checkpoints = history.list(&root, &path).unwrap().checkpoints;
        assert_eq!(checkpoints.len(), 2);
        assert_eq!(checkpoints[0].reason, CheckpointReason::AfterAgentEdit);
        assert_eq!(checkpoints[1].reason, CheckpointReason::BeforeAgentEdit);

        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "saved\n");
        app.execute_command(CommandAction::Redo);
        assert_eq!(app.active_tab().unwrap().document.text, "agent draft\n");
        assert_eq!(fs::read_to_string(path).unwrap(), "saved\n");
    }

    #[test]
    fn agent_proposal_acceptance_blocks_when_recovery_cannot_store_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let recovery = RecoveryJournal::new(state.path().join("recovery"));
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(recovery.clone()))
                .unwrap();
        let revision = revision_label(app.active_tab().unwrap().document.source_revision);
        let source = "\"".repeat(8 * 1024 * 1024);

        let response = app.receive_agent_proposal(
            0,
            &path,
            "note.md",
            &revision,
            ProposalChange::ReplaceSource { source },
            Some("test-agent"),
        );

        assert!(matches!(response, AgentResponse::PendingReview { .. }));
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));

        assert_eq!(app.active_tab().unwrap().document.text, "saved\n");
        assert!(!app.active_tab().unwrap().document.is_dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");
        assert!(recovery.load(&path).unwrap().is_none());
        assert!(app.pending_agent_proposal.is_some());
        assert!(
            app.status_message
                .as_deref()
                .unwrap()
                .contains("Recovery not saved")
        );
        assert!(!app.status_message.as_deref().unwrap().contains("Applied"));
    }

    #[test]
    fn agent_overlay_pointer_actions_match_the_visible_labels() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join("note.md");
        fs::write(&path, "saved\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let recovery = RecoveryJournal::new(state.path().join("recovery"));
        let mut app =
            App::with_state_services(workspace, Config::default(), None, Some(recovery)).unwrap();
        let frame = Rect::new(0, 0, 120, 40);

        app.open_agent_sharing();
        let sharing_area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let sharing_inner = ui::overlay_inner_area(sharing_area);
        let footer_row = sharing_inner.bottom().saturating_sub(1);
        app.handle_overlay_click(sharing_area, sharing_inner.x.saturating_sub(1), footer_row);
        assert!(matches!(app.overlay, Some(Overlay::AgentSharing { .. })));
        assert!(app.agent_session.is_some());
        app.handle_overlay_click(sharing_area, sharing_inner.x.saturating_add(13), footer_row);
        assert!(matches!(app.overlay, Some(Overlay::AgentSharing { .. })));
        assert!(app.agent_session.is_some());

        app.handle_overlay_click(sharing_area, sharing_inner.x.saturating_add(12), footer_row);
        assert!(app.overlay.is_none());
        assert!(app.agent_session.is_none());

        app.open_agent_sharing();
        let sharing_area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let sharing_inner = ui::overlay_inner_area(sharing_area);
        let footer_row = sharing_inner.bottom().saturating_sub(1);
        app.handle_overlay_click(sharing_area, sharing_inner.x.saturating_add(17), footer_row);
        assert!(matches!(app.overlay, Some(Overlay::AgentSharing { .. })));
        app.handle_overlay_click(sharing_area, sharing_inner.x.saturating_add(44), footer_row);
        assert!(app.overlay.is_none());
        assert!(app.agent_session.is_some());
        app.revoke_agent_sharing();

        let revision = revision_label(app.active_tab().unwrap().document.source_revision);
        let response = app.receive_agent_proposal(
            0,
            &path,
            "note.md",
            &revision,
            ProposalChange::ReplaceSource {
                source: "rejected\n".to_owned(),
            },
            Some("test-agent"),
        );
        assert!(matches!(response, AgentResponse::PendingReview { .. }));
        let proposal_area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let proposal_inner = ui::overlay_inner_area(proposal_area);
        let footer_row = proposal_inner.bottom().saturating_sub(1);
        app.handle_overlay_click(
            proposal_area,
            proposal_inner.x.saturating_sub(1),
            footer_row,
        );
        assert!(matches!(app.overlay, Some(Overlay::AgentProposal { .. })));
        assert!(app.pending_agent_proposal.is_some());
        app.handle_overlay_click(
            proposal_area,
            proposal_inner.x.saturating_add(21),
            footer_row,
        );
        assert!(matches!(app.overlay, Some(Overlay::AgentProposal { .. })));
        assert!(app.pending_agent_proposal.is_some());

        app.handle_overlay_click(
            proposal_area,
            proposal_inner.x.saturating_add(34),
            footer_row,
        );
        assert!(app.overlay.is_none());
        assert!(app.pending_agent_proposal.is_none());
        assert_eq!(app.active_tab().unwrap().document.text, "saved\n");

        let response = app.receive_agent_proposal(
            0,
            &path,
            "note.md",
            &revision,
            ProposalChange::ReplaceSource {
                source: "accepted\n".to_owned(),
            },
            Some("test-agent"),
        );
        assert!(matches!(response, AgentResponse::PendingReview { .. }));
        let proposal_area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let proposal_inner = ui::overlay_inner_area(proposal_area);
        app.handle_overlay_click(
            proposal_area,
            proposal_inner.x.saturating_add(20),
            proposal_inner.bottom().saturating_sub(1),
        );
        assert!(app.overlay.is_none());
        assert!(app.pending_agent_proposal.is_none());
        assert_eq!(app.active_tab().unwrap().document.text, "accepted\n");
    }

    #[test]
    fn agent_proposals_reject_stale_revisions_and_invalid_unicode_ranges() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "café\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let mut app = App::new(workspace).unwrap();
        let stale_revision = revision_label(app.active_tab().unwrap().document.source_revision);
        app.active_tab_mut().unwrap().editor.insert_str("new ");
        app.sync_active_document();

        let response = app.receive_agent_proposal(
            0,
            &path,
            "note.md",
            &stale_revision,
            ProposalChange::ReplaceSource {
                source: "stale\n".to_owned(),
            },
            None,
        );
        assert!(matches!(
            response,
            AgentResponse::Error { ref message } if message.contains("stale")
        ));
        assert!(app.pending_agent_proposal.is_none());

        assert_eq!(
            apply_proposal_change(
                "café",
                ProposalChange::ReplaceRanges {
                    edits: vec![RangeEdit {
                        start: 4,
                        end: 5,
                        replacement: "e".to_owned(),
                    }],
                },
            ),
            Err("agent ranges must use UTF-8 byte boundaries".to_owned())
        );
        assert_eq!(
            apply_proposal_change(
                "abcdef",
                ProposalChange::ReplaceRanges {
                    edits: vec![
                        RangeEdit {
                            start: 2,
                            end: 4,
                            replacement: "X".to_owned(),
                        },
                        RangeEdit {
                            start: 3,
                            end: 5,
                            replacement: "Y".to_owned(),
                        },
                    ],
                },
            ),
            Err("agent ranges must not overlap".to_owned())
        );
    }

    #[test]
    fn agent_proposal_cannot_move_to_an_identical_active_document() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.md");
        let second = root.join("second.md");
        fs::write(&first, "same source\n").unwrap();
        fs::write(&second, "same source\n").unwrap();
        let workspace = Workspace::from_target(&root).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&first).unwrap();
        let first_revision = revision_label(app.active_tab().unwrap().document.source_revision);

        app.open_document(&second).unwrap();
        let second_index = app.active_tab.unwrap();
        assert_eq!(
            first_revision,
            revision_label(app.active_tab().unwrap().document.source_revision)
        );

        let response = app.receive_agent_proposal(
            second_index,
            &second,
            "first.md",
            &first_revision,
            ProposalChange::ReplaceSource {
                source: "wrong target\n".to_owned(),
            },
            None,
        );

        assert!(matches!(
            response,
            AgentResponse::Error { ref message }
                if message.contains("target is not the active document")
        ));
        assert!(app.pending_agent_proposal.is_none());
        assert!(
            app.tabs
                .iter()
                .all(|tab| tab.document.text == "same source\n")
        );
    }

    #[test]
    fn agent_session_reads_the_exact_unsaved_buffer_until_explicit_revocation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "saved\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        app.open_agent_sharing();
        let session = app.agent_session.as_ref().unwrap();
        let socket = session.socket_path().to_path_buf();
        let token = session.token().to_owned();
        let client_socket = socket.clone();
        let client = thread::spawn(move || {
            crate::agent_bridge::request(&client_socket, &token, AgentAction::Read).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if app.poll_agent_requests() {
                break;
            }
            assert!(Instant::now() < deadline, "agent read was not delivered");
            thread::sleep(Duration::from_millis(5));
        }

        let response = client.join().unwrap();
        assert!(matches!(
            response,
            AgentResponse::Document {
                ref source,
                dirty: true,
                ..
            } if source == "draft saved\n"
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved\n");

        app.overlay = None;
        app.close_active_discarding();
        assert!(socket.exists());
        assert!(app.agent_session.is_some());
        app.revoke_agent_sharing();
        assert!(!socket.exists());
        assert!(app.agent_session.is_none());
    }

    #[test]
    fn agent_workspace_keeps_missing_open_buffers_but_excludes_symlink_replacements() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("active.md"), "saved\n").unwrap();
        fs::write(root.join("linked.md"), "linked buffer\n").unwrap();
        fs::write(root.join("notes/context.txt"), "disk context\n").unwrap();
        fs::write(root.join(".git/hidden.md"), "hidden\n").unwrap();
        fs::write(root.join("unreadable.md"), [0xff, 0xfe]).unwrap();
        let workspace = Workspace::from_target(root).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&root.join("active.md")).unwrap();
        app.active_tab_mut().unwrap().editor.insert_str("draft ");
        app.sync_active_document();
        app.open_document(&root.join("linked.md")).unwrap();
        fs::remove_file(root.join("active.md")).unwrap();
        fs::remove_file(root.join("linked.md")).unwrap();
        std::os::unix::fs::symlink("notes/context.txt", root.join("linked.md")).unwrap();
        app.open_agent_sharing();

        let session = app.agent_session.as_ref().unwrap();
        let socket = session.socket_path().to_path_buf();
        let token = session.token().to_owned();
        let client = thread::spawn(move || {
            crate::agent_bridge::request(&socket, &token, AgentAction::ReadWorkspace).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if app.poll_agent_requests() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "agent workspace read was not delivered"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let response = client.join().unwrap();
        let AgentResponse::Workspace {
            documents,
            warnings,
        } = response
        else {
            panic!("expected a workspace response");
        };

        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].path, "active.md");
        assert_eq!(documents[0].source, "draft saved\n");
        assert!(documents[0].dirty);
        assert!(documents[0].open);
        assert!(!documents[0].conflict);
        assert_eq!(documents[1].path, "notes/context.txt");
        assert_eq!(documents[1].source, "disk context\n");
        assert!(!documents[1].dirty);
        assert!(!documents[1].open);
        assert!(!documents[1].conflict);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("unreadable.md"))
        );
        assert!(
            documents
                .iter()
                .all(|document| !document.path.contains(".git") && document.path != "linked.md")
        );
    }

    #[test]
    fn agent_open_snapshot_preflights_each_source_before_cloning() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("large.md"), "0123456789").unwrap();
        let workspace = Workspace::from_target(root).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&root.join("large.md")).unwrap();

        let error = app.agent_open_documents(8, 1_024).unwrap_err();

        assert!(matches!(
            error,
            AgentResponse::Error { ref message }
                if message.contains("large.md") && message.contains("per-document")
        ));
    }

    #[test]
    fn agent_open_snapshot_preflights_its_aggregate_before_cloning() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("first.md"), "first source").unwrap();
        fs::write(root.join("second.md"), "second source").unwrap();
        let workspace = Workspace::from_target(root).unwrap();
        let mut app = App::new(workspace).unwrap();
        app.open_document(&root.join("first.md")).unwrap();
        app.open_document(&root.join("second.md")).unwrap();
        let snapshots = app.agent_open_documents(1_024, usize::MAX).unwrap();
        let first = &snapshots[0];
        let first_document = AgentWorkspaceDocument {
            path: first.relative.display().to_string(),
            source: first.source.clone(),
            revision: first.revision.clone(),
            dirty: first.dirty,
            open: true,
            conflict: first.conflict,
        };
        let empty = AgentResponse::Workspace {
            documents: Vec::new(),
            warnings: Vec::new(),
        };
        let one_document_limit =
            json_size(&empty).unwrap() + json_size(&first_document).unwrap() + 1;

        let error = app
            .agent_open_documents(1_024, usize::try_from(one_document_limit).unwrap())
            .unwrap_err();

        assert!(matches!(
            error,
            AgentResponse::Error { ref message } if message.contains("response exceeds")
        ));
    }

    #[test]
    fn agent_workspace_rejects_a_document_above_its_read_limit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("large.md"), "0123456789").unwrap();
        let workspace = Workspace::from_target(root).unwrap();

        let response =
            agent_workspace_response_with_limits(&workspace, Vec::new(), 8, 1_024, || false);

        assert!(matches!(
            response,
            AgentResponse::Error { ref message }
                if message.contains("large.md") && message.contains("per-document")
        ));
    }

    #[test]
    fn agent_workspace_rejects_an_aggregate_response_above_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("first.md"), "first source").unwrap();
        fs::write(root.join("second.md"), "second source").unwrap();
        let workspace = Workspace::from_target(root).unwrap();
        let complete =
            agent_workspace_response_with_limits(&workspace, Vec::new(), 1_024, usize::MAX, || {
                false
            });
        let AgentResponse::Workspace { documents, .. } = complete else {
            panic!("expected an unbounded workspace response");
        };
        let empty = AgentResponse::Workspace {
            documents: Vec::new(),
            warnings: Vec::new(),
        };
        let one_document_limit = json_size(&empty).unwrap() + json_size(&documents[0]).unwrap() + 1;

        let response = agent_workspace_response_with_limits(
            &workspace,
            Vec::new(),
            1_024,
            usize::try_from(one_document_limit).unwrap(),
            || false,
        );

        assert!(matches!(
            response,
            AgentResponse::Error { ref message } if message.contains("response exceeds")
        ));
    }

    #[test]
    fn local_history_restore_uses_the_consented_mixed_ending_target() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = directory.path().join("mixed.md");
        let original = b"one\r\ntwo\n";
        fs::write(&path, original).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        store.set_enabled(&workspace.root, true).unwrap();
        store
            .capture(
                &workspace.root,
                &path,
                "checkpoint\n",
                CheckpointReason::Manual,
            )
            .unwrap();
        let mut app = app_with_history(workspace, store.clone());

        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.active_tab().unwrap().document.line_ending,
            LineEnding::Mixed
        );
        assert_eq!(
            app.active_tab()
                .unwrap()
                .document
                .mixed_line_ending_target(),
            Some(LineEnding::Crlf)
        );

        execute_palette_action(&mut app, CommandAction::LocalHistory);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

        let document = &app.active_tab().unwrap().document;
        assert_eq!(document.text, "checkpoint\n");
        assert_eq!(document.line_ending, LineEnding::Crlf);
        assert_eq!(document.mixed_line_ending_target(), Some(LineEnding::Crlf));
        assert_eq!(fs::read(&path).unwrap(), original);

        app.execute_command(CommandAction::Undo);
        assert_eq!(app.active_tab().unwrap().document.text, "one\r\ntwo\n");
        assert_eq!(
            app.active_tab().unwrap().document.line_ending,
            LineEnding::Mixed
        );
        app.execute_command(CommandAction::Redo);
        assert_eq!(app.active_tab().unwrap().document.text, "checkpoint\n");
        assert_eq!(
            app.active_tab().unwrap().document.line_ending,
            LineEnding::Crlf
        );
        assert_eq!(fs::read(&path).unwrap(), original);

        let checkpoints = store.list(&app.workspace.root, &path).unwrap().checkpoints;
        assert_eq!(checkpoints[0].reason, CheckpointReason::BeforeRestore);
        assert_eq!(checkpoints[0].source.as_bytes(), original);
    }

    #[test]
    fn local_history_captures_successful_save_and_clean_external_reload() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "one\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        store.set_enabled(&workspace.root, true).unwrap();
        let mut app = app_with_history(workspace, store.clone());

        app.active_tab_mut().unwrap().replace_all("two\n").unwrap();
        app.save_active();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two\n");
        let saved = store.list(&app.workspace.root, &path).unwrap().checkpoints;
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].reason, CheckpointReason::PreviousSavedVersion);
        assert_eq!(saved[0].source, "one\n");

        app.save_active();
        assert_eq!(
            store
                .list(&app.workspace.root, &path)
                .unwrap()
                .checkpoints
                .len(),
            1
        );

        fs::write(&path, "external\n").unwrap();
        assert!(app.poll_document(0, true));
        assert_eq!(app.active_tab().unwrap().document.text, "external\n");
        let reloaded = store.list(&app.workspace.root, &path).unwrap().checkpoints;
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded[0].reason, CheckpointReason::BeforeExternalReload);
        assert_eq!(reloaded[0].source, "two\n");
        assert_eq!(reloaded[1].reason, CheckpointReason::PreviousSavedVersion);
    }

    #[test]
    fn local_history_rename_retargets_lineage_but_save_as_starts_fresh() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let original = directory.path().join("note.md");
        fs::write(&original, "source\n").unwrap();
        let workspace = Workspace::from_target(&original).unwrap();
        let original = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        store.set_enabled(&workspace.root, true).unwrap();
        let mut app = app_with_history(workspace, store.clone());

        execute_palette_action(&mut app, CommandAction::CreateCheckpoint);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.apply_workspace_input(WorkspaceInputAction::Rename, &original, "renamed.md",));
        let renamed = app.workspace.root.join("renamed.md");
        let renamed_history = store
            .list(&app.workspace.root, &renamed)
            .unwrap()
            .checkpoints;
        assert_eq!(renamed_history.len(), 1);
        assert_eq!(
            renamed_history[0].document_path,
            PathBuf::from("renamed.md")
        );
        assert_eq!(renamed_history[0].captured_path, PathBuf::from("note.md"));
        assert!(
            store
                .list(&app.workspace.root, &original)
                .unwrap()
                .checkpoints
                .is_empty()
        );

        assert!(app.apply_path_action(PathAction::SaveAs, "copy.md"));
        let copy = app.workspace.root.join("copy.md");
        assert_eq!(app.active_tab().unwrap().document.path, copy);
        assert!(
            store
                .list(&app.workspace.root, &copy)
                .unwrap()
                .checkpoints
                .is_empty()
        );
        assert_eq!(
            store
                .list(&app.workspace.root, &renamed)
                .unwrap()
                .checkpoints
                .len(),
            1
        );
    }

    #[test]
    fn local_history_clear_defaults_to_cancel_and_revalidates_exact_ids() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "one\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        store.set_enabled(&workspace.root, true).unwrap();
        store
            .capture(&workspace.root, &path, "one\n", CheckpointReason::Manual)
            .unwrap();
        let mut app = app_with_history(workspace, store.clone());

        app.request_history_clear(HistoryClearScope::Document, &path);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            store
                .list(&app.workspace.root, &path)
                .unwrap()
                .checkpoints
                .len(),
            1
        );

        app.request_history_clear(HistoryClearScope::Document, &path);
        store
            .capture(
                &app.workspace.root,
                &path,
                "two\n",
                CheckpointReason::Manual,
            )
            .unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        let unchanged = store.list(&app.workspace.root, &path).unwrap().checkpoints;
        assert_eq!(unchanged.len(), 2);
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory { ref status, .. })
                if status.contains("Clear canceled · Local History changed")
        ));

        app.request_history_clear(HistoryClearScope::Document, &path);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(
            store
                .list(&app.workspace.root, &path)
                .unwrap()
                .checkpoints
                .is_empty()
        );
    }

    #[test]
    fn local_history_pointer_uses_visible_panes_and_exact_clear_actions() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "current\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        store.set_enabled(&workspace.root, true).unwrap();
        store
            .capture(&workspace.root, &path, "first\n", CheckpointReason::Manual)
            .unwrap();
        store
            .capture(&workspace.root, &path, "second\n", CheckpointReason::Manual)
            .unwrap();
        let mut app = app_with_history(workspace, store.clone());

        execute_palette_action(&mut app, CommandAction::LocalHistory);
        let frame = Rect::new(0, 0, 120, 40);
        let area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let list = ui::history_list_area(area);
        app.handle_overlay_click(area, list.x, list.y.saturating_add(1));
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory { selected: 1, .. })
        ));

        let diff = ui::history_diff_area(area);
        app.ui_regions.overlay = Some(area);
        app.handle_overlay_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: diff.x,
            row: diff.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory {
                focus: HistoryFocus::Diff,
                diff_scroll: 1,
                ..
            })
        ));

        app.request_history_clear(HistoryClearScope::Document, &path);
        let confirm_area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let inner = ui::overlay_inner_area(confirm_area);
        app.handle_overlay_click(confirm_area, inner.x, inner.bottom().saturating_sub(2));
        assert!(matches!(
            app.overlay,
            Some(Overlay::HistoryClearConfirm { .. })
        ));

        app.handle_overlay_click(
            confirm_area,
            inner.x.saturating_add(30),
            inner.bottom().saturating_sub(3),
        );
        assert_eq!(
            store
                .list(&app.workspace.root, &path)
                .unwrap()
                .checkpoints
                .len(),
            2
        );
        assert!(matches!(app.overlay, Some(Overlay::LocalHistory { .. })));

        app.request_history_clear(HistoryClearScope::Document, &path);
        let confirm_area = ui::overlay_area(frame, app.overlay.as_ref().unwrap());
        let inner = ui::overlay_inner_area(confirm_area);
        app.handle_overlay_click(confirm_area, inner.x, inner.bottom().saturating_sub(3));
        assert!(
            store
                .list(&app.workspace.root, &path)
                .unwrap()
                .checkpoints
                .is_empty()
        );
    }

    #[test]
    fn local_history_restore_rechecks_revision_and_document_protection() {
        let directory = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "saved\n").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let path = workspace.initial_file.clone().unwrap();
        let store = CheckpointStore::new(state.path().to_path_buf());
        store.set_enabled(&workspace.root, true).unwrap();
        store
            .capture(
                &workspace.root,
                &path,
                "checkpoint\n",
                CheckpointReason::Manual,
            )
            .unwrap();
        let mut app = app_with_history(workspace, store.clone());

        execute_palette_action(&mut app, CommandAction::LocalHistory);
        app.active_tab_mut()
            .unwrap()
            .replace_all("newer\n")
            .unwrap();
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.text, "newer\n");
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory { ref status, .. })
                if status.contains("source changed while history was open")
        ));

        app.active_tab_mut().unwrap().document.conflict = true;
        execute_palette_action(&mut app, CommandAction::LocalHistory);
        app.handle_overlay_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(app.active_tab().unwrap().document.text, "newer\n");
        assert!(matches!(
            app.overlay,
            Some(Overlay::LocalHistory { ref status, .. })
                if status.contains("file changed outside TermDraft")
        ));
        assert_eq!(
            store
                .list(&app.workspace.root, &path)
                .unwrap()
                .checkpoints
                .len(),
            1
        );
    }

    #[test]
    fn local_history_diff_is_checkpoint_to_current_and_bounded() {
        let (diff, truncated) = build_history_diff("old α\n", "new β\n");
        assert!(!truncated);
        assert_eq!(
            diff,
            vec![
                HistoryDiffLine {
                    kind: HistoryDiffKind::Removed,
                    text: "old α".to_owned(),
                },
                HistoryDiffLine {
                    kind: HistoryDiffKind::Added,
                    text: "new β".to_owned(),
                },
                HistoryDiffLine {
                    kind: HistoryDiffKind::Context,
                    text: String::new(),
                },
            ]
        );

        let common = (0..500)
            .map(|line| format!("same {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (diff, truncated) = build_history_diff(
            &format!("{common}\nold tail"),
            &format!("{common}\nnew tail"),
        );
        assert!(!truncated);
        assert!(
            diff.iter()
                .any(|line| { line.kind == HistoryDiffKind::Removed && line.text == "old tail" })
        );
        assert!(
            diff.iter()
                .any(|line| { line.kind == HistoryDiffKind::Added && line.text == "new tail" })
        );
        assert!(diff.iter().any(|line| line.kind == HistoryDiffKind::Notice));

        let checkpoint = (0..500)
            .map(|line| format!("old {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let current = (0..500)
            .map(|line| format!("new {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (diff, truncated) = build_history_diff(&checkpoint, &current);
        assert!(truncated);
        assert_eq!(diff.len(), HISTORY_DIFF_MAX_OUTPUT_LINES);
    }

    #[test]
    fn responsiveness_gate_many_tabs_keeps_switching_bounded_without_eviction() {
        let directory = tempfile::tempdir().unwrap();
        let source = (0..256).fold(String::new(), |mut source, line| {
            writeln!(source, "Paragraph {line}: local-first Markdown source.").unwrap();
            source
        });
        let paths = (0..24)
            .map(|index| {
                let path = directory.path().join(format!("note-{index:02}.md"));
                fs::write(&path, &source).unwrap();
                path
            })
            .collect::<Vec<_>>();
        let workspace = Workspace::from_target(directory.path()).unwrap();
        let mut app = App::with_state_services(workspace, Config::default(), None, None).unwrap();

        let open_started = Instant::now();
        for path in &paths {
            app.open_document(path).unwrap();
            assert!(app.preview_for_draw().is_some());
        }
        let open_elapsed = open_started.elapsed();
        let switch_started = Instant::now();
        for _ in 0..240 {
            app.switch_tab(1);
            let tab = app.active_tab().unwrap();
            let cache = tab.preview_cache.as_ref().unwrap();
            assert_eq!(cache.key.path, tab.document.path);
            assert_eq!(cache.key.revision, tab.document.source_revision);
        }
        let switch_elapsed = switch_started.elapsed();
        let tracked_source_bytes = app
            .tabs
            .iter()
            .map(|tab| {
                tab.document.text.len()
                    + tab.document.saved_text.len()
                    + tab.editor.lines().iter().map(String::len).sum::<usize>()
                    + tab
                        .inline_source_lines
                        .iter()
                        .map(String::len)
                        .sum::<usize>()
                    + tab
                        .inline_editor
                        .lines()
                        .iter()
                        .map(String::len)
                        .sum::<usize>()
                    + tab.preview_cache.as_ref().map_or(0, |cache| {
                        cache
                            .rendered
                            .text
                            .lines
                            .iter()
                            .flat_map(|line| &line.spans)
                            .map(|span| span.content.len())
                            .sum::<usize>()
                    })
            })
            .sum::<usize>();
        eprintln!(
            "many-tab gate: open={open_elapsed:?} switch={switch_elapsed:?} tracked_source_bytes={tracked_source_bytes}"
        );

        let open_limit = if cfg!(debug_assertions) {
            Duration::from_secs(6)
        } else {
            Duration::from_millis(1_500)
        };
        let switch_limit = if cfg!(debug_assertions) {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(250)
        };
        assert_eq!(app.tabs.len(), paths.len());
        assert!(open_elapsed < open_limit);
        assert!(switch_elapsed < switch_limit);
        assert!(tracked_source_bytes <= source.len() * paths.len() * 6);
    }
}
