"""Literal in-process search over validated workspace Markdown paths."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

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
    """Unsaved source that replaces disk content for one active document."""

    path: Path
    text: str


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
) -> TextSearchResult:
    """Search validated Markdown paths without invoking external commands.

    Paths are sorted and deduplicated so a result limit is stable even if the
    caller's workspace index order changes. An active override is included even
    when its path disappeared from the latest disk scan.
    """
    needle = query.casefold()
    if not needle or limit <= 0:
        return TextSearchResult((), ())

    candidates = set(files)
    if active_override is not None:
        candidates.add(active_override.path)
    ordered_paths = sorted(candidates, key=_path_sort_key)

    matches: list[TextSearchMatch] = []
    warnings: list[str] = []
    for path in ordered_paths:
        if active_override is not None and path == active_override.path:
            text = active_override.text
        else:
            try:
                text = load_file(path).text
            except (OSError, PersistenceError) as error:
                warnings.append(f"Cannot search {path}: {error}")
                continue

        if len(matches) >= limit:
            continue
        for line_number, line in enumerate(text.splitlines()):
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
