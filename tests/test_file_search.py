"""Tests for in-process workspace file search."""

from pathlib import Path

from termwriter.services.file_search import search_files


def test_file_search_prefers_filename_prefixes(tmp_path: Path) -> None:
    files = (
        tmp_path / "archive" / "project-plan.md",
        tmp_path / "planning.md",
        tmp_path / "notes.md",
    )

    matches = search_files(files, "plan", root=tmp_path)

    assert matches == (tmp_path / "planning.md", tmp_path / "archive" / "project-plan.md")
