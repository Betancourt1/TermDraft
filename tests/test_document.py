"""Tests for document state and derived values."""

from pathlib import Path

from termdraft.models.document import (
    Document,
    FileSnapshot,
    LineEndingStyle,
    analyze_line_endings,
)


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


def test_explicit_discard_restores_saved_source_and_clears_recovery_state() -> None:
    document = make_document()
    document.restore_recovery(
        "recovered change",
        "utf-8",
        FileSnapshot(exists=True, digest="older"),
    )

    document.discard_changes()

    assert document.text == "saved"
    assert not document.dirty
    assert not document.conflict
    assert not document.recovery_saved
    assert document.recovery_base_snapshot is None
    assert document.last_save_status == "Changes discarded"


def test_word_count_handles_markdown_and_unicode() -> None:
    document = make_document("# Café\n\n**naïve** don't 東京")

    assert document.word_count == 4


def test_empty_document_has_no_words() -> None:
    assert make_document("").word_count == 0


def test_line_ending_analysis_detects_uniform_and_mixed_sources() -> None:
    assert analyze_line_endings("one\ntwo\n") == (LineEndingStyle.LF, "\n")
    assert analyze_line_endings("one\r\ntwo\r\n") == (LineEndingStyle.CRLF, "\r\n")
    assert analyze_line_endings("one\rtwo\r") == (LineEndingStyle.CR, "\r")
    assert analyze_line_endings("one\r\ntwo\n") == (LineEndingStyle.MIXED, "\r\n")
    assert analyze_line_endings("one\ntwo\r\n") == (LineEndingStyle.MIXED, "\r\n")
    assert analyze_line_endings("no separator") == (LineEndingStyle.NONE, None)


def test_document_exposes_mixed_line_ending_normalization_target() -> None:
    document = make_document("one\r\ntwo\n")

    assert document.has_mixed_line_endings
    assert document.line_ending_label == "MIXED→CRLF"

    document.update_text("one\r\ntwo\r\n")
    assert document.line_ending_label == "CRLF"

    document.update_text(document.saved_text)
    assert document.line_ending_label == "MIXED→CRLF"

    document.update_text("one\r\ntwo\r\n")
    document.mark_saved(FileSnapshot(exists=True, digest="normalized"))

    assert not document.has_mixed_line_endings
    assert document.line_ending_label == "CRLF"


def test_recovered_draft_tracks_whether_disk_changed_since_its_baseline() -> None:
    document = make_document("disk")
    document.snapshot = FileSnapshot(exists=True, digest="current")

    document.restore_recovery(
        "draft",
        "utf-8",
        FileSnapshot(exists=True, digest="older", device=1, inode=1),
    )

    assert document.dirty
    assert document.recovery_saved
    assert document.recovery_conflict
    assert document.conflict
    assert document.last_save_status == "Recovered conflict"

    document.accept_unchanged_snapshot(FileSnapshot(exists=True, digest="current"))
    assert document.conflict

    document.mark_saved(FileSnapshot(exists=True, digest="saved"))
    assert not document.recovery_saved
    assert not document.recovery_conflict
