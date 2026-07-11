"""Tests for document state and derived values."""

from pathlib import Path

from termwriter.models.document import Document, FileSnapshot


def make_document(text: str = "saved") -> Document:
    return Document(
        path=Path("note.md"),
        text=text,
        saved_text=text,
        snapshot=FileSnapshot(exists=True, digest="baseline"),
    )


def test_dirty_tracks_edits_and_reverts() -> None:
    document = make_document()

    document.update_text("changed")
    assert document.dirty

    document.update_text("saved")
    assert not document.dirty


def test_mark_saved_advances_the_baseline() -> None:
    document = make_document()
    document.update_text("changed")
    snapshot = FileSnapshot(exists=True, digest="new")

    document.mark_saved(snapshot)

    assert not document.dirty
    assert document.saved_text == "changed"
    assert document.snapshot is snapshot


def test_word_count_handles_markdown_and_unicode() -> None:
    document = make_document("# Café\n\n**naïve** don't 東京")

    assert document.word_count == 4


def test_empty_document_has_no_words() -> None:
    assert make_document("").word_count == 0
