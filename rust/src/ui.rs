//! Ratatui rendering for the preserved `TermDraft` layout and overlays.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};

use crate::app::{
    App, ConfirmAction, ConflictKind, FileFinderFocus, FindFocus, Focus, MixedLineEndingContext,
    Mode, Overlay, TextInput, UiRegions, ViewMode, WorkspaceSearchFocus, command_candidates,
    text_search_mode_label,
};
use crate::coordinate_diagnostic::CoordinateDiagnostic;
use crate::document::LineEnding;
use crate::editor::apply_inline_preview;
use crate::markdown_help::MARKDOWN_SYNTAX_HELP;
use crate::search::{TextMatch, TextSearchMode};
use crate::semantic_blocks::{ReaderPresentation, SemanticBlockMap};

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
        let conflict = if tab.document.conflict { " !" } else { "" };
        let style = if Some(index) == app.active_tab {
            Style::new().fg(BRIGHT).bold()
        } else {
            Style::new().fg(MUTED)
        };
        spans.push(Span::styled(format!(" {name}{dirty}{conflict} "), style));
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
                format!(
                    " │ {} words{} │ {location}",
                    tab.document.word_count(),
                    mixed_line_ending_status(
                        tab.document.line_ending,
                        tab.document.mixed_line_ending_target()
                    )
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
        Overlay::Help => popup(frame.area(), 78, 29),
        Overlay::MarkdownHelp { .. } => popup(frame.area(), 76, 30),
        Overlay::SemanticInspector { .. } => popup(frame.area(), 82, 34),
        Overlay::SemanticReader { .. } => popup(frame.area(), 82, 36),
        Overlay::CoordinateInspector { .. } => popup(frame.area(), 76, 16),
        Overlay::Palette { .. } => popup(frame.area(), 76, 22),
        Overlay::FileFinder { .. } => popup(frame.area(), 76, 23),
        Overlay::RecentDocuments { .. }
        | Overlay::SearchResults { .. }
        | Overlay::Outline { .. } => popup(frame.area(), 76, 20),
        Overlay::Find { .. } => popup(frame.area(), 70, 13),
        Overlay::WorkspaceSearch { .. } => popup(frame.area(), 80, 24),
        Overlay::PathInput { .. } | Overlay::WorkspaceInput { .. } => popup(frame.area(), 66, 7),
        Overlay::Recovery { .. } | Overlay::MixedLineEndings { .. } => popup(frame.area(), 70, 9),
        Overlay::Conflict { .. } => popup(frame.area(), 74, 10),
        Overlay::TrashConfirm { .. } => popup(frame.area(), 70, 8),
        Overlay::Confirm(_) | Overlay::Message(_) => popup(frame.area(), 62, 7),
    };
    frame.render_widget(Clear, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER))
        .style(Style::new().bg(BACKGROUND))
        .padding(Padding::horizontal(2));
    match overlay {
        Overlay::Help => draw_help(frame, app, area, block),
        Overlay::MarkdownHelp { scroll } => {
            draw_markdown_help(frame, area, block, *scroll);
        }
        Overlay::SemanticInspector { mapping, selected } => {
            draw_semantic_inspector(frame, area, block, mapping, *selected);
        }
        Overlay::SemanticReader { mapping, scroll } => {
            draw_semantic_reader(frame, area, block, mapping, *scroll);
        }
        Overlay::CoordinateInspector {
            diagnostic,
            screen_position,
        } => {
            draw_coordinate_inspector(frame, area, block, diagnostic, *screen_position);
        }
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
                        Span::styled(
                            format!("  {}", command_shortcut(app, command.action)),
                            Style::new().fg(MUTED),
                        ),
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
        Overlay::FileFinder {
            query,
            filter,
            focus,
            selected,
            error,
        } => draw_file_finder(
            frame,
            app,
            area,
            block,
            query,
            filter,
            *focus,
            *selected,
            error.as_deref(),
        ),
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
        Overlay::Find {
            query,
            replacement,
            case_sensitive,
            focus,
            matches,
            selected,
            read_only,
            ..
        } => draw_document_find(
            frame,
            area,
            block,
            query,
            replacement,
            *case_sensitive,
            *focus,
            matches.len(),
            *selected,
            *read_only,
        ),
        Overlay::WorkspaceSearch {
            query,
            filter,
            mode,
            case_sensitive,
            focus,
            results,
            selected,
            status,
        } => draw_workspace_search(
            frame,
            app,
            area,
            block,
            query,
            filter,
            *mode,
            *case_sensitive,
            *focus,
            results,
            *selected,
            status,
        ),
        Overlay::PathInput { action, input } => draw_input(
            frame,
            area,
            block.title(action.title()),
            input,
            "Workspace-relative .md, .markdown, or .txt path · Enter confirms",
        ),
        Overlay::WorkspaceInput {
            action,
            source,
            input,
        } => {
            let relative = app.workspace.relative(source);
            let footer = match action {
                crate::app::WorkspaceInputAction::Create => {
                    format!(
                        "Location: {} · trailing / creates a folder",
                        relative.display()
                    )
                }
                crate::app::WorkspaceInputAction::Rename => {
                    "Enter one new basename · no replacement".to_owned()
                }
                crate::app::WorkspaceInputAction::Move => {
                    "Enter a workspace-relative destination · no replacement".to_owned()
                }
            };
            draw_input(frame, area, block.title(action.title()), input, &footer);
        }
        Overlay::TrashConfirm {
            source,
            is_directory,
        } => {
            let relative = app.workspace.relative(source);
            let detail = if *is_directory {
                "Everything inside it will move too, including hidden files."
            } else {
                "The file can be recovered from the operating system Trash."
            };
            let text = Text::from(vec![
                Line::from(format!("Move {} to Trash?", relative.display()))
                    .style(Style::new().fg(TEXT)),
                Line::from(detail).style(Style::new().fg(MUTED)),
                Line::from(""),
                Line::from("y  Move to Trash     Esc  Cancel").style(Style::new().fg(BRIGHT)),
            ]);
            frame.render_widget(
                Paragraph::new(text).block(block.title(" Move to Trash ")),
                area,
            );
        }
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
        Overlay::MixedLineEndings {
            context, target, ..
        } => {
            let cancel = if *context == MixedLineEndingContext::Open {
                "Cancel opening"
            } else {
                "Keep read-only"
            };
            let detail = match context {
                MixedLineEndingContext::Open => {
                    "The file contains more than one newline style. Disk stays unchanged until edit and save."
                }
                MixedLineEndingContext::Reload => {
                    "The external version contains mixed newlines. Accept before editing the reloaded source."
                }
                MixedLineEndingContext::Recovery => {
                    "The recovered source contains mixed newlines. Accept before editing the draft."
                }
            };
            let text = Text::from(vec![
                Line::from(detail).style(Style::new().fg(TEXT)),
                Line::from(format!(
                    "The first edit will normalize newlines to {}.",
                    line_ending_label(*target)
                ))
                .style(Style::new().fg(MUTED)),
                Line::from(""),
                Line::from(format!("Enter/e  Edit and normalize     Esc  {cancel}"))
                    .style(Style::new().fg(BRIGHT)),
            ]);
            frame.render_widget(
                Paragraph::new(text).block(block.title(" Mixed line endings ")),
                area,
            );
        }
        Overlay::Conflict {
            kind,
            can_reload,
            allow_continue,
        } => {
            let detail = match kind {
                ConflictKind::Changed => {
                    "This file changed outside TermDraft. The local draft was not written."
                }
                ConflictKind::Missing => {
                    "This file no longer exists. The original path will not be recreated."
                }
                ConflictKind::Unavailable => {
                    "This file cannot be read or verified. The original path will not be changed."
                }
            };
            let mut actions = "s  Save local as…".to_owned();
            if *can_reload {
                actions.push_str("     r  Reload external");
            }
            if *allow_continue {
                actions.push_str("     n  Continue without copy");
            }
            actions.push_str("     Esc  Cancel");
            let text = Text::from(vec![
                Line::from(detail).style(Style::new().fg(TEXT)),
                Line::from("Local source remains available in this tab.")
                    .style(Style::new().fg(MUTED)),
                Line::from(""),
                Line::from(actions).style(Style::new().fg(BRIGHT)),
            ]);
            frame.render_widget(
                Paragraph::new(text).block(block.title(" External conflict ")),
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

fn draw_markdown_help(frame: &mut Frame, area: Rect, block: Block<'_>, scroll: u16) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Markdown syntax "), area);
    let [content, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(
        Paragraph::new(MARKDOWN_SYNTAX_HELP)
            .style(Style::new().fg(TEXT))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content,
    );
    frame.render_widget(
        Paragraph::new("↑↓ / PgUp/PgDn scroll · F1 / Enter / Esc close")
            .style(Style::new().fg(MUTED)),
        footer,
    );
}

fn draw_semantic_inspector(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    mapping: &SemanticBlockMap,
    selected: usize,
) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Semantic source blocks "), area);
    let [intro, list_area, detail_area, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(6),
        Constraint::Length(10),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new("Parser ranges are read-only and use zero-based, end-exclusive internals.")
            .style(Style::new().fg(MUTED))
            .wrap(Wrap { trim: false }),
        intro,
    );

    let segments = mapping.segments();
    let items = if segments.is_empty() {
        vec![ListItem::new("No semantic blocks in this document")]
    } else {
        segments
            .iter()
            .map(|segment| {
                let detail = segment
                    .detail
                    .as_deref()
                    .map_or_else(String::new, |detail| format!(" · {detail}"));
                ListItem::new(format!(
                    "{}{detail} · lines {}-{} · chars {}-{}",
                    segment.kind.label(),
                    segment.start_line + 1,
                    segment.end_line.max(segment.start_line + 1),
                    segment.start_character,
                    segment.end_character,
                ))
            })
            .collect()
    };
    let selection = (!segments.is_empty()).then(|| selected.min(segments.len() - 1));
    let mut state = ratatui::widgets::ListState::default().with_selected(selection);
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .highlight_style(Style::new().bg(Color::Rgb(44, 44, 44)).fg(BRIGHT).bold()),
        list_area,
        &mut state,
    );

    let detail = segments.get(selected).map_or_else(
        || "Nothing mapped. Empty documents keep an empty source map.".to_owned(),
        |segment| {
            let mut characters = segment.source.chars();
            let mut preview = characters.by_ref().take(1_200).collect::<String>();
            if characters.next().is_some() {
                preview.push_str("\n… preview truncated");
            }
            format!(
                "{} · [{}, {}) lines · [{}, {}) characters\n\n{preview}",
                segment.kind.label(),
                segment.start_line,
                segment.end_line,
                segment.start_character,
                segment.end_character,
            )
        },
    );
    frame.render_widget(
        Paragraph::new(detail)
            .style(Style::new().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: false }),
        detail_area,
    );
    frame.render_widget(
        Paragraph::new("↑↓ / PgUp/PgDn select · Enter jump to source · Esc close")
            .style(Style::new().fg(MUTED)),
        footer,
    );
}

fn draw_semantic_reader(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    mapping: &SemanticBlockMap,
    scroll: u16,
) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Experimental semantic reading "), area);
    let [intro, content, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(
            "Headings and paragraphs render independently. Every other construct stays visible as exact Markdown source.",
        )
        .style(Style::new().fg(MUTED))
        .wrap(Wrap { trim: false }),
        intro,
    );

    let mut lines = Vec::new();
    for (segment, presentation) in mapping.reader_segments() {
        let presentation_label = match presentation {
            ReaderPresentation::Rendered => "rendered",
            ReaderPresentation::SourceFallback => "source fallback",
        };
        lines.push(
            Line::from(format!(
                "{} · lines {}-{} · {presentation_label}",
                segment.kind.label(),
                segment.start_line + 1,
                segment.end_line.max(segment.start_line + 1),
            ))
            .style(Style::new().fg(MUTED).bold()),
        );
        match presentation {
            ReaderPresentation::Rendered => {
                lines.extend(tui_markdown::from_str(&segment.source).lines);
            }
            ReaderPresentation::SourceFallback => {
                lines.extend(segment.source.split('\n').map(|source| {
                    Line::from(source.to_owned()).style(Style::new().fg(TEXT).bg(PANEL))
                }));
            }
        }
        lines.push(Line::from(""));
    }
    if lines.is_empty() {
        lines.push(
            Line::from("This document has no visible source blocks.").style(Style::new().fg(MUTED)),
        );
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::new().fg(TEXT))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content,
    );
    frame.render_widget(
        Paragraph::new("↑↓ / PgUp/PgDn scroll · Enter / Esc return to source")
            .style(Style::new().fg(MUTED)),
        footer,
    );
}

fn draw_coordinate_inspector(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    diagnostic: &CoordinateDiagnostic,
    screen_position: Option<(u16, u16)>,
) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Cursor coordinate diagnostic "), area);
    let [content, footer] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let terminal = screen_position.map_or_else(
        || "Terminal screen: unavailable".to_owned(),
        |(row, cell)| format!("Terminal screen: row {row}, cell {cell}"),
    );
    let wrap_warning = if diagnostic.wrap_splits_grapheme {
        "yes — unsafe for block editing"
    } else {
        "no"
    };
    let text = Text::from(vec![
        Line::from(format!(
            "Source character offset: {}",
            diagnostic.source_offset
        )),
        Line::from(format!("UTF-8 byte offset: {}", diagnostic.utf8_byte_offset)),
        Line::from(format!(
            "Logical location: line {}, column {}",
            diagnostic.logical_line, diagnostic.logical_column
        )),
        Line::from(format!(
            "Wrapped location: row {}, cell {}",
            diagnostic.visual_row, diagnostic.visual_cell
        )),
        Line::from(terminal),
        Line::from(format!(
            "At grapheme boundary: {}",
            if diagnostic.grapheme_boundary {
                "yes"
            } else {
                "no"
            }
        )),
        Line::from(format!("Wrap splits a grapheme: {wrap_warning}")),
        Line::from(""),
        Line::from(
            "Coordinates are a read-only snapshot. Terminal width rules, IME input, and bidirectional text remain outside this diagnostic.",
        )
        .style(Style::new().fg(MUTED)),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::new().fg(TEXT))
            .wrap(Wrap { trim: false }),
        content,
    );
    frame.render_widget(
        Paragraph::new("Enter / Esc close").style(Style::new().fg(MUTED)),
        footer,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_file_finder(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    block: Block<'_>,
    query: &TextInput,
    filter: &TextInput,
    focus: FileFinderFocus,
    selected: usize,
    stored_error: Option<&str>,
) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Find text file "), area);
    let [
        query_area,
        filter_area,
        status_area,
        results_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(labeled_input_line(
            "Path",
            query,
            focus == FileFinderFocus::Query,
            false,
        )),
        query_area,
    );
    frame.render_widget(
        Paragraph::new(labeled_input_line(
            "Filter",
            filter,
            focus == FileFinderFocus::Filter,
            false,
        )),
        filter_area,
    );

    let (candidates, current_error) =
        match app.filtered_file_candidates(&query.value, &filter.value) {
            Ok(candidates) => (candidates, None),
            Err(error) => (Vec::new(), Some(error)),
        };
    let error = current_error.as_deref().or(stored_error);
    let (status, items, selection) = if let Some(error) = error {
        (
            format!("Invalid file filter: {error}"),
            vec![ListItem::new("Invalid file filter")],
            None,
        )
    } else {
        let count = candidates.len();
        let noun = if count == 1 { "file" } else { "files" };
        let mut status = format!("{count} {noun} · fuzzy path matching");
        if !filter.value.trim().is_empty() {
            status.push_str(" · ");
            status.push_str(filter.value.trim());
        }
        let items = if candidates.is_empty() {
            vec![ListItem::new("No matching text files")]
        } else {
            candidates
                .iter()
                .map(|index| ListItem::new(app.entries[*index].relative.display().to_string()))
                .collect()
        };
        let selection = (!candidates.is_empty()).then_some(selected.min(count.saturating_sub(1)));
        (status, items, selection)
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::new().fg(MUTED)),
        status_area,
    );
    let list = List::new(items)
        .highlight_symbol("› ")
        .highlight_style(Style::new().bg(Color::Rgb(44, 44, 44)).fg(BRIGHT).bold());
    let mut state = ratatui::widgets::ListState::default().with_selected(selection);
    frame.render_stateful_widget(list, results_area, &mut state);
    frame.render_widget(
        Paragraph::new("Tab fields · ↓ results · Enter open · Esc close")
            .style(Style::new().fg(MUTED)),
        footer_area,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_document_find(
    frame: &mut Frame,
    area: Rect,
    block: Block<'_>,
    query: &TextInput,
    replacement: &TextInput,
    case_sensitive: bool,
    focus: FindFocus,
    match_count: usize,
    selected: Option<usize>,
    read_only: bool,
) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Find and replace "), area);
    let [
        query_area,
        replace_area,
        case_area,
        status_area,
        actions_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(labeled_input_line(
            "Find",
            query,
            focus == FindFocus::Query,
            false,
        )),
        query_area,
    );
    frame.render_widget(
        Paragraph::new(labeled_input_line(
            "Replace",
            replacement,
            focus == FindFocus::Replacement,
            read_only,
        )),
        replace_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![control_span(
            &format!("[{}] Match case", if case_sensitive { "x" } else { " " }),
            focus == FindFocus::Case,
            true,
        )])),
        case_area,
    );
    let status = if query.value.is_empty() {
        "Enter text to find".to_owned()
    } else if let Some(selected) = selected {
        format!("{} of {match_count}", selected + 1)
    } else {
        "No matches".to_owned()
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::new().fg(MUTED)),
        status_area,
    );
    let has_match = selected.is_some();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            control_span(" Previous ", focus == FindFocus::Previous, has_match),
            Span::raw(" "),
            control_span(" Next ", focus == FindFocus::Next, has_match),
            Span::raw(" "),
            control_span(
                " Replace ",
                focus == FindFocus::Replace,
                has_match && !read_only,
            ),
            Span::raw(" "),
            control_span(
                " Replace all ",
                focus == FindFocus::ReplaceAll,
                has_match && !read_only,
            ),
        ])),
        actions_area,
    );
    frame.render_widget(
        Paragraph::new("Tab controls · F3 / Shift+F3 navigate · Esc close")
            .style(Style::new().fg(MUTED)),
        footer_area,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_workspace_search(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    block: Block<'_>,
    query: &TextInput,
    filter: &TextInput,
    mode: TextSearchMode,
    case_sensitive: bool,
    focus: WorkspaceSearchFocus,
    results: &[TextMatch],
    selected: usize,
    status: &str,
) {
    let inner = block.inner(area);
    frame.render_widget(block.title(" Search workspace text "), area);
    let [
        query_area,
        options_area,
        filter_area,
        status_area,
        results_area,
        footer_area,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(labeled_input_line(
            "Query",
            query,
            focus == WorkspaceSearchFocus::Query,
            false,
        )),
        query_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Mode ", Style::new().fg(MUTED)),
            control_span(
                text_search_mode_label(mode),
                focus == WorkspaceSearchFocus::Mode,
                true,
            ),
            Span::raw("   "),
            control_span(
                &format!("[{}] Match case", if case_sensitive { "x" } else { " " }),
                focus == WorkspaceSearchFocus::Case,
                true,
            ),
        ])),
        options_area,
    );
    frame.render_widget(
        Paragraph::new(labeled_input_line(
            "Filter",
            filter,
            focus == WorkspaceSearchFocus::Filter,
            false,
        )),
        filter_area,
    );
    frame.render_widget(
        Paragraph::new(status.to_owned()).style(Style::new().fg(MUTED)),
        status_area,
    );
    let items = if results.is_empty() {
        let placeholder = if status == "Enter a query to search Markdown source." {
            "No search yet"
        } else if status == "Searching…" {
            "Searching…"
        } else if status.starts_with("Search failed:") {
            "Search failed"
        } else {
            "No matching source lines"
        };
        vec![ListItem::new(placeholder)]
    } else {
        results
            .iter()
            .map(|result| {
                ListItem::new(format!(
                    "{}:{}:{}  {}",
                    app.workspace.relative(&result.path).display(),
                    result.line + 1,
                    result.column + 1,
                    result.preview
                ))
            })
            .collect()
    };
    let list = List::new(items)
        .highlight_symbol("› ")
        .highlight_style(Style::new().bg(Color::Rgb(44, 44, 44)).fg(BRIGHT).bold());
    let selection = (!results.is_empty()).then_some(selected.min(results.len().saturating_sub(1)));
    let mut state = ratatui::widgets::ListState::default().with_selected(selection);
    frame.render_stateful_widget(list, results_area, &mut state);
    frame.render_widget(
        Paragraph::new("Enter searches/opens · Tab fields · ←→ mode · Esc close")
            .style(Style::new().fg(MUTED)),
        footer_area,
    );
}

fn labeled_input_line<'a>(
    label: &'a str,
    input: &'a TextInput,
    focused: bool,
    disabled: bool,
) -> Line<'a> {
    let byte = input.byte_cursor();
    let label_style = if focused {
        Style::new().fg(BRIGHT).bold()
    } else {
        Style::new().fg(MUTED)
    };
    let text_style = if disabled {
        Style::new().fg(MUTED)
    } else {
        Style::new().fg(TEXT)
    };
    let cursor = if focused && !disabled { "█" } else { " " };
    Line::from(vec![
        Span::styled(format!("{label:<8}"), label_style),
        Span::styled(&input.value[..byte], text_style),
        Span::styled(cursor, Style::new().fg(BRIGHT)),
        Span::styled(&input.value[byte..], text_style),
    ])
}

fn control_span(label: &str, focused: bool, enabled: bool) -> Span<'static> {
    let style = if !enabled {
        Style::new().fg(MUTED)
    } else if focused {
        Style::new().fg(BRIGHT).bg(Color::Rgb(44, 44, 44)).bold()
    } else {
        Style::new().fg(TEXT)
    };
    Span::styled(label.to_owned(), style)
}

#[allow(clippy::too_many_lines)]
fn draw_help(frame: &mut Frame, app: &App, area: Rect, block: Block<'_>) {
    let lines = vec![
        help_line(
            "MODE",
            shortcut(app, &["command_write_mode"]),
            "Enter WRITE mode",
        ),
        help_line("MODE", "Esc", "Return to COMMAND mode"),
        help_line(
            "DOCUMENT",
            shortcut(app, &["command_save", "save"]),
            "Save safely",
        ),
        help_line(
            "DOCUMENT",
            shortcut(app, &["command_save_as", "save_as"]),
            "Save as a new path",
        ),
        help_line(
            "DOCUMENT",
            shortcut(app, &["command_duplicate_document"]),
            "Duplicate current source",
        ),
        help_line(
            "DOCUMENT",
            shortcut(app, &["command_quit", "quit"]),
            "Quit safely",
        ),
        help_line("NAVIGATE", "h j k l", "Move cursor"),
        help_line(
            "NAVIGATE",
            shortcut(app, &["command_previous_tab", "command_next_tab"]),
            "Previous / next tab",
        ),
        help_line(
            "NAVIGATE",
            shortcut(app, &["command_find_file", "find_file"]),
            "Find a file",
        ),
        help_line(
            "NAVIGATE",
            shortcut(app, &["command_recent_documents", "recent_documents"]),
            "Recent documents",
        ),
        help_line(
            "NAVIGATE",
            shortcut(app, &["command_search_text", "search_text"]),
            "Search workspace text",
        ),
        help_line(
            "NAVIGATE",
            shortcut(app, &["command_find_replace", "find_replace"]),
            "Find in document",
        ),
        help_line(
            "NAVIGATE",
            shortcut(app, &["command_document_outline", "document_outline"]),
            "Document outline",
        ),
        help_line(
            "VIEW",
            shortcut(app, &["command_toggle_explorer", "toggle_explorer"]),
            "Show or hide Files",
        ),
        help_line("FILES", "a", "Create a file or folder"),
        help_line("FILES", "c / x / p", "Copy / cut / paste"),
        help_line("FILES", "r / m / d", "Rename / move / Trash"),
        help_line(
            "VIEW",
            shortcut(app, &["command_toggle_preview", "toggle_preview"]),
            "Show / hide preview",
        ),
        help_line(
            "EDIT",
            shortcut(app, &["command_undo", "command_redo"]),
            "Undo / redo",
        ),
        help_line(
            "EDIT",
            shortcut(app, &["command_inspect_semantic_blocks"]),
            "Inspect semantic source blocks",
        ),
        help_line(
            "EDIT",
            shortcut(app, &["command_read_semantic_blocks"]),
            "Read semantic blocks",
        ),
        help_line(
            "VIEW",
            shortcut(app, &["command_markdown_help"]),
            "Markdown syntax",
        ),
        help_line(
            "VIEW",
            shortcut(app, &["command_inspect_cursor_coordinates"]),
            "Cursor coordinates",
        ),
        help_line(
            "MENU",
            shortcut(app, &["command_open_palette", "command_palette"]),
            "Open grouped command menu",
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(block.title(" Shortcuts "))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn shortcut(app: &App, ids: &[&str]) -> String {
    ids.iter()
        .filter_map(|id| app.config.keybindings.binding(id))
        .map(|binding| binding.text.replace(',', " / "))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn command_shortcut(app: &App, action: crate::app::CommandAction) -> String {
    use crate::app::CommandAction;

    let ids: &[&str] = match action {
        CommandAction::Save => &["command_save", "save"],
        CommandAction::SaveAs => &["command_save_as", "save_as"],
        CommandAction::Duplicate => &["command_duplicate_document"],
        CommandAction::Create => return "a".to_owned(),
        CommandAction::CopyEntry => return "c".to_owned(),
        CommandAction::CutEntry => return "x".to_owned(),
        CommandAction::PasteEntry => return "p".to_owned(),
        CommandAction::RenameEntry => return "r".to_owned(),
        CommandAction::MoveEntry => return "m".to_owned(),
        CommandAction::TrashEntry => return "d".to_owned(),
        CommandAction::CloseTab => &["command_close_tab", "close_tab"],
        CommandAction::Quit => &["command_quit", "quit"],
        CommandAction::FileFinder => &["command_find_file", "find_file"],
        CommandAction::RecentDocuments => &["command_recent_documents", "recent_documents"],
        CommandAction::WorkspaceSearch => &["command_search_text", "search_text"],
        CommandAction::Find => &["command_find_replace", "find_replace"],
        CommandAction::Outline => &["command_document_outline", "document_outline"],
        CommandAction::ToggleExplorer => &["command_toggle_explorer", "toggle_explorer"],
        CommandAction::TogglePreview => &["command_toggle_preview", "toggle_preview"],
        CommandAction::WriteMode => &["command_write_mode"],
        CommandAction::CommandMode => return "Esc".to_owned(),
        CommandAction::Undo => &["command_undo", "undo"],
        CommandAction::Redo => &["command_redo", "redo"],
        CommandAction::MarkdownHelp => &["command_markdown_help"],
        CommandAction::InspectSemanticBlocks => &["command_inspect_semantic_blocks"],
        CommandAction::ReadSemanticBlocks => &["command_read_semantic_blocks"],
        CommandAction::InspectCursorCoordinates => &["command_inspect_cursor_coordinates"],
        CommandAction::Help => &["command_show_help", "show_help"],
    };
    shortcut(app, ids)
}

fn help_line(group: &str, key: impl Into<String>, label: &str) -> Line<'static> {
    let key = key.into();
    Line::from(vec![
        Span::styled(format!("{group:<10}"), Style::new().fg(MUTED).bold()),
        Span::styled(format!("{key:<16}"), Style::new().fg(BRIGHT).bold()),
        Span::styled(label.to_owned(), Style::new().fg(TEXT)),
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

const fn line_ending_label(line_ending: LineEnding) -> &'static str {
    match line_ending {
        LineEnding::Crlf => "CRLF",
        LineEnding::Cr => "CR",
        LineEnding::None | LineEnding::Lf | LineEnding::Mixed => "LF",
    }
}

fn mixed_line_ending_status(line_ending: LineEnding, target: Option<LineEnding>) -> String {
    if line_ending != LineEnding::Mixed {
        return String::new();
    }
    format!(
        " │ MIXED→{}",
        line_ending_label(target.unwrap_or(LineEnding::Lf))
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::config::{Config, StartupView};
    use crate::coordinate_diagnostic::diagnose_coordinate;
    use crate::semantic_blocks::map_semantic_blocks;
    use crate::workspace::Workspace;

    fn rendered(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

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

    #[test]
    fn renders_all_diagnostic_windows_with_the_preserved_popup_chrome() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let source = "# Heading\n\nParagraph\n\n- exact item\n";
        fs::write(&path, source).unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(110, 44)).unwrap();

        app.overlay = Some(Overlay::MarkdownHelp { scroll: 0 });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("Markdown syntax"));
        assert!(screen.contains("Headings"));
        assert!(screen.contains("links and footnotes remain visible but inert"));

        let mapping = map_semantic_blocks(source);
        app.overlay = Some(Overlay::SemanticInspector {
            mapping: mapping.clone(),
            selected: 0,
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("Semantic source blocks"));
        assert!(screen.contains("Parser ranges are read-only"));
        assert!(screen.contains("heading · H1 · lines 1-1"));
        assert!(screen.contains("heading · [0, 1) lines"));

        app.overlay = Some(Overlay::SemanticInspector {
            mapping: SemanticBlockMap::default(),
            selected: 0,
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("No semantic blocks in this document"));
        assert!(screen.contains("Nothing mapped. Empty documents keep an empty source map."));

        app.overlay = Some(Overlay::SemanticReader { mapping, scroll: 0 });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("Experimental semantic reading"));
        assert!(screen.contains("heading · lines 1-1 · rendered"));
        assert!(screen.contains("bullet list · lines 5-5 · source fallback"));
        assert!(screen.contains("- exact item"));

        app.overlay = Some(Overlay::CoordinateInspector {
            diagnostic: diagnose_coordinate(source, (0, 1), 60, 4).unwrap(),
            screen_position: Some((8, 12)),
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("Cursor coordinate diagnostic"));
        assert!(screen.contains("Source character offset: 1"));
        assert!(screen.contains("Logical location: line 0, column 1"));
        assert!(screen.contains("Terminal screen: row 8, cell 12"));
        assert!(screen.contains("Coordinates are a read-only snapshot"));
    }

    #[test]
    fn renders_mixed_and_conflict_actions_without_changing_the_shell() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "source").unwrap();
        let workspace = Workspace::from_target(&path).unwrap();
        let mut app = App::new(workspace).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();

        app.overlay = Some(Overlay::MixedLineEndings {
            tab_index: 0,
            previous_active: None,
            context: MixedLineEndingContext::Open,
            target: LineEnding::Crlf,
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("Mixed line endings"));
        assert!(screen.contains("Edit and normalize"));
        assert!(screen.contains("Cancel opening"));
        assert!(screen.contains("CRLF"));

        app.overlay = Some(Overlay::Conflict {
            kind: ConflictKind::Missing,
            can_reload: false,
            allow_continue: true,
        });
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let screen = rendered(&terminal);
        assert!(screen.contains("External conflict"));
        assert!(screen.contains("Save local as"));
        assert!(screen.contains("Continue without copy"));
        assert!(!screen.contains("Reload external"));
    }
}
