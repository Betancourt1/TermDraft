//! Ratatui event/update coordinator.

use std::collections::HashMap;
use std::fs;
use std::io::{self, stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::widgets::ListState;
use tui_textarea::{CursorMove, TextArea};

use crate::config::{Config, EditorConfig, StartupMode, StartupView};
use crate::continuation::{EnterAction, action_for};
use crate::document::{Document, Encoding, LineEnding};
use crate::editor::{
    apply_editor_config, source_from_textarea, style_cursor, textarea_from_source,
};
use crate::persistence::{LoadedFile, SaveError, load_file, save_atomic};
use crate::recovery::{RecoveryEntry, RecoveryJournal};
use crate::search::{TextMatch, fuzzy_score, heading_outline, search_text};
use crate::session::{DocumentViewState, MAX_SESSION_DOCUMENTS, SessionState, SessionStore};
use crate::ui;
use crate::workspace::{Workspace, WorkspaceEntry};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Command,
    Write,
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
    Source,
}

impl ViewMode {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inline => "INLINE",
            Self::Split => "SPLIT",
            Self::Source => "SOURCE",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Inline => Self::Split,
            Self::Split => Self::Source,
            Self::Source => Self::Inline,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    Explorer,
    Editor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmAction {
    Quit,
    CloseTab,
}

#[derive(Clone, Debug)]
pub enum Overlay {
    Help,
    Palette {
        input: TextInput,
        selected: usize,
    },
    FileFinder {
        input: TextInput,
        selected: usize,
    },
    RecentDocuments {
        paths: Vec<PathBuf>,
        selected: usize,
    },
    Find {
        input: TextInput,
    },
    WorkspaceSearch {
        input: TextInput,
    },
    SearchResults {
        results: Vec<TextMatch>,
        selected: usize,
    },
    Outline {
        items: Vec<(usize, usize, String)>,
        selected: usize,
    },
    PathInput {
        action: PathAction,
        input: TextInput,
    },
    Recovery {
        entry: Box<RecoveryEntry>,
    },
    Confirm(ConfirmAction),
    Message(String),
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
    Duplicate,
}

impl PathAction {
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Create => " Create file ",
            Self::SaveAs => " Save as ",
            Self::Duplicate => " Duplicate document ",
        }
    }

    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::SaveAs => "save",
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
    CloseTab,
    Quit,
    FileFinder,
    RecentDocuments,
    WorkspaceSearch,
    Find,
    Outline,
    ToggleExplorer,
    CycleView,
    WriteMode,
    CommandMode,
    Undo,
    Redo,
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
        label: "Save as",
        shortcut: "W",
        action: CommandAction::SaveAs,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Duplicate document",
        shortcut: "D",
        action: CommandAction::Duplicate,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Recent documents",
        shortcut: "o",
        action: CommandAction::RecentDocuments,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Save",
        shortcut: "w",
        action: CommandAction::Save,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Close tab",
        shortcut: "C",
        action: CommandAction::CloseTab,
    },
    CommandSpec {
        group: "DOCUMENT",
        label: "Quit safely",
        shortcut: "q",
        action: CommandAction::Quit,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Find file",
        shortcut: "f",
        action: CommandAction::FileFinder,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Search workspace",
        shortcut: "/",
        action: CommandAction::WorkspaceSearch,
    },
    CommandSpec {
        group: "NAVIGATE",
        label: "Document outline",
        shortcut: "S",
        action: CommandAction::Outline,
    },
    CommandSpec {
        group: "FILES",
        label: "Create Markdown file",
        shortcut: "a",
        action: CommandAction::Create,
    },
    CommandSpec {
        group: "FILES",
        label: "Show or hide files",
        shortcut: "e",
        action: CommandAction::ToggleExplorer,
    },
    CommandSpec {
        group: "MODE",
        label: "Enter WRITE mode",
        shortcut: "i",
        action: CommandAction::WriteMode,
    },
    CommandSpec {
        group: "MODE",
        label: "Enter COMMAND mode",
        shortcut: "Esc",
        action: CommandAction::CommandMode,
    },
    CommandSpec {
        group: "EDIT",
        label: "Find in document",
        shortcut: "s",
        action: CommandAction::Find,
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
        group: "VIEW",
        label: "Cycle preview mode",
        shortcut: "v",
        action: CommandAction::CycleView,
    },
    CommandSpec {
        group: "VIEW",
        label: "Show shortcuts",
        shortcut: "?",
        action: CommandAction::Help,
    },
];

pub struct EditorTab {
    pub document: Document,
    pub editor: TextArea<'static>,
}

impl EditorTab {
    fn from_path(path: &Path, config: &EditorConfig) -> anyhow::Result<Self> {
        let loaded = load_file(path)?;
        Ok(Self::from_loaded(loaded, config))
    }

    fn from_loaded(loaded: LoadedFile, config: &EditorConfig) -> Self {
        let mut editor = textarea_from_source(&loaded.text);
        apply_editor_config(&mut editor, config);
        Self {
            document: loaded.into_document(),
            editor,
        }
    }

    pub fn sync_document(&mut self) {
        self.document.text = source_from_textarea(&self.editor);
    }
}

pub struct App {
    pub workspace: Workspace,
    pub config: Config,
    pub entries: Vec<WorkspaceEntry>,
    pub explorer_state: ListState,
    pub tabs: Vec<EditorTab>,
    pub active_tab: Option<usize>,
    pub mode: Mode,
    pub view_mode: ViewMode,
    pub focus: Focus,
    pub show_explorer: bool,
    pub overlay: Option<Overlay>,
    pub status_message: Option<String>,
    pub preview_scroll: u16,
    session_store: Option<SessionStore>,
    last_session_state: Option<SessionState>,
    recovery_journal: Option<RecoveryJournal>,
    published_recovery: HashMap<PathBuf, (String, String)>,
    recent_paths: Vec<PathBuf>,
    session_views: HashMap<PathBuf, DocumentViewState>,
    should_quit: bool,
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
        Self::with_state_services(workspace, config, session_store, recovery_journal)
    }

    fn with_state_services(
        workspace: Workspace,
        config: Config,
        session_store: Option<SessionStore>,
        recovery_journal: Option<RecoveryJournal>,
    ) -> anyhow::Result<Self> {
        let entries = workspace.scan();
        let selected = workspace
            .initial_file
            .as_ref()
            .and_then(|initial| entries.iter().position(|entry| entry.path == *initial));
        let mut explorer_state = ListState::default();
        explorer_state.select(selected.or_else(|| (!entries.is_empty()).then_some(0)));

        let initial_file = workspace.initial_file.clone();
        let restore_open_tabs = initial_file.is_none();
        let startup_mode = config.editor.startup_mode;
        let view_mode = match config.editor.view_mode {
            StartupView::Inline => ViewMode::Inline,
            StartupView::Split => ViewMode::Split,
        };
        let mut app = Self {
            workspace,
            config,
            entries,
            explorer_state,
            tabs: Vec::new(),
            active_tab: None,
            mode: Mode::Command,
            view_mode,
            focus: Focus::Editor,
            show_explorer: true,
            overlay: None,
            status_message: None,
            preview_scroll: 0,
            session_store,
            last_session_state: None,
            recovery_journal,
            published_recovery: HashMap::new(),
            recent_paths: Vec::new(),
            session_views: HashMap::new(),
            should_quit: false,
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
        if restore_open_tabs {
            for path in &state.open_paths {
                let _ = self.open_document(path);
            }
            if let Some(active) = &state.active_path
                && let Some(index) = self
                    .tabs
                    .iter()
                    .position(|tab| tab.document.path == *active)
            {
                self.active_tab = Some(index);
            }
        }
        for tab in &mut self.tabs {
            if let Some(view) = state.view_for(&tab.document.path) {
                tab.editor.move_cursor(CursorMove::Jump(
                    u16::try_from(view.line).unwrap_or(u16::MAX),
                    u16::try_from(view.column).unwrap_or(u16::MAX),
                ));
            }
        }
        self.recent_paths = stored_recent;
        self.session_views = stored_views;
        if let Some(active) = self.active_tab().map(|tab| tab.document.path.clone()) {
            self.mark_recent(&active);
        }
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
        let cursor = self.tabs[index].editor.cursor();
        let mut editor = textarea_from_source(&entry.text);
        apply_editor_config(&mut editor, &self.config.editor);
        style_cursor(&mut editor, self.mode);
        editor.move_cursor(CursorMove::Jump(
            u16::try_from(cursor.0).unwrap_or(u16::MAX),
            u16::try_from(cursor.1).unwrap_or(u16::MAX),
        ));
        self.tabs[index].editor = editor;
        self.tabs[index].document.text.clone_from(&entry.text);
        self.tabs[index].document.encoding = entry.encoding;
        self.tabs[index].document.conflict = conflict;
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

    fn discard_all_recovery(&mut self) {
        let paths = self
            .tabs
            .iter()
            .map(|tab| tab.document.path.clone())
            .collect::<Vec<_>>();
        for path in paths {
            self.discard_recovery_for(&path);
        }
    }

    fn current_session_state(&self) -> SessionState {
        let mut views = self.session_views.clone();
        for tab in &self.tabs {
            let (line, column) = tab.editor.cursor();
            views.insert(
                tab.document.path.clone(),
                DocumentViewState {
                    path: tab.document.path.clone(),
                    line,
                    column,
                },
            );
        }
        SessionState {
            workspace_root: self.workspace.root.clone(),
            active_path: self.active_tab().map(|tab| tab.document.path.clone()),
            documents: self
                .recent_paths
                .iter()
                .filter_map(|path| views.get(path).cloned())
                .collect(),
            open_paths: self
                .tabs
                .iter()
                .map(|tab| tab.document.path.clone())
                .collect(),
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

    fn cache_active_view(&mut self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let (line, column) = tab.editor.cursor();
        let view = DocumentViewState {
            path: tab.document.path.clone(),
            line,
            column,
        };
        self.session_views.insert(view.path.clone(), view);
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
        let path = self.workspace.validate_document_path(path)?;
        if let Some(index) = self.tabs.iter().position(|tab| tab.document.path == path) {
            self.cache_active_view();
            self.active_tab = Some(index);
            self.focus = Focus::Editor;
            self.mark_recent(&path);
            return Ok(());
        }
        self.cache_active_view();
        let mut tab = EditorTab::from_path(&path, &self.config.editor)?;
        style_cursor(&mut tab.editor, self.mode);
        if let Some(view) = self.session_views.get(&path) {
            tab.editor.move_cursor(CursorMove::Jump(
                u16::try_from(view.line).unwrap_or(u16::MAX),
                u16::try_from(view.column).unwrap_or(u16::MAX),
            ));
        }
        let mixed = !tab.document.is_editable();
        self.tabs.push(tab);
        self.active_tab = Some(self.tabs.len() - 1);
        self.focus = Focus::Editor;
        self.preview_scroll = 0;
        self.status_message = mixed.then(|| {
            "Mixed line endings · read-only until normalization is implemented".to_owned()
        });
        self.mark_recent(&path);
        self.enforce_active_read_only();
        self.offer_recovery(&path);
        Ok(())
    }

    fn save_active(&mut self) {
        self.sync_active_document();
        let Some(tab) = self.active_tab_mut() else {
            self.status_message = Some("No document to save".to_owned());
            return;
        };
        if !tab.document.is_editable() {
            self.status_message = Some("Mixed line endings remain read-only".to_owned());
            return;
        }
        if tab.document.conflict {
            self.status_message = Some("CONFLICT · use Save As to preserve this draft".to_owned());
            return;
        }
        if !tab.document.is_dirty() {
            self.status_message = Some("Already saved".to_owned());
            return;
        }

        let result = save_atomic(
            &tab.document.path,
            &tab.document.text,
            tab.document.encoding,
            tab.document.line_ending,
            Some(&tab.document.snapshot),
            false,
        );
        let mut saved_path = None;
        match result {
            Ok(snapshot) => {
                tab.document.mark_saved(snapshot);
                saved_path = Some(tab.document.path.clone());
                self.status_message = Some("Saved".to_owned());
            }
            Err(SaveError::Conflict) => {
                tab.document.conflict = true;
                self.status_message = Some("CONFLICT · file changed on disk".to_owned());
            }
            Err(error) => self.status_message = Some(format!("Save failed · {error}")),
        }
        if let Some(path) = saved_path {
            self.discard_recovery_for(&path);
        }
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
        self.focus = Focus::Editor;
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

    fn request_quit(&mut self) {
        self.sync_active_document();
        if self.tabs.iter().any(|tab| tab.document.is_dirty()) {
            self.overlay = Some(Overlay::Confirm(ConfirmAction::Quit));
        } else {
            self.should_quit = true;
        }
    }

    fn close_active(&mut self) {
        self.sync_active_document();
        let Some(index) = self.active_tab else {
            return;
        };
        if self.tabs[index].document.is_dirty() {
            self.overlay = Some(Overlay::Confirm(ConfirmAction::CloseTab));
            return;
        }
        self.close_active_discarding();
    }

    fn close_active_discarding(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        self.cache_active_view();
        self.tabs.remove(index);
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
        self.cache_active_view();
        self.active_tab = usize::try_from(next).ok();
        if let Some(path) = self.active_tab().map(|tab| tab.document.path.clone()) {
            self.mark_recent(&path);
        }
        self.preview_scroll = 0;
        self.enforce_active_read_only();
    }

    fn move_editor(&mut self, movement: CursorMove) {
        if let Some(tab) = self.active_tab_mut() {
            tab.editor.move_cursor(movement);
        }
    }

    fn execute_command(&mut self, action: CommandAction) {
        self.overlay = None;
        match action {
            CommandAction::Save => self.save_active(),
            CommandAction::SaveAs => self.open_path_input(PathAction::SaveAs),
            CommandAction::Duplicate => self.open_path_input(PathAction::Duplicate),
            CommandAction::Create => self.open_path_input(PathAction::Create),
            CommandAction::CloseTab => self.close_active(),
            CommandAction::Quit => self.request_quit(),
            CommandAction::FileFinder => {
                self.overlay = Some(Overlay::FileFinder {
                    input: TextInput::default(),
                    selected: 0,
                });
            }
            CommandAction::RecentDocuments => self.open_recent_documents(),
            CommandAction::WorkspaceSearch => {
                self.overlay = Some(Overlay::WorkspaceSearch {
                    input: TextInput::default(),
                });
            }
            CommandAction::Find => {
                self.overlay = Some(Overlay::Find {
                    input: TextInput::default(),
                });
            }
            CommandAction::Outline => self.open_outline(),
            CommandAction::ToggleExplorer => {
                self.show_explorer = !self.show_explorer;
                self.focus = if self.show_explorer {
                    Focus::Explorer
                } else {
                    Focus::Editor
                };
            }
            CommandAction::CycleView => {
                self.view_mode = self.view_mode.next();
                self.preview_scroll = 0;
            }
            CommandAction::WriteMode => self.set_mode(Mode::Write),
            CommandAction::CommandMode => self.set_mode(Mode::Command),
            CommandAction::Undo => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.editor.undo();
                    tab.sync_document();
                }
            }
            CommandAction::Redo => {
                if let Some(tab) = self.active_tab_mut() {
                    tab.editor.redo();
                    tab.sync_document();
                }
            }
            CommandAction::Help => self.overlay = Some(Overlay::Help),
        }
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
            self.overlay = Some(Overlay::Outline { items, selected: 0 });
        }
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
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let text = tab.document.text.clone();
        let encoding = tab.document.encoding;
        let line_ending = tab.document.line_ending;
        match save_atomic(&target, &text, encoding, line_ending, None, true) {
            Ok(snapshot) if action == PathAction::SaveAs => {
                let previous = self.active_tab().map(|tab| tab.document.path.clone());
                if let Some(tab) = self.active_tab_mut() {
                    tab.document.path.clone_from(&target);
                    tab.document.mark_saved(snapshot);
                }
                if let Some(previous) = previous.as_deref() {
                    self.retarget_recent(previous, &target);
                }
                self.refresh_entries(Some(&target));
                self.status_message = Some(format!(
                    "Saved as {}",
                    self.workspace.relative(&target).display()
                ));
                if let Some(previous) = previous {
                    self.discard_recovery_for(&previous);
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
        self.entries = self.workspace.scan();
        let selected = selected_path
            .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
            .or_else(|| (!self.entries.is_empty()).then_some(0));
        self.explorer_state.select(selected);
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.overlay.is_some() {
            self.handle_overlay_key(key);
            return;
        }
        if self.handle_global_key(key) {
            return;
        }
        match self.mode {
            Mode::Write => self.handle_write_key(key),
            Mode::Command => self.handle_command_key(key),
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent) -> bool {
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Char('s') => self.save_active(),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('b') => self.execute_command(CommandAction::ToggleExplorer),
            KeyCode::Char('p') => self.execute_command(CommandAction::FileFinder),
            KeyCode::Char('o') => self.execute_command(CommandAction::RecentDocuments),
            KeyCode::Char('f') => self.execute_command(CommandAction::Find),
            KeyCode::Char('e') => self.execute_command(CommandAction::CycleView),
            KeyCode::PageDown => self.switch_tab(1),
            KeyCode::PageUp => self.switch_tab(-1),
            _ => return false,
        }
        true
    }

    fn handle_write_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.set_mode(Mode::Command);
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
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let modified = self.active_tab_mut().is_some_and(|tab| {
            if auto_continue && !tab.editor.is_selecting() {
                let (row, column) = tab.editor.cursor();
                match action_for(tab.editor.lines(), row, column) {
                    EnterAction::Continue(marker) => {
                        tab.editor.insert_str(format!("\n{marker}"));
                        true
                    }
                    EnterAction::EndMarker(characters) => {
                        tab.editor.delete_str(characters);
                        tab.editor.insert_newline();
                        true
                    }
                    EnterAction::Plain => tab.editor.input(key),
                }
            } else {
                tab.editor.input(key)
            }
        });
        if modified {
            self.sync_active_document();
            self.status_message = None;
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        if self.focus == Focus::Explorer {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_explorer(-1);
                    return;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_explorer(1);
                    return;
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    self.open_selected_entry();
                    return;
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.focus = Focus::Editor;
                    return;
                }
                KeyCode::Char('a') => {
                    self.open_path_input(PathAction::Create);
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('i') => self.set_mode(Mode::Write),
            KeyCode::Char('w') => self.save_active(),
            KeyCode::Char('W') => self.open_path_input(PathAction::SaveAs),
            KeyCode::Char('D') => self.open_path_input(PathAction::Duplicate),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('e') => self.execute_command(CommandAction::ToggleExplorer),
            KeyCode::Char('f') => self.execute_command(CommandAction::FileFinder),
            KeyCode::Char('o') => self.execute_command(CommandAction::RecentDocuments),
            KeyCode::Char('/') => self.execute_command(CommandAction::WorkspaceSearch),
            KeyCode::Char('s') => self.execute_command(CommandAction::Find),
            KeyCode::Char('S') => self.execute_command(CommandAction::Outline),
            KeyCode::Char('v') => self.execute_command(CommandAction::CycleView),
            KeyCode::Char('u') => self.execute_command(CommandAction::Undo),
            KeyCode::Char('U') => self.execute_command(CommandAction::Redo),
            KeyCode::Char(':') => {
                self.overlay = Some(Overlay::Palette {
                    input: TextInput::default(),
                    selected: 0,
                });
            }
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('[') => self.switch_tab(-1),
            KeyCode::Char(']') => self.switch_tab(1),
            KeyCode::Char('C') => self.close_active(),
            KeyCode::Char('h') | KeyCode::Left => self.move_editor(CursorMove::Back),
            KeyCode::Char('j') | KeyCode::Down => self.move_editor(CursorMove::Down),
            KeyCode::Char('k') | KeyCode::Up => self.move_editor(CursorMove::Up),
            KeyCode::Char('l') | KeyCode::Right => self.move_editor(CursorMove::Forward),
            KeyCode::Char('0') | KeyCode::Home => self.move_editor(CursorMove::Head),
            KeyCode::Char('$') | KeyCode::End => self.move_editor(CursorMove::End),
            KeyCode::Char('g') => self.move_editor(CursorMove::Top),
            KeyCode::Char('G') => self.move_editor(CursorMove::Bottom),
            KeyCode::Tab if self.show_explorer => {
                self.focus = if self.focus == Focus::Explorer {
                    Focus::Editor
                } else {
                    Focus::Explorer
                };
            }
            _ => {}
        }
    }

    fn move_explorer(&mut self, direction: isize) {
        if self.entries.is_empty() {
            return;
        }
        let current = self.explorer_state.selected().unwrap_or_default();
        let maximum = self.entries.len() - 1;
        let next = if direction < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(maximum)
        };
        self.explorer_state.select(Some(next));
    }

    fn open_selected_entry(&mut self) {
        let Some(index) = self.explorer_state.selected() else {
            return;
        };
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        if entry.is_dir {
            self.status_message = Some(format!(
                "{} · folders are expanded",
                entry.relative.display()
            ));
            return;
        }
        let path = entry.path.clone();
        if let Err(error) = self.open_document(&path) {
            self.status_message = Some(format!("Open failed · {error}"));
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_overlay_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.overlay = None;
            return;
        }
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        let keep = match &mut overlay {
            Overlay::Help | Overlay::Message(_) => key.code != KeyCode::Enter,
            Overlay::Confirm(action) => match key.code {
                KeyCode::Char('y') => match action {
                    ConfirmAction::Quit => {
                        let saved = self.save_all_dirty();
                        self.should_quit = saved;
                        !saved
                    }
                    ConfirmAction::CloseTab => {
                        self.save_active();
                        let saved = self.active_tab().is_none_or(|tab| !tab.document.is_dirty());
                        if saved {
                            self.close_active_discarding();
                        }
                        !saved
                    }
                },
                KeyCode::Char('n') => {
                    match action {
                        ConfirmAction::Quit => {
                            self.discard_all_recovery();
                            for tab in &mut self.tabs {
                                tab.document.saved_text.clone_from(&tab.document.text);
                                tab.document.conflict = false;
                            }
                            self.should_quit = true;
                        }
                        ConfirmAction::CloseTab => {
                            if let Some(path) =
                                self.active_tab().map(|tab| tab.document.path.clone())
                            {
                                self.discard_recovery_for(&path);
                            }
                            self.close_active_discarding();
                        }
                    }
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
            Overlay::FileFinder { input, selected } => {
                let candidates = self.file_candidates(&input.value);
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
                        if let Some(index) = candidates.get(*selected) {
                            let path = self.entries[*index].path.clone();
                            if let Err(error) = self.open_document(&path) {
                                self.status_message = Some(format!("Open failed · {error}"));
                            }
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
            Overlay::Find { input } => match key.code {
                KeyCode::Enter => {
                    self.find_in_document(&input.value);
                    false
                }
                _ if edit_text_input(input, key) => true,
                _ => true,
            },
            Overlay::WorkspaceSearch { input } => match key.code {
                KeyCode::Enter => {
                    self.run_workspace_search(&input.value);
                    false
                }
                _ if edit_text_input(input, key) => true,
                _ => true,
            },
            Overlay::PathInput { action, input } => match key.code {
                KeyCode::Enter => !self.apply_path_action(*action, &input.value),
                _ if edit_text_input(input, key) => true,
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
            Overlay::Outline { items, selected } => match key.code {
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                    true
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(items.len().saturating_sub(1));
                    true
                }
                KeyCode::Enter => {
                    if let Some((line, _, _)) = items.get(*selected) {
                        self.move_editor(CursorMove::Jump(
                            u16::try_from(*line).unwrap_or(u16::MAX),
                            0,
                        ));
                    }
                    false
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
        match overlay {
            Overlay::Palette { input, selected } | Overlay::FileFinder { input, selected } => {
                input.insert(text);
                *selected = 0;
                true
            }
            Overlay::Find { input }
            | Overlay::WorkspaceSearch { input }
            | Overlay::PathInput { input, .. } => {
                input.insert(text);
                true
            }
            _ => false,
        }
    }

    #[must_use]
    pub fn file_candidates(&self, query: &str) -> Vec<usize> {
        let mut candidates = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| !entry.is_dir)
            .filter_map(|(index, entry)| {
                fuzzy_score(query, &entry.relative.to_string_lossy()).map(|score| (score, index))
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|(left_score, left), (right_score, right)| {
            right_score.cmp(left_score).then_with(|| {
                self.entries[*left]
                    .relative
                    .cmp(&self.entries[*right].relative)
            })
        });
        candidates
            .into_iter()
            .take(100)
            .map(|(_, index)| index)
            .collect()
    }

    fn find_in_document(&mut self, query: &str) {
        if query.is_empty() {
            return;
        }
        let Some(tab) = self.active_tab_mut() else {
            return;
        };
        if tab.editor.set_search_pattern(query).is_ok() && tab.editor.search_forward(false) {
            self.status_message = Some(format!("Found · {query}"));
        } else {
            self.status_message = Some(format!("No match · {query}"));
        }
    }

    fn run_workspace_search(&mut self, query: &str) {
        if query.is_empty() {
            return;
        }
        self.sync_active_document();
        let mut results = Vec::new();
        for entry in self.entries.iter().filter(|entry| !entry.is_dir) {
            let text = self
                .tabs
                .iter()
                .find(|tab| tab.document.path == entry.path)
                .map_or_else(
                    || load_file(&entry.path).map_or_else(|_| String::new(), |file| file.text),
                    |tab| tab.document.text.clone(),
                );
            results.extend(search_text(&entry.path, &text, query, 100 - results.len()));
            if results.len() == 100 {
                break;
            }
        }
        if results.is_empty() {
            self.status_message = Some(format!("No workspace match · {query}"));
        } else {
            self.overlay = Some(Overlay::SearchResults {
                results,
                selected: 0,
            });
        }
    }

    fn open_search_result(&mut self, result: &TextMatch) {
        if let Err(error) = self.open_document(&result.path) {
            self.status_message = Some(format!("Open failed · {error}"));
            return;
        }
        self.move_editor(CursorMove::Jump(
            u16::try_from(result.line).unwrap_or(u16::MAX),
            u16::try_from(result.column).unwrap_or(u16::MAX),
        ));
    }

    fn poll_external_state(&mut self) -> bool {
        self.sync_active_document();
        let mut changed = self.poll_active_document();
        let selected_path = self
            .explorer_state
            .selected()
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.path.clone());
        let entries = self.workspace.scan();
        if entries != self.entries {
            self.entries = entries;
            let selection = selected_path
                .as_ref()
                .and_then(|path| self.entries.iter().position(|entry| entry.path == *path))
                .or_else(|| (!self.entries.is_empty()).then_some(0));
            self.explorer_state.select(selection);
            changed = true;
        }
        changed
    }

    fn poll_active_document(&mut self) -> bool {
        let Some(index) = self.active_tab else {
            return false;
        };
        let path = self.tabs[index].document.path.clone();
        let baseline = self.tabs[index].document.snapshot.clone();
        match load_file(&path) {
            Ok(loaded) if loaded.snapshot == baseline => false,
            Ok(loaded) => {
                let same_content = loaded.snapshot.sha256 == baseline.sha256;
                let same_origin = loaded.snapshot.same_origin(&baseline);
                let dirty = self.tabs[index].document.text != self.tabs[index].document.saved_text;
                if dirty && (!same_content || !same_origin) {
                    if !self.tabs[index].document.conflict {
                        self.tabs[index].document.conflict = true;
                        self.status_message =
                            Some("CONFLICT · file changed outside TermDraft".to_owned());
                        return true;
                    }
                    return false;
                }
                if same_content {
                    self.tabs[index].document.snapshot = loaded.snapshot;
                    return false;
                }

                let cursor = self.tabs[index].editor.cursor();
                let mut tab = EditorTab::from_loaded(loaded, &self.config.editor);
                style_cursor(&mut tab.editor, self.mode);
                tab.editor.move_cursor(CursorMove::Jump(
                    u16::try_from(cursor.0).unwrap_or(u16::MAX),
                    u16::try_from(cursor.1).unwrap_or(u16::MAX),
                ));
                self.tabs[index] = tab;
                self.status_message = Some("Reloaded external changes".to_owned());
                true
            }
            Err(error) => {
                if self.tabs[index].document.conflict {
                    false
                } else {
                    self.tabs[index].document.conflict = true;
                    self.status_message = Some(format!("CONFLICT · file unavailable · {error}"));
                    true
                }
            }
        }
    }

    fn save_all_dirty(&mut self) -> bool {
        let original = self.active_tab;
        for index in 0..self.tabs.len() {
            if !self.tabs[index].document.is_dirty() {
                continue;
            }
            self.active_tab = Some(index);
            self.save_active();
            if self.tabs[index].document.is_dirty() {
                return false;
            }
        }
        self.active_tab = original;
        true
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

#[must_use]
pub fn command_candidates(query: &str) -> Vec<CommandSpec> {
    let mut commands = COMMANDS
        .iter()
        .copied()
        .filter_map(|command| {
            fuzzy_score(query, &format!("{} {}", command.group, command.label))
                .map(|score| (score, command))
        })
        .collect::<Vec<_>>();
    commands.sort_by(|(left_score, _), (right_score, _)| right_score.cmp(left_score));
    commands.into_iter().map(|(_, command)| command).collect()
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
    let mut app = App::with_config(workspace, config)?;
    ratatui::run(|terminal| -> anyhow::Result<()> {
        let mut terminal_extras = TerminalExtrasGuard::enable()?;
        let result = (|| {
            let mut rendered_mode = app.mode;
            let mut needs_draw = true;
            let mut next_disk_poll = Instant::now() + Duration::from_secs(2);
            let mut next_recovery_flush = Instant::now() + Duration::from_millis(500);
            while !app.should_quit {
                if needs_draw {
                    terminal.draw(|frame| ui::draw(frame, &mut app))?;
                    needs_draw = false;
                }
                if event::poll(Duration::from_millis(250))? {
                    match event::read()? {
                        Event::Key(key)
                            if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                        {
                            app.handle_key(key);
                        }
                        Event::Paste(text) => {
                            if !app.paste_into_overlay(&text)
                                && app.mode == Mode::Write
                                && let Some(tab) = app
                                    .active_tab_mut()
                                    .filter(|tab| tab.document.is_editable())
                            {
                                tab.editor.insert_str(text);
                                tab.sync_document();
                            }
                        }
                        _ => {}
                    }
                    needs_draw = true;
                }
                if rendered_mode != app.mode {
                    let shape = if app.mode == Mode::Write {
                        SetCursorStyle::BlinkingBar
                    } else {
                        SetCursorStyle::SteadyBlock
                    };
                    execute!(stdout(), shape)?;
                    rendered_mode = app.mode;
                }
                if Instant::now() >= next_disk_poll {
                    needs_draw |= app.poll_external_state();
                    app.persist_session_if_changed();
                    next_disk_poll = Instant::now() + Duration::from_secs(2);
                }
                if Instant::now() >= next_recovery_flush {
                    app.persist_recovery();
                    next_recovery_flush = Instant::now() + Duration::from_millis(500);
                }
            }
            app.persist_recovery();
            app.persist_session_if_changed();
            Ok(())
        })();
        terminal_extras
            .restore()
            .context("restore mouse and cursor state")?;
        result
    })
}

struct TerminalExtrasGuard {
    active: bool,
}

impl TerminalExtrasGuard {
    fn enable() -> io::Result<Self> {
        execute!(stdout(), EnableMouseCapture)?;
        let guard = Self { active: true };
        if let Err(error) = execute!(stdout(), SetCursorStyle::SteadyBlock) {
            drop(guard);
            return Err(error);
        }
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.active {
            execute!(
                stdout(),
                DisableMouseCapture,
                SetCursorStyle::DefaultUserShape
            )?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TerminalExtrasGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(
                stdout(),
                DisableMouseCapture,
                SetCursorStyle::DefaultUserShape
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_groups_preserve_frontend_contract() {
        let groups = COMMANDS
            .iter()
            .map(|command| command.group)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            groups,
            std::collections::BTreeSet::from([
                "DOCUMENT", "EDIT", "FILES", "MODE", "NAVIGATE", "VIEW"
            ])
        );
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
        app.overlay = Some(Overlay::Find {
            input: TextInput::default(),
        });

        assert!(app.paste_into_overlay("café\nneedle"));

        let Some(Overlay::Find { input }) = &app.overlay else {
            panic!("find popup closed unexpectedly");
        };
        assert_eq!(input.value, "caféneedle");
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "source"
        );
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

        fs::write(&path, "external").unwrap();
        assert!(app.poll_external_state());
        assert_eq!(
            source_from_textarea(&app.active_tab().unwrap().editor),
            "external"
        );
        assert!(!app.active_tab().unwrap().document.conflict);

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
    fn external_poll_refreshes_the_workspace_tree() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("one.md");
        fs::write(&path, "one").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();

        fs::write(directory.path().join("two.md"), "two").unwrap();

        assert!(app.poll_external_state());
        assert!(
            app.entries
                .iter()
                .any(|entry| entry.relative == Path::new("two.md"))
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
                    DocumentViewState {
                        path: first.clone(),
                        line: 1,
                        column: 2,
                    },
                    DocumentViewState {
                        path: second.clone(),
                        line: 1,
                        column: 3,
                    },
                ],
                open_paths: vec![first.clone(), second.clone()],
            })
            .unwrap();
        let workspace = Workspace::from_target(&root).unwrap();

        let app =
            App::with_state_services(workspace, Config::default(), Some(store), None).unwrap();

        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab().unwrap().document.path, second);
        assert_eq!(app.active_tab().unwrap().editor.cursor(), (1, 3));
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
                    DocumentViewState {
                        path: missing,
                        line: 0,
                        column: 0,
                    },
                    DocumentViewState {
                        path: closed.clone(),
                        line: 0,
                        column: 0,
                    },
                    DocumentViewState {
                        path: current.clone(),
                        line: 0,
                        column: 0,
                    },
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
}
