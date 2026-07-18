//! `TextArea` setup and source-faithful inline Markdown presentation.

use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use regex::Regex;
use tui_textarea::{CursorMove, CursorRenderMode, TextArea, WrapMode};

use crate::app::Mode;
use crate::config::EditorConfig;
use crate::markdown::{code_block_label, render_markdown};

const MUTED: Color = Color::Rgb(92, 92, 92);
const TEXT: Color = Color::Rgb(218, 218, 218);
const BRIGHT: Color = Color::Rgb(242, 242, 242);
const CODE_BACKGROUND: Color = Color::Rgb(28, 28, 28);

#[must_use]
pub fn textarea_from_source(source: &str) -> TextArea<'static> {
    let lines = source.split('\n').map(ToOwned::to_owned).collect();
    let mut editor = TextArea::new(lines);
    editor.set_wrap_mode(WrapMode::WordOrGlyph);
    editor.set_line_number_style(Style::new().fg(Color::Rgb(72, 72, 72)));
    editor.set_cursor_line_style(Style::new());
    editor.set_cursor_render_mode(CursorRenderMode::Hidden);
    editor.set_selection_style(Style::new().bg(Color::Rgb(68, 68, 68)));
    editor.set_placeholder_text(
        "Focus Files and press Enter to open a document. Press ? for shortcuts.",
    );
    editor.set_placeholder_style(Style::new().fg(MUTED));
    style_cursor(&mut editor, Mode::Command);
    editor
}

pub fn apply_editor_config(editor: &mut TextArea<'_>, config: &EditorConfig) {
    editor.set_wrap_mode(if config.soft_wrap {
        WrapMode::WordOrGlyph
    } else {
        WrapMode::None
    });
    if config.show_line_numbers {
        editor.set_line_number_style(Style::new().fg(Color::Rgb(72, 72, 72)));
    } else {
        editor.remove_line_number();
    }
}

pub fn style_cursor(editor: &mut TextArea<'_>, mode: Mode) {
    let style = match mode {
        Mode::Command => Style::new().fg(Color::Black).bg(Color::Gray),
        Mode::Write => Style::new().fg(BRIGHT).add_modifier(Modifier::UNDERLINED),
    };
    editor.set_cursor_style(style);
}

#[must_use]
pub fn source_from_textarea(editor: &TextArea<'_>) -> String {
    editor.lines().join("\n")
}

/// Resolve a terminal cell to the nearest cursor position in the rendered editor.
#[must_use]
pub fn cursor_at_screen_position(
    editor: &TextArea<'_>,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<(usize, usize)> {
    if area.is_empty() || !area.contains(Position::new(column, row)) {
        return None;
    }

    let line_number_width = editor.line_number_style().map_or(0, |_| {
        u16::try_from(editor.lines().len().max(1).ilog10() + 3).unwrap_or(u16::MAX)
    });
    let content_width = area.width.saturating_sub(line_number_width).max(1);
    let line_heights = editor
        .lines()
        .iter()
        .map(|line| rendered_line_height(editor, line, content_width))
        .collect::<Vec<_>>();
    let (cursor_row, cursor_column) = editor.cursor();
    let cursor_line_position = rendered_line_cursor(
        editor,
        &editor.lines()[cursor_row],
        content_width,
        cursor_column,
    )?;
    let cursor_visual_row =
        line_heights[..cursor_row].iter().sum::<usize>() + usize::from(cursor_line_position.y);
    let cursor_screen_row = editor
        .rendered_cursor_position()
        .or_else(|| render_cursor(&editor.clone(), area))?
        .y
        .saturating_sub(area.y);
    let target_visual_row = cursor_visual_row.saturating_sub(usize::from(cursor_screen_row))
        + usize::from(row.saturating_sub(area.y));
    let mut line_start = 0;
    let target_source_row = line_heights.iter().position(|height| {
        let contains_target = target_visual_row < line_start + height;
        line_start += height;
        contains_target
    })?;
    let target_line_row =
        target_visual_row.saturating_sub(line_start - line_heights[target_source_row]);
    let target_column = column.saturating_sub(area.x.saturating_add(line_number_width));
    let mut probe = line_probe(editor, &editor.lines()[target_source_row]);
    cursor_in_rendered_area(
        &mut probe,
        Rect::new(
            0,
            0,
            content_width,
            u16::try_from(line_heights[target_source_row]).unwrap_or(u16::MAX),
        ),
        target_column,
        u16::try_from(target_line_row).unwrap_or(u16::MAX),
    )
    .map(|(_, column)| (target_source_row, column))
}

fn rendered_line_height(editor: &TextArea<'_>, line: &str, width: u16) -> usize {
    usize::from(line_probe(editor, line).measure(width).content_rows.max(1))
}

fn rendered_line_cursor(
    editor: &TextArea<'_>,
    line: &str,
    width: u16,
    column: usize,
) -> Option<Position> {
    let mut probe = line_probe(editor, line);
    probe.move_cursor(CursorMove::Jump(
        0,
        u16::try_from(column).unwrap_or(u16::MAX),
    ));
    let height = probe.measure(width).content_rows.max(1);
    render_cursor(&probe, Rect::new(0, 0, width, height))
}

fn line_probe(editor: &TextArea<'_>, line: &str) -> TextArea<'static> {
    let mut probe = TextArea::new(vec![line.to_owned()]);
    probe.set_wrap_mode(editor.wrap_mode());
    probe.set_tab_length(editor.tab_length());
    probe
}

fn cursor_in_rendered_area(
    probe: &mut TextArea<'_>,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<(usize, usize)> {
    let mut position = render_cursor(probe, area)?;
    let vertical_move = if row < position.y {
        CursorMove::Up
    } else {
        CursorMove::Down
    };
    while position.y != row {
        let before = probe.cursor();
        probe.move_cursor(vertical_move);
        if probe.cursor() == before {
            break;
        }
        let next = render_cursor(probe, area)?;
        if position.y.abs_diff(row) <= next.y.abs_diff(row) {
            probe.move_cursor(CursorMove::Jump(
                u16::try_from(before.0).unwrap_or(u16::MAX),
                u16::try_from(before.1).unwrap_or(u16::MAX),
            ));
            break;
        }
        position = next;
    }

    let target_row = position.y;
    while position.x != column {
        let before = probe.cursor();
        let horizontal_move = if column < position.x {
            CursorMove::Back
        } else {
            CursorMove::Forward
        };
        probe.move_cursor(horizontal_move);
        if probe.cursor() == before {
            break;
        }
        let next = render_cursor(probe, area)?;
        if next.y != target_row || position.x.abs_diff(column) < next.x.abs_diff(column) {
            probe.move_cursor(CursorMove::Jump(
                u16::try_from(before.0).unwrap_or(u16::MAX),
                u16::try_from(before.1).unwrap_or(u16::MAX),
            ));
            break;
        }
        position = next;
    }

    Some(probe.cursor())
}

fn render_cursor(editor: &TextArea<'_>, area: Rect) -> Option<Position> {
    let mut buffer = Buffer::empty(area);
    editor.render(area, &mut buffer);
    editor.rendered_cursor_position()
}

/// Build a presentation-only editor copy with every inactive line rendered as Markdown.
#[must_use]
pub fn inline_preview_editor<'a>(editor: &TextArea<'a>) -> TextArea<'a> {
    let source_lines = editor.lines().to_vec();
    let cursor_line = editor.cursor().0;
    let table_lines = table_line_contexts(&source_lines);
    let code_lines = code_line_contexts(&source_lines);
    let rendered_lines = source_lines
        .iter()
        .enumerate()
        .map(|(row, source)| {
            if row == cursor_line {
                source.clone()
            } else {
                code_lines[row].as_ref().map_or_else(
                    || render_inline_line(source, table_lines[row].as_ref()),
                    |context| render_inline_code_line(source, context),
                )
            }
        })
        .collect();

    let mut rendered = editor.clone();
    rendered.set_lines(rendered_lines, editor.cursor());
    apply_inline_styles(
        &mut rendered,
        &source_lines,
        cursor_line,
        &table_lines,
        &code_lines,
    );
    rendered
}

/// Update the raw active line without resetting the inline preview viewport.
pub fn sync_inline_preview_cursor(
    rendered: &mut TextArea<'_>,
    source: &TextArea<'_>,
    previous_cursor_line: usize,
) {
    let source_lines = source.lines();
    let cursor = source.cursor();
    let cursor_line = cursor.0;
    let table_lines = table_line_contexts(source_lines);
    let code_lines = code_line_contexts(source_lines);

    for row in [previous_cursor_line, cursor_line] {
        let Some(source_line) = source_lines.get(row) else {
            continue;
        };
        let line = if row == cursor_line {
            source_line.clone()
        } else {
            code_lines[row].as_ref().map_or_else(
                || render_inline_line(source_line, table_lines[row].as_ref()),
                |context| render_inline_code_line(source_line, context),
            )
        };
        replace_presentation_line(rendered, row, &line);
    }

    rendered.cancel_selection();
    rendered.move_cursor(tui_textarea::CursorMove::Jump(
        u16::try_from(cursor.0).unwrap_or(u16::MAX),
        u16::try_from(cursor.1).unwrap_or(u16::MAX),
    ));
    rendered.clear_custom_highlight();
    apply_inline_styles(
        rendered,
        source_lines,
        cursor_line,
        &table_lines,
        &code_lines,
    );
}

fn replace_presentation_line(editor: &mut TextArea<'_>, row: usize, replacement: &str) {
    if editor
        .lines()
        .get(row)
        .is_some_and(|line| line == replacement)
    {
        return;
    }
    editor.move_cursor(tui_textarea::CursorMove::Jump(
        u16::try_from(row).unwrap_or(u16::MAX),
        0,
    ));
    if editor.lines().get(row).is_some_and(|line| !line.is_empty()) {
        editor.delete_line_by_end();
    }
    editor.insert_str(replacement);
}

fn apply_inline_styles(
    editor: &mut TextArea<'_>,
    lines: &[String],
    cursor_line: usize,
    table_lines: &[Option<TableLineContext>],
    code_lines: &[Option<CodeLineContext>],
) {
    for (row, line) in lines.iter().enumerate() {
        if row == cursor_line {
            continue;
        }
        if let Some(context) = &code_lines[row] {
            let end = editor.lines()[row].len();
            let style = match context.kind {
                CodeLineKind::Opening => Style::new().fg(BRIGHT).bg(CODE_BACKGROUND).bold(),
                CodeLineKind::Body => Style::new().fg(TEXT).bg(CODE_BACKGROUND),
                CodeLineKind::Closing => Style::new().fg(MUTED).bg(CODE_BACKGROUND),
            };
            editor.custom_highlight(((row, 0), (row, end)), style, 40);
            continue;
        }
        if line.is_empty() {
            continue;
        }
        highlight_heading(editor, row, line);
        highlight_wrapped(editor, row, line, bold(), Style::new().fg(BRIGHT).bold());
        highlight_wrapped(
            editor,
            row,
            line,
            bold_underscore(),
            Style::new().fg(BRIGHT).bold(),
        );
        highlight_wrapped(
            editor,
            row,
            line,
            strike(),
            Style::new().fg(MUTED).crossed_out(),
        );
        highlight_wrapped(editor, row, line, inline_code(), Style::new().fg(BRIGHT));
        highlight_wrapped(
            editor,
            row,
            line,
            emphasis(),
            Style::new().fg(TEXT).italic(),
        );
        highlight_wrapped(
            editor,
            row,
            line,
            emphasis_underscore(),
            Style::new().fg(TEXT).italic(),
        );
        highlight_links(editor, row, line);
        highlight_markers(editor, row, line);
        if table_lines[row]
            .as_ref()
            .is_some_and(|context| context.kind == TableLineKind::Header)
        {
            let end = editor.lines()[row].len();
            editor.custom_highlight(((row, 0), (row, end)), Style::new().fg(BRIGHT).bold(), 5);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodeLineKind {
    Opening,
    Body,
    Closing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodeLineContext {
    kind: CodeLineKind,
    label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenFence {
    marker: char,
    length: usize,
    label: String,
}

fn code_line_contexts(lines: &[String]) -> Vec<Option<CodeLineContext>> {
    let mut contexts = vec![None; lines.len()];
    let mut open: Option<OpenFence> = None;
    for (row, line) in lines.iter().enumerate() {
        if let Some(fence) = &open {
            if is_closing_fence(line, fence) {
                contexts[row] = Some(CodeLineContext {
                    kind: CodeLineKind::Closing,
                    label: fence.label.clone(),
                });
                open = None;
            } else {
                contexts[row] = Some(CodeLineContext {
                    kind: CodeLineKind::Body,
                    label: fence.label.clone(),
                });
            }
            continue;
        }
        let Some(fence) = opening_fence(line) else {
            continue;
        };
        contexts[row] = Some(CodeLineContext {
            kind: CodeLineKind::Opening,
            label: fence.label.clone(),
        });
        open = Some(fence);
    }
    contexts
}

fn opening_fence(source: &str) -> Option<OpenFence> {
    let indentation = source.len() - source.trim_start_matches(' ').len();
    if indentation > 3 {
        return None;
    }
    let content = &source[indentation..];
    let marker = content.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let length = content
        .chars()
        .take_while(|character| *character == marker)
        .count();
    if length < 3 {
        return None;
    }
    let info = content.get(length..)?.trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some(OpenFence {
        marker,
        length,
        label: code_block_label((!info.is_empty()).then_some(info)),
    })
}

fn is_closing_fence(source: &str, open: &OpenFence) -> bool {
    let indentation = source.len() - source.trim_start_matches(' ').len();
    if indentation > 3 {
        return false;
    }
    let content = &source[indentation..];
    let length = content
        .chars()
        .take_while(|character| *character == open.marker)
        .count();
    length >= open.length
        && content
            .get(length..)
            .is_some_and(|remainder| remainder.trim().is_empty())
}

fn render_inline_code_line(source: &str, context: &CodeLineContext) -> String {
    match context.kind {
        CodeLineKind::Opening => format!("┌─ {}", context.label),
        CodeLineKind::Body => format!("│ {source}"),
        CodeLineKind::Closing => "└─".to_owned(),
    }
}

fn render_inline_line(source: &str, table_line: Option<&TableLineContext>) -> String {
    match table_line {
        Some(context) if context.kind == TableLineKind::Separator => {
            compact_table_separator(&context.widths)
        }
        Some(context) => compact_table_row(source, context),
        None => {
            let (indentation, markdown) = inline_list_source(source);
            let rendered = render_markdown(markdown)
                .lines
                .into_iter()
                .filter_map(|line| {
                    let text = line
                        .spans
                        .into_iter()
                        .map(|span| span.content.into_owned())
                        .collect::<String>();
                    (!text.is_empty()).then_some(text)
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!("{indentation}{rendered}")
        }
    }
}

fn inline_list_source(source: &str) -> (&str, &str) {
    let Some(captures) = list_item().captures(source) else {
        return ("", source);
    };
    let indentation = captures.get(1).map_or("", |capture| capture.as_str());
    (indentation, &source[indentation.len()..])
}

fn highlight_heading(editor: &mut TextArea<'_>, row: usize, line: &str) {
    let Some(captures) = heading().captures(line) else {
        return;
    };
    let Some(marker) = captures.get(1) else {
        return;
    };
    let level = marker
        .as_str()
        .chars()
        .filter(|character| *character == '#')
        .count();
    let color = match level {
        1 => Color::Rgb(246, 246, 246),
        2 => Color::Rgb(232, 232, 232),
        3 => Color::Rgb(218, 218, 218),
        _ => Color::Rgb(196, 196, 196),
    };
    let end = editor.lines()[row].len();
    editor.custom_highlight(((row, 0), (row, end)), Style::new().fg(color).bold(), 5);
}

fn highlight_wrapped(
    editor: &mut TextArea<'_>,
    row: usize,
    line: &str,
    regex: &Regex,
    style: Style,
) {
    for captures in regex.captures_iter(line) {
        let Some(content) = captures.get(1) else {
            continue;
        };
        add_rendered_text(editor, row, content.as_str(), style, 30);
    }
}

fn highlight_links(editor: &mut TextArea<'_>, row: usize, line: &str) {
    for (pattern, style) in [
        (link(), Style::new().fg(TEXT).underlined()),
        (image(), Style::new().fg(TEXT).italic()),
    ] {
        for captures in pattern.captures_iter(line) {
            let Some(label) = captures.get(1) else {
                continue;
            };
            add_rendered_text(editor, row, label.as_str(), style, 30);
        }
    }
}

fn highlight_markers(editor: &mut TextArea<'_>, row: usize, line: &str) {
    if let Some(captures) = task().captures(line)
        && let Some(state) = captures.get(1)
    {
        add_rendered_text(
            editor,
            row,
            if state.as_str().eq_ignore_ascii_case("x") {
                "☑"
            } else {
                "☐"
            },
            Style::new().fg(BRIGHT).bold(),
            25,
        );
    }
    if bullet().is_match(line) {
        add_rendered_text(editor, row, "•", Style::new().fg(MUTED), 25);
    }
    if quote().is_match(line) {
        add_rendered_text(editor, row, "│", Style::new().fg(MUTED), 25);
    }
    add_rendered_text(editor, row, "│", Style::new().fg(MUTED), 10);
}

fn add_rendered_text(
    editor: &mut TextArea<'_>,
    row: usize,
    text: &str,
    style: Style,
    priority: u8,
) {
    let rendered_line = editor.lines()[row].clone();
    for (start, _) in rendered_line.match_indices(text) {
        editor.custom_highlight(((row, start), (row, start + text.len())), style, priority);
    }
}

fn heading() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^(#{1,6}\s+)").expect("valid heading regex"))
}

fn quote() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\s{0,3}(>)\s?").expect("valid quote regex"))
}

fn bold() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\*\*(.+?)\*\*").expect("valid bold regex"))
}

fn bold_underscore() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"__(.+?)__").expect("valid bold regex"))
}

fn emphasis() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?:^|\s)\*([^*\n]+?)\*").expect("valid emphasis regex"))
}

fn emphasis_underscore() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"_([^_\n]+?)_").expect("valid emphasis regex"))
}

fn strike() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"~~(.+?)~~").expect("valid strike regex"))
}

fn inline_code() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"`([^`\n]+?)`").expect("valid inline-code regex"))
}

fn link() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\[([^\]\n]+)\]\([^\)\n]+\)").expect("valid link regex"))
}

fn image() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"!\[([^\]\n]*)\]\([^\)\n]+\)").expect("valid image regex"))
}

fn task() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\[([ xX])\]").expect("valid task regex"))
}

fn bullet() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\s*([-+*])\s+").expect("valid bullet regex"))
}

fn list_item() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^([ \t]*)(?:[-+*]|\d+[.)])(?:[ \t]+|$)").expect("valid list-item regex")
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TableLineKind {
    Header,
    Separator,
    Body,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TableAlignment {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TableLineContext {
    kind: TableLineKind,
    widths: Vec<usize>,
    alignments: Vec<TableAlignment>,
}

fn table_line_contexts(lines: &[String]) -> Vec<Option<TableLineContext>> {
    let mut contexts = vec![None; lines.len()];
    let mut row = 0;
    while row + 1 < lines.len() {
        if !is_table_header(lines, row) {
            row += 1;
            continue;
        }

        let separator = row + 1;
        let mut end = separator + 1;
        while end < lines.len() && looks_like_table_row(&lines[end]) {
            end += 1;
        }
        let cells = (row..end)
            .filter(|table_row| *table_row != separator)
            .map(|table_row| rendered_table_cells(&lines[table_row]))
            .collect::<Vec<_>>();
        let columns = cells.iter().map(Vec::len).max().unwrap_or_default();
        let widths = (0..columns)
            .map(|column| {
                cells
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| unicode_width::UnicodeWidthStr::width(cell.as_str()))
                    .max()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let alignments = table_cells(&lines[separator])
            .into_iter()
            .map(|cell| table_alignment(&cell))
            .collect::<Vec<_>>();

        for (table_row, context) in contexts.iter_mut().enumerate().take(end).skip(row) {
            let kind = if table_row == row {
                TableLineKind::Header
            } else if table_row == separator {
                TableLineKind::Separator
            } else {
                TableLineKind::Body
            };
            *context = Some(TableLineContext {
                kind,
                widths: widths.clone(),
                alignments: alignments.clone(),
            });
        }
        row = end;
    }
    contexts
}

fn is_table_header(lines: &[String], row: usize) -> bool {
    row + 1 < lines.len()
        && looks_like_table_row(&lines[row])
        && is_table_separator(&lines[row + 1])
        && table_cell_count(&lines[row]) == table_cell_count(&lines[row + 1])
}

fn is_table_separator(source: &str) -> bool {
    table_separator().is_match(source)
}

fn looks_like_table_row(source: &str) -> bool {
    !source.starts_with("    ") && !table_pipe_positions(source).is_empty()
}

fn table_cell_count(source: &str) -> usize {
    let positions = table_pipe_positions(source);
    let stripped = source.trim();
    positions.len() + 1
        - usize::from(stripped.starts_with('|'))
        - usize::from(stripped.ends_with('|'))
}

fn table_pipe_positions(source: &str) -> Vec<usize> {
    let characters = source.chars().collect::<Vec<_>>();
    characters
        .iter()
        .enumerate()
        .filter_map(|(index, character)| {
            (*character == '|' && (index == 0 || characters[index - 1] != '\\')).then_some(index)
        })
        .collect()
}

fn table_cells(source: &str) -> Vec<String> {
    let trimmed = source.trim();
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for character in trimmed.chars() {
        if character == '|' && !escaped {
            cells.push(std::mem::take(&mut cell));
        } else {
            cell.push(character);
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    cells.push(cell);
    if trimmed.starts_with('|') {
        cells.remove(0);
    }
    if trimmed.ends_with('|') {
        cells.pop();
    }
    cells
        .into_iter()
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn rendered_table_cells(source: &str) -> Vec<String> {
    table_cells(source)
        .into_iter()
        .map(|cell| render_inline_line(&cell, None))
        .collect()
}

fn compact_table_row(source: &str, context: &TableLineContext) -> String {
    let cells = rendered_table_cells(source);
    let mut row = String::from("│");
    for (column, width) in context.widths.iter().enumerate() {
        let cell = cells.get(column).map_or("", String::as_str);
        let padding = width.saturating_sub(unicode_width::UnicodeWidthStr::width(cell));
        let alignment = context
            .alignments
            .get(column)
            .copied()
            .unwrap_or(TableAlignment::Left);
        let (left, right) = match alignment {
            TableAlignment::Left => (0, padding),
            TableAlignment::Center => (padding / 2, padding - (padding / 2)),
            TableAlignment::Right => (padding, 0),
        };
        row.push(' ');
        row.push_str(&" ".repeat(left));
        row.push_str(cell);
        row.push_str(&" ".repeat(right));
        row.push_str(" │");
    }
    row
}

fn compact_table_separator(widths: &[usize]) -> String {
    let mut separator = String::from("├");
    for (column, width) in widths.iter().enumerate() {
        separator.push_str(&"─".repeat(width + 2));
        separator.push(if column + 1 == widths.len() {
            '┤'
        } else {
            '┼'
        });
    }
    separator
}

fn table_alignment(cell: &str) -> TableAlignment {
    match (cell.trim().starts_with(':'), cell.trim().ends_with(':')) {
        (true, true) => TableAlignment::Center,
        (false, true) => TableAlignment::Right,
        _ => TableAlignment::Left,
    }
}

fn table_separator() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s{0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)+\|?[ \t]*$")
            .expect("valid table-separator regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textarea_round_trip_preserves_trailing_newline() {
        let editor = textarea_from_source("one\ntwo\n");
        assert_eq!(source_from_textarea(&editor), "one\ntwo\n");
    }

    #[test]
    fn inline_preview_accepts_non_ascii_offsets() {
        let mut editor = textarea_from_source("á **bold**\ncurrent");
        editor.move_cursor(tui_textarea::CursorMove::Down);
        let rendered = inline_preview_editor(&editor);
        assert_eq!(rendered.lines()[0], "á bold");
        assert_eq!(source_from_textarea(&editor), "á **bold**\ncurrent");
    }

    #[test]
    fn inline_preview_renders_inactive_lines_and_keeps_current_source() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut editor = textarea_from_source(
            "**current**\n# Heading\n**bold** and [link](url)\n- [x] finished\n| A | B |\n|---|---|\n| 1 | 2 |",
        );
        editor.remove_line_number();
        let rendered_editor = inline_preview_editor(&editor);
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(&rendered_editor, frame.area()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let screen = (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("Heading"));
        assert!(!screen.contains("# Heading"));
        assert!(screen.contains("bold"));
        assert!(screen.contains("**current**"));
        assert!(screen.contains("link"));
        assert!(!screen.contains("(url)"));
        assert!(screen.contains("• ☑"));
        assert!(screen.contains("├───┼───┤"));
        assert_eq!(rendered_editor.lines()[0], "**current**");
        assert_eq!(
            source_from_textarea(&editor),
            "**current**\n# Heading\n**bold** and [link](url)\n- [x] finished\n| A | B |\n|---|---|\n| 1 | 2 |"
        );

        editor.move_cursor(tui_textarea::CursorMove::Down);
        let moved = inline_preview_editor(&editor);
        assert_eq!(moved.lines()[0], "current");
        assert_eq!(moved.lines()[1], "# Heading");
        assert_eq!(
            source_from_textarea(&editor),
            "**current**\n# Heading\n**bold** and [link](url)\n- [x] finished\n| A | B |\n|---|---|\n| 1 | 2 |"
        );
    }

    #[test]
    fn inline_preview_aligns_complete_tables() {
        let editor = textarea_from_source(
            "current\n| Name | Count | Note |\n| :--- | ---: | :---: |\n| Ada | 7 | Hi |",
        );
        let rendered = inline_preview_editor(&editor);

        assert_eq!(rendered.lines()[1], "│ Name │ Count │ Note │");
        assert_eq!(rendered.lines()[2], "├──────┼───────┼──────┤");
        assert_eq!(rendered.lines()[3], "│ Ada  │     7 │  Hi  │");
        assert_eq!(
            source_from_textarea(&editor),
            "current\n| Name | Count | Note |\n| :--- | ---: | :---: |\n| Ada | 7 | Hi |"
        );
    }

    #[test]
    fn inline_preview_preserves_nested_list_indentation() {
        let editor = textarea_from_source(
            "current\n- Parent\n  - Nested bullet\n   1. Nested number\n    - Deep item",
        );
        let rendered = inline_preview_editor(&editor);

        assert_eq!(rendered.lines()[1], "• Parent");
        assert_eq!(rendered.lines()[2], "  • Nested bullet");
        assert_eq!(rendered.lines()[3], "   1. Nested number");
        assert_eq!(rendered.lines()[4], "    • Deep item");
        assert_eq!(
            source_from_textarea(&editor),
            "current\n- Parent\n  - Nested bullet\n   1. Nested number\n    - Deep item"
        );
    }

    #[test]
    fn inline_preview_labels_and_frames_fenced_code() {
        let editor = textarea_from_source(
            "current\n```bash\necho hello\n```\n\n```python\nprint('hello')\n```\n\n```\nplain\n```",
        );
        let rendered = inline_preview_editor(&editor);

        assert_eq!(rendered.lines()[1], "┌─ BASH");
        assert_eq!(rendered.lines()[2], "│ echo hello");
        assert_eq!(rendered.lines()[3], "└─");
        assert_eq!(rendered.lines()[5], "┌─ CODE · PYTHON");
        assert_eq!(rendered.lines()[6], "│ print('hello')");
        assert_eq!(rendered.lines()[9], "┌─ CODE");
        assert_eq!(rendered.lines()[10], "│ plain");
        assert_eq!(
            source_from_textarea(&editor),
            "current\n```bash\necho hello\n```\n\n```python\nprint('hello')\n```\n\n```\nplain\n```"
        );
    }
}
