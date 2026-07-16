"""Single-line Markdown presentation for the inline editor view."""

from __future__ import annotations

import re
from collections.abc import Sequence
from typing import Literal

from rich.style import Style
from rich.text import Text

_HEADING = re.compile(r"^( {0,3})(#{1,6})([ \t]+|$)")
_QUOTE = re.compile(r"^( {0,3})>([ \t]?)")
_UNORDERED_LIST = re.compile(r"^(\s*)([-+*])([ \t]+)")
_TASK = re.compile(r"^(\s*(?:[-+*]|\d+[.)])[ \t]+)\[([ xX])\]")
_THEMATIC_BREAK = re.compile(r"^\s{0,3}(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$")
_FENCE = re.compile(r"^( {0,3})(`{3,}|~{3,})")
_IMAGE = re.compile(r"!\[([^]\n]*)\]\(([^)\n]+)\)")
_LINK = re.compile(r"(?<!!)\[([^]\n]+)\]\(([^)\n]+)\)")
_CODE = re.compile(r"(`+)(.+?)\1")
_STRONG = re.compile(r"(\*\*|__)(.+?)\1")
_STRIKE = re.compile(r"(~~)(.+?)\1")
_EMPHASIS = re.compile(r"(?<!\*)\*([^*\n]+)\*(?!\*)|(?<!_)_([^_\n]+)_(?!_)")
_TABLE_SEPARATOR = re.compile(r"^ {0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)+\|?[ \t]*$")

TableLineKind = Literal["header", "separator", "body"]

_HEADING_PRESENTATION = (
    ("|", Style(bold=True)),
    ("|", Style(bold=True)),
    ("|", Style()),
    (".", Style()),
    (".", Style(italic=True)),
    (".", Style(italic=True)),
)


def table_line_kind(lines: Sequence[str], line_index: int) -> TableLineKind | None:
    """Identify lines that belong to one ordinary GFM table."""
    source = lines[line_index]

    if _is_table_header(lines, line_index):
        return "header"
    if _is_table_separator(source) and _is_table_header(lines, line_index - 1):
        return "separator"
    if not _looks_like_table_row(source):
        return None

    for preceding_index in range(line_index - 1, 0, -1):
        preceding = lines[preceding_index]
        if _is_table_separator(preceding):
            return "body" if _is_table_header(lines, preceding_index - 1) else None
        if not _looks_like_table_row(preceding):
            break
    return None


def render_inline_preview_line(
    source: str,
    *,
    table_line: TableLineKind | None = None,
) -> Text:
    """Render one inactive source line without changing its character positions."""
    if table_line == "separator":
        return _render_table_separator(source)

    characters = list(source)
    styles: list[tuple[Style, int, int]] = []

    thematic_break = _THEMATIC_BREAK.fullmatch(source)
    if thematic_break is not None:
        for index, character in enumerate(characters):
            if not character.isspace():
                characters[index] = "─"
        return Text("".join(characters), style="dim", end="", no_wrap=True)

    if match := _HEADING.match(source):
        marker, style = _HEADING_PRESENTATION[len(match.group(2)) - 1]
        _blank(characters, match.start(2), match.end(3))
        marker_position = match.end(2) - 1
        characters[marker_position] = marker
        styles.append((style, marker_position, len(source)))
    elif match := _QUOTE.match(source):
        characters[match.start(0) + len(match.group(1))] = "│"
        styles.append((Style(italic=True, dim=True), match.end(0), len(source)))
    elif match := _UNORDERED_LIST.match(source):
        characters[match.start(2)] = "•"

    if match := _TASK.match(source):
        marker_start = match.start(2) - 1
        marker = "☑" if match.group(2).casefold() == "x" else "☐"
        characters[marker_start : marker_start + 3] = [marker, " ", " "]

    if match := _FENCE.match(source):
        _blank(characters, match.start(2), match.end(2))
        styles.append((Style(dim=True), match.end(2), len(source)))

    protected: list[tuple[int, int]] = []
    for match in _CODE.finditer(source):
        _blank(characters, match.start(1), match.end(1))
        _blank(characters, match.end(2), match.end(0))
        styles.append((Style(reverse=True), match.start(2), match.end(2)))
        protected.append(match.span())

    for pattern, style in ((_IMAGE, Style(italic=True)), (_LINK, Style(underline=True))):
        for match in pattern.finditer(source):
            if _overlaps(match.span(), protected):
                continue
            label_start, label_end = match.span(1)
            _blank(characters, match.start(0), label_start)
            _blank(characters, label_end, match.end(0))
            styles.append((style, label_start, label_end))
            protected.append(match.span())

    for pattern, style in (
        (_STRONG, Style(bold=True)),
        (_STRIKE, Style(strike=True)),
    ):
        for match in pattern.finditer(source):
            if _overlaps(match.span(), protected):
                continue
            _blank(characters, match.start(1), match.end(1))
            _blank(characters, match.end(2), match.end(0))
            styles.append((style, match.start(2), match.end(2)))

    for match in _EMPHASIS.finditer(source):
        if _overlaps(match.span(), protected):
            continue
        content_group = 1 if match.group(1) is not None else 2
        content_start, content_end = match.span(content_group)
        _blank(characters, match.start(0), content_start)
        _blank(characters, content_end, match.end(0))
        styles.append((Style(italic=True), content_start, content_end))

    if table_line in {"header", "body"}:
        for position in _table_pipe_positions(source):
            characters[position] = "│"
            styles.append((Style(dim=True), position, position + 1))
        if table_line == "header":
            styles.append((Style(bold=True), 0, len(source)))

    rendered = Text("".join(characters), end="", no_wrap=True)
    for style, start, end in styles:
        rendered.stylize(style, start, end)
    return rendered


def _blank(characters: list[str], start: int, end: int) -> None:
    characters[start:end] = [" "] * (end - start)


def _overlaps(candidate: tuple[int, int], ranges: list[tuple[int, int]]) -> bool:
    start, end = candidate
    return any(start < other_end and other_start < end for other_start, other_end in ranges)


def _is_table_header(lines: Sequence[str], line_index: int) -> bool:
    if not 0 <= line_index < len(lines) - 1:
        return False
    header = lines[line_index]
    separator = lines[line_index + 1]
    return (
        _looks_like_table_row(header)
        and _is_table_separator(separator)
        and _table_cell_count(header) == _table_cell_count(separator)
    )


def _is_table_separator(source: str) -> bool:
    return _TABLE_SEPARATOR.fullmatch(source) is not None


def _looks_like_table_row(source: str) -> bool:
    return not source.startswith("    ") and bool(_table_pipe_positions(source))


def _table_cell_count(source: str) -> int:
    positions = _table_pipe_positions(source)
    stripped = source.strip()
    return len(positions) + 1 - stripped.startswith("|") - stripped.endswith("|")


def _table_pipe_positions(source: str) -> tuple[int, ...]:
    return tuple(
        index
        for index, character in enumerate(source)
        if character == "|" and (index == 0 or source[index - 1] != "\\")
    )


def _render_table_separator(source: str) -> Text:
    characters = list(source)
    positions = _table_pipe_positions(source)
    non_space = [index for index, character in enumerate(source) if not character.isspace()]
    if not non_space:
        return Text(source, end="", no_wrap=True)

    start, end = non_space[0], non_space[-1]
    pipes = set(positions)
    for index in range(start, end + 1):
        characters[index] = "┼" if index in pipes else "─"
    if positions and positions[0] == start:
        characters[start] = "├"
    if positions and positions[-1] == end:
        characters[end] = "┤"
    return Text("".join(characters), style="dim", end="", no_wrap=True)
