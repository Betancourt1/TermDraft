"""Workspace-relative include and exclude glob filters."""

from __future__ import annotations

import unicodedata
from dataclasses import dataclass
from fnmatch import fnmatchcase
from functools import cache
from pathlib import Path


class PathFilterError(ValueError):
    """Raised when a path-filter expression is not workspace relative."""


@dataclass(frozen=True, slots=True)
class PathFilter:
    """Case-insensitive include and exclude globs for workspace paths.

    Includes are ORed and exclusions always win. When there are no includes,
    every workspace path is included unless an exclusion matches it.
    """

    includes: tuple[str, ...]
    excludes: tuple[str, ...]

    def matches(self, path: Path, *, root: Path) -> bool:
        """Return whether a path beneath ``root`` passes this filter."""
        try:
            relative = path.relative_to(root).as_posix()
        except ValueError:
            return False

        included = not self.includes or any(
            _matches_glob(relative, pattern) for pattern in self.includes
        )
        if not included:
            return False
        return not any(_matches_glob(relative, pattern) for pattern in self.excludes)


def parse_path_filter(expression: str | None) -> PathFilter | None:
    """Parse comma-separated globs, with ``!`` marking exclusions.

    Globs use POSIX workspace-relative paths. A one-component glob such as
    ``*.md`` matches a basename at any depth, while ``**`` matches zero or more
    complete path components.
    """
    if expression is None or not expression.strip():
        return None

    includes: list[str] = []
    excludes: list[str] = []
    for raw_term in expression.split(","):
        term = raw_term.strip()
        if not term:
            raise PathFilterError("empty patterns are not allowed")

        excluded = term.startswith("!")
        pattern = term[1:].strip() if excluded else term
        if not pattern:
            raise PathFilterError("an exclusion requires a glob after '!'")
        if pattern.startswith("/"):
            raise PathFilterError(f"patterns must be workspace relative: {pattern}")

        parts = tuple(part for part in pattern.split("/") if part)
        if ".." in parts:
            raise PathFilterError(f"parent path components are not allowed: {pattern}")

        normalized = "/".join(parts)
        if not normalized:
            raise PathFilterError("patterns must name a workspace path")
        (excludes if excluded else includes).append(normalized)

    return PathFilter(tuple(includes), tuple(excludes))


def _matches_glob(relative_path: str, pattern: str) -> bool:
    path_parts = tuple(_normalize(part) for part in relative_path.split("/"))
    pattern_parts = tuple(_normalize(part) for part in pattern.split("/"))

    if len(pattern_parts) == 1:
        return fnmatchcase(path_parts[-1], pattern_parts[0])

    @cache
    def match(path_index: int, pattern_index: int) -> bool:
        if pattern_index == len(pattern_parts):
            return path_index == len(path_parts)
        pattern_part = pattern_parts[pattern_index]
        if pattern_part == "**":
            return match(path_index, pattern_index + 1) or (
                path_index < len(path_parts) and match(path_index + 1, pattern_index)
            )
        return (
            path_index < len(path_parts)
            and fnmatchcase(path_parts[path_index], pattern_part)
            and match(path_index + 1, pattern_index + 1)
        )

    return match(0, 0)


def _normalize(value: str) -> str:
    return unicodedata.normalize("NFC", value).casefold()
