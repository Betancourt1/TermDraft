"""Tests for safe in-process workspace text search."""

from __future__ import annotations

import codecs
from pathlib import Path
from time import monotonic

import pytest

from termwriter.services.persistence import LoadedFile, load_file
from termwriter.services.text_search import (
    MAX_PREVIEW_LENGTH,
    MAX_REGEX_LENGTH,
    TextSearchMode,
    TextSearchOptions,
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


def test_whole_word_search_respects_unicode_boundaries_and_casefolding(
    tmp_path: Path,
) -> None:
    path = tmp_path / "unicode.md"
    path.write_text("Straße\nSTRASSE\nStrassen\nVorstraße\n", encoding="utf-8")

    result = search_text(
        (path,),
        "strasse",
        options=TextSearchOptions(mode=TextSearchMode.WHOLE_WORD),
    )

    assert [(match.line, match.column) for match in result.matches] == [(0, 0), (1, 0)]
    assert result.error is None


def test_whole_word_search_treats_combining_marks_as_word_characters(
    tmp_path: Path,
) -> None:
    path = tmp_path / "unicode.md"
    standalone = "cafe\N{COMBINING ACUTE ACCENT}"
    path.write_text(f"{standalone}ine\n{standalone}\n", encoding="utf-8")

    result = search_text(
        (path,),
        standalone,
        options=TextSearchOptions(mode=TextSearchMode.WHOLE_WORD),
    )

    assert [(match.line, match.column) for match in result.matches] == [(1, 0)]


def test_literal_and_whole_word_search_can_be_case_sensitive(tmp_path: Path) -> None:
    path = tmp_path / "case.md"
    path.write_text("Word\nword\nsWord\n", encoding="utf-8")

    literal = search_text(
        (path,),
        "Word",
        options=TextSearchOptions(case_sensitive=True),
    )
    whole_word = search_text(
        (path,),
        "Word",
        options=TextSearchOptions(
            mode=TextSearchMode.WHOLE_WORD,
            case_sensitive=True,
        ),
    )

    assert [match.line for match in literal.matches] == [0, 2]
    assert [match.line for match in whole_word.matches] == [0]


def test_regex_search_supports_unicode_case_insensitive_patterns_and_source_columns(
    tmp_path: Path,
) -> None:
    path = tmp_path / "regex.md"
    path.write_text("Préfix CAFÉ-42 suffix\nCafé-no-number\n", encoding="utf-8")

    result = search_text(
        (path,),
        r"café-\d+",
        options=TextSearchOptions(mode=TextSearchMode.REGEX),
    )

    assert [(match.line, match.column, match.preview) for match in result.matches] == [
        (0, 7, "Préfix CAFÉ-42 suffix")
    ]
    assert result.error is None


def test_regex_search_can_be_case_sensitive_and_returns_one_match_per_line(
    tmp_path: Path,
) -> None:
    path = tmp_path / "regex.md"
    path.write_text("ID-1 ID-2\nid-3\n", encoding="utf-8")

    result = search_text(
        (path,),
        r"ID-\d",
        options=TextSearchOptions(
            mode=TextSearchMode.REGEX,
            case_sensitive=True,
        ),
    )

    assert [(match.line, match.column) for match in result.matches] == [(0, 0)]


def test_invalid_or_oversized_regex_returns_error_without_reading_files(
    tmp_path: Path,
) -> None:
    missing = tmp_path / "missing.md"
    regex_options = TextSearchOptions(mode=TextSearchMode.REGEX)

    invalid = search_text((missing,), "[", options=regex_options)
    oversized = search_text(
        (missing,),
        "x" * (MAX_REGEX_LENGTH + 1),
        options=regex_options,
    )

    assert invalid.matches == ()
    assert invalid.warnings == ()
    assert invalid.error is not None
    assert invalid.error.startswith("Invalid regular expression:")
    assert oversized.matches == ()
    assert oversized.warnings == ()
    assert oversized.error == (f"Regular expression is limited to {MAX_REGEX_LENGTH} characters.")


def test_pathological_regex_times_out_without_holding_the_search_worker(
    tmp_path: Path,
) -> None:
    path = tmp_path / "long.md"
    path.write_text("a" * 20_000 + "!", encoding="utf-8")

    started = monotonic()
    result = search_text(
        (path,),
        r"(a+)+$",
        options=TextSearchOptions(mode=TextSearchMode.REGEX),
    )

    assert monotonic() - started < 1
    assert result.matches == ()
    assert result.error == "Regular expression timed out on a source line."


def test_regex_search_includes_logical_empty_and_trailing_lines(tmp_path: Path) -> None:
    empty = tmp_path / "empty.md"
    trailing = tmp_path / "trailing.md"
    empty.write_text("", encoding="utf-8")
    trailing.write_text("alpha\n\n", encoding="utf-8")
    options = TextSearchOptions(
        mode=TextSearchMode.REGEX,
        case_sensitive=True,
    )

    empty_result = search_text((empty,), r"^$", options=options)
    trailing_result = search_text((trailing,), r"^$", options=options)

    assert [(match.line, match.column) for match in empty_result.matches] == [(0, 0)]
    assert [(match.line, match.column) for match in trailing_result.matches] == [
        (1, 0),
        (2, 0),
    ]


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


def test_file_filter_matches_case_insensitive_workspace_relative_posix_glob(
    tmp_path: Path,
) -> None:
    docs = tmp_path / "docs"
    deep = docs / "deep"
    deep.mkdir(parents=True)
    direct = docs / "Guide.MD"
    nested = deep / "nested.md"
    root_file = tmp_path / "root.md"
    direct.write_text("needle direct\n", encoding="utf-8")
    nested.write_text("needle nested\n", encoding="utf-8")
    root_file.write_text("needle root\n", encoding="utf-8")

    direct_result = search_text(
        (root_file, nested, direct),
        "needle",
        options=TextSearchOptions(file_filter=" DOCS/*.md "),
        root=tmp_path,
    )
    nested_result = search_text(
        (root_file, nested, direct),
        "needle",
        options=TextSearchOptions(file_filter="docs/**/*.md"),
        root=tmp_path,
    )
    basename_result = search_text(
        (root_file, nested, direct),
        "needle",
        options=TextSearchOptions(file_filter="*.md"),
        root=tmp_path,
    )

    assert [match.path for match in direct_result.matches] == [direct]
    assert {match.path for match in nested_result.matches} == {direct, nested}
    assert {match.path for match in basename_result.matches} == {direct, nested, root_file}


def test_compound_file_filter_ors_includes_and_applies_exclusions(tmp_path: Path) -> None:
    docs = tmp_path / "docs"
    drafts = docs / "drafts"
    drafts.mkdir(parents=True)
    guide = docs / "guide.md"
    draft = drafts / "idea.md"
    journal = tmp_path / "journal.markdown"
    private = tmp_path / "private.markdown"
    ignored = tmp_path / "ignored.md"
    for path in (guide, draft, journal, private, ignored):
        path.write_text("needle\n", encoding="utf-8")

    result = search_text(
        (ignored, private, journal, draft, guide),
        "needle",
        options=TextSearchOptions(
            file_filter="docs/**/*.md, *.markdown, !docs/drafts/**, !private.markdown"
        ),
        root=tmp_path,
    )

    assert [match.path for match in result.matches] == [guide, journal]
    assert result.error is None


@pytest.mark.parametrize(
    "file_filter",
    ("*.md,,!drafts/**", "!", "/absolute/*.md", "../*.md"),
)
def test_invalid_compound_file_filter_returns_error_before_loading(
    tmp_path: Path,
    file_filter: str,
) -> None:
    result = search_text(
        (tmp_path / "missing.md",),
        "needle",
        options=TextSearchOptions(file_filter=file_filter),
        root=tmp_path,
    )

    assert result.matches == ()
    assert result.warnings == ()
    assert result.error is not None
    assert result.error.startswith("Invalid file filter:")


def test_file_filter_is_applied_before_loading_or_using_active_override(
    tmp_path: Path,
) -> None:
    included = tmp_path / "included.md"
    missing_excluded = tmp_path / "excluded.markdown"
    included.write_text("needle on disk\n", encoding="utf-8")

    result = search_text(
        (included, missing_excluded),
        "needle",
        options=TextSearchOptions(file_filter="*.md"),
        root=tmp_path,
        active_override=TextSearchOverride(
            missing_excluded,
            "needle in memory\n",
        ),
    )

    assert [match.path for match in result.matches] == [included]
    assert result.warnings == ()


def test_file_filter_requires_workspace_root_before_reading_files(tmp_path: Path) -> None:
    missing = tmp_path / "missing.md"

    result = search_text(
        (missing,),
        "needle",
        options=TextSearchOptions(file_filter="*.md"),
    )

    assert result.matches == ()
    assert result.warnings == ()
    assert result.error == "A workspace root is required when using a file filter."


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


def test_clean_active_override_prefers_disk_and_falls_back_if_missing(
    tmp_path: Path,
) -> None:
    active = tmp_path / "active.md"
    missing = tmp_path / "missing.md"
    active.write_text("fresh disk needle\n", encoding="utf-8")

    disk_result = search_text(
        (active,),
        "needle",
        active_override=TextSearchOverride(active, "stale memory", prefer_disk=True),
    )
    fallback_result = search_text(
        (),
        "draft",
        active_override=TextSearchOverride(missing, "local draft", prefer_disk=True),
    )

    assert [match.preview for match in disk_result.matches] == ["fresh disk needle"]
    assert [match.path for match in fallback_result.matches] == [missing]
    assert len(fallback_result.warnings) == 1
    assert "Using open source" in fallback_result.warnings[0]


def test_long_preview_is_bounded_and_keeps_the_match_visible(tmp_path: Path) -> None:
    path = tmp_path / "long.md"
    path.write_text("a" * 300 + " needle " + "z" * 300, encoding="utf-8")

    result = search_text((path,), "needle")

    preview = result.matches[0].preview
    assert len(preview) <= MAX_PREVIEW_LENGTH
    assert "needle" in preview
    assert preview.startswith("…")
    assert preview.endswith("…")


def test_cancelled_search_stops_before_loading_more_files(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("needle\n", encoding="utf-8")
    second.write_text("needle\n", encoding="utf-8")
    loaded_paths: list[Path] = []
    safe_load = load_file

    def tracked_load(path: Path) -> LoadedFile:
        loaded_paths.append(path)
        return safe_load(path)

    monkeypatch.setattr("termwriter.services.text_search.load_file", tracked_load)

    result = search_text(
        (first, second),
        "needle",
        should_cancel=lambda: bool(loaded_paths),
    )

    assert loaded_paths == [first]
    assert result.matches == ()


def test_cancelled_search_stops_between_lines(tmp_path: Path) -> None:
    path = tmp_path / "large.md"
    path.write_text("no match\n" * 100, encoding="utf-8")
    checks = 0

    def cancel_during_matching() -> bool:
        nonlocal checks
        checks += 1
        return checks > 5

    search_text((path,), "needle", should_cancel=cancel_during_matching)

    assert checks == 6


def test_fuzzy_text_search_ranks_matches_by_tightness_before_source_order(
    tmp_path: Path,
) -> None:
    path = tmp_path / "notes.md"
    path.write_text(
        "alpha beta gamma\nprefix abg\nxxabg\nabg\n",
        encoding="utf-8",
    )

    result = search_text(
        (path,),
        "abg",
        options=TextSearchOptions(mode=TextSearchMode.FUZZY),
    )

    assert [(match.line, match.column) for match in result.matches] == [
        (3, 0),
        (1, 7),
        (2, 2),
        (0, 4),
    ]


def test_fuzzy_text_search_applies_limit_after_global_ranking(tmp_path: Path) -> None:
    early = tmp_path / "a.md"
    late = tmp_path / "z.md"
    early.write_text("alpha beta gamma\n", encoding="utf-8")
    late.write_text("abg\n", encoding="utf-8")

    result = search_text(
        (early, late),
        "abg",
        limit=1,
        options=TextSearchOptions(mode=TextSearchMode.FUZZY),
    )

    assert [match.path for match in result.matches] == [late]


def test_fuzzy_text_search_preserves_unicode_source_column(tmp_path: Path) -> None:
    path = tmp_path / "unicode.md"
    path.write_text("X Straße\n", encoding="utf-8")

    result = search_text(
        (path,),
        "ss",
        options=TextSearchOptions(mode=TextSearchMode.FUZZY),
    )

    assert [(match.line, match.column) for match in result.matches] == [(0, 6)]


def test_fuzzy_text_search_matches_canonically_equivalent_unicode(
    tmp_path: Path,
) -> None:
    path = tmp_path / "unicode.md"
    path.write_text("Prefix Cafe\N{COMBINING ACUTE ACCENT} notes\n", encoding="utf-8")

    result = search_text(
        (path,),
        "CAFÉ",
        options=TextSearchOptions(mode=TextSearchMode.FUZZY),
    )

    assert [(match.line, match.column) for match in result.matches] == [(0, 7)]


def test_fuzzy_text_search_checks_cancellation_within_a_source_line(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "long.md"
    path.write_text("a" * 10_000 + "!", encoding="utf-8")
    loaded = False
    checks_after_load = 0
    safe_load = load_file

    def tracked_load(source_path: Path) -> LoadedFile:
        nonlocal loaded
        result = safe_load(source_path)
        loaded = True
        return result

    def cancel_during_line() -> bool:
        nonlocal checks_after_load
        if not loaded:
            return False
        checks_after_load += 1
        return checks_after_load >= 4

    monkeypatch.setattr("termwriter.services.text_search.load_file", tracked_load)

    result = search_text(
        (path,),
        "a" * 500 + "z",
        should_cancel=cancel_during_line,
        options=TextSearchOptions(mode=TextSearchMode.FUZZY),
    )

    assert checks_after_load >= 4
    assert result.matches == ()


def test_fuzzy_text_search_respects_case_sensitive_option(tmp_path: Path) -> None:
    path = tmp_path / "case.md"
    path.write_text("Alpha Beta\n", encoding="utf-8")

    insensitive = search_text(
        (path,),
        "ab",
        options=TextSearchOptions(mode=TextSearchMode.FUZZY),
    )
    sensitive = search_text(
        (path,),
        "ab",
        options=TextSearchOptions(
            mode=TextSearchMode.FUZZY,
            case_sensitive=True,
        ),
    )

    assert [(match.line, match.column) for match in insensitive.matches] == [(0, 4)]
    assert sensitive.matches == ()
