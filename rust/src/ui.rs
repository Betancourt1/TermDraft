//! Ratatui rendering for the preserved `TermDraft` layout and overlays.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};

use crate::app::{
    App, ConfirmAction, Focus, Mode, Overlay, TextInput, UiRegions, ViewMode, command_candidates,
};
use crate::editor::apply_inline_preview;

const BACKGROUND: Color = Color::Black;
const SURFACE: Color = Color::Rgb(16, 16, 16);
const PANEL: Color = Color::Rgb(28, 28, 28);
const BORDER: Color = Color::Rgb(58, 58, 58);
const TEXT: Color = Color::Rgb(218, 218, 218);
const MUTED: Color = Color::Rgb(118, 118, 118);
const BRIGHT: Color = Color::Rgb(242, 242, 242);

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.update_viewport_width(frame.area().width);
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
    let regions = workspace_regions(app, area);
    app.ui_regions = regions;
    if let Some(explorer) = regions.explorer {
        draw_explorer(frame, app, explorer);
    }
    if let Some(divider) = regions.explorer_divider {
        frame.render_widget(Block::new().style(Style::new().bg(BORDER)), divider);
    }
    draw_workbench(frame, app, regions);
}

#[must_use]
pub fn workspace_regions(app: &App, area: Rect) -> UiRegions {
    let mut regions = UiRegions {
        workspace: area,
        workbench: area,
        ..UiRegions::default()
    };
    if app.show_explorer {
        let maximum = area.width.saturating_sub(20).min(48);
        let minimum = 20.min(maximum);
        let explorer_width = app.explorer_width.clamp(minimum, maximum);
        let [explorer, divider, workbench] = Layout::horizontal([
            Constraint::Length(explorer_width),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .areas(area);
        regions.explorer = Some(explorer);
        regions.explorer_list = Some(Rect {
            y: explorer.y.saturating_add(1),
            height: explorer.height.saturating_sub(1),
            ..explorer
        });
        regions.explorer_divider = Some(divider);
        regions.workbench = workbench;
    }
    if app.active_tab().is_none() {
        return regions;
    }
    match (app.editor_is_visible(), app.preview_is_visible()) {
        (true, true) => {
            let available = regions.workbench.width.saturating_sub(1);
            let minimum = 20.min(available / 2);
            let requested =
                u16::try_from(u32::from(available) * u32::from(app.split_percent) / 100)
                    .unwrap_or(available / 2);
            let editor_width = requested.clamp(minimum, available.saturating_sub(minimum));
            let [editor, divider, preview] = Layout::horizontal([
                Constraint::Length(editor_width),
                Constraint::Length(1),
                Constraint::Min(1),
            ])
            .areas(regions.workbench);
            regions.editor = Some(editor);
            regions.workbench_divider = Some(divider);
            regions.preview = Some(preview);
        }
        (true, false) => regions.editor = Some(regions.workbench),
        (false, true) => regions.preview = Some(regions.workbench),
        (false, false) => {}
    }
    regions
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

fn draw_workbench(frame: &mut Frame, app: &mut App, regions: UiRegions) {
    if app.active_tab().is_none() {
        let text = Text::from(vec![
            Line::from("No document open").style(Style::new().fg(BRIGHT).bold()),
            Line::from(""),
            Line::from("Tab focuses Files · Enter opens a file · f searches by name")
                .style(Style::new().fg(MUTED)),
        ]);
        frame.render_widget(
            Paragraph::new(text).alignment(Alignment::Center),
            centered(regions.workbench, 80),
        );
        return;
    }
    if let Some(editor) = regions.editor {
        let inline = app.view_mode == ViewMode::Inline && regions.preview.is_none();
        draw_editor(frame, app, editor, inline);
    }
    if let Some(divider) = regions.workbench_divider {
        frame.render_widget(Block::new().style(Style::new().bg(BORDER)), divider);
    }
    if let Some(preview) = regions.preview {
        draw_preview(frame, app, preview);
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

fn draw_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(source) = app.active_tab().map(|tab| tab.document.text.clone()) else {
        return;
    };
    let text = tui_markdown::from_str(&source);
    let title_style = if app.focus == Focus::Preview {
        Style::new().fg(BRIGHT).bold()
    } else {
        Style::new().fg(TEXT).bold()
    };
    let block = Block::new()
        .title(Line::from(" Preview ").style(title_style))
        .borders(Borders::TOP)
        .border_style(Style::new().fg(if app.focus == Focus::Preview {
            MUTED
        } else {
            BORDER
        }))
        .padding(Padding::horizontal(2));
    let area = centered(area, 104);
    let inner = block.inner(area);
    let content_width = usize::from(inner.width.max(1));
    let line_count = text
        .lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(content_width))
        .sum::<usize>();
    let preview = Paragraph::new(text)
        .block(block)
        .style(Style::new().fg(TEXT))
        .wrap(Wrap { trim: false });
    app.preview_page = inner.height.max(1);
    app.preview_max_scroll =
        u16::try_from(line_count.saturating_sub(usize::from(inner.height))).unwrap_or(u16::MAX);
    app.preview_scroll = app.preview_scroll.min(app.preview_max_scroll);
    let preview = preview.scroll((app.preview_scroll, 0));
    frame.render_widget(preview, area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let focus = match app.focus {
        Focus::Explorer => " · FILES",
        Focus::Editor => "",
        Focus::Preview => " · PREVIEW",
    };
    let mut spans = vec![Span::styled(
        format!(" {}{focus} ", app.mode.label()),
        Style::new().fg(BRIGHT).bold(),
    )];
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
        let location = if app.focus == Focus::Preview {
            let percentage = if app.preview_max_scroll == 0 {
                100
            } else {
                u32::from(app.preview_scroll) * 100 / u32::from(app.preview_max_scroll)
            };
            format!("Preview {percentage}%")
        } else {
            format!("{}:{}", row + 1, column + 1)
        };
        spans.extend([
            Span::styled(
                format!("│ {}{dirty}", relative.display()),
                Style::new().fg(TEXT),
            ),
            Span::styled(
                format!(" │ {} words │ {location}", tab.document.word_count()),
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
        Overlay::FileFinder { .. }
        | Overlay::RecentDocuments { .. }
        | Overlay::SearchResults { .. }
        | Overlay::Outline { .. } => popup(frame.area(), 76, 20),
        Overlay::Find { .. } | Overlay::WorkspaceSearch { .. } | Overlay::PathInput { .. } => {
            popup(frame.area(), 66, 7)
        }
        Overlay::Recovery { .. } => popup(frame.area(), 70, 9),
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
        Overlay::Palette { input, selected } => {
            let commands = command_candidates(&input.value);
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
                Some(input),
                items,
                *selected,
            );
        }
        Overlay::FileFinder { input, selected } => {
            let items = app
                .file_candidates(&input.value)
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
                Some(input),
                items,
                *selected,
            );
        }
        Overlay::RecentDocuments { paths, selected } => {
            let active = app.active_tab().map(|tab| tab.document.path.as_path());
            let items = paths
                .iter()
                .map(|path| {
                    let mut line = Line::from(app.workspace.relative(path).display().to_string())
                        .style(Style::new().fg(TEXT));
                    if active == Some(path.as_path()) {
                        line.push_span(Span::styled("  · current", Style::new().fg(MUTED)));
                    }
                    line
                })
                .collect();
            draw_picker(
                frame,
                area,
                block.title(" Recent documents "),
                None,
                items,
                *selected,
            );
        }
        Overlay::Find { input } => draw_input(
            frame,
            area,
            block.title(" Find in document "),
            input,
            "Enter finds next · Esc cancels",
        ),
        Overlay::WorkspaceSearch { input } => draw_input(
            frame,
            area,
            block.title(" Search workspace "),
            input,
            "Literal search · up to 100 results",
        ),
        Overlay::PathInput { action, input } => draw_input(
            frame,
            area,
            block.title(action.title()),
            input,
            "Workspace-relative .md, .markdown, or .txt path · Enter confirms",
        ),
        Overlay::Recovery { entry } => {
            let relative = app.workspace.relative(&entry.document_path);
            let conflict = app
                .tabs
                .iter()
                .find(|tab| tab.document.path == entry.document_path)
                .is_none_or(|tab| !entry.baseline_matches(&tab.document.snapshot));
            let detail = if conflict {
                "Disk changed since capture; restoring keeps the draft as a conflict."
            } else {
                "The saved disk baseline still matches this unsaved draft."
            };
            let text = Text::from(vec![
                Line::from(format!("Unsaved source found for {}", relative.display()))
                    .style(Style::new().fg(TEXT)),
                Line::from(detail).style(Style::new().fg(MUTED)),
                Line::from(""),
                Line::from("r  Restore     d  Use disk     Esc  Later")
                    .style(Style::new().fg(BRIGHT)),
            ]);
            frame.render_widget(
                Paragraph::new(text).block(block.title(" Crash recovery ")),
                area,
            );
        }
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
                None,
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
                None,
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
        help_line("NAVIGATE", "o / Ctrl+O", "Recent documents"),
        help_line("NAVIGATE", "/", "Search workspace text"),
        help_line("NAVIGATE", "s / Ctrl+F", "Find in document"),
        help_line("NAVIGATE", "S", "Document outline"),
        help_line("VIEW", "e / Ctrl+B", "Show or hide Files"),
        help_line("FILES", "a", "Create a Markdown file"),
        help_line("VIEW", "v / Ctrl+E", "Show / hide preview"),
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
    input: Option<&TextInput>,
    items: Vec<Line<'_>>,
    selected: usize,
) {
    let [input_area, results, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(block.inner(area));
    frame.render_widget(block, area);
    if let Some(value) = input {
        frame.render_widget(Paragraph::new(input_line(value)), input_area);
    }
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

fn draw_input(frame: &mut Frame, area: Rect, block: Block<'_>, input: &TextInput, footer: &str) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input_area, hint] =
        Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(input_line(input)), input_area);
    frame.render_widget(
        Paragraph::new(footer.to_owned()).style(Style::new().fg(MUTED)),
        hint,
    );
}

fn input_line(input: &TextInput) -> Line<'_> {
    let byte = input.byte_cursor();
    Line::from(vec![
        Span::styled("> ", Style::new().fg(MUTED)),
        Span::styled(&input.value[..byte], Style::new().fg(BRIGHT)),
        Span::styled("█", Style::new().fg(BRIGHT)),
        Span::styled(&input.value[byte..], Style::new().fg(TEXT)),
    ])
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
    use crate::config::{Config, StartupView};
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

    #[test]
    fn split_preview_uses_the_preserved_shell_and_focus_status() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "# Note\nbody").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut config = Config::default();
        config.editor.view_mode = StartupView::Split;
        let mut app = App::with_config(workspace, config).unwrap();
        app.focus = Focus::Preview;
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();

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
        assert!(rendered.contains("Preview"));
        assert!(rendered.contains("COMMAND · PREVIEW"));
        assert!(rendered.contains("Preview 100%"));
    }
}
