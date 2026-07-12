"""Small in-process file search for validated workspace paths."""

from __future__ import annotations

import unicodedata
from dataclasses import dataclass
from pathlib import Path

from termwriter.services.path_filter import PathFilter


@dataclass(frozen=True, slots=True)
class _FuzzyMatch:
    start: int
    gap_count: int
    span: int


def search_files(
    files: tuple[Path, ...],
    query: str,
    *,
    root: Path,
    limit: int = 50,
    path_filter: PathFilter | None = None,
) -> tuple[Path, ...]:
    """Rank normalized substring and subsequence matches deterministically."""
    needle = _normalize(query.strip())
    if not needle:
        filtered = (
            files
            if path_filter is None
            else tuple(path for path in files if path_filter.matches(path, root=root))
        )
        return filtered[:limit]

    ranked: list[tuple[tuple[int, int, int, int, str, str], Path]] = []
    for path in files:
        if path_filter is not None and not path_filter.matches(path, root=root):
            continue
        relative = path.relative_to(root).as_posix()
        relative_folded = _normalize(relative)
        name_folded = _normalize(path.name)

        if needle in relative_folded:
            score = 0
            if name_folded.startswith(needle):
                score += 100
            elif needle in name_folded:
                score += 70
            if any(part.startswith(needle) for part in relative_folded.split("/")):
                score += 30
            score -= len(relative)
            rank = (0, -score, 0, 0, relative_folded, relative)
            ranked.append((rank, path))
            continue

        fuzzy_match = _find_fuzzy_match(name_folded, needle)
        match_scope = 1
        if fuzzy_match is None:
            fuzzy_match = _find_fuzzy_match(relative_folded, needle)
            match_scope = 2
        if fuzzy_match is None:
            continue

        rank = (
            match_scope,
            fuzzy_match.gap_count,
            fuzzy_match.span,
            fuzzy_match.start,
            relative_folded,
            relative,
        )
        ranked.append((rank, path))

    ranked.sort(key=lambda item: item[0])
    return tuple(path for _, path in ranked[:limit])


def _find_fuzzy_match(haystack: str, needle: str) -> _FuzzyMatch | None:
    """Find the tightest greedy subsequence, preferring its earliest start."""
    best: _FuzzyMatch | None = None
    first = haystack.find(needle[0])
    while first >= 0:
        cursor = first + 1
        last = first
        for character in needle[1:]:
            last = haystack.find(character, cursor)
            if last < 0:
                break
            cursor = last + 1
        else:
            span = last - first + 1
            candidate = _FuzzyMatch(first, span - len(needle), span)
            if best is None or (
                candidate.gap_count,
                candidate.span,
                candidate.start,
            ) < (best.gap_count, best.span, best.start):
                best = candidate

        first = haystack.find(needle[0], first + 1)

    return best


def _normalize(value: str) -> str:
    return unicodedata.normalize("NFC", value).casefold()
