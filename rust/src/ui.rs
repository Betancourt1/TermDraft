//! Ratatui rendering for the preserved `TermDraft` layout and overlays.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};

use crate::app::{App, ConfirmAction, Focus, Mode, Overlay, ViewMode, command_candidates};
use crate::editor::apply_inline_preview;

const BACKGROUND: Color = Color::Black;
const SURFACE: Color = Color::Rgb(16, 16, 16);
const PANEL: Color = Color::Rgb(28, 28, 28);
const BORDER: Color = Color::Rgb(58, 58, 58);
const TEXT: Color = Color::Rgb(218, 218, 218);
const MUTED: Color = Color::Rgb(118, 118, 118);
const BRIGHT: Color = Color::Rgb(242, 242, 242);

pub fn draw(frame: &mut Frame, app: &mut App) {
    frame.render_widget(
        Block::new().style(Style::new().bg(BACKGROUND)),
        frame.area(),
    );
    let tab_height = u16::from(app.tabs.len() > 1);
    let [title_area, tabs_area, main_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(tab_height),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_title(frame, app, title_area);
    if tab_height > 0 {
        draw_tabs(frame, app, tabs_area);
    }
    draw_workspace(frame, app, main_area);
    draw_status(frame, app, status_area);
    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, app, overlay);
    }
}

fn draw_title(frame: &mut Frame, app: &App, area: Rect) {
    let root = app.workspace.root.display();
    let title = Line::from(vec![
        Span::styled("TermDraft", Style::new().fg(BRIGHT).bold()),
        Span::styled(" · ", Style::new().fg(MUTED)),
        Span::styled(root.to_string(), Style::new().fg(TEXT)),
        Span::styled("  RUST PORT", Style::new().fg(MUTED).bold()),
    ]);
    frame.render_widget(
        Paragraph::new(title)
            .style(Style::new().bg(PANEL))
            .block(Block::new().padding(Padding::horizontal(1))),
        area,
    );
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (index, tab) in app.tabs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" │ ", Style::new().fg(BORDER)));
        }
        let name = tab
            .document
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let dirty = if tab.document.is_dirty() { " ●" } else { "" };
        let style = if Some(index) == app.active_tab {
            Style::new().fg(BRIGHT).bold()
        } else {
            Style::new().fg(MUTED)
        };
        spans.push(Span::styled(format!(" {name}{dirty} "), style));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(SURFACE)),
        area,
    );
}

fn draw_workspace(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.show_explorer {
        let explorer_width = area.width.clamp(20, 34).min(area.width.saturating_sub(20));
        let [explorer, divider, workbench] = Layout::horizontal([
            Constraint::Length(explorer_width),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(area);
        draw_explorer(frame, app, explorer);
        frame.render_widget(Block::new().style(Style::new().bg(BORDER)), divider);
        draw_workbench(frame, app, workbench);
    } else {
        draw_workbench(frame, app, area);
    }
}

fn draw_explorer(frame: &mut Frame, app: &mut App, area: Rect) {
    let title_style = if app.focus == Focus::Explorer {
        Style::new().fg(BRIGHT).bold()
    } else {
        Style::new().fg(TEXT).bold()
    };
    let block = Block::new()
        .title(Line::from(" Files ").style(title_style))
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER))
        .style(Style::new().bg(SURFACE));
    let items = app.entries.iter().map(|entry| {
        let indent = "  ".repeat(entry.depth);
        let icon = if entry.is_dir { "▸" } else { "◆" };
        let name = entry
            .relative
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let icon_style = if entry.is_dir {
            Style::new().fg(Color::Rgb(142, 142, 142))
        } else {
            Style::new().fg(Color::Rgb(184, 184, 184))
        };
        ListItem::new(Line::from(vec![
            Span::raw(indent),
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(name.to_string(), Style::new().fg(TEXT)),
        ]))
    });
    let list = List::new(items)
        .block(block)
        .highlight_symbol("› ")
        .highlight_style(Style::new().bg(Color::Rgb(42, 42, 42)).fg(BRIGHT).bold());
    frame.render_stateful_widget(list, area, &mut app.explorer_state);
}

fn draw_workbench(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.active_tab().is_none() {
        let text = Text::from(vec![
            Line::from("No document open").style(Style::new().fg(BRIGHT).bold()),
            Line::from(""),
            Line::from("Tab focuses Files · Enter opens a file · f searches by name")
                .style(Style::new().fg(MUTED)),
        ]);
        frame.render_widget(
            Paragraph::new(text).alignment(Alignment::Center),
            centered(area, 80),
        );
        return;
    }
    match app.view_mode {
        ViewMode::Split if area.width >= 42 => {
            let [editor, divider, preview] = Layout::horizontal([
                Constraint::Percentage(50),
                Constraint::Length(1),
                Constraint::Percentage(50),
            ])
            .areas(area);
            draw_editor(frame, app, editor, false);
            frame.render_widget(Block::new().style(Style::new().bg(BORDER)), divider);
            draw_preview(frame, app, preview);
        }
        ViewMode::Inline => draw_editor(frame, app, area, true),
        ViewMode::Split | ViewMode::Source => draw_editor(frame, app, area, false),
    }
}

fn draw_editor(frame: &mut Frame, app: &mut App, area: Rect, inline: bool) {
    let area = centered(area, 108);
    let mode = app.mode;
    let show_cursor = app.overlay.is_none() && app.focus == Focus::Editor;
    let Some(tab) = app.active_tab_mut() else {
        return;
    };
    if inline {
        apply_inline_preview(&mut tab.editor);
    }
    if mode == Mode::Command {
        tab.editor
            .set_cursor_line_style(Style::new().bg(Color::Rgb(10, 10, 10)));
    }
    frame.render_widget(&tab.editor, area);
    if show_cursor && let Some(position) = tab.editor.rendered_cursor_position() {
        frame.set_cursor_position(position);
    }
    tab.editor.clear_custom_highlight();
    tab.editor.set_cursor_line_style(Style::new());
}

fn draw_preview(frame: &mut Frame, app: &App, area: Rect) {
    let Some(tab) = app.active_tab() else {
        return;
    };
    let text = tui_markdown::from_str(&tab.document.text);
    let block = Block::new()
        .title(Line::from(" Preview ").style(Style::new().fg(TEXT).bold()))
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER));
    let preview = Paragraph::new(text)
        .block(block)
        .style(Style::new().fg(TEXT))
        .wrap(Wrap { trim: false })
        .scroll((app.preview_scroll, 0));
    frame.render_widget(preview, centered(area, 104));
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(
            format!(" {} ", app.mode.label()),
            Style::new().fg(BRIGHT).bold(),
        ),
        Span::styled(
            format!("{} ", app.view_mode.label()),
            Style::new().fg(MUTED),
        ),
    ];
    if let Some(tab) = app.active_tab() {
        let relative = app.workspace.relative(&tab.document.path);
        let dirty = if tab.document.conflict {
            " ⚠ conflict"
        } else if tab.document.is_dirty() {
            " ● modified"
        } else {
            ""
        };
        let (row, column) = tab.editor.cursor();
        spans.extend([
            Span::styled(
                format!("│ {}{dirty}", relative.display()),
                Style::new().fg(TEXT),
            ),
            Span::styled(
                format!(
                    " │ {} words │ {}:{}",
                    tab.document.word_count(),
                    row + 1,
                    column + 1
                ),
                Style::new().fg(MUTED),
            ),
        ]);
    }
    if let Some(message) = &app.status_message {
        spans.push(Span::styled(
            format!(" │ {message}"),
            Style::new().fg(BRIGHT),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(PANEL)),
        area,
    );
}

#[allow(clippy::too_many_lines)]
fn draw_overlay(frame: &mut Frame, app: &App, overlay: &Overlay) {
    let area = match overlay {
        Overlay::Help => popup(frame.area(), 78, 24),
        Overlay::Palette { .. } => popup(frame.area(), 76, 22),
        Overlay::FileFinder { .. } | Overlay::SearchResults { .. } | Overlay::Outline { .. } => {
            popup(frame.area(), 76, 20)
        }
        Overlay::Find { .. } | Overlay::WorkspaceSearch { .. } | Overlay::PathInput { .. } => {
            popup(frame.area(), 66, 7)
        }
        Overlay::Confirm(_) | Overlay::Message(_) => popup(frame.area(), 62, 7),
    };
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER))
        .style(Style::new().bg(BACKGROUND))
        .padding(Padding::horizontal(2));
    match overlay {
        Overlay::Help => draw_help(frame, area, block),
        Overlay::Palette { query, selected } => {
            let commands = command_candidates(query);
            let items = commands
                .iter()
                .map(|command| {
                    Line::from(vec![
                        Span::styled(
                            format!("{:<10}", command.group),
                            Style::new().fg(MUTED).bold(),
                        ),
                        Span::styled(command.label, Style::new().fg(TEXT)),
                        Span::styled(format!("  {}", command.shortcut), Style::new().fg(MUTED)),
                    ])
                })
                .collect();
            draw_picker(
                frame,
                area,
                block.title(" Commands "),
                query,
                items,
                *selected,
            );
        }
        Overlay::FileFinder { query, selected } => {
            let items = app
                .file_candidates(query)
                .into_iter()
                .take(100)
                .map(|index| {
                    let entry = &app.entries[index];
                    Line::from(entry.relative.display().to_string()).style(Style::new().fg(TEXT))
                })
                .collect();
            draw_picker(
                frame,
                area,
                block.title(" Find file "),
                query,
                items,
                *selected,
            );
        }
        Overlay::Find { query } => draw_input(
            frame,
            area,
            block.title(" Find in document "),
            query,
            "Enter finds next · Esc cancels",
        ),
        Overlay::WorkspaceSearch { query } => draw_input(
            frame,
            area,
            block.title(" Search workspace "),
            query,
            "Literal search · up to 100 results",
        ),
        Overlay::PathInput { action, value } => draw_input(
            frame,
            area,
            block.title(action.title()),
            value,
            "Workspace-relative .md, .markdown, or .txt path · Enter confirms",
        ),
        Overlay::SearchResults { results, selected } => {
            let items = results
                .iter()
                .map(|result| {
                    let relative = app.workspace.relative(&result.path);
                    Line::from(vec![
                        Span::styled(
                            format!("{}:{}  ", relative.display(), result.line + 1),
                            Style::new().fg(MUTED),
                        ),
                        Span::styled(result.preview.clone(), Style::new().fg(TEXT)),
                    ])
                })
                .collect();
            draw_picker(
                frame,
                area,
                block.title(" Search results "),
                "",
                items,
                *selected,
            );
        }
        Overlay::Outline { items, selected } => {
            let lines = items
                .iter()
                .map(|(_, level, title)| {
                    Line::from(format!("{}{}", "  ".repeat(level.saturating_sub(1)), title))
                        .style(Style::new().fg(TEXT))
                })
                .collect();
            draw_picker(
                frame,
                area,
                block.title(" Document outline "),
                "",
                lines,
                *selected,
            );
        }
        Overlay::Confirm(action) => {
            let message = match action {
                ConfirmAction::Quit => "Unsaved documents remain. Quit and discard them?",
                ConfirmAction::CloseTab => {
                    "This document has unsaved changes. Close and discard them?"
                }
            };
            let text = Text::from(vec![
                Line::from(message).style(Style::new().fg(TEXT)),
                Line::from(""),
                Line::from("y  Save     n  Discard     Esc  Cancel").style(Style::new().fg(MUTED)),
            ]);
            frame.render_widget(
                Paragraph::new(text).block(block.title(" Unsaved changes ")),
                area,
            );
        }
        Overlay::Message(message) => frame.render_widget(
            Paragraph::new(message.as_str()).block(block.title(" TermDraft ")),
            area,
        ),
    }
}

fn draw_help(frame: &mut Frame, area: Rect, block: Block<'_>) {
    let lines = vec![
        help_line("MODE", "i", "Enter WRITE mode"),
        help_line("MODE", "Esc", "Return to COMMAND mode"),
        help_line("DOCUMENT", "w / Ctrl+S", "Save safely"),
        help_line("DOCUMENT", "W", "Save as a new path"),
        help_line("DOCUMENT", "D", "Duplicate current source"),
        help_line("DOCUMENT", "q / Ctrl+Q", "Quit safely"),
        help_line("NAVIGATE", "h j k l", "Move cursor"),
        help_line("NAVIGATE", "[ / ]", "Previous / next tab"),
        help_line("NAVIGATE", "f / Ctrl+P", "Find a file"),
        help_line("NAVIGATE", "/", "Search workspace text"),
        help_line("NAVIGATE", "s / Ctrl+F", "Find in document"),
        help_line("NAVIGATE", "S", "Document outline"),
        help_line("VIEW", "e / Ctrl+B", "Show or hide Files"),
        help_line("FILES", "a", "Create a Markdown file"),
        help_line("VIEW", "v / Ctrl+E", "Inline / split / source"),
        help_line("EDIT", "u / U", "Undo / redo"),
        help_line("MENU", ":", "Open grouped command menu"),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(block.title(" Shortcuts "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn help_line(group: &'static str, key: &'static str, label: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{group:<10}"), Style::new().fg(MUTED).bold()),
        Span::styled(format!("{key:<16}"), Style::new().fg(BRIGHT).bold()),
        Span::styled(label, Style::new().fg(TEXT)),
    ])
}

fn draw_picker(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    query: &str,
    items: Vec<Line<'_>>,
    selected: usize,
) {
    let [input, results, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(block.inner(area));
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(format!("> {query}")).style(Style::new().fg(BRIGHT)),
        input,
    );
    let list = List::new(items.into_iter().map(ListItem::new))
        .highlight_symbol("› ")
        .highlight_style(Style::new().bg(Color::Rgb(44, 44, 44)).fg(BRIGHT).bold());
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, results, &mut state);
    frame.render_widget(
        Paragraph::new("↑↓ select · Enter open · Esc close").style(Style::new().fg(MUTED)),
        footer,
    );
}

fn draw_input(frame: &mut Frame, area: Rect, block: Block<'_>, query: &str, footer: &str) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input, hint] =
        Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(format!("> {query}█")).style(Style::new().fg(BRIGHT)),
        input,
    );
    frame.render_widget(
        Paragraph::new(footer.to_owned()).style(Style::new().fg(MUTED)),
        hint,
    );
}

fn centered(area: Rect, max_width: u16) -> Rect {
    let width = area.width.min(max_width);
    let [center] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    center
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    let [vertical] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [center] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(vertical);
    center
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::workspace::Workspace;

    #[test]
    fn renders_the_preserved_application_shell() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# Note\nbody").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("TermDraft"));
        assert!(rendered.contains("RUST PORT"));
        assert!(rendered.contains("Files"));
        assert!(rendered.contains("COMMAND"));
        assert!(rendered.contains("note.md"));
        assert!(
            app.active_tab()
                .unwrap()
                .editor
                .rendered_cursor_position()
                .is_some()
        );
    }
}
