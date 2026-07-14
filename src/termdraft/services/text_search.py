"""In-process search over validated workspace Markdown paths."""

from __future__ import annotations

import unicodedata
from collections import defaultdict, deque
from collections.abc import Callable
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

import regex

from termdraft.models.workspace import path_spelling_key, paths_are_spelling_aliases
from termdraft.services.path_filter import PathFilterError, parse_path_filter
from termdraft.services.persistence import PersistenceError, load_file

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
    FUZZY = "fuzzy"


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


@dataclass(frozen=True, slots=True)
class _FuzzyLineMatch:
    column: int
    rank: tuple[int, int, int, int]


def search_text(
    files: tuple[Path, ...],
    query: str,
    *,
    limit: int = DEFAULT_RESULT_LIMIT,
    active_override: TextSearchOverride | None = None,
    overrides: tuple[TextSearchOverride, ...] = (),
    should_cancel: Callable[[], bool] | None = None,
    options: TextSearchOptions | None = None,
    root: Path | None = None,
) -> TextSearchResult:
    """Search validated Markdown paths without invoking external commands.

    Paths are sorted and deduplicated so a result limit is stable even if the
    caller's workspace index order changes. Open-source overrides are included
    even when their paths disappeared from the latest disk scan.
    """
    if not query or limit <= 0:
        return TextSearchResult((), ())

    options = options or TextSearchOptions()
    matcher, error = _build_matcher(query, options)
    if error is not None:
        return TextSearchResult((), (), error)

    try:
        path_filter = parse_path_filter(options.file_filter)
    except PathFilterError as error:
        return TextSearchResult((), (), f"Invalid file filter: {error}")
    if path_filter is not None and root is None:
        return TextSearchResult(
            (),
            (),
            "A workspace root is required when using a file filter.",
        )

    canonical_overrides = {
        override.path: override
        for item in (*overrides, *((active_override,) if active_override is not None else ()))
        if (override := _canonical_override(files, item)) is not None
    }
    candidates = set(files)
    candidates.update(canonical_overrides)
    ordered_paths = sorted(candidates, key=_path_sort_key)

    matches: list[TextSearchMatch] = []
    fuzzy_matches: list[tuple[tuple[int, int, int, int, str, str, int, int], TextSearchMatch]] = []
    warnings: list[str] = []
    fuzzy_mode = options.mode is TextSearchMode.FUZZY
    fuzzy_needle = ""
    if fuzzy_mode:
        transformed_query = _fuzzy_form_with_columns(
            query,
            case_sensitive=options.case_sensitive,
            should_cancel=should_cancel,
        )
        if transformed_query is None:
            return TextSearchResult((), ())
        fuzzy_needle = transformed_query[0]
    for path in ordered_paths:
        if should_cancel is not None and should_cancel():
            break
        if not fuzzy_mode and len(matches) >= limit:
            break
        if path_filter is not None and (root is None or not path_filter.matches(path, root=root)):
            continue
        override = canonical_overrides.get(path)
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
            fuzzy_rank: tuple[int, int, int, int] | None = None
            column: int | None
            if fuzzy_mode:
                fuzzy_match = _fuzzy_line_match(
                    line,
                    fuzzy_needle,
                    case_sensitive=options.case_sensitive,
                    should_cancel=should_cancel,
                )
                if should_cancel is not None and should_cancel():
                    break
                if fuzzy_match is None:
                    continue
                column = fuzzy_match.column
                fuzzy_rank = fuzzy_match.rank
            else:
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

            result_match = TextSearchMatch(
                path=path,
                line=line_number,
                column=column,
                preview=_line_preview(line, column),
            )
            if fuzzy_rank is not None:
                path_key = _path_sort_key(path)
                rank = (
                    *fuzzy_rank,
                    *path_key,
                    line_number,
                    column,
                )
                fuzzy_matches.append((rank, result_match))
                if len(fuzzy_matches) >= limit * 2:
                    fuzzy_matches.sort(key=lambda item: item[0])
                    del fuzzy_matches[limit:]
            else:
                matches.append(result_match)
                if len(matches) >= limit:
                    break

    if fuzzy_mode:
        fuzzy_matches.sort(key=lambda item: item[0])
        matches = [match for _, match in fuzzy_matches[:limit]]
    return TextSearchResult(tuple(matches), tuple(warnings))


def _build_matcher(
    query: str,
    options: TextSearchOptions,
) -> tuple[Callable[[str], int | None], str | None]:
    if options.mode is TextSearchMode.FUZZY:
        return _no_match, None

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


def _fuzzy_line_match(
    line: str,
    needle: str,
    *,
    case_sensitive: bool,
    should_cancel: Callable[[], bool] | None = None,
) -> _FuzzyLineMatch | None:
    transformed = _fuzzy_form_with_columns(
        line,
        case_sensitive=case_sensitive,
        should_cancel=should_cancel,
    )
    if transformed is None:
        return None
    haystack, source_columns = transformed

    best: _FuzzyLineMatch | None = None
    first = haystack.find(needle[0])
    candidate_count = 0
    while first >= 0:
        if candidate_count % 256 == 0 and should_cancel is not None and should_cancel():
            return None
        candidate_count += 1
        cursor = first + 1
        last = first
        for needle_index, character in enumerate(needle[1:]):
            if needle_index % 256 == 0 and should_cancel is not None and should_cancel():
                return None
            last = haystack.find(character, cursor)
            if last < 0:
                break
            cursor = last + 1
        else:
            span = last - first + 1
            boundary_penalty = int(first > 0 and _is_word_character(haystack[first - 1]))
            rank = (
                span - len(needle),
                boundary_penalty,
                first,
                len(haystack),
            )
            candidate = _FuzzyLineMatch(source_columns[first], rank)
            if best is None or candidate.rank < best.rank:
                best = candidate

        first = haystack.find(needle[0], first + 1)

    return best


def _fuzzy_form_with_columns(
    value: str,
    *,
    case_sensitive: bool,
    should_cancel: Callable[[], bool] | None = None,
) -> tuple[str, list[int]] | None:
    """Normalize fuzzy text canonically while retaining source coordinates."""
    characters: list[str] = []
    source_columns: list[int] = []
    for column, character in enumerate(value):
        if column % 256 == 0 and should_cancel is not None and should_cancel():
            return None
        folded = character if case_sensitive else character.casefold()
        for normalized_character in unicodedata.normalize("NFD", folded):
            characters.append(normalized_character)
            source_columns.append(column)

    intermediate = "".join(characters)
    normalized = unicodedata.normalize("NFD", intermediate)
    if normalized == intermediate:
        return normalized, source_columns

    columns_by_character: dict[str, deque[int]] = defaultdict(deque)
    for character, column in zip(characters, source_columns, strict=True):
        columns_by_character[character].append(column)

    normalized_columns: list[int] = []
    for index, character in enumerate(normalized):
        if index % 256 == 0 and should_cancel is not None and should_cancel():
            return None
        normalized_columns.append(columns_by_character[character].popleft())
    return normalized, normalized_columns


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
