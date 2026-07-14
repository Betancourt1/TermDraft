"""Exact active-document matching and replacement helpers."""

from termdraft.services.document_search import (
    DocumentSearchMatch,
    find_document_matches,
    location_to_offset,
    offset_to_location,
    replace_document_matches,
)


def test_literal_matches_are_non_overlapping_source_spans() -> None:
    assert find_document_matches("aaaa", "aa", case_sensitive=True) == (
        DocumentSearchMatch(0, 2),
        DocumentSearchMatch(2, 4),
    )


def test_case_insensitive_matches_preserve_unicode_source_spans() -> None:
    source = "Straße and STRASSE"

    matches = find_document_matches(source, "strasse")

    assert matches == (DocumentSearchMatch(0, 6), DocumentSearchMatch(11, 18))
    assert tuple(source[match.start : match.end] for match in matches) == ("Straße", "STRASSE")
    assert find_document_matches(source, "strasse", case_sensitive=True) == ()


def test_replace_all_uses_the_captured_non_overlapping_matches() -> None:
    source = "one ONE one"
    matches = find_document_matches(source, "one")

    assert replace_document_matches(source, matches, "two") == "two two two"


def test_source_offsets_and_locations_support_mixed_line_endings() -> None:
    source = "one\r\ntwo\nthree\rfour"

    assert location_to_offset(source, (0, 3)) == 3
    assert location_to_offset(source, (1, 2)) == 7
    assert location_to_offset(source, (3, 4)) == len(source)
    assert offset_to_location(source, 7) == (1, 2)
    assert offset_to_location(source, len(source)) == (3, 4)
