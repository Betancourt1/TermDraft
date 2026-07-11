"""Small in-process file search for validated workspace paths."""

from __future__ import annotations

from pathlib import Path


def search_files(
    files: tuple[Path, ...],
    query: str,
    *,
    root: Path,
    limit: int = 50,
) -> tuple[Path, ...]:
    """Rank case-insensitive substring matches without external commands."""
    needle = query.strip().casefold()
    if not needle:
        return files[:limit]

    ranked: list[tuple[int, str, Path]] = []
    for path in files:
        relative = path.relative_to(root).as_posix()
        relative_folded = relative.casefold()
        name_folded = path.name.casefold()
        if needle not in relative_folded:
            continue

        score = 0
        if name_folded.startswith(needle):
            score += 100
        elif needle in name_folded:
            score += 70
        if any(part.startswith(needle) for part in relative_folded.split("/")):
            score += 30
        score -= len(relative)
        ranked.append((-score, relative_folded, path))

    ranked.sort()
    return tuple(path for _, _, path in ranked[:limit])
