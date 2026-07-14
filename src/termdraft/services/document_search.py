"""Literal find and replace helpers for one in-memory document."""

from __future__ import annotations

from bisect import bisect_right
from dataclasses import dataclass

import regex


@dataclass(frozen=True, slots=True)
class DocumentSearchMatch:
    """One non-overlapping match represented by source offsets."""

    start: int
    end: int


def find_document_matches(
    source: str,
    query: str,
    *,
    case_sensitive: bool = False,
) -> tuple[DocumentSearchMatch, ...]:
    """Return every literal source span for a non-empty query."""
    if not query:
        return ()
    flags = regex.VERSION1
    if not case_sensitive:
        flags |= regex.IGNORECASE | regex.FULLCASE
    pattern = regex.compile(regex.escape(query), flags)
    return tuple(
        DocumentSearchMatch(match.start(), match.end()) for match in pattern.finditer(source)
    )


def replace_document_matches(
    source: str,
    matches: tuple[DocumentSearchMatch, ...],
    replacement: str,
) -> str:
    """Replace an ordered set of non-overlapping matches in one pass."""
    if not matches:
        return source
    pieces: list[str] = []
    position = 0
    for match in matches:
        pieces.extend((source[position : match.start], replacement))
        position = match.end
    pieces.append(source[position:])
    return "".join(pieces)


def location_to_offset(source: str, location: tuple[int, int]) -> int:
    """Convert a TextArea-style row and column into a bounded source offset."""
    starts, ends = _line_boundaries(source)
    row = min(max(location[0], 0), len(starts) - 1)
    column = min(max(location[1], 0), ends[row] - starts[row])
    return starts[row] + column


def offset_to_location(source: str, offset: int) -> tuple[int, int]:
    """Convert a bounded source offset into a TextArea-style row and column."""
    starts, ends = _line_boundaries(source)
    bounded = min(max(offset, 0), len(source))
    row = bisect_right(starts, bounded) - 1
    return row, min(bounded, ends[row]) - starts[row]


def _line_boundaries(source: str) -> tuple[list[int], list[int]]:
    starts = [0]
    ends: list[int] = []
    for ending in regex.finditer(r"\r\n|\r|\n", source):
        ends.append(ending.start())
        starts.append(ending.end())
    ends.append(len(source))
    return starts, ends
