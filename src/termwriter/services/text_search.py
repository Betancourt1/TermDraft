"""In-process search over validated workspace Markdown paths."""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path, PurePosixPath

from termwriter.models.workspace import path_spelling_key, paths_are_spelling_aliases
from termwriter.services.persistence import PersistenceError, load_file

DEFAULT_RESULT_LIMIT = 100
MAX_PREVIEW_LENGTH = 160
MAX_REGEX_LENGTH = 500


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

        for line_number, line in enumerate(text.splitlines()):
            if should_cancel is not None and should_cancel():
                break
            column = matcher(line)
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
        flags = 0 if options.case_sensitive else re.IGNORECASE
        try:
            pattern = re.compile(query, flags)
        except re.error as error:
            return _no_match, f"Invalid regular expression: {error}"
        return lambda line: _regex_column(line, pattern), None

    if options.case_sensitive:
        needle = query
        source_pattern = query
        map_casefolded_column = False
    else:
        needle = query.casefold()
        source_pattern = needle
        map_casefolded_column = True

    if options.mode is TextSearchMode.WHOLE_WORD:
        prefix = r"(?<!\w)" if _is_word_character(source_pattern[0]) else ""
        suffix = r"(?!\w)" if _is_word_character(source_pattern[-1]) else ""
        pattern = re.compile(f"{prefix}{re.escape(source_pattern)}{suffix}")

        def match_whole_word(line: str) -> int | None:
            if map_casefolded_column:
                folded_line, source_columns = _casefold_with_columns(line)
                match = pattern.search(folded_line)
                return None if match is None else source_columns[match.start()]
            match = pattern.search(line)
            return None if match is None else match.start()

        return match_whole_word, None

    if map_casefolded_column:
        return lambda line: _casefolded_column(line, needle), None
    return lambda line: _literal_column(line, needle), None


def _no_match(_line: str) -> int | None:
    return None


def _regex_column(line: str, pattern: re.Pattern[str]) -> int | None:
    match = pattern.search(line)
    return None if match is None else match.start()


def _literal_column(line: str, needle: str) -> int | None:
    column = line.find(needle)
    return None if column < 0 else column


def _is_word_character(character: str) -> bool:
    return re.match(r"\w", character) is not None


def _matches_file_filter(path: Path, pattern: str, root: Path | None) -> bool:
    if root is None:
        return False
    try:
        relative_path = path.relative_to(root).as_posix()
    except ValueError:
        return False
    return PurePosixPath(relative_path.casefold()).match(pattern.casefold())


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
