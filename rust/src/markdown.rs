//! Semantic Markdown rendering for the read-only preview pane.

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TEXT: Color = Color::Rgb(218, 218, 218);
const MUTED: Color = Color::Rgb(118, 118, 118);
const BRIGHT: Color = Color::Rgb(242, 242, 242);
const CODE_BACKGROUND: Color = Color::Rgb(28, 28, 28);

#[derive(Debug)]
struct ListState {
    next: Option<u64>,
}

#[derive(Debug, Default)]
struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: String,
    header_rows: usize,
}

#[derive(Debug, Default)]
struct MarkdownRenderer {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<ListState>,
    item_depth: usize,
    quote_depth: usize,
    code_block: bool,
    table: Option<TableState>,
    table_width: Option<usize>,
}

/// Parse Markdown into terminal-native semantic lines without source markers.
#[must_use]
pub fn render_markdown(source: &str) -> Text<'static> {
    render_markdown_inner(source, None)
}

/// Render Markdown while keeping table borders within the available terminal width.
#[must_use]
pub fn render_markdown_with_width(source: &str, width: usize) -> Text<'static> {
    render_markdown_inner(source, Some(width.max(1)))
}

fn render_markdown_inner(source: &str, table_width: Option<usize>) -> Text<'static> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);

    let mut renderer = MarkdownRenderer {
        table_width,
        ..MarkdownRenderer::default()
    };
    for event in Parser::new_ext(source, options) {
        renderer.handle(event);
    }
    renderer.flush_line();
    Text::from(renderer.lines)
}

impl MarkdownRenderer {
    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) | Event::InlineMath(code) => {
                self.push_styled(&code, Style::new().fg(BRIGHT).bg(CODE_BACKGROUND));
            }
            Event::DisplayMath(math) => {
                self.separate_block();
                self.push_styled(&math, Style::new().fg(BRIGHT).italic());
                self.flush_line();
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.separate_block();
                self.current.push(Span::styled(
                    "────────────────────────".to_owned(),
                    Style::new().fg(MUTED),
                ));
                self.flush_line();
            }
            Event::TaskListMarker(checked) => {
                self.push_styled(if checked { "☑ " } else { "☐ " }, Style::new().fg(BRIGHT));
            }
            Event::FootnoteReference(label) => {
                self.push_styled(&format!("[{label}]"), Style::new().fg(BRIGHT).underlined());
            }
            Event::Html(_) | Event::InlineHtml(_) => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.item_depth == 0 {
                    self.separate_block();
                }
            }
            Tag::Heading { level, .. } => {
                self.separate_block();
                self.push_style(heading_style(level));
            }
            Tag::BlockQuote(_) => {
                if self.quote_depth == 0 {
                    self.separate_block();
                }
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.separate_block();
                self.ensure_quote_prefix();
                self.current.push(Span::styled(
                    format!("┌─ {}", code_block_label(code_block_info(&kind))),
                    Style::new().fg(BRIGHT).bg(CODE_BACKGROUND).bold(),
                ));
                self.flush_line();
                self.code_block = true;
            }
            Tag::List(next) => self.lists.push(ListState { next }),
            Tag::Item => self.start_list_item(),
            Tag::Emphasis => self.push_style(Style::new().italic()),
            Tag::Strong | Tag::DefinitionListTitle => self.push_style(Style::new().bold()),
            Tag::Strikethrough => self.push_style(Style::new().crossed_out()),
            Tag::Superscript | Tag::Subscript => self.push_style(Style::new().dim()),
            Tag::Link { .. } => self.push_style(Style::new().fg(BRIGHT).underlined()),
            Tag::Image { .. } => {
                self.push_styled("▧ ", Style::new().fg(MUTED));
                self.push_style(Style::new().italic());
            }
            Tag::FootnoteDefinition(label) => {
                self.separate_block();
                self.push_styled(&format!("[{label}] "), Style::new().fg(BRIGHT).bold());
            }
            Tag::DefinitionList => self.separate_block(),
            Tag::DefinitionListDefinition => {
                self.flush_line();
                self.push_styled("  ", Style::new());
            }
            Tag::Table(alignments) => {
                self.separate_block();
                self.table = Some(TableState {
                    alignments,
                    ..TableState::default()
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.row.clear();
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.cell.clear();
                }
            }
            Tag::MetadataBlock(_) => self.push_style(Style::new().fg(MUTED)),
            Tag::HtmlBlock => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::FootnoteDefinition | TagEnd::DefinitionListDefinition => {
                self.flush_line();
            }
            TagEnd::Heading(_) => {
                self.pop_style();
                self.flush_line();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.flush_line();
                self.code_block = false;
                self.ensure_quote_prefix();
                self.current.push(Span::styled(
                    "└─".to_owned(),
                    Style::new().fg(MUTED).bg(CODE_BACKGROUND),
                ));
                self.flush_line();
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush_line();
                self.item_depth = self.item_depth.saturating_sub(1);
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::DefinitionListTitle
            | TagEnd::MetadataBlock(_) => self.pop_style(),
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table {
                    table.row.push(table.cell.trim().to_owned());
                    table.cell.clear();
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    if !table.row.is_empty() {
                        table.rows.push(std::mem::take(&mut table.row));
                    }
                    table.header_rows = table.rows.len();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table
                    && !table.row.is_empty()
                {
                    table.rows.push(std::mem::take(&mut table.row));
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.render_table(&table);
                }
            }
            TagEnd::HtmlBlock | TagEnd::DefinitionList => {}
        }
    }

    fn start_list_item(&mut self) {
        self.flush_line();
        self.item_depth += 1;
        self.ensure_quote_prefix();
        self.current
            .push(Span::raw("  ".repeat(self.lists.len().saturating_sub(1))));
        let marker = self.lists.last_mut().map_or_else(
            || "• ".to_owned(),
            |list| match &mut list.next {
                Some(next) => {
                    let marker = format!("{next}. ");
                    *next += 1;
                    marker
                }
                None => "• ".to_owned(),
            },
        );
        self.current
            .push(Span::styled(marker, Style::new().fg(MUTED)));
    }

    fn push_text(&mut self, text: &str) {
        if let Some(table) = &mut self.table {
            table.cell.push_str(text);
            return;
        }
        for (index, part) in text.split('\n').enumerate() {
            if index > 0 {
                self.flush_line();
            }
            if !part.is_empty() {
                self.push_styled(part, self.current_style());
            }
        }
    }

    fn push_styled(&mut self, text: &str, style: Style) {
        if let Some(table) = &mut self.table {
            table.cell.push_str(text);
            return;
        }
        let starts_line = self.current.is_empty();
        self.ensure_quote_prefix();
        if starts_line && self.code_block {
            self.current.push(Span::styled(
                "│ ".to_owned(),
                Style::new().fg(MUTED).bg(CODE_BACKGROUND),
            ));
        }
        let style = if self.code_block {
            style.patch(Style::new().fg(TEXT).bg(CODE_BACKGROUND))
        } else if self.quote_depth > 0 {
            style.patch(Style::new().fg(TEXT).italic())
        } else {
            style
        };
        self.current.push(Span::styled(text.to_owned(), style));
    }

    fn ensure_quote_prefix(&mut self) {
        if self.current.is_empty() && self.quote_depth > 0 {
            self.current.push(Span::styled(
                "│ ".repeat(self.quote_depth),
                Style::new().fg(MUTED),
            ));
        }
    }

    fn separate_block(&mut self) {
        self.flush_line();
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(Line::default());
        }
    }

    fn flush_line(&mut self) {
        if !self.current.is_empty() {
            self.lines
                .push(Line::from(std::mem::take(&mut self.current)));
        }
    }

    fn current_style(&self) -> Style {
        self.styles
            .last()
            .copied()
            .unwrap_or_else(|| Style::new().fg(TEXT))
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(self.current_style().patch(style));
    }

    fn pop_style(&mut self) {
        self.styles.pop();
    }

    fn render_table(&mut self, table: &TableState) {
        if table.rows.is_empty() {
            return;
        }
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or_default();
        let natural_widths = (0..columns)
            .map(|column| {
                table
                    .rows
                    .iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| UnicodeWidthStr::width(cell.as_str()))
                    .max()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let widths = fit_table_widths(natural_widths, self.table_width);
        self.lines.push(table_border('┌', '┬', '┐', &widths));
        for (index, row) in table.rows.iter().enumerate() {
            let wrapped_cells = widths
                .iter()
                .enumerate()
                .map(|(column, width)| {
                    wrap_cell(row.get(column).map_or("", String::as_str), *width)
                })
                .collect::<Vec<_>>();
            let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
            for line_index in 0..row_height {
                let mut spans = vec![Span::styled("│ ", Style::new().fg(MUTED))];
                for (column, width) in widths.iter().enumerate().take(columns) {
                    let cell = wrapped_cells[column]
                        .get(line_index)
                        .map_or("", String::as_str);
                    let style = if index < table.header_rows {
                        Style::new().fg(BRIGHT).bold()
                    } else {
                        Style::new().fg(TEXT)
                    };
                    let alignment = table
                        .alignments
                        .get(column)
                        .copied()
                        .unwrap_or(Alignment::None);
                    spans.push(Span::styled(align_cell(cell, *width, alignment), style));
                    let divider = if column + 1 == columns {
                        " │"
                    } else {
                        " │ "
                    };
                    spans.push(Span::styled(divider, Style::new().fg(MUTED)));
                }
                self.lines.push(Line::from(spans));
            }
            if index + 1 == table.header_rows {
                self.lines.push(table_border('├', '┼', '┤', &widths));
            }
        }
        self.lines.push(table_border('└', '┴', '┘', &widths));
    }
}

fn code_block_info<'a>(kind: &'a CodeBlockKind<'a>) -> Option<&'a str> {
    match kind {
        CodeBlockKind::Indented => None,
        CodeBlockKind::Fenced(info) => Some(info.as_ref()),
    }
}

#[must_use]
pub(crate) fn code_block_label(info: Option<&str>) -> String {
    let Some(language) = info.and_then(|value| value.split_whitespace().next()) else {
        return "CODE".to_owned();
    };
    if matches!(
        language.to_ascii_lowercase().as_str(),
        "bash" | "sh" | "shell" | "zsh"
    ) {
        "BASH".to_owned()
    } else {
        format!("CODE · {}", language.to_uppercase())
    }
}

fn align_cell(cell: &str, width: usize, alignment: Alignment) -> String {
    let padding = width.saturating_sub(UnicodeWidthStr::width(cell));
    let (left, right) = match alignment {
        Alignment::Right => (padding, 0),
        Alignment::Center => (padding / 2, padding - (padding / 2)),
        Alignment::None | Alignment::Left => (0, padding),
    };
    format!("{}{cell}{}", " ".repeat(left), " ".repeat(right))
}

fn fit_table_widths(mut widths: Vec<usize>, max_width: Option<usize>) -> Vec<usize> {
    let Some(max_width) = max_width else {
        return widths;
    };
    let border_width = widths.len().saturating_mul(3).saturating_add(1);
    let available = max_width.saturating_sub(border_width);
    let natural_total = widths.iter().sum::<usize>();
    if natural_total <= available {
        return widths;
    }

    let preferred_floors = widths
        .iter()
        .map(|width| (*width).clamp(1, 3))
        .collect::<Vec<_>>();
    let floors = if preferred_floors.iter().sum::<usize>() <= available {
        preferred_floors
    } else {
        vec![1; widths.len()]
    };
    let mut total = natural_total;
    while total > available {
        let Some((column, _)) = widths
            .iter()
            .enumerate()
            .filter(|(column, width)| **width > floors[*column])
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[column] -= 1;
        total -= 1;
    }
    widths
}

fn wrap_cell(cell: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in cell.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        let gap = usize::from(!current.is_empty());
        if word_width <= width
            && UnicodeWidthStr::width(current.as_str()) + gap + word_width <= width
        {
            if gap == 1 {
                current.push(' ');
            }
            current.push_str(word);
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if word_width <= width {
            current.push_str(word);
            continue;
        }

        let mut chunk_width = 0;
        for grapheme in word.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if !current.is_empty() && chunk_width + grapheme_width > width {
                lines.push(std::mem::take(&mut current));
                chunk_width = 0;
            }
            current.push_str(grapheme);
            chunk_width += grapheme_width;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn heading_style(level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 => Style::new().fg(BRIGHT).bold().underlined(),
        HeadingLevel::H2 => Style::new().fg(BRIGHT).bold(),
        HeadingLevel::H3 => Style::new().fg(Color::Rgb(226, 226, 226)).bold(),
        HeadingLevel::H4 => Style::new().fg(TEXT),
        HeadingLevel::H5 | HeadingLevel::H6 => Style::new().fg(TEXT).italic(),
    }
}

fn table_border(left: char, middle: char, right: char, widths: &[usize]) -> Line<'static> {
    let mut border = String::new();
    border.push(left);
    for (index, width) in widths.iter().enumerate() {
        border.push_str(&"─".repeat(width + 2));
        border.push(if index + 1 == widths.len() {
            right
        } else {
            middle
        });
    }
    Line::from(Span::styled(border, Style::new().fg(MUTED)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &Text<'_>) -> String {
        text.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn preview_renders_markdown_semantics_instead_of_source_markers() {
        let source = "# Heading\n\nA **bold** [link](https://example.com).\n\n```rust\nlet x = 1;\n```\n\n- [x] done\n\n| Name | Role |\n|---|---|\n| Ada | Writer |";
        let rendered = render_markdown(source);
        let screen = plain(&rendered);

        assert!(screen.contains("Heading"));
        assert!(!screen.contains("# Heading"));
        assert!(screen.contains("A bold link."));
        assert!(!screen.contains("https://example.com"));
        assert!(screen.contains("let x = 1;"));
        assert!(!screen.contains("```"));
        assert!(screen.contains("• ☑ done"));
        assert!(screen.contains("│ Name │ Role   │"));
        assert!(screen.contains("│ Ada  │ Writer │"));
    }

    #[test]
    fn preview_aligns_table_columns_from_the_complete_table() {
        let rendered =
            render_markdown("| Name | Count | Note |\n| :--- | ---: | :---: |\n| Ada | 7 | Hi |");
        let screen = plain(&rendered);

        assert!(screen.contains("│ Name │ Count │ Note │"));
        assert!(screen.contains("├──────┼───────┼──────┤"));
        assert!(screen.contains("│ Ada  │     7 │  Hi  │"));
    }

    #[test]
    fn preview_wraps_wide_cells_inside_the_table_border() {
        let rendered = render_markdown_with_width(
            "| Key | Action |\n| --- | --- |\n| K | An action that wraps |",
            24,
        );
        let screen = plain(&rendered);

        assert_eq!(
            screen,
            "┌─────┬────────────────┐\n\
│ Key │ Action         │\n\
├─────┼────────────────┤\n\
│ K   │ An action that │\n\
│     │ wraps          │\n\
└─────┴────────────────┘"
        );
        assert!(
            screen
                .lines()
                .all(|line| UnicodeWidthStr::width(line) <= 24)
        );
    }

    #[test]
    fn preview_indents_nested_lists() {
        let rendered = render_markdown("- Parent\n  - Child\n    1. Grandchild");

        assert_eq!(plain(&rendered), "• Parent\n  • Child\n    1. Grandchild");
    }

    #[test]
    fn preview_labels_bash_and_other_code_blocks() {
        let rendered = render_markdown(
            "```bash\necho hello\n```\n\n```python\nprint('hello')\n```\n\n```\nplain\n```",
        );
        let screen = plain(&rendered);

        assert!(screen.contains("┌─ BASH\n│ echo hello\n└─"));
        assert!(screen.contains("┌─ CODE · PYTHON\n│ print('hello')\n└─"));
        assert!(screen.contains("┌─ CODE\n│ plain\n└─"));
    }
}
