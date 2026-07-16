//! Conservative, read-only Markdown source ranges for inspection and block reading.

use std::ops::Range;

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};

/// A top-level source family recognized by the semantic inspector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticBlockKind {
    Heading,
    Paragraph,
    BulletList,
    OrderedList,
    Quote,
    Alert,
    Table,
    DefinitionList,
    FencedCode,
    IndentedCode,
    ThematicBreak,
    FootnoteDefinition,
    LinkReferenceDefinition,
    Separator,
    UnmappedSource,
}

impl SemanticBlockKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Paragraph => "paragraph",
            Self::BulletList => "bullet list",
            Self::OrderedList => "ordered list",
            Self::Quote => "quote",
            Self::Alert => "alert",
            Self::Table => "table",
            Self::DefinitionList => "definition list",
            Self::FencedCode => "fenced code",
            Self::IndentedCode => "indented code",
            Self::ThematicBreak => "thematic break",
            Self::FootnoteDefinition => "footnote definition",
            Self::LinkReferenceDefinition => "link reference definition",
            Self::Separator => "separator",
            Self::UnmappedSource => "unmapped source",
        }
    }

    #[must_use]
    pub const fn is_gap(self) -> bool {
        matches!(self, Self::Separator | Self::UnmappedSource)
    }
}

/// How the experimental reader presents a safe source segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReaderPresentation {
    Rendered,
    SourceFallback,
}

/// One exact source slice. Line and scalar ranges are zero-based and end-exclusive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticBlock {
    pub kind: SemanticBlockKind,
    pub start_line: usize,
    pub end_line: usize,
    pub start_character: usize,
    pub end_character: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub source: String,
    pub detail: Option<String>,
}

impl SemanticBlock {
    /// Separators are hidden; simple prose renders and every other family keeps exact source.
    #[must_use]
    pub const fn reader_presentation(&self) -> Option<ReaderPresentation> {
        match self.kind {
            SemanticBlockKind::Separator => None,
            SemanticBlockKind::Heading | SemanticBlockKind::Paragraph => {
                Some(ReaderPresentation::Rendered)
            }
            _ => Some(ReaderPresentation::SourceFallback),
        }
    }
}

/// Mapped parser blocks plus every exact source gap between them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SemanticBlockMap {
    pub blocks: Vec<SemanticBlock>,
    pub gaps: Vec<SemanticBlock>,
}

impl SemanticBlockMap {
    /// Blocks and gaps in source order, as used by the inspector.
    #[must_use]
    pub fn segments(&self) -> Vec<&SemanticBlock> {
        let mut segments = self.blocks.iter().chain(&self.gaps).collect::<Vec<_>>();
        segments.sort_by_key(|block| block.start_byte);
        segments
    }

    /// Visible experimental-reader segments and their safe presentation mode.
    pub fn reader_segments(
        &self,
    ) -> impl Iterator<Item = (&SemanticBlock, ReaderPresentation)> + '_ {
        self.segments().into_iter().filter_map(|block| {
            block
                .reader_presentation()
                .map(|presentation| (block, presentation))
        })
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    kind: SemanticBlockKind,
    range: Range<usize>,
    detail: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct LineOffset {
    byte: usize,
    character: usize,
}

/// Map supported top-level Markdown blocks without changing the source.
#[must_use]
pub fn map_semantic_blocks(source: &str) -> SemanticBlockMap {
    if source.is_empty() {
        return SemanticBlockMap::default();
    }

    let offsets = line_offsets(source);
    let options = Options::ENABLE_GFM
        | Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_DEFINITION_LIST;
    let parser = Parser::new_ext(source, options);
    let mut candidates = parser
        .reference_definitions()
        .iter()
        .map(|(label, definition)| Candidate {
            kind: SemanticBlockKind::LinkReferenceDefinition,
            range: definition.span.clone(),
            detail: Some(format!("[{}]", label.to_uppercase())),
        })
        .collect::<Vec<_>>();

    let mut depth = 0_usize;
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0
                    && let Some((kind, detail)) = block_from_tag(&tag)
                {
                    candidates.push(Candidate {
                        kind,
                        range,
                        detail,
                    });
                }
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Rule if depth == 0 => candidates.push(Candidate {
                kind: SemanticBlockKind::ThematicBreak,
                range,
                detail: None,
            }),
            _ => {}
        }
    }

    candidates.sort_by(|left, right| {
        left.range
            .start
            .cmp(&right.range.start)
            .then_with(|| right.range.end.cmp(&left.range.end))
    });

    let mut blocks = Vec::new();
    let mut gaps = Vec::new();
    let mut cursor_byte = 0;
    let mut cursor_line = 0;
    for candidate in candidates {
        let (start_line, end_line) = line_range(&offsets, candidate.range);
        let start_byte = offsets[start_line].byte;
        let end_byte = offsets[end_line].byte;
        if end_byte <= start_byte || start_byte < cursor_byte {
            continue;
        }
        if cursor_byte < start_byte {
            gaps.push(source_block(
                gap_kind(&source[cursor_byte..start_byte]),
                cursor_line,
                start_line,
                &offsets,
                source,
                None,
            ));
        }
        blocks.push(source_block(
            candidate.kind,
            start_line,
            end_line,
            &offsets,
            source,
            candidate.detail,
        ));
        cursor_byte = end_byte;
        cursor_line = end_line;
    }
    if cursor_byte < source.len() {
        gaps.push(source_block(
            gap_kind(&source[cursor_byte..]),
            cursor_line,
            offsets.len() - 1,
            &offsets,
            source,
            None,
        ));
    }

    SemanticBlockMap { blocks, gaps }
}

fn block_from_tag(tag: &Tag<'_>) -> Option<(SemanticBlockKind, Option<String>)> {
    let block = match tag {
        Tag::Heading { level, .. } => (
            SemanticBlockKind::Heading,
            Some(heading_detail(*level).to_owned()),
        ),
        Tag::Paragraph | Tag::HtmlBlock => (SemanticBlockKind::Paragraph, None),
        Tag::BlockQuote(Some(kind)) => (
            SemanticBlockKind::Alert,
            Some(alert_detail(*kind).to_owned()),
        ),
        Tag::BlockQuote(None) => (SemanticBlockKind::Quote, None),
        Tag::List(Some(start)) => (
            SemanticBlockKind::OrderedList,
            (*start != 1).then(|| format!("starts at {start}")),
        ),
        Tag::List(None) => (SemanticBlockKind::BulletList, None),
        Tag::CodeBlock(CodeBlockKind::Fenced(info)) => (
            SemanticBlockKind::FencedCode,
            (!info.trim().is_empty()).then(|| info.trim().to_owned()),
        ),
        Tag::CodeBlock(CodeBlockKind::Indented) => (SemanticBlockKind::IndentedCode, None),
        Tag::Table(_) => (SemanticBlockKind::Table, None),
        Tag::DefinitionList => (SemanticBlockKind::DefinitionList, None),
        Tag::FootnoteDefinition(_) => (SemanticBlockKind::FootnoteDefinition, None),
        _ => return None,
    };
    Some(block)
}

const fn heading_detail(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "H1",
        HeadingLevel::H2 => "H2",
        HeadingLevel::H3 => "H3",
        HeadingLevel::H4 => "H4",
        HeadingLevel::H5 => "H5",
        HeadingLevel::H6 => "H6",
    }
}

const fn alert_detail(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "NOTE",
        BlockQuoteKind::Tip => "TIP",
        BlockQuoteKind::Important => "IMPORTANT",
        BlockQuoteKind::Warning => "WARNING",
        BlockQuoteKind::Caution => "CAUTION",
    }
}

fn gap_kind(source: &str) -> SemanticBlockKind {
    if source.trim().is_empty() {
        SemanticBlockKind::Separator
    } else {
        SemanticBlockKind::UnmappedSource
    }
}

fn source_block(
    kind: SemanticBlockKind,
    start_line: usize,
    end_line: usize,
    offsets: &[LineOffset],
    source: &str,
    detail: Option<String>,
) -> SemanticBlock {
    let start = offsets[start_line];
    let end = offsets[end_line];
    SemanticBlock {
        kind,
        start_line,
        end_line,
        start_character: start.character,
        end_character: end.character,
        start_byte: start.byte,
        end_byte: end.byte,
        source: source[start.byte..end.byte].to_owned(),
        detail,
    }
}

fn line_range(offsets: &[LineOffset], range: Range<usize>) -> (usize, usize) {
    let start_line = offsets
        .partition_point(|offset| offset.byte <= range.start)
        .saturating_sub(1);
    let end_line = offsets
        .partition_point(|offset| offset.byte < range.end)
        .min(offsets.len() - 1);
    (
        start_line,
        end_line.max(start_line + 1).min(offsets.len() - 1),
    )
}

fn line_offsets(source: &str) -> Vec<LineOffset> {
    let mut offsets = vec![LineOffset {
        byte: 0,
        character: 0,
    }];
    let bytes = source.as_bytes();
    let mut byte = 0;
    let mut character = 0;
    while byte < bytes.len() {
        let value = source[byte..]
            .chars()
            .next()
            .expect("byte offset stays on a character boundary");
        if value == '\r' && bytes.get(byte + 1) == Some(&b'\n') {
            byte += 2;
            character += 2;
            offsets.push(LineOffset { byte, character });
        } else {
            byte += value.len_utf8();
            character += 1;
            if matches!(value, '\r' | '\n') {
                offsets.push(LineOffset { byte, character });
            }
        }
    }
    if offsets
        .last()
        .is_some_and(|offset| offset.byte != source.len())
    {
        offsets.push(LineOffset { byte, character });
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_top_level_blocks_and_reader_modes() {
        let source = concat!(
            "# Heading\n\n",
            "Paragraph.\n\n",
            "- item\n  - nested\n\n",
            "> [!NOTE]\n> alert\n\n",
            "| A | B |\n|---|---|\n| 1 | 2 |\n\n",
            "Term\n: Definition\n\n",
            "```python\nprint(1)\n```\n\n",
            "    indented\n\n",
            "---\n\n",
            "Text[^note]\n\n",
            "[^note]: Footnote\n",
        );
        let mapping = map_semantic_blocks(source);

        assert_eq!(
            mapping
                .blocks
                .iter()
                .map(|block| block.kind)
                .collect::<Vec<_>>(),
            vec![
                SemanticBlockKind::Heading,
                SemanticBlockKind::Paragraph,
                SemanticBlockKind::BulletList,
                SemanticBlockKind::Alert,
                SemanticBlockKind::Table,
                SemanticBlockKind::DefinitionList,
                SemanticBlockKind::FencedCode,
                SemanticBlockKind::IndentedCode,
                SemanticBlockKind::ThematicBreak,
                SemanticBlockKind::Paragraph,
                SemanticBlockKind::FootnoteDefinition,
            ]
        );
        assert_eq!(mapping.blocks[0].detail.as_deref(), Some("H1"));
        assert_eq!(mapping.blocks[3].detail.as_deref(), Some("NOTE"));
        assert_eq!(mapping.blocks[6].detail.as_deref(), Some("python"));

        let modes = mapping
            .reader_segments()
            .map(|(block, mode)| (block.kind, mode))
            .collect::<Vec<_>>();
        assert!(modes.iter().all(|(kind, mode)| {
            *mode
                == if matches!(
                    kind,
                    SemanticBlockKind::Heading | SemanticBlockKind::Paragraph
                ) {
                    ReaderPresentation::Rendered
                } else {
                    ReaderPresentation::SourceFallback
                }
        }));
    }

    #[test]
    fn ranges_are_lossless_for_unicode_and_mixed_line_endings() {
        let source = "# Café ☕\r\n\rParagraph 日本語\n";
        let mapping = map_semantic_blocks(source);
        let segments = mapping.segments();

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.source.as_str())
                .collect::<String>(),
            source
        );
        for segment in segments {
            assert_eq!(
                &source[segment.start_byte..segment.end_byte],
                segment.source
            );
            assert_eq!(
                source[..segment.start_byte].chars().count(),
                segment.start_character
            );
            assert_eq!(
                source[..segment.end_byte].chars().count(),
                segment.end_character
            );
        }
    }

    #[test]
    fn nested_containers_keep_one_outer_source_range() {
        for (source, expected) in [
            (
                "> quote\n> - item\n>   - nested\n",
                SemanticBlockKind::Quote,
            ),
            (
                "- item\n  > quote\n  > continuation\n",
                SemanticBlockKind::BulletList,
            ),
        ] {
            let mapping = map_semantic_blocks(source);
            assert_eq!(mapping.blocks.len(), 1);
            assert_eq!(mapping.blocks[0].kind, expected);
            assert_eq!(mapping.blocks[0].source, source);
        }
    }

    #[test]
    fn reference_definitions_remain_exact_source_fallbacks() {
        let source = "Text [reference].\r\n\r\n[reference]: https://example.com\r\n";
        let mapping = map_semantic_blocks(source);
        let reference = mapping
            .blocks
            .iter()
            .find(|block| block.kind == SemanticBlockKind::LinkReferenceDefinition)
            .expect("reference definition is mapped");

        assert_eq!(reference.source, "[reference]: https://example.com\r\n");
        assert_eq!(reference.detail.as_deref(), Some("[REFERENCE]"));
        assert_eq!(
            reference.reader_presentation(),
            Some(ReaderPresentation::SourceFallback)
        );
    }

    #[test]
    fn unterminated_fence_stays_inside_source() {
        let source = "```python\nprint('open')";
        let mapping = map_semantic_blocks(source);

        assert_eq!(mapping.blocks.len(), 1);
        assert_eq!(mapping.blocks[0].kind, SemanticBlockKind::FencedCode);
        assert_eq!(mapping.blocks[0].end_byte, source.len());
        assert_eq!(mapping.blocks[0].source, source);
    }
}
