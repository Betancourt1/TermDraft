"""In-process search over validated workspace Markdown paths."""

from __future__ import annotations

import unicodedata
from collections.abc import Callable
from dataclasses import dataclass
from enum import StrEnum
from fnmatch import fnmatchcase
from functools import cache
from pathlib import Path

import regex

from termwriter.models.workspace import path_spelling_key, paths_are_spelling_aliases
from termwriter.services.persistence import PersistenceError, load_file

DEFAULT_RESULT_LIMIT = 100
MAX_PREVIEW_LENGTH = 160
MAX_REGEX_LENGTH = 500
REGEX_TIMEOUT_SECONDS = 0.05


class _RegexTimedOut(Exception):
    """Raised internally when one source line exceeds the regex budget."""


class TextSearchMode(StrEnum):
    """Supported source matching strategies."""

    LITERAL = "literal"
    WHOLE_WORD = "whole_word"
    REGEX = "regex"


@dataclass(frozen=True, slots=True)
class TextSearchOptions:
    """Optional matching and workspace-relative file filtering controls."""

    mode: TextSearchMode = TextSearchMode.LITERAL
    file_filter: str | None = None
    case_sensitive: bool = False


@dataclass(frozen=True, slots=True)
class TextSearchMatch:
    """One matching source line using zero-based source coordinates."""

    path: Path
    line: int
    column: int
    preview: str


@dataclass(frozen=True, slots=True)
class TextSearchOverride:
    """Active source and whether current disk content should take precedence."""

    path: Path
    text: str
    prefer_disk: bool = False


@dataclass(frozen=True, slots=True)
class TextSearchResult:
    """Bounded matches plus non-fatal warnings or an invalid-query error."""

    matches: tuple[TextSearchMatch, ...]
    warnings: tuple[str, ...]
    error: str | None = None


def search_text(
    files: tuple[Path, ...],
    query: str,
    *,
    limit: int = DEFAULT_RESULT_LIMIT,
    active_override: TextSearchOverride | None = None,
    should_cancel: Callable[[], bool] | None = None,
    options: TextSearchOptions | None = None,
    root: Path | None = None,
) -> TextSearchResult:
    """Search validated Markdown paths without invoking external commands.

    Paths are sorted and deduplicated so a result limit is stable even if the
    caller's workspace index order changes. An active override is included even
    when its path disappeared from the latest disk scan.
    """
    if not query or limit <= 0:
        return TextSearchResult((), ())

    options = options or TextSearchOptions()
    matcher, error = _build_matcher(query, options)
    if error is not None:
        return TextSearchResult((), (), error)

    file_filter = options.file_filter.strip() if options.file_filter else None
    if file_filter and root is None:
        return TextSearchResult(
            (),
            (),
            "A workspace root is required when using a file filter.",
        )

    active_override = _canonical_override(files, active_override)
    candidates = set(files)
    if active_override is not None:
        candidates.add(active_override.path)
    ordered_paths = sorted(candidates, key=_path_sort_key)

    matches: list[TextSearchMatch] = []
    warnings: list[str] = []
    for path in ordered_paths:
        if should_cancel is not None and should_cancel():
            break
        if len(matches) >= limit:
            break
        if file_filter and not _matches_file_filter(path, file_filter, root):
            continue
        override = (
            active_override
            if active_override is not None and path == active_override.path
            else None
        )
        if override is not None and not override.prefer_disk:
            text = override.text
        else:
            try:
                text = load_file(path).text
            except (OSError, PersistenceError) as error:
                if override is not None:
                    text = override.text
                    warnings.append(f"Using open source for {path}: {error}")
                else:
                    warnings.append(f"Cannot search {path}: {error}")
                    continue

        if should_cancel is not None and should_cancel():
            break

        for line_number, line in enumerate(_logical_lines(text)):
            if should_cancel is not None and should_cancel():
                break
            try:
                column = matcher(line)
            except _RegexTimedOut:
                return TextSearchResult(
                    (),
                    tuple(warnings),
                    "Regular expression timed out on a source line.",
                )
            if column is None:
                continue
            matches.append(
                TextSearchMatch(
                    path=path,
                    line=line_number,
                    column=column,
                    preview=_line_preview(line, column),
                )
            )
            if len(matches) >= limit:
                break

    return TextSearchResult(tuple(matches), tuple(warnings))


def _build_matcher(
    query: str,
    options: TextSearchOptions,
) -> tuple[Callable[[str], int | None], str | None]:
    if options.mode is TextSearchMode.REGEX:
        if len(query) > MAX_REGEX_LENGTH:
            return _no_match, f"Regular expression is limited to {MAX_REGEX_LENGTH} characters."
        flags = regex.VERSION1
        if not options.case_sensitive:
            flags |= regex.IGNORECASE | regex.FULLCASE
        try:
            pattern = regex.compile(query, flags)
        except regex.error as error:
            return _no_match, f"Invalid regular expression: {error}"
        return lambda line: _regex_column(line, pattern), None

    if options.mode is TextSearchMode.WHOLE_WORD:
        flags = regex.VERSION1 | regex.WORD
        if not options.case_sensitive:
            flags |= regex.IGNORECASE | regex.FULLCASE
        word_character = r"[\w\p{M}]"
        prefix = f"(?<!{word_character})" if _is_word_character(query[0]) else ""
        suffix = f"(?!{word_character})" if _is_word_character(query[-1]) else ""
        pattern = regex.compile(f"{prefix}{regex.escape(query)}{suffix}", flags)
        return lambda line: _regex_column(line, pattern), None

    if options.case_sensitive:
        needle = query
        map_casefolded_column = False
    else:
        needle = query.casefold()
        map_casefolded_column = True

    if map_casefolded_column:
        return lambda line: _casefolded_column(line, needle), None
    return lambda line: _literal_column(line, needle), None


def _no_match(_line: str) -> int | None:
    return None


def _regex_column(line: str, pattern: regex.Pattern[str]) -> int | None:
    try:
        match = pattern.search(
            line,
            timeout=REGEX_TIMEOUT_SECONDS,
            concurrent=True,
        )
    except TimeoutError as error:
        raise _RegexTimedOut from error
    return None if match is None else match.start()


def _literal_column(line: str, needle: str) -> int | None:
    column = line.find(needle)
    return None if column < 0 else column


def _is_word_character(character: str) -> bool:
    return (
        character == "_" or character.isalnum() or unicodedata.category(character).startswith("M")
    )


def _matches_file_filter(path: Path, pattern: str, root: Path | None) -> bool:
    if root is None:
        return False
    try:
        relative_path = path.relative_to(root).as_posix()
    except ValueError:
        return False
    path_parts = tuple(part.casefold() for part in relative_path.split("/"))
    pattern_parts = tuple(part.casefold() for part in pattern.split("/") if part)
    if not pattern_parts:
        return False
    if len(pattern_parts) == 1:
        return fnmatchcase(path_parts[-1], pattern_parts[0])

    @cache
    def match(path_index: int, pattern_index: int) -> bool:
        if pattern_index == len(pattern_parts):
            return path_index == len(path_parts)
        part = pattern_parts[pattern_index]
        if part == "**":
            return match(path_index, pattern_index + 1) or (
                path_index < len(path_parts) and match(path_index + 1, pattern_index)
            )
        return (
            path_index < len(path_parts)
            and fnmatchcase(path_parts[path_index], part)
            and match(path_index + 1, pattern_index + 1)
        )

    return match(0, 0)


def _logical_lines(text: str) -> list[str]:
    """Match TextArea's logical empty line at an empty source or trailing newline."""
    if not text:
        return [""]
    lines = text.splitlines()
    if text.endswith(("\r", "\n")):
        lines.append("")
    return lines


def _canonical_override(
    files: tuple[Path, ...],
    override: TextSearchOverride | None,
) -> TextSearchOverride | None:
    """Use the workspace path spelling for an existing case-insensitive alias."""
    if override is None:
        return None
    for candidate in files:
        if candidate == override.path:
            return override
    override_key = path_spelling_key(override.path)
    for candidate in files:
        if path_spelling_key(candidate) != override_key:
            continue
        if paths_are_spelling_aliases(candidate, override.path):
            return TextSearchOverride(candidate, override.text, override.prefer_disk)
    return override


def _path_sort_key(path: Path) -> tuple[str, str]:
    path_text = path.as_posix()
    return path_text.casefold(), path_text


def _casefolded_column(line: str, needle: str) -> int | None:
    folded_line, source_columns = _casefold_with_columns(line)
    folded_column = folded_line.find(needle)
    if folded_column < 0:
        return None
    return source_columns[folded_column]


def _casefold_with_columns(line: str) -> tuple[str, list[int]]:
    folded_parts: list[str] = []
    source_columns: list[int] = []
    for column, character in enumerate(line):
        folded_character = character.casefold()
        folded_parts.append(folded_character)
        source_columns.extend([column] * len(folded_character))

    return "".join(folded_parts), source_columns


def _line_preview(line: str, column: int) -> str:
    if len(line) <= MAX_PREVIEW_LENGTH:
        return line

    start = max(0, column - MAX_PREVIEW_LENGTH // 3)
    prefix = "…" if start else ""
    content_width = MAX_PREVIEW_LENGTH - len(prefix) - 1
    end = start + content_width
    suffix = "…" if end < len(line) else ""
    if not suffix:
        content_width += 1
        end = start + content_width
    return prefix + line[start:end] + suffix
