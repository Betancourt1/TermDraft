"""Tests for shared workspace-relative path filters."""

from pathlib import Path

import pytest

from termdraft.services.path_filter import PathFilterError, parse_path_filter


def test_empty_path_filter_is_disabled() -> None:
    assert parse_path_filter(None) is None
    assert parse_path_filter("  ") is None


def test_compound_filter_ors_includes_and_exclusions_win(tmp_path: Path) -> None:
    path_filter = parse_path_filter(
        " docs/**/*.md, *.markdown, !docs/drafts/**, !private.markdown "
    )

    assert path_filter is not None
    assert path_filter.matches(tmp_path / "docs" / "guide.md", root=tmp_path)
    assert path_filter.matches(tmp_path / "docs" / "deep" / "guide.md", root=tmp_path)
    assert path_filter.matches(tmp_path / "notes.markdown", root=tmp_path)
    assert not path_filter.matches(tmp_path / "docs" / "drafts" / "idea.md", root=tmp_path)
    assert not path_filter.matches(tmp_path / "private.markdown", root=tmp_path)
    assert not path_filter.matches(tmp_path / "notes.txt", root=tmp_path)


def test_exclusion_only_filter_starts_with_all_workspace_paths(tmp_path: Path) -> None:
    path_filter = parse_path_filter("!archive/**, !private.md")

    assert path_filter is not None
    assert path_filter.matches(tmp_path / "notes" / "public.md", root=tmp_path)
    assert not path_filter.matches(tmp_path / "archive" / "old.md", root=tmp_path)
    assert not path_filter.matches(tmp_path / "notes" / "private.md", root=tmp_path)


def test_filter_is_unicode_normalized_and_case_insensitive(tmp_path: Path) -> None:
    path_filter = parse_path_filter("CAFÉ/**/*.MD")
    decomposed = tmp_path / "cafe\N{COMBINING ACUTE ACCENT}" / "Résumé.md"

    assert path_filter is not None
    assert path_filter.matches(decomposed, root=tmp_path)
    assert not path_filter.matches(tmp_path.parent / "CAFÉ" / "outside.md", root=tmp_path)


@pytest.mark.parametrize(
    "expression",
    (
        "*.md,,!archive/**",
        "*.md, ",
        "!",
        "/absolute/*.md",
        "../*.md",
        "docs/../*.md",
    ),
)
def test_invalid_filter_terms_are_rejected(expression: str) -> None:
    with pytest.raises(PathFilterError):
        parse_path_filter(expression)
