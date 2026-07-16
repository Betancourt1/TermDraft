//! `TextArea` setup and source-faithful inline Markdown presentation.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use regex::{Match, Regex};
use tui_textarea::{CursorRenderMode, TextArea, WrapMode};

use crate::app::Mode;

const MUTED: Color = Color::Rgb(92, 92, 92);
const TEXT: Color = Color::Rgb(218, 218, 218);
const BRIGHT: Color = Color::Rgb(242, 242, 242);
const HIDDEN: Color = Color::Rgb(0, 0, 0);

#[must_use]
pub fn textarea_from_source(source: &str) -> TextArea<'static> {
    let lines = source.split('\n').map(ToOwned::to_owned).collect();
    let mut editor = TextArea::new(lines);
    editor.set_wrap_mode(WrapMode::WordOrGlyph);
    editor.set_line_number_style(Style::new().fg(Color::Rgb(72, 72, 72)));
    editor.set_cursor_line_style(Style::new());
    editor.set_cursor_render_mode(CursorRenderMode::Cell);
    editor.set_selection_style(Style::new().bg(Color::Rgb(68, 68, 68)));
    editor.set_placeholder_text(
        "Focus Files and press Enter to open a document. Press ? for shortcuts.",
    );
    editor.set_placeholder_style(Style::new().fg(MUTED));
    style_cursor(&mut editor, Mode::Command);
    editor
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

/// Apply presentation-only highlights to every inactive logical line.
pub fn apply_inline_preview(editor: &mut TextArea<'_>) {
    let lines = editor.lines().to_vec();
    let cursor_line = editor.cursor().0;
    editor.clear_custom_highlight();

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
    }
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
    let end = line.len();
    editor.custom_highlight(
        ((row, marker.end()), (row, end)),
        Style::new().fg(color).bold(),
        5,
    );
    add_match(editor, row, marker, Style::new().fg(HIDDEN), 30);
}

fn highlight_wrapped(
    editor: &mut TextArea<'_>,
    row: usize,
    line: &str,
    regex: &Regex,
    style: Style,
) {
    for captures in regex.captures_iter(line) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let Some(content) = captures.get(1) else {
            continue;
        };
        add_match(editor, row, whole, Style::new().fg(HIDDEN), 20);
        add_match(editor, row, content, style, 30);
    }
}

fn highlight_links(editor: &mut TextArea<'_>, row: usize, line: &str) {
    for captures in link().captures_iter(line) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let Some(label) = captures.get(1) else {
            continue;
        };
        add_match(editor, row, whole, Style::new().fg(HIDDEN), 20);
        add_match(editor, row, label, Style::new().fg(TEXT).underlined(), 30);
    }
}

fn highlight_markers(editor: &mut TextArea<'_>, row: usize, line: &str) {
    for marker in task().find_iter(line) {
        add_match(editor, row, marker, Style::new().fg(BRIGHT).bold(), 25);
    }
    if let Some(marker) = bullet().find(line) {
        add_match(editor, row, marker, Style::new().fg(MUTED), 25);
    }
    for marker in table_pipe().find_iter(line) {
        add_match(editor, row, marker, Style::new().fg(MUTED), 10);
    }
}

fn add_match(
    editor: &mut TextArea<'_>,
    row: usize,
    matched: Match<'_>,
    style: Style,
    priority: u8,
) {
    editor.custom_highlight(
        ((row, matched.start()), (row, matched.end())),
        style,
        priority,
    );
}

fn heading() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^(#{1,6}\s+)").expect("valid heading regex"))
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

fn task() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\[[ xX]\]").expect("valid task regex"))
}

fn bullet() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\s*[-+*]\s+").expect("valid bullet regex"))
}

fn table_pipe() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"\|").expect("valid table regex"))
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
        apply_inline_preview(&mut editor);
        assert_eq!(source_from_textarea(&editor), "á **bold**\ncurrent");
    }

    #[test]
    fn inline_preview_hides_markers_without_changing_source() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut editor = textarea_from_source("current\n**bold**");
        editor.remove_line_number();
        apply_inline_preview(&mut editor);
        let mut terminal = Terminal::new(TestBackend::new(20, 3)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(&editor, frame.area()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 1)].fg, HIDDEN);
        assert_eq!(buffer[(1, 1)].fg, HIDDEN);
        assert_eq!(buffer[(2, 1)].fg, BRIGHT);
        assert_eq!(source_from_textarea(&editor), "current\n**bold**");
    }
}
