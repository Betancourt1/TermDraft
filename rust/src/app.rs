//! Ratatui event/update coordinator.

use std::fs;
use std::io::stdout;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::widgets::ListState;
use tui_textarea::{CursorMove, TextArea};

use crate::document::Document;
use crate::editor::{source_from_textarea, style_cursor, textarea_from_source};
use crate::persistence::{SaveError, load_file, save_atomic};
use crate::search::{TextMatch, fuzzy_score, heading_outline, search_text};
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
        query: String,
        selected: usize,
    },
    FileFinder {
        query: String,
        selected: usize,
    },
    Find {
        query: String,
    },
    WorkspaceSearch {
        query: String,
    },
    SearchResults {
        results: Vec<TextMatch>,
        selected: usize,
    },
    Outline {
        items: Vec<(usize, usize, String)>,
        selected: usize,
    },
    Confirm(ConfirmAction),
    Message(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandAction {
    Save,
    CloseTab,
    Quit,
    FileFinder,
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
    fn from_path(path: &Path) -> anyhow::Result<Self> {
        let loaded = load_file(path)?;
        let editor = textarea_from_source(&loaded.text);
        Ok(Self {
            document: loaded.into_document(),
            editor,
        })
    }

    pub fn sync_document(&mut self) {
        self.document.text = source_from_textarea(&self.editor);
    }
}

pub struct App {
    pub workspace: Workspace,
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
    should_quit: bool,
}

impl App {
    /// Build an application and open an explicit file target when one was supplied.
    ///
    /// # Errors
    ///
    /// Returns an error when the initial file cannot be loaded safely.
    pub fn new(workspace: Workspace) -> anyhow::Result<Self> {
        let entries = workspace.scan();
        let selected = workspace
            .initial_file
            .as_ref()
            .and_then(|initial| entries.iter().position(|entry| entry.path == *initial));
        let mut explorer_state = ListState::default();
        explorer_state.select(selected.or_else(|| (!entries.is_empty()).then_some(0)));

        let initial_file = workspace.initial_file.clone();
        let mut app = Self {
            workspace,
            entries,
            explorer_state,
            tabs: Vec::new(),
            active_tab: None,
            mode: Mode::Command,
            view_mode: ViewMode::Inline,
            focus: Focus::Editor,
            show_explorer: true,
            overlay: None,
            status_message: None,
            preview_scroll: 0,
            should_quit: false,
        };
        if let Some(path) = initial_file {
            app.open_document(&path)?;
        } else if app.entries.is_empty() {
            app.focus = Focus::Explorer;
            app.status_message = Some("Empty workspace · create or add a Markdown file".to_owned());
        }
        Ok(app)
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

    fn open_document(&mut self, path: &Path) -> anyhow::Result<()> {
        let path = self.workspace.validate_document_path(path)?;
        if let Some(index) = self.tabs.iter().position(|tab| tab.document.path == path) {
            self.active_tab = Some(index);
            self.focus = Focus::Editor;
            return Ok(());
        }
        let tab = EditorTab::from_path(&path)?;
        let mixed = !tab.document.is_editable();
        self.tabs.push(tab);
        self.active_tab = Some(self.tabs.len() - 1);
        self.focus = Focus::Editor;
        self.preview_scroll = 0;
        self.status_message = mixed.then(|| {
            "Mixed line endings · read-only until normalization is implemented".to_owned()
        });
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
        match result {
            Ok(snapshot) => {
                tab.document.mark_saved(snapshot);
                self.status_message = Some("Saved".to_owned());
            }
            Err(SaveError::Conflict) => {
                tab.document.conflict = true;
                self.status_message = Some("CONFLICT · file changed on disk".to_owned());
            }
            Err(error) => self.status_message = Some(format!("Save failed · {error}")),
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        if mode == Mode::Write
            && self
                .active_tab()
                .is_none_or(|tab| !tab.document.is_editable())
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
        self.tabs.remove(index);
        self.active_tab = if self.tabs.is_empty() {
            None
        } else {
            Some(index.min(self.tabs.len() - 1))
        };
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
        self.active_tab = usize::try_from(next).ok();
        self.preview_scroll = 0;
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
            CommandAction::CloseTab => self.close_active(),
            CommandAction::Quit => self.request_quit(),
            CommandAction::FileFinder => {
                self.overlay = Some(Overlay::FileFinder {
                    query: String::new(),
                    selected: 0,
                });
            }
            CommandAction::WorkspaceSearch => {
                self.overlay = Some(Overlay::WorkspaceSearch {
                    query: String::new(),
                });
            }
            CommandAction::Find => {
                self.overlay = Some(Overlay::Find {
                    query: String::new(),
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
        let modified = self
            .active_tab_mut()
            .is_some_and(|tab| tab.editor.input(key));
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
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('i') => self.set_mode(Mode::Write),
            KeyCode::Char('w') => self.save_active(),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('e') => self.execute_command(CommandAction::ToggleExplorer),
            KeyCode::Char('f') => self.execute_command(CommandAction::FileFinder),
            KeyCode::Char('/') => self.execute_command(CommandAction::WorkspaceSearch),
            KeyCode::Char('s') => self.execute_command(CommandAction::Find),
            KeyCode::Char('S') => self.execute_command(CommandAction::Outline),
            KeyCode::Char('v') => self.execute_command(CommandAction::CycleView),
            KeyCode::Char('u') => self.execute_command(CommandAction::Undo),
            KeyCode::Char('U') => self.execute_command(CommandAction::Redo),
            KeyCode::Char(':') => {
                self.overlay = Some(Overlay::Palette {
                    query: String::new(),
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
                KeyCode::Char('y') | KeyCode::Enter => {
                    match action {
                        ConfirmAction::Quit => self.should_quit = true,
                        ConfirmAction::CloseTab => self.close_active_discarding(),
                    }
                    false
                }
                KeyCode::Char('n') => false,
                _ => true,
            },
            Overlay::Palette { query, selected } => {
                let candidates = command_candidates(query);
                match key.code {
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        true
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(candidates.len().saturating_sub(1));
                        true
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *selected = 0;
                        true
                    }
                    KeyCode::Char(character) => {
                        query.push(character);
                        *selected = 0;
                        true
                    }
                    KeyCode::Enter => {
                        if let Some(command) = candidates.get(*selected) {
                            self.execute_command(command.action);
                        }
                        false
                    }
                    _ => true,
                }
            }
            Overlay::FileFinder { query, selected } => {
                let candidates = self.file_candidates(query);
                match key.code {
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        true
                    }
                    KeyCode::Down => {
                        *selected = (*selected + 1).min(candidates.len().saturating_sub(1));
                        true
                    }
                    KeyCode::Backspace => {
                        query.pop();
                        *selected = 0;
                        true
                    }
                    KeyCode::Char(character) => {
                        query.push(character);
                        *selected = 0;
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
                    _ => true,
                }
            }
            Overlay::Find { query } => match key.code {
                KeyCode::Backspace => {
                    query.pop();
                    true
                }
                KeyCode::Char(character) => {
                    query.push(character);
                    true
                }
                KeyCode::Enter => {
                    self.find_in_document(query);
                    false
                }
                _ => true,
            },
            Overlay::WorkspaceSearch { query } => match key.code {
                KeyCode::Backspace => {
                    query.pop();
                    true
                }
                KeyCode::Char(character) => {
                    query.push(character);
                    true
                }
                KeyCode::Enter => {
                    self.run_workspace_search(query);
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
        candidates.into_iter().map(|(_, index)| index).collect()
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
                    || fs::read_to_string(&entry.path).unwrap_or_default(),
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
    let mut app = App::new(workspace)?;
    ratatui::run(|terminal| -> anyhow::Result<()> {
        execute!(stdout(), EnableMouseCapture, SetCursorStyle::SteadyBlock)?;
        let result = (|| {
            let mut rendered_mode = app.mode;
            let mut needs_draw = true;
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
                        Event::Paste(text) if app.mode == Mode::Write => {
                            if let Some(tab) = app.active_tab_mut() {
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
            }
            Ok(())
        })();
        execute!(
            stdout(),
            DisableMouseCapture,
            SetCursorStyle::DefaultUserShape
        )
        .context("restore mouse and cursor state")?;
        result
    })
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
}
