"""Literal in-process search over validated workspace Markdown paths."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from termwriter.models.workspace import path_spelling_key, paths_are_spelling_aliases
from termwriter.services.persistence import PersistenceError, load_file

DEFAULT_RESULT_LIMIT = 100
MAX_PREVIEW_LENGTH = 160


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
    """Bounded matches plus non-fatal file loading warnings."""

    matches: tuple[TextSearchMatch, ...]
    warnings: tuple[str, ...]


def search_text(
    files: tuple[Path, ...],
    query: str,
    *,
    limit: int = DEFAULT_RESULT_LIMIT,
    active_override: TextSearchOverride | None = None,
    should_cancel: Callable[[], bool] | None = None,
) -> TextSearchResult:
    """Search validated Markdown paths without invoking external commands.

    Paths are sorted and deduplicated so a result limit is stable even if the
    caller's workspace index order changes. An active override is included even
    when its path disappeared from the latest disk scan.
    """
    needle = query.casefold()
    if not needle or limit <= 0:
        return TextSearchResult((), ())

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
            column = _casefolded_column(line, needle)
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
    folded_parts: list[str] = []
    source_columns: list[int] = []
    for column, character in enumerate(line):
        folded_character = character.casefold()
        folded_parts.append(folded_character)
        source_columns.extend([column] * len(folded_character))

    folded_column = "".join(folded_parts).find(needle)
    if folded_column < 0:
        return None
    return source_columns[folded_column]


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
