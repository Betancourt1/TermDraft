"""Tests for in-process workspace file search."""

from pathlib import Path

from termdraft.services.file_search import search_files
from termdraft.services.path_filter import parse_path_filter


def test_file_search_prefers_filename_prefixes(tmp_path: Path) -> None:
    files = (
        tmp_path / "archive" / "project-plan.md",
        tmp_path / "planning.md",
        tmp_path / "notes.md",
    )

    matches = search_files(files, "plan", root=tmp_path)

    assert matches == (tmp_path / "planning.md", tmp_path / "archive" / "project-plan.md")


def test_file_search_finds_and_ranks_subsequence_matches(tmp_path: Path) -> None:
    substring = tmp_path / "rdm-notes.md"
    tight = tmp_path / "readme.md"
    loose = tmp_path / "random.md"
    directory_only = tmp_path / "research" / "daily" / "map.md"

    matches = search_files(
        (directory_only, loose, tight, substring),
        "rdm",
        root=tmp_path,
    )

    assert matches == (substring, tight, loose, directory_only)


def test_file_search_fuzzy_order_is_stable_across_input_order(tmp_path: Path) -> None:
    first = tmp_path / "a" / "r_d.md"
    second = tmp_path / "b" / "r_d.md"

    forward = search_files((first, second), "rd", root=tmp_path)
    reversed_order = search_files((second, first), "rd", root=tmp_path)

    assert forward == reversed_order == (first, second)


def test_file_search_normalizes_unicode_before_fuzzy_matching(tmp_path: Path) -> None:
    decomposed = tmp_path / "Cafe\N{COMBINING ACUTE ACCENT} Notes.markdown"

    assert search_files((decomposed,), "CAFÉN", root=tmp_path) == (decomposed,)


def test_file_search_can_reuse_compound_path_filter(tmp_path: Path) -> None:
    public = tmp_path / "docs" / "public.md"
    private = tmp_path / "docs" / "private.md"
    other = tmp_path / "other.markdown"
    path_filter = parse_path_filter("docs/**/*.md, !**/private.md")

    assert path_filter is not None
    matches = search_files(
        (private, other, public),
        "",
        root=tmp_path,
        path_filter=path_filter,
    )

    assert matches == (public,)
