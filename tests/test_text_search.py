"""Tests for safe in-process workspace text search."""

from __future__ import annotations

import codecs
from pathlib import Path

import pytest

from termwriter.services.persistence import LoadedFile, load_file
from termwriter.services.text_search import (
    MAX_PREVIEW_LENGTH,
    TextSearchOverride,
    search_text,
)


def test_search_is_literal_case_insensitive_and_returns_one_match_per_line(
    tmp_path: Path,
) -> None:
    path = tmp_path / "notes.md"
    path.write_text("Alpha alpha\nLiteral [x] and .\nNo match\n", encoding="utf-8")

    alpha = search_text((path,), "ALPHA")
    literal = search_text((path,), "[x]")

    assert [(match.line, match.column, match.preview) for match in alpha.matches] == [
        (0, 0, "Alpha alpha")
    ]
    assert [(match.line, match.column) for match in literal.matches] == [(1, 8)]
    assert alpha.warnings == ()


def test_search_reports_zero_based_unicode_source_columns(tmp_path: Path) -> None:
    path = tmp_path / "unicode.md"
    path.write_text("ßx CAFÉ\n", encoding="utf-8")

    expanded_fold = search_text((path,), "x")
    accented = search_text((path,), "café")

    assert expanded_fold.matches[0].column == 1
    assert accented.matches[0].column == 3


def test_search_handles_utf8_bom_crlf_and_missing_final_newline(tmp_path: Path) -> None:
    path = tmp_path / "windows.markdown"
    path.write_bytes(codecs.BOM_UTF8 + "First\r\nCafé needle".encode())

    result = search_text((path,), "NEEDLE")

    assert len(result.matches) == 1
    assert result.matches[0].path == path
    assert (result.matches[0].line, result.matches[0].column) == (1, 5)
    assert result.matches[0].preview == "Café needle"


def test_empty_query_and_non_positive_limit_do_not_read_files(tmp_path: Path) -> None:
    missing = tmp_path / "missing.md"

    assert search_text((missing,), "") == search_text((), "anything", limit=0)
    assert search_text((missing,), "anything", limit=-1).matches == ()
    assert search_text((missing,), "anything", limit=-1).warnings == ()


def test_limit_is_deterministic_across_unsorted_and_duplicate_paths(tmp_path: Path) -> None:
    a_path = tmp_path / "a.md"
    b_path = tmp_path / "b.md"
    a_path.write_text("hit a1\nhit a2\n", encoding="utf-8")
    b_path.write_text("hit b1\n", encoding="utf-8")

    result = search_text((b_path, a_path, a_path), "hit", limit=2)

    assert [(match.path.name, match.line) for match in result.matches] == [
        ("a.md", 0),
        ("a.md", 1),
    ]


def test_file_failures_become_warnings_without_hiding_other_matches(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    valid = tmp_path / "a-valid.md"
    invalid = tmp_path / "b-invalid.md"
    unreadable = tmp_path / "c-unreadable.md"
    missing = tmp_path / "d-missing.md"
    valid.write_text("needle\n", encoding="utf-8")
    invalid.write_bytes(b"\xff")
    unreadable.write_text("needle\n", encoding="utf-8")
    safe_load = load_file

    def fail_one_file(path: Path) -> LoadedFile:
        if path == unreadable:
            raise PermissionError("permission denied")
        return safe_load(path)

    monkeypatch.setattr("termwriter.services.text_search.load_file", fail_one_file)

    result = search_text((missing, invalid, unreadable, valid), "needle")

    assert [(match.path, match.line) for match in result.matches] == [(valid, 0)]
    assert len(result.warnings) == 3
    assert str(invalid) in result.warnings[0]
    assert "valid UTF-8" in result.warnings[0]
    assert str(unreadable) in result.warnings[1]
    assert "permission denied" in result.warnings[1]
    assert str(missing) in result.warnings[2]


def test_active_override_replaces_disk_text_and_can_include_a_missing_path(
    tmp_path: Path,
) -> None:
    active = tmp_path / "active.md"
    other = tmp_path / "other.md"
    missing_active = tmp_path / "deleted.md"
    active.write_text("disk only\n", encoding="utf-8")
    other.write_text("local needle on disk\n", encoding="utf-8")

    dirty_result = search_text(
        (active, other),
        "local needle",
        active_override=TextSearchOverride(active, "local needle in memory\n"),
    )
    missing_result = search_text(
        (other,),
        "draft",
        active_override=TextSearchOverride(missing_active, "unsaved draft\n"),
    )

    assert [(match.path, match.preview) for match in dirty_result.matches] == [
        (active, "local needle in memory"),
        (other, "local needle on disk"),
    ]
    assert [(match.path, match.preview) for match in missing_result.matches] == [
        (missing_active, "unsaved draft")
    ]
    assert missing_result.warnings == ()


def test_long_preview_is_bounded_and_keeps_the_match_visible(tmp_path: Path) -> None:
    path = tmp_path / "long.md"
    path.write_text("a" * 300 + " needle " + "z" * 300, encoding="utf-8")

    result = search_text((path,), "needle")

    preview = result.matches[0].preview
    assert len(preview) <= MAX_PREVIEW_LENGTH
    assert "needle" in preview
    assert preview.startswith("…")
    assert preview.endswith("…")
