"""Read-only source and wrapped-editor coordinate diagnostics."""

from __future__ import annotations

import re
from dataclasses import dataclass

import regex
from textual.widgets.text_area import Document, WrappedDocument

_LINE_ENDING = re.compile(r"\r\n|\r|\n")


@dataclass(frozen=True, slots=True)
class CoordinateDiagnostic:
    """The same cursor described in source, logical, and visual units."""

    source_offset: int
    utf8_byte_offset: int
    logical_line: int
    logical_column: int
    visual_row: int
    visual_cell: int
    grapheme_boundary: bool
    wrap_splits_grapheme: bool


def diagnose_coordinate(
    source: str,
    location: tuple[int, int],
    *,
    wrap_width: int,
    tab_width: int = 4,
) -> CoordinateDiagnostic:
    """Describe a valid TextArea cursor without normalizing the source."""
    if wrap_width < 0:
        raise ValueError("wrap_width cannot be negative")
    if tab_width <= 0:
        raise ValueError("tab_width must be positive")

    line, column = location
    starts, ends = _logical_lines(source)
    if line < 0 or line >= len(starts):
        raise ValueError("logical line is outside the source")
    if column < 0 or column > ends[line] - starts[line]:
        raise ValueError("logical column is outside the source line")

    source_offset = starts[line] + column
    line_source = source[starts[line] : ends[line]]
    boundaries = _grapheme_boundaries(line_source)
    wrapped = WrappedDocument(Document(source), width=wrap_width, tab_width=tab_width)
    visual = wrapped.location_to_offset(location)

    return CoordinateDiagnostic(
        source_offset=source_offset,
        utf8_byte_offset=len(source[:source_offset].encode("utf-8")),
        logical_line=line,
        logical_column=column,
        visual_row=visual.y,
        visual_cell=visual.x,
        grapheme_boundary=column in boundaries,
        wrap_splits_grapheme=any(
            wrap_offset not in boundaries for wrap_offset in wrapped.get_offsets(line)
        ),
    )


def _logical_lines(source: str) -> tuple[tuple[int, ...], tuple[int, ...]]:
    starts = [0]
    ends: list[int] = []
    for match in _LINE_ENDING.finditer(source):
        ends.append(match.start())
        starts.append(match.end())
    ends.append(len(source))
    return tuple(starts), tuple(ends)


def _grapheme_boundaries(source_line: str) -> frozenset[int]:
    boundaries = {0}
    boundaries.update(match.end() for match in regex.finditer(r"\X", source_line))
    return frozenset(boundaries)
