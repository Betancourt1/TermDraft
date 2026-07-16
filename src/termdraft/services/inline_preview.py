"""Single-line Markdown presentation for the inline editor view."""

from __future__ import annotations

import re

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


def render_inline_preview_line(source: str) -> Text:
    """Render one inactive source line without changing its character positions."""
    characters = list(source)
    styles: list[tuple[Style, int, int]] = []

    thematic_break = _THEMATIC_BREAK.fullmatch(source)
    if thematic_break is not None:
        for index, character in enumerate(characters):
            if not character.isspace():
                characters[index] = "─"
        return Text("".join(characters), style="dim", end="", no_wrap=True)

    if match := _HEADING.match(source):
        _blank(characters, match.start(2), match.end(3))
        styles.append((Style(bold=True), match.end(3), len(source)))
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

    rendered = Text("".join(characters), end="", no_wrap=True)
    for style, start, end in styles:
        rendered.stylize(style, start, end)
    return rendered


def _blank(characters: list[str], start: int, end: int) -> None:
    characters[start:end] = [" "] * (end - start)


def _overlaps(candidate: tuple[int, int], ranges: list[tuple[int, int]]) -> bool:
    start, end = candidate
    return any(start < other_end and other_start < end for other_start, other_end in ranges)
