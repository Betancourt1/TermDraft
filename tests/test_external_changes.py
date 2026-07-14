"""Tests for external-change and conflict classification."""

from __future__ import annotations

import os
from dataclasses import FrozenInstanceError
from pathlib import Path

import pytest

from termdraft.models.document import Document, FileSnapshot
from termdraft.services.external_changes import (
    DiskProbe,
    ExternalChangeKind,
    classify_external_change,
    detect_external_change,
    probe_file,
)
from termdraft.services.persistence import load_file


def open_document(path: Path) -> Document:
    loaded = load_file(path)
    return Document(
        path=path,
        text=loaded.text,
        saved_text=loaded.text,
        snapshot=loaded.snapshot,
        encoding=loaded.encoding,
    )


def test_unchanged_file_is_not_reported_as_external_change(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("same", encoding="utf-8")

    change = detect_external_change(open_document(path))

    assert change.kind is ExternalChangeKind.UNCHANGED


def test_disk_probe_is_immutable() -> None:
    probe = DiskProbe(Path("note.md"), FileSnapshot.missing())

    with pytest.raises(FrozenInstanceError):
        probe.error = "changed"  # type: ignore[misc]


def test_probe_file_returns_the_requested_path_and_snapshot(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("source", encoding="utf-8")

    probe = probe_file(path)

    assert probe.path == path
    assert probe.snapshot is not None
    assert probe.snapshot.exists
    assert probe.error is None


def test_probe_file_represents_a_missing_file_as_a_successful_probe(tmp_path: Path) -> None:
    path = tmp_path / "missing.md"

    probe = probe_file(path)

    assert probe.snapshot is not None
    assert not probe.snapshot.exists
    assert probe.error is None


def test_probe_file_captures_an_inaccessible_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"

    def fail(_path: Path) -> FileSnapshot:
        raise PermissionError("permission denied")

    monkeypatch.setattr("termdraft.services.external_changes.snapshot_file", fail)

    probe = probe_file(path)

    assert probe == DiskProbe(path, None, "permission denied")


def test_classification_is_pure_and_does_not_probe_disk(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    baseline = FileSnapshot(exists=True, digest="same", device=1, inode=2)
    current = FileSnapshot(exists=True, digest="same", device=1, inode=2)

    def fail(_path: Path) -> FileSnapshot:
        raise AssertionError("classification must not access disk")

    monkeypatch.setattr("termdraft.services.external_changes.snapshot_file", fail)

    change = classify_external_change(
        baseline,
        dirty=False,
        probe=DiskProbe(Path("note.md"), current),
    )

    assert change.kind is ExternalChangeKind.UNCHANGED
    assert change.snapshot is current


@pytest.mark.parametrize("dirty", [False, True])
def test_unchanged_disk_is_unchanged_even_with_local_edits(dirty: bool) -> None:
    baseline = FileSnapshot(exists=True, digest="same", device=1, inode=2)
    current = FileSnapshot(
        exists=True,
        digest="same",
        mtime_ns=999,
        device=1,
        inode=2,
    )

    change = classify_external_change(
        baseline,
        dirty=dirty,
        probe=DiskProbe(Path("note.md"), current),
    )

    assert change.kind is ExternalChangeKind.UNCHANGED
    assert change.snapshot is current


@pytest.mark.parametrize(
    ("dirty", "expected"),
    [
        (False, ExternalChangeKind.MODIFIED),
        (True, ExternalChangeKind.CONFLICT),
    ],
)
def test_same_content_from_a_replaced_file_uses_local_state(
    dirty: bool,
    expected: ExternalChangeKind,
) -> None:
    baseline = FileSnapshot(exists=True, digest="same", device=1, inode=2)
    replacement = FileSnapshot(exists=True, digest="same", device=1, inode=3)

    change = classify_external_change(
        baseline,
        dirty=dirty,
        probe=DiskProbe(Path("note.md"), replacement),
    )

    assert change.kind is expected
    assert change.snapshot is replacement


@pytest.mark.parametrize(
    ("dirty", "expected"),
    [
        (False, ExternalChangeKind.MODIFIED),
        (True, ExternalChangeKind.CONFLICT),
    ],
)
def test_changed_content_uses_local_state(
    dirty: bool,
    expected: ExternalChangeKind,
) -> None:
    baseline = FileSnapshot(exists=True, digest="base", device=1, inode=2)
    current = FileSnapshot(exists=True, digest="external", device=1, inode=2)

    change = classify_external_change(
        baseline,
        dirty=dirty,
        probe=DiskProbe(Path("note.md"), current),
    )

    assert change.kind is expected


@pytest.mark.parametrize(
    ("dirty", "expected"),
    [
        (False, ExternalChangeKind.DELETED),
        (True, ExternalChangeKind.CONFLICT),
    ],
)
def test_missing_file_uses_local_state(
    dirty: bool,
    expected: ExternalChangeKind,
) -> None:
    baseline = FileSnapshot(exists=True, digest="base")
    missing = FileSnapshot.missing()

    change = classify_external_change(
        baseline,
        dirty=dirty,
        probe=DiskProbe(Path("note.md"), missing),
    )

    assert change.kind is expected
    assert change.snapshot is missing


@pytest.mark.parametrize("dirty", [False, True])
def test_failed_probe_is_inaccessible_regardless_of_local_state(dirty: bool) -> None:
    change = classify_external_change(
        FileSnapshot(exists=True, digest="base"),
        dirty=dirty,
        probe=DiskProbe(Path("note.md"), None, "permission denied"),
    )

    assert change.kind is ExternalChangeKind.INACCESSIBLE
    assert change.snapshot is None
    assert change.detail == "permission denied"


def test_external_modification_is_detected_even_with_same_metadata(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("aaaa", encoding="utf-8")
    document = open_document(path)
    original_stat = path.stat()
    path.write_text("bbbb", encoding="utf-8")
    os.utime(path, ns=(original_stat.st_atime_ns, original_stat.st_mtime_ns))

    change = detect_external_change(document)

    assert change.kind is ExternalChangeKind.MODIFIED


def test_same_content_replacement_is_detected_by_identity(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    replacement = tmp_path / "replacement.md"
    path.write_text("same", encoding="utf-8")
    document = open_document(path)
    replacement.write_text("same", encoding="utf-8")
    replacement.replace(path)

    change = detect_external_change(document)

    assert change.kind is ExternalChangeKind.MODIFIED


def test_metadata_only_touch_refreshes_snapshot_without_a_content_conflict(
    tmp_path: Path,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("same", encoding="utf-8")
    document = open_document(path)
    original_mtime = document.snapshot.mtime_ns
    assert original_mtime is not None
    os.utime(path, ns=(path.stat().st_atime_ns, original_mtime + 1_000_000))

    change = detect_external_change(document)

    assert change.kind is ExternalChangeKind.UNCHANGED
    assert change.snapshot is not None
    assert change.snapshot.mtime_ns != original_mtime


def test_local_and_external_modifications_are_a_conflict(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    document = open_document(path)
    document.update_text("local")
    path.write_text("external", encoding="utf-8")

    change = detect_external_change(document)

    assert change.kind is ExternalChangeKind.CONFLICT


def test_deleted_open_file_is_detected(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    document = open_document(path)
    path.unlink()

    change = detect_external_change(document)

    assert change.kind is ExternalChangeKind.DELETED
