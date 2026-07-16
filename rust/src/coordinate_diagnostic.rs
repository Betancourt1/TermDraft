//! Read-only conversion between source, logical cursor, and wrapped terminal coordinates.

use std::ops::Range;

use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The same cursor expressed in source scalars, UTF-8 bytes, logical text, and terminal cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoordinateDiagnostic {
    /// Unicode-scalar offset from the start of the complete source.
    pub source_offset: usize,
    pub utf8_byte_offset: usize,
    pub logical_line: usize,
    /// Unicode-scalar offset within the logical line.
    pub logical_column: usize,
    pub visual_row: usize,
    pub visual_cell: usize,
    pub grapheme_boundary: bool,
    pub wrap_splits_grapheme: bool,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CoordinateError {
    #[error("tab width must be positive")]
    InvalidTabWidth,
    #[error("logical line {line} is outside the {line_count}-line source")]
    LineOutside { line: usize, line_count: usize },
    #[error("logical column {column} is outside a line containing {line_length} characters")]
    ColumnOutside { column: usize, line_length: usize },
}

#[derive(Clone, Copy, Debug)]
struct LogicalLine {
    start_byte: usize,
    end_byte: usize,
    start_character: usize,
}

/// Diagnose a cursor whose line and column use zero-based Unicode-scalar coordinates.
///
/// A wrap width of zero disables wrapping. Tabs advance to the next `tab_width` cell stop.
///
/// # Errors
///
/// Returns [`CoordinateError`] when the tab width is zero or the logical location is outside the
/// exact source.
pub fn diagnose_coordinate(
    source: &str,
    location: (usize, usize),
    wrap_width: usize,
    tab_width: usize,
) -> Result<CoordinateDiagnostic, CoordinateError> {
    if tab_width == 0 {
        return Err(CoordinateError::InvalidTabWidth);
    }

    let (line_index, column) = location;
    let lines = logical_lines(source);
    let line = lines.get(line_index).ok_or(CoordinateError::LineOutside {
        line: line_index,
        line_count: lines.len(),
    })?;
    let line_source = &source[line.start_byte..line.end_byte];
    let line_length = line_source.chars().count();
    if column > line_length {
        return Err(CoordinateError::ColumnOutside {
            column,
            line_length,
        });
    }

    let byte_in_line = byte_at_character(line_source, column);
    let utf8_byte_offset = line.start_byte + byte_in_line;
    let boundaries = grapheme_boundaries(line_source);
    let wrap_offsets = compute_wrap_offsets(line_source, wrap_width, tab_width);
    let section_index = wrap_offsets.partition_point(|offset| *offset <= column);
    let section_start = section_index
        .checked_sub(1)
        .map_or(0, |index| wrap_offsets[index]);
    let section_source = character_slice(line_source, section_start..column);
    let rows_before = lines[..line_index]
        .iter()
        .map(|logical| {
            let source_line = &source[logical.start_byte..logical.end_byte];
            compute_wrap_offsets(source_line, wrap_width, tab_width).len() + 1
        })
        .sum::<usize>();

    Ok(CoordinateDiagnostic {
        source_offset: line.start_character + column,
        utf8_byte_offset,
        logical_line: line_index,
        logical_column: column,
        visual_row: rows_before + section_index,
        visual_cell: expanded_cell_width(section_source, tab_width),
        grapheme_boundary: boundaries.binary_search(&column).is_ok(),
        wrap_splits_grapheme: wrap_offsets
            .iter()
            .any(|offset| boundaries.binary_search(offset).is_err()),
    })
}

fn logical_lines(source: &str) -> Vec<LogicalLine> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut byte = 0;
    let mut character = 0;
    let mut line_start_byte = 0;
    let mut line_start_character = 0;

    while byte < bytes.len() {
        let value = source[byte..]
            .chars()
            .next()
            .expect("byte offset stays on a character boundary");
        if matches!(value, '\r' | '\n') {
            lines.push(LogicalLine {
                start_byte: line_start_byte,
                end_byte: byte,
                start_character: line_start_character,
            });
            if value == '\r' && bytes.get(byte + 1) == Some(&b'\n') {
                byte += 2;
                character += 2;
            } else {
                byte += 1;
                character += 1;
            }
            line_start_byte = byte;
            line_start_character = character;
        } else {
            byte += value.len_utf8();
            character += 1;
        }
    }
    lines.push(LogicalLine {
        start_byte: line_start_byte,
        end_byte: source.len(),
        start_character: line_start_character,
    });
    lines
}

fn grapheme_boundaries(source: &str) -> Vec<usize> {
    let mut boundaries = vec![0];
    let mut character = 0;
    for grapheme in source.graphemes(true) {
        character += grapheme.chars().count();
        boundaries.push(character);
    }
    boundaries
}

fn compute_wrap_offsets(source: &str, width: usize, tab_width: usize) -> Vec<usize> {
    if width == 0 || source.is_empty() {
        return Vec::new();
    }

    let characters = source.chars().collect::<Vec<_>>();
    let boundaries = character_byte_boundaries(source);
    let tab_widths = tab_widths(source, &characters, &boundaries, tab_width);
    let mut offsets = Vec::new();
    let mut cell_offset = 0;

    for chunk in chunks(&characters) {
        let chunk_source = &source[boundaries[chunk.start]..boundaries[chunk.end]];
        let chunk_width = chunk_source
            .split('\t')
            .map(UnicodeWidthStr::width)
            .sum::<usize>()
            + tab_widths[chunk.clone()].iter().sum::<usize>();
        let remaining = width.saturating_sub(cell_offset);
        if chunk_width <= remaining {
            cell_offset += chunk_width;
        } else if chunk_width > width {
            let folded = folded_line_lengths(&characters, &tab_widths, chunk.clone(), width);
            let mut start = chunk.start;
            for (index, length) in folded.iter().copied().enumerate() {
                if start != 0 {
                    offsets.push(start);
                }
                if index + 1 == folded.len() {
                    let end = start + length;
                    let line = &source[boundaries[start]..boundaries[end]];
                    cell_offset = line.split('\t').map(UnicodeWidthStr::width).sum::<usize>()
                        + tab_widths[start..end].iter().sum::<usize>();
                } else {
                    start += length;
                }
            }
        } else if cell_offset != 0 && chunk.start != 0 {
            offsets.push(chunk.start);
            cell_offset = chunk_width;
        }
    }
    offsets
}

fn chunks(characters: &[char]) -> Vec<Range<usize>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        let mut end = start;
        if characters[start].is_whitespace() {
            while end < characters.len() && characters[end].is_whitespace() {
                end += 1;
            }
        } else {
            while end < characters.len() && !characters[end].is_whitespace() {
                end += 1;
            }
            while end < characters.len() && characters[end].is_whitespace() {
                end += 1;
            }
        }
        chunks.push(start..end);
        start = end;
    }
    chunks
}

fn folded_line_lengths(
    characters: &[char],
    tab_widths: &[usize],
    range: Range<usize>,
    width: usize,
) -> Vec<usize> {
    let mut lengths = vec![0];
    let mut total_width = 0;
    for index in range {
        let cell_width = if characters[index] == '\t' {
            tab_widths[index]
        } else {
            characters[index].width().unwrap_or(0)
        };
        if total_width + cell_width > width {
            lengths.push(1);
            total_width = cell_width;
        } else {
            *lengths
                .last_mut()
                .expect("folded lines always contain one line") += 1;
            total_width += cell_width;
        }
    }
    lengths
}

fn tab_widths(
    source: &str,
    characters: &[char],
    boundaries: &[usize],
    tab_width: usize,
) -> Vec<usize> {
    let mut widths = vec![0; characters.len()];
    let mut cell_position = 0;
    let mut segment_start = 0;
    for (index, value) in characters.iter().enumerate() {
        if *value != '\t' {
            continue;
        }
        cell_position += source[boundaries[segment_start]..boundaries[index]].width();
        let expansion = tab_width - cell_position % tab_width;
        widths[index] = expansion;
        cell_position += expansion;
        segment_start = index + 1;
    }
    widths
}

fn expanded_cell_width(source: &str, tab_width: usize) -> usize {
    let mut width = 0;
    let mut sections = source.split('\t').peekable();
    while let Some(section) = sections.next() {
        width += section.width();
        if sections.peek().is_some() {
            width += tab_width - width % tab_width;
        }
    }
    width
}

fn character_byte_boundaries(source: &str) -> Vec<usize> {
    source
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(source.len()))
        .collect()
}

fn byte_at_character(source: &str, character: usize) -> usize {
    source
        .char_indices()
        .map(|(byte, _)| byte)
        .nth(character)
        .unwrap_or(source.len())
}

fn character_slice(source: &str, range: Range<usize>) -> &str {
    let start = byte_at_character(source, range.start);
    let end = byte_at_character(source, range.end);
    &source[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_exact_line_endings_and_utf8_offsets() {
        for (source, location, expected) in [
            ("a\nb", (1, 0), 2),
            ("a\rb", (1, 0), 2),
            ("a\r\nb", (1, 0), 3),
            ("a\r\nb\nc\rd", (3, 1), 8),
            ("a\r\n", (1, 0), 3),
            ("", (0, 0), 0),
        ] {
            let diagnostic = diagnose_coordinate(source, location, 20, 4).unwrap();
            assert_eq!(diagnostic.source_offset, expected);
        }

        let diagnostic = diagnose_coordinate("a🙂", (0, 2), 20, 4).unwrap();
        assert_eq!(diagnostic.source_offset, 2);
        assert_eq!(diagnostic.utf8_byte_offset, 5);
    }

    #[test]
    fn matches_wrapped_terminal_cell_coordinates() {
        for (source, location, width, expected) in [
            ("abcdefgh", (0, 6), 4, (1, 2)),
            ("a\tb", (0, 2), 8, (0, 4)),
            ("a\tb", (0, 2), 4, (1, 0)),
            ("界界", (0, 1), 2, (1, 0)),
            ("🙂🙂", (0, 1), 2, (1, 0)),
            ("👨‍👩‍👧‍👦x", (0, 7), 4, (0, 2)),
        ] {
            let diagnostic = diagnose_coordinate(source, location, width, 4).unwrap();
            assert_eq!(
                (diagnostic.visual_row, diagnostic.visual_cell),
                expected,
                "{source:?} at {location:?}"
            );
        }
    }

    #[test]
    fn distinguishes_grapheme_boundaries_from_scalar_columns() {
        let inside = diagnose_coordinate("e\u{301}x", (0, 1), 4, 4).unwrap();
        let boundary = diagnose_coordinate("e\u{301}x", (0, 2), 4, 4).unwrap();
        assert_eq!(inside.visual_cell, 1);
        assert_eq!(boundary.visual_cell, 1);
        assert!(!inside.grapheme_boundary);
        assert!(boundary.grapheme_boundary);

        let family = diagnose_coordinate("👨‍👩‍👧‍👦x", (0, 7), 2, 4).unwrap();
        assert_eq!((family.visual_row, family.visual_cell), (4, 0));
        assert!(family.grapheme_boundary);
        assert!(family.wrap_splits_grapheme);
    }

    #[test]
    fn rejects_invalid_locations_and_tab_width() {
        assert_eq!(
            diagnose_coordinate("", (1, 0), 1, 4),
            Err(CoordinateError::LineOutside {
                line: 1,
                line_count: 1
            })
        );
        assert_eq!(
            diagnose_coordinate("x", (0, 2), 1, 4),
            Err(CoordinateError::ColumnOutside {
                column: 2,
                line_length: 1
            })
        );
        assert_eq!(
            diagnose_coordinate("x", (0, 0), 1, 0),
            Err(CoordinateError::InvalidTabWidth)
        );
    }
}
