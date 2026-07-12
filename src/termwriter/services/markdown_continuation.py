"""Pure Markdown continuation rules for the editor's Enter key."""

from __future__ import annotations

import re
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class MarkdownContinuationEdit:
    """A source range replacement and the cursor position after applying it."""

    start_line: int
    start_column: int
    end_line: int
    end_column: int
    text: str
    cursor_line: int
    cursor_column: int


@dataclass(frozen=True, slots=True)
class _LogicalLine:
    text: str
    ending: str


_LINE_ENDING = re.compile(r"\r\n|\n|\r")
_LIST_PREFIX = re.compile(
    r"^(?P<context>[ \t]*(?:>[ \t]*)*)"
    r"(?:(?P<unordered>[-+*])|(?P<number>\d+)(?P<delimiter>[.)]))"
    r"(?P<gap>[ \t]+)"
)
_QUOTE_PREFIX = re.compile(r"^(?P<prefix>[ \t]*(?:>[ \t]*)+)")
_TASK_PREFIX = re.compile(r"(?P<marker>\[[ xX]\])(?P<gap>[ \t]*)(?P<body>.*)$")
_FENCE = re.compile(r"^ {0,3}(?P<fence>`{3,}|~{3,})(?P<rest>.*)$")
_THEMATIC_BREAK = re.compile(r"^ {0,3}(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$")


def continuation_edit(
    source: str,
    cursor_line: int,
    cursor_column: int,
) -> MarkdownContinuationEdit | None:
    """Return the Markdown edit to apply instead of a normal newline.

    Positions are zero-based logical line and Unicode code-point columns. ``None``
    means that Enter should keep its normal editor behavior.
    """
    lines = _logical_lines(source)
    if cursor_line < 0 or cursor_line >= len(lines):
        return None

    line = lines[cursor_line].text
    if cursor_column < 0 or cursor_column > len(line):
        return None
    if _inside_fenced_code(lines, cursor_line):
        return None
    if _THEMATIC_BREAK.fullmatch(line):
        return None

    list_match = _LIST_PREFIX.match(line)
    if list_match is not None:
        prefix_end = list_match.end()
        task_match = _TASK_PREFIX.match(line, prefix_end)
        task_body = ""
        task_gap = ""
        if task_match is not None:
            prefix_end = task_match.start("body")
            task_body = task_match.group("body")
            task_gap = task_match.group("gap")

        if cursor_column < prefix_end:
            return None

        body = task_body if task_match is not None else line[prefix_end:]
        if not body.strip():
            return _termination_edit(lines, cursor_line, line)

        marker = list_match.group("unordered")
        if marker is None:
            marker = _increment_ordered_marker(list_match)
        prefix = list_match.group("context") + marker + list_match.group("gap")
        if task_match is not None:
            prefix += "[ ]" + (task_gap or " ")
        return _insertion_edit(lines, cursor_line, cursor_column, prefix)

    quote_match = _QUOTE_PREFIX.match(line)
    if quote_match is None or cursor_column < quote_match.end():
        return None
    if not line[quote_match.end() :].strip():
        return _termination_edit(lines, cursor_line, line)
    return _insertion_edit(
        lines,
        cursor_line,
        cursor_column,
        quote_match.group("prefix"),
    )


def _logical_lines(source: str) -> tuple[_LogicalLine, ...]:
    lines: list[_LogicalLine] = []
    start = 0
    for match in _LINE_ENDING.finditer(source):
        lines.append(_LogicalLine(source[start : match.start()], match.group()))
        start = match.end()
    lines.append(_LogicalLine(source[start:], ""))
    return tuple(lines)


def _newline_for(lines: tuple[_LogicalLine, ...], line_number: int) -> str:
    if lines[line_number].ending:
        return lines[line_number].ending
    for previous in range(line_number - 1, -1, -1):
        if lines[previous].ending:
            return lines[previous].ending
    for following in range(line_number + 1, len(lines)):
        if lines[following].ending:
            return lines[following].ending
    return "\n"


def _insertion_edit(
    lines: tuple[_LogicalLine, ...],
    line_number: int,
    column: int,
    prefix: str,
) -> MarkdownContinuationEdit:
    return MarkdownContinuationEdit(
        start_line=line_number,
        start_column=column,
        end_line=line_number,
        end_column=column,
        text=_newline_for(lines, line_number) + prefix,
        cursor_line=line_number + 1,
        cursor_column=len(prefix),
    )


def _termination_edit(
    lines: tuple[_LogicalLine, ...],
    line_number: int,
    line: str,
) -> MarkdownContinuationEdit:
    return MarkdownContinuationEdit(
        start_line=line_number,
        start_column=0,
        end_line=line_number,
        end_column=len(line),
        text=_newline_for(lines, line_number),
        cursor_line=line_number + 1,
        cursor_column=0,
    )


def _increment_ordered_marker(match: re.Match[str]) -> str:
    number = match.group("number")
    incremented = str(int(number) + 1)
    if len(number) > 1 and number.startswith("0"):
        incremented = incremented.zfill(len(number))
    return incremented + match.group("delimiter")


def _inside_fenced_code(lines: tuple[_LogicalLine, ...], line_number: int) -> bool:
    fence_character: str | None = None
    fence_length = 0
    fence_quote_depth = 0
    for logical_line in lines[:line_number]:
        quote_match = _QUOTE_PREFIX.match(logical_line.text)
        quote_depth = 0
        candidate = logical_line.text
        if quote_match is not None:
            quote_depth = quote_match.group("prefix").count(">")
            candidate = logical_line.text[quote_match.end() :]
        match = _FENCE.match(candidate)
        if match is None:
            continue
        fence = match.group("fence")
        if fence_character is None:
            fence_character = fence[0]
            fence_length = len(fence)
            fence_quote_depth = quote_depth
            continue
        if (
            quote_depth == fence_quote_depth
            and fence[0] == fence_character
            and len(fence) >= fence_length
            and not match.group("rest").strip()
        ):
            fence_character = None
            fence_length = 0
            fence_quote_depth = 0
    return fence_character is not None
