//! `TextArea` setup and source-faithful inline Markdown presentation.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use regex::Regex;
use tui_textarea::{CursorRenderMode, TextArea, WrapMode};

use crate::app::Mode;
use crate::config::EditorConfig;

const MUTED: Color = Color::Rgb(92, 92, 92);
const TEXT: Color = Color::Rgb(218, 218, 218);
const BRIGHT: Color = Color::Rgb(242, 242, 242);

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

/// Build a presentation-only editor copy with every inactive line rendered as Markdown.
#[must_use]
pub fn inline_preview_editor<'a>(editor: &TextArea<'a>) -> TextArea<'a> {
    let source_lines = editor.lines().to_vec();
    let cursor_line = editor.cursor().0;
    let table_lines = (0..source_lines.len())
        .map(|row| table_line_kind(&source_lines, row))
        .collect::<Vec<_>>();
    let rendered_lines = source_lines
        .iter()
        .enumerate()
        .map(|(row, source)| {
            if row == cursor_line {
                source.clone()
            } else {
                render_inline_line(source, table_lines[row])
            }
        })
        .collect();

    let mut rendered = editor.clone();
    rendered.set_lines(rendered_lines, editor.cursor());
    apply_inline_styles(&mut rendered, &source_lines, cursor_line, &table_lines);
    rendered
}

fn apply_inline_styles(
    editor: &mut TextArea<'_>,
    lines: &[String],
    cursor_line: usize,
    table_lines: &[Option<TableLineKind>],
) {
    for (row, line) in lines.iter().enumerate() {
        if row == cursor_line || line.is_empty() {
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
        highlight_links(editor, row, line);
        highlight_markers(editor, row, line);
        if table_lines[row] == Some(TableLineKind::Header) {
            let end = editor.lines()[row].len();
            editor.custom_highlight(((row, 0), (row, end)), Style::new().fg(BRIGHT).bold(), 5);
        }
    }
}

fn render_inline_line(source: &str, table_line: Option<TableLineKind>) -> String {
    let mut characters = source.chars().collect::<Vec<_>>();

    if table_line == Some(TableLineKind::Separator) {
        render_table_separator(&mut characters);
        return characters.into_iter().collect();
    }
    if thematic_break().is_match(source) {
        for character in &mut characters {
            if !character.is_whitespace() {
                *character = '─';
            }
        }
        return characters.into_iter().collect();
    }
    if let Some(captures) = heading().captures(source)
        && let Some(marker) = captures.get(1)
    {
        blank_bytes(&mut characters, source, marker.start(), marker.end());
    } else if let Some(captures) = quote().captures(source)
        && let Some(marker) = captures.get(1)
    {
        replace_byte(&mut characters, source, marker.start(), '│');
    } else if let Some(captures) = bullet().captures(source)
        && let Some(marker) = captures.get(1)
    {
        replace_byte(&mut characters, source, marker.start(), '•');
    }

    if let Some(captures) = task().captures(source)
        && let Some(state) = captures.get(1)
    {
        let state_index = byte_to_char(source, state.start());
        let marker_start = state_index.saturating_sub(1);
        let marker = if state.as_str().eq_ignore_ascii_case("x") {
            '☑'
        } else {
            '☐'
        };
        if marker_start + 2 < characters.len() {
            characters[marker_start..marker_start + 3].copy_from_slice(&[marker, ' ', ' ']);
        }
    }
    if let Some(captures) = fence().captures(source)
        && let Some(marker) = captures.get(1)
    {
        blank_bytes(&mut characters, source, marker.start(), marker.end());
    }

    blank_wrapped_markers(&mut characters, source, inline_code());
    blank_wrapped_markers(&mut characters, source, image());
    blank_wrapped_markers(&mut characters, source, link());
    blank_wrapped_markers(&mut characters, source, bold());
    blank_wrapped_markers(&mut characters, source, bold_underscore());
    blank_wrapped_markers(&mut characters, source, strike());
    blank_wrapped_markers(&mut characters, source, emphasis());
    blank_wrapped_markers(&mut characters, source, emphasis_underscore());

    if matches!(
        table_line,
        Some(TableLineKind::Header | TableLineKind::Body)
    ) {
        for position in table_pipe_positions(source) {
            characters[position] = '│';
        }
    }
    characters.into_iter().collect()
}

fn blank_wrapped_markers(characters: &mut [char], source: &str, regex: &Regex) {
    for captures in regex.captures_iter(source) {
        let (Some(whole), Some(content)) = (captures.get(0), captures.get(1)) else {
            continue;
        };
        blank_bytes(characters, source, whole.start(), content.start());
        blank_bytes(characters, source, content.end(), whole.end());
    }
}

fn blank_bytes(characters: &mut [char], source: &str, start: usize, end: usize) {
    let start = byte_to_char(source, start);
    let end = byte_to_char(source, end);
    characters[start..end].fill(' ');
}

fn replace_byte(characters: &mut [char], source: &str, byte: usize, replacement: char) {
    if let Some(character) = characters.get_mut(byte_to_char(source, byte)) {
        *character = replacement;
    }
}

fn byte_to_char(source: &str, byte: usize) -> usize {
    source[..byte].chars().count()
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
    let rendered_line = editor.lines()[row].clone();
    let start = char_to_byte(&rendered_line, byte_to_char(line, marker.end()));
    let end = rendered_line.len();
    editor.custom_highlight(((row, start), (row, end)), Style::new().fg(color).bold(), 5);
    add_match(editor, row, line, marker, Style::new().fg(MUTED), 30);
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
        add_match(editor, row, line, content, style, 30);
    }
}

fn highlight_links(editor: &mut TextArea<'_>, row: usize, line: &str) {
    for captures in link().captures_iter(line) {
        let Some(label) = captures.get(1) else {
            continue;
        };
        add_match(
            editor,
            row,
            line,
            label,
            Style::new().fg(TEXT).underlined(),
            30,
        );
    }
}

fn highlight_markers(editor: &mut TextArea<'_>, row: usize, line: &str) {
    for marker in task().find_iter(line) {
        add_match(
            editor,
            row,
            line,
            marker,
            Style::new().fg(BRIGHT).bold(),
            25,
        );
    }
    if let Some(marker) = bullet().find(line) {
        add_match(editor, row, line, marker, Style::new().fg(MUTED), 25);
    }
    for marker in table_pipe().find_iter(line) {
        add_match(editor, row, line, marker, Style::new().fg(MUTED), 10);
    }
}

fn add_match(
    editor: &mut TextArea<'_>,
    row: usize,
    source: &str,
    matched: regex::Match<'_>,
    style: Style,
    priority: u8,
) {
    let rendered_line = editor.lines()[row].clone();
    let start = char_to_byte(&rendered_line, byte_to_char(source, matched.start()));
    let end = char_to_byte(&rendered_line, byte_to_char(source, matched.end()));
    editor.custom_highlight(((row, start), (row, end)), style, priority);
}

fn char_to_byte(source: &str, character: usize) -> usize {
    source
        .char_indices()
        .nth(character)
        .map_or(source.len(), |(byte, _)| byte)
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

fn table_pipe() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\|").expect("valid table regex"))
}

fn thematic_break() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s{0,3}(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$")
            .expect("valid thematic-break regex")
    })
}

fn fence() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\s{0,3}(`{3,}|~{3,})").expect("valid fence regex"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TableLineKind {
    Header,
    Separator,
    Body,
}

fn table_line_kind(lines: &[String], row: usize) -> Option<TableLineKind> {
    if is_table_header(lines, row) {
        return Some(TableLineKind::Header);
    }
    if is_table_separator(&lines[row]) && row > 0 && is_table_header(lines, row - 1) {
        return Some(TableLineKind::Separator);
    }
    if !looks_like_table_row(&lines[row]) {
        return None;
    }
    for preceding in (1..row).rev() {
        if is_table_separator(&lines[preceding]) {
            return is_table_header(lines, preceding - 1).then_some(TableLineKind::Body);
        }
        if !looks_like_table_row(&lines[preceding]) {
            break;
        }
    }
    None
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

fn render_table_separator(characters: &mut [char]) {
    let Some(start) = characters
        .iter()
        .position(|character| !character.is_whitespace())
    else {
        return;
    };
    let end = characters
        .iter()
        .rposition(|character| !character.is_whitespace())
        .unwrap_or(start);
    for character in &mut characters[start..=end] {
        *character = if *character == '|' { '┼' } else { '─' };
    }
    if characters[start] == '┼' {
        characters[start] = '├';
    }
    if characters[end] == '┼' {
        characters[end] = '┤';
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
        assert_eq!(rendered.lines()[0], "á   bold  ");
        assert_eq!(source_from_textarea(&editor), "á **bold**\ncurrent");
    }

    #[test]
    fn inline_preview_renders_inactive_lines_and_keeps_current_source() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut editor = textarea_from_source(
            "current\n# Heading\n**bold** and [link](url)\n- [x] finished\n| A | B |\n|---|---|\n| 1 | 2 |",
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
        assert!(!screen.contains("**"));
        assert!(screen.contains("link"));
        assert!(!screen.contains("(url)"));
        assert!(screen.contains("• ☑"));
        assert!(screen.contains("├───┼───┤"));
        assert_eq!(rendered_editor.lines()[0], "current");
        assert_eq!(
            source_from_textarea(&editor),
            "current\n# Heading\n**bold** and [link](url)\n- [x] finished\n| A | B |\n|---|---|\n| 1 | 2 |"
        );
    }
}
