"""Tests for the separate crash-recovery journal."""

from __future__ import annotations

import json
import os
import stat
from datetime import UTC, datetime, timedelta
from pathlib import Path
from threading import Event, Thread
from unittest.mock import patch

import pytest

from termwriter.models.document import FileSnapshot
from termwriter.services.recovery import (
    RecoveryError,
    RecoveryJournal,
    RecoveryRecord,
    default_recovery_root,
)

_BASE_DIGEST = "a" * 64
_BASE_SNAPSHOT = FileSnapshot(
    exists=True,
    digest=_BASE_DIGEST,
    size=4,
    mtime_ns=123,
    mode=0o600,
    device=10,
    inode=20,
    parent_device=10,
    parent_inode=2,
)


def _quarantine_draft(
    journal: RecoveryJournal,
    *,
    document_path: Path,
    workspace_root: Path,
    text: str,
    encoding: str = "utf-8",
    updated_at: datetime | None = None,
) -> RecoveryRecord:
    journal.save(
        document_path=document_path,
        workspace_root=workspace_root,
        text=text,
        encoding=encoding,
        base_snapshot=FileSnapshot.missing(),
    )
    active_path = journal.path_for(document_path)
    if updated_at is not None:
        payload = json.loads(active_path.read_text(encoding="utf-8"))
        payload["updated_at"] = updated_at.isoformat()
        active_path.write_text(json.dumps(payload), encoding="utf-8")
    active_record = next(
        record for record in journal.list_entries() if record.journal_path == active_path
    )
    quarantine_path = journal.quarantine(active_record)
    return next(
        record for record in journal.list_quarantined() if record.journal_path == quarantine_path
    )


def test_recovery_round_trip_preserves_exact_source(tmp_path: Path) -> None:
    state_root = tmp_path / "state"
    workspace = tmp_path / "notes"
    document = workspace / "café 東京.md"
    text = "# Café\r\n\r\n東京\nlast line without newline"
    journal = RecoveryJournal(state_root)

    saved = journal.save(
        document_path=document,
        workspace_root=workspace,
        text=text,
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    loaded = journal.load(document)

    assert loaded == saved
    assert loaded is not None
    assert loaded.text == text
    assert loaded.updated_at.utcoffset() is not None
    assert journal.path_for(document).parent == state_root
    assert journal.path_for(document).name == f"{journal.path_for(document).stem}.json"
    assert document.name not in journal.path_for(document).name
    assert stat.S_IMODE(state_root.stat().st_mode) == 0o700
    assert stat.S_IMODE(journal.path_for(document).stat().st_mode) == 0o600


def test_recovery_write_replaces_from_same_directory_and_syncs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    state_root = tmp_path / "state"
    document = tmp_path / "note.md"
    journal = RecoveryJournal(state_root)
    real_replace = os.replace
    real_fsync = os.fsync
    replacements: list[tuple[Path, Path]] = []
    syncs: list[int] = []

    def tracking_replace(
        source: str | os.PathLike[str], destination: str | os.PathLike[str]
    ) -> None:
        source_path = Path(source)
        destination_path = Path(destination)
        replacements.append((source_path, destination_path))
        assert source_path.parent == destination_path.parent == state_root
        real_replace(source, destination)

    def tracking_fsync(descriptor: int) -> None:
        syncs.append(descriptor)
        real_fsync(descriptor)

    monkeypatch.setattr("termwriter.services.recovery.os.replace", tracking_replace)
    monkeypatch.setattr("termwriter.services.recovery.os.fsync", tracking_fsync)

    journal.save(
        document_path=document,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    assert len(replacements) == 1
    assert len(syncs) == 2
    assert list(state_root.glob("*.tmp")) == []


def test_failed_replace_keeps_previous_journal_and_cleans_temporary_file(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    state_root = tmp_path / "state"
    document = tmp_path / "note.md"
    journal = RecoveryJournal(state_root)
    journal.save(
        document_path=document,
        workspace_root=tmp_path,
        text="first draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    def broken_replace(source: object, destination: object) -> None:
        del source, destination
        raise OSError("injected replacement failure")

    monkeypatch.setattr("termwriter.services.recovery.os.replace", broken_replace)

    with pytest.raises(RecoveryError, match="Cannot save recovery journal"):
        journal.save(
            document_path=document,
            workspace_root=tmp_path,
            text="second draft",
            encoding="utf-8",
            base_snapshot=_BASE_SNAPSHOT,
        )

    loaded = journal.load(document)
    assert loaded is not None
    assert loaded.text == "first draft"
    assert list(state_root.glob("*.tmp")) == []


def test_failed_file_sync_leaves_no_partial_journal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    state_root = tmp_path / "state"
    document = tmp_path / "note.md"
    journal = RecoveryJournal(state_root)

    def broken_fsync(descriptor: int) -> None:
        del descriptor
        raise OSError("injected sync failure")

    monkeypatch.setattr("termwriter.services.recovery.os.fsync", broken_fsync)

    with pytest.raises(RecoveryError, match="Cannot save recovery journal"):
        journal.save(
            document_path=document,
            workspace_root=tmp_path,
            text="draft",
            encoding="utf-8",
            base_snapshot=_BASE_SNAPSHOT,
        )

    assert journal.load(document) is None
    assert list(state_root.glob("*.tmp")) == []


@pytest.mark.parametrize(
    "payload, message",
    [
        (b"not JSON", "Invalid recovery journal"),
        (json.dumps({"version": 99}).encode(), "Unsupported recovery journal version"),
        (json.dumps({"version": True}).encode(), "Unsupported recovery journal version"),
        (
            json.dumps(
                {
                    "version": 1,
                    "document_path": "/tmp/note.md",
                    "workspace_root": "/tmp",
                    "text": "draft",
                    "encoding": "latin-1",
                    "base_digest": None,
                    "updated_at": "2026-07-11T12:00:00+00:00",
                }
            ).encode(),
            "Unsupported recovery encoding",
        ),
    ],
)
def test_invalid_recovery_data_fails_safely(
    tmp_path: Path,
    payload: bytes,
    message: str,
) -> None:
    document = tmp_path / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    journal.path_for(document).write_bytes(payload)

    with pytest.raises(RecoveryError, match=message):
        journal.load(document)


def test_recovery_rejects_a_journal_for_another_document(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    expected = tmp_path / "expected.md"
    other = tmp_path / "other.md"
    entry = journal.save(
        document_path=other,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8-sig",
        base_snapshot=FileSnapshot.missing(parent_device=10, parent_inode=2),
    )
    journal.path_for(expected).parent.mkdir(parents=True, exist_ok=True)
    journal.path_for(expected).write_bytes(journal.path_for(entry.document_path).read_bytes())

    with pytest.raises(RecoveryError, match="path mismatch"):
        journal.load(expected)


def test_delete_is_idempotent(tmp_path: Path) -> None:
    document = tmp_path / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=document,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    journal.delete(document)
    journal.delete(document)

    assert journal.load(document) is None


def test_publish_returns_exact_record_for_conditional_deletion(tmp_path: Path) -> None:
    document = tmp_path / "note.md"
    journal = RecoveryJournal(tmp_path / "state")

    record = journal.publish(
        document_path=document,
        workspace_root=tmp_path,
        text="draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    assert record.entry is not None
    assert record.entry.text == "draft"
    assert record == journal.record_for(document)
    journal.delete_expected(document, fingerprint=record.fingerprint)
    assert journal.record_for(document) is None


def test_conditional_delete_preserves_a_newer_publication(tmp_path: Path) -> None:
    document = tmp_path / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    older = journal.publish(
        document_path=document,
        workspace_root=tmp_path,
        text="older",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    journal.save(
        document_path=document,
        workspace_root=tmp_path,
        text="newer",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    with pytest.raises(RecoveryError, match="changed after cleanup was requested"):
        journal.delete_expected(document, fingerprint=older.fingerprint)

    current = journal.load(document)
    assert current is not None
    assert current.text == "newer"


def test_conditional_delete_preserves_entry_that_appeared_after_expected_absence(
    tmp_path: Path,
) -> None:
    document = tmp_path / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    assert journal.record_for(document) is None
    journal.save(
        document_path=document,
        workspace_root=tmp_path,
        text="other instance",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    with pytest.raises(RecoveryError, match="appeared after cleanup was requested"):
        journal.delete_expected(document, fingerprint=None)

    assert journal.load(document) is not None


def test_different_documents_use_different_journals(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")

    first = journal.path_for(tmp_path / "first.md")
    second = journal.path_for(tmp_path / "second.md")

    assert first != second
    assert len(first.stem) == 64
    assert len(second.stem) == 64


def test_default_root_honors_xdg_state_home(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))

    assert default_recovery_root() == tmp_path / "termwriter" / "recovery"


def test_legacy_digest_only_journal_loads_with_unknown_origin(tmp_path: Path) -> None:
    document = tmp_path / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    payload = {
        "version": 1,
        "document_path": str(document),
        "workspace_root": str(tmp_path),
        "text": "legacy draft",
        "encoding": "utf-8",
        "base_digest": _BASE_DIGEST,
        "updated_at": "2026-07-11T12:00:00+00:00",
    }
    journal.path_for(document).write_text(json.dumps(payload), encoding="utf-8")

    entry = journal.load(document)

    assert entry is not None
    assert entry.base_snapshot.digest == _BASE_DIGEST
    assert entry.base_snapshot.inode is None
    assert entry.base_snapshot.parent_inode is None


def test_workspace_scan_finds_missing_document_recovery_and_filters_other_roots(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    other_workspace = tmp_path / "other"
    workspace.mkdir()
    other_workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    wanted = workspace / "deleted.md"
    journal.save(
        document_path=wanted,
        workspace_root=workspace,
        text="recover me",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    journal.save(
        document_path=other_workspace / "other.md",
        workspace_root=other_workspace,
        text="other",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    result = journal.scan_workspace(workspace)

    assert tuple(entry.document_path for entry in result.entries) == (wanted,)
    assert result.warnings == ()


def test_workspace_scan_reports_corrupt_entry_without_losing_valid_entries(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    wanted = workspace / "deleted.md"
    journal.save(
        document_path=wanted,
        workspace_root=workspace,
        text="recover me",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    (journal.state_root / f"{'f' * 64}.json").write_text("not json", encoding="utf-8")

    result = journal.scan_workspace(workspace)

    assert tuple(entry.document_path for entry in result.entries) == (wanted,)
    assert len(result.warnings) == 1


def test_inventory_exposes_corrupt_entry_without_reading_it_as_a_draft(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    other_workspace = tmp_path / "other"
    workspace.mkdir()
    other_workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    document = workspace / "valid.md"
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="valid draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    journal.save(
        document_path=other_workspace / "other.md",
        workspace_root=other_workspace,
        text="other draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    corrupt_path = journal.state_root / f"{'f' * 64}.json"
    corrupt_bytes = b"not UTF-8: \xff\x00"
    corrupt_path.write_bytes(corrupt_bytes)

    records = journal.list_entries(workspace)
    valid = next(record for record in records if record.journal_path == journal.path_for(document))
    corrupt = next(record for record in records if record.journal_path == corrupt_path)

    assert valid.entry is not None
    assert not valid.is_corrupt
    assert corrupt.entry is None
    assert corrupt.is_corrupt
    assert "Invalid recovery journal" in (corrupt.error or "")
    assert corrupt_path.read_bytes() == corrupt_bytes
    assert all(
        record.entry is None or record.entry.workspace_root == workspace for record in records
    )


def test_corrupt_entry_can_be_quarantined_without_changing_its_bytes(
    tmp_path: Path,
) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    corrupt_path = journal.state_root / f"{'e' * 64}.json"
    corrupt_bytes = b"{definitely not JSON}\r\n"
    corrupt_path.write_bytes(corrupt_bytes)
    (record,) = journal.list_entries()

    destination = journal.quarantine(record)

    assert not corrupt_path.exists()
    assert destination == journal.state_root / "quarantine" / corrupt_path.name
    assert destination.read_bytes() == corrupt_bytes
    assert journal.list_entries() == ()


def test_quarantine_collision_preserves_both_files(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    corrupt_path = journal.state_root / f"{'d' * 64}.json"
    corrupt_path.write_bytes(b"new corrupt draft")
    (record,) = journal.list_entries()
    quarantine_path = journal.state_root / "quarantine" / corrupt_path.name
    quarantine_path.parent.mkdir()
    quarantine_path.write_bytes(b"older quarantined draft")

    with pytest.raises(RecoveryError, match="already contains"):
        journal.quarantine(record)

    assert corrupt_path.read_bytes() == b"new corrupt draft"
    assert quarantine_path.read_bytes() == b"older quarantined draft"


def test_quarantine_refuses_corrupt_entry_changed_after_listing(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    corrupt_path = journal.state_root / f"{'c' * 64}.json"
    corrupt_path.write_bytes(b"first corrupt draft")
    (record,) = journal.list_entries()
    corrupt_path.write_bytes(b"newer corrupt draft")

    with pytest.raises(RecoveryError, match="changed after it was listed"):
        journal.quarantine(record)

    assert corrupt_path.read_bytes() == b"newer corrupt draft"
    assert not (journal.state_root / "quarantine").exists()


def test_quarantine_lock_preserves_replacement_started_after_final_verification(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "draft.md"
    journal = RecoveryJournal(tmp_path / "state")
    writer = RecoveryJournal(journal.state_root)
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="older unsaved draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (record,) = journal.list_entries(workspace)
    replacement_started = Event()
    replacement_finished = Event()

    def replace_journal() -> None:
        replacement_started.set()
        writer.save(
            document_path=document,
            workspace_root=workspace,
            text="newer unsaved draft",
            encoding="utf-8",
            base_snapshot=FileSnapshot.missing(),
        )
        replacement_finished.set()

    replacement = Thread(target=replace_journal)
    original_verify = journal._verify_record
    verification_count = 0

    def verify_then_replace(item: RecoveryRecord) -> RecoveryRecord:
        nonlocal verification_count
        current = original_verify(item)
        verification_count += 1
        if verification_count == 2:
            replacement.start()
            assert replacement_started.wait(1)
            assert not replacement_finished.wait(0.05)
        return current

    with patch.object(journal, "_verify_record", side_effect=verify_then_replace):
        destination = journal.quarantine(record)

    replacement.join(timeout=2)
    assert not replacement.is_alive()
    active = journal.load(document)
    assert active is not None
    assert active.text == "newer unsaved draft"
    assert json.loads(destination.read_text(encoding="utf-8"))["text"] == "older unsaved draft"


def test_quarantine_inventory_marks_records_and_filters_trusted_workspaces(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    other_workspace = tmp_path / "other"
    workspace.mkdir()
    other_workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    wanted = workspace / "wanted.md"
    other = other_workspace / "other.md"
    for document, root in ((wanted, workspace), (other, other_workspace)):
        journal.save(
            document_path=document,
            workspace_root=root,
            text=document.stem,
            encoding="utf-8",
            base_snapshot=FileSnapshot.missing(),
        )
        record = next(
            item
            for item in journal.list_entries()
            if item.entry is not None and item.entry.document_path == document
        )
        journal.quarantine(record)
    corrupt_path = journal.state_root / "quarantine" / f"{'f' * 64}.json"
    corrupt_bytes = b"not UTF-8: \xff\x00"
    corrupt_path.write_bytes(corrupt_bytes)

    records = journal.list_quarantined(workspace)

    trusted = next(record for record in records if record.entry is not None)
    corrupt = next(record for record in records if record.entry is None)
    assert trusted.entry is not None
    assert trusted.entry.document_path == wanted
    assert trusted.quarantined
    assert not trusted.is_corrupt
    assert corrupt.journal_path == corrupt_path
    assert corrupt.quarantined
    assert corrupt.is_corrupt
    assert corrupt_path.read_bytes() == corrupt_bytes
    assert all(
        record.entry is None or record.entry.workspace_root == workspace for record in records
    )


def test_restore_quarantined_preserves_exact_journal_bytes(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "café 東京.md"
    journal = RecoveryJournal(tmp_path / "state")
    saved = journal.save(
        document_path=document,
        workspace_root=workspace,
        text="# Café\r\n\r\n東京\nno final newline",
        encoding="utf-8-sig",
        base_snapshot=_BASE_SNAPSHOT,
    )
    active_path = journal.path_for(document)
    original_bytes = active_path.read_bytes()
    (active_record,) = journal.list_entries(workspace)
    journal.quarantine(active_record)
    (quarantined_record,) = journal.list_quarantined(workspace)

    restored = journal.restore_quarantined(quarantined_record)

    assert restored == saved
    assert active_path.read_bytes() == original_bytes
    assert journal.load(document) == saved
    assert journal.list_quarantined() == ()


def test_restore_quarantined_never_replaces_an_active_draft(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="archived draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    quarantine_path = journal.quarantine(active_record)
    archived_bytes = quarantine_path.read_bytes()
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="new active draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    active_bytes = journal.path_for(document).read_bytes()
    (quarantined_record,) = journal.list_quarantined(workspace)

    with pytest.raises(RecoveryError, match="active recovery entry already exists"):
        journal.restore_quarantined(quarantined_record)

    assert journal.path_for(document).read_bytes() == active_bytes
    assert quarantine_path.read_bytes() == archived_bytes


def test_restore_quarantined_rejects_corrupt_entry_without_creating_active_file(
    tmp_path: Path,
) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    corrupt_path = quarantine_root / f"{'e' * 64}.json"
    corrupt_bytes = b"{not valid JSON}\r\n"
    corrupt_path.write_bytes(corrupt_bytes)
    (record,) = journal.list_quarantined()

    with pytest.raises(RecoveryError, match="Cannot restore a corrupt recovery entry"):
        journal.restore_quarantined(record)

    assert corrupt_path.read_bytes() == corrupt_bytes
    assert journal.list_entries() == ()


def test_restore_quarantined_refuses_entry_changed_after_listing(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="listed draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    quarantine_path = journal.quarantine(active_record)
    (record,) = journal.list_quarantined(workspace)
    replacement_bytes = quarantine_path.read_bytes().replace(b"listed", b"newest")
    quarantine_path.write_bytes(replacement_bytes)

    with pytest.raises(RecoveryError, match="changed after it was listed"):
        journal.restore_quarantined(record)

    assert quarantine_path.read_bytes() == replacement_bytes
    assert journal.list_entries() == ()


def test_restore_failure_keeps_quarantined_bytes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="keep me",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    quarantine_path = journal.quarantine(active_record)
    quarantined_bytes = quarantine_path.read_bytes()
    (record,) = journal.list_quarantined(workspace)

    def broken_link(*args: object, **kwargs: object) -> None:
        del args, kwargs
        raise OSError("injected restore failure")

    monkeypatch.setattr("termwriter.services.recovery.os.link", broken_link)

    with pytest.raises(RecoveryError, match="injected restore failure"):
        journal.restore_quarantined(record)

    assert quarantine_path.read_bytes() == quarantined_bytes
    assert journal.list_entries() == ()


def test_restore_sync_failure_leaves_two_exact_safe_copies(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="keep me",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    quarantine_path = journal.quarantine(active_record)
    quarantined_bytes = quarantine_path.read_bytes()
    (record,) = journal.list_quarantined(workspace)

    def broken_sync(directory: Path) -> None:
        assert directory == journal.state_root
        raise OSError("injected directory sync failure")

    monkeypatch.setattr("termwriter.services.recovery._sync_directory", broken_sync)

    with pytest.raises(RecoveryError, match="injected directory sync failure"):
        journal.restore_quarantined(record)

    assert quarantine_path.read_bytes() == quarantined_bytes
    assert journal.path_for(document).read_bytes() == quarantined_bytes


def test_restore_lock_preserves_writer_started_during_final_verification(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    writer = RecoveryJournal(journal.state_root)
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="archived draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    journal.quarantine(active_record)
    (record,) = journal.list_quarantined(workspace)
    replacement_started = Event()
    replacement_finished = Event()

    def replace_journal() -> None:
        replacement_started.set()
        writer.save(
            document_path=document,
            workspace_root=workspace,
            text="new active draft",
            encoding="utf-8",
            base_snapshot=FileSnapshot.missing(),
        )
        replacement_finished.set()

    replacement = Thread(target=replace_journal)
    original_verify = journal._verify_record
    verification_count = 0

    def verify_then_replace(item: RecoveryRecord) -> RecoveryRecord:
        nonlocal verification_count
        current = original_verify(item)
        verification_count += 1
        if verification_count == 2:
            replacement.start()
            assert replacement_started.wait(1)
            assert not replacement_finished.wait(0.05)
        return current

    with patch.object(journal, "_verify_record", side_effect=verify_then_replace):
        restored = journal.restore_quarantined(record)

    replacement.join(timeout=2)
    assert not replacement.is_alive()
    assert restored.text == "archived draft"
    active = journal.load(document)
    assert active is not None
    assert active.text == "new active draft"
    assert journal.list_quarantined() == ()


def test_permanent_delete_is_guarded_and_can_remove_corrupt_quarantine(
    tmp_path: Path,
) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    corrupt_path = quarantine_root / f"{'a' * 64}.json"
    corrupt_path.write_bytes(b"corrupt draft bytes")
    (record,) = journal.list_quarantined()

    journal.delete_quarantined(record)

    assert not corrupt_path.exists()
    assert journal.list_quarantined() == ()


def test_permanent_delete_refuses_quarantine_changed_after_listing(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    corrupt_path = quarantine_root / f"{'b' * 64}.json"
    corrupt_path.write_bytes(b"listed bytes")
    (record,) = journal.list_quarantined()
    corrupt_path.write_bytes(b"newer bytes")

    with pytest.raises(RecoveryError, match="changed after it was listed"):
        journal.delete_quarantined(record)

    assert corrupt_path.read_bytes() == b"newer bytes"


def test_permanent_delete_fails_closed_when_bytes_could_not_be_fingerprinted(
    tmp_path: Path,
) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    quarantine_path = quarantine_root / f"{'c' * 64}.json"
    quarantine_path.write_bytes(b"old bytes")
    try:
        quarantine_path.chmod(0)
        (record,) = journal.list_quarantined()
        if record.has_content_fingerprint:
            pytest.skip("Filesystem permissions did not prevent reading the quarantine")

        quarantine_path.chmod(0o600)
        quarantine_path.write_bytes(b"new bytes")
        quarantine_path.chmod(0)

        with pytest.raises(RecoveryError, match="could not be fingerprinted"):
            journal.delete_quarantined(record)

        assert quarantine_path.exists()
        quarantine_path.chmod(0o600)
        assert quarantine_path.read_bytes() == b"new bytes"
    finally:
        if quarantine_path.exists():
            quarantine_path.chmod(0o600)


def test_quarantine_operations_reject_wrong_record_location(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    outside = tmp_path / "do-not-delete.json"
    outside.write_bytes(b"important")
    forged = RecoveryRecord(
        journal_path=outside,
        fingerprint="not trusted",
        error="forged",
        quarantined=True,
    )

    with pytest.raises(RecoveryError, match="outside the recovery quarantine"):
        journal.delete_quarantined(forged)

    assert outside.read_bytes() == b"important"


def test_quarantine_inventory_rejects_symlinked_storage(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    journal.state_root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    (journal.state_root / "quarantine").symlink_to(outside, target_is_directory=True)

    with pytest.raises(RecoveryError, match="not a real directory"):
        journal.list_quarantined()


def test_export_quarantined_preserves_exact_source_and_archive(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    text = "# Café\r\n\r\n東京\nno final newline"
    record = _quarantine_draft(
        journal,
        document_path=workspace / "original.md",
        workspace_root=workspace,
        text=text,
        encoding="utf-8-sig",
    )
    archived_bytes = record.journal_path.read_bytes()
    destination = workspace / "exports" / "recovered.markdown"
    destination.parent.mkdir()

    result = journal.export_quarantined(record, destination=destination)

    assert destination.read_bytes() == text.encode("utf-8-sig")
    assert result.snapshot.exists
    assert result.snapshot.digest is not None
    assert record.journal_path.read_bytes() == archived_bytes
    assert journal.list_quarantined(workspace) == (record,)


def test_export_quarantined_never_overwrites_an_existing_file(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    record = _quarantine_draft(
        journal,
        document_path=workspace / "original.md",
        workspace_root=workspace,
        text="archived draft",
    )
    archived_bytes = record.journal_path.read_bytes()
    destination = workspace / "occupied.md"
    destination.write_text("existing document", encoding="utf-8")

    with pytest.raises(RecoveryError, match="destination already exists"):
        journal.export_quarantined(record, destination=destination)

    assert destination.read_text(encoding="utf-8") == "existing document"
    assert record.journal_path.read_bytes() == archived_bytes


def test_export_quarantined_refuses_entry_changed_after_listing(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    record = _quarantine_draft(
        journal,
        document_path=workspace / "original.md",
        workspace_root=workspace,
        text="listed draft",
    )
    replacement_bytes = record.journal_path.read_bytes().replace(b"listed", b"newest")
    record.journal_path.write_bytes(replacement_bytes)
    destination = workspace / "recovered.md"

    with pytest.raises(RecoveryError, match="changed after it was listed"):
        journal.export_quarantined(record, destination=destination)

    assert not destination.exists()
    assert record.journal_path.read_bytes() == replacement_bytes


@pytest.mark.parametrize(
    "destination_name, message",
    [
        ("../outside.md", "outside its workspace"),
        ("recovered.txt", "not Markdown"),
    ],
)
def test_export_quarantined_rejects_invalid_destination(
    tmp_path: Path,
    destination_name: str,
    message: str,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    record = _quarantine_draft(
        journal,
        document_path=workspace / "original.md",
        workspace_root=workspace,
        text="keep archived",
    )

    with pytest.raises(RecoveryError, match=message):
        journal.export_quarantined(
            record,
            destination=workspace / destination_name,
        )

    assert record.journal_path.exists()


def test_export_quarantined_rejects_a_symlink_escape(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    outside = tmp_path / "outside"
    workspace.mkdir()
    outside.mkdir()
    (workspace / "linked").symlink_to(outside, target_is_directory=True)
    journal = RecoveryJournal(tmp_path / "state")
    record = _quarantine_draft(
        journal,
        document_path=workspace / "original.md",
        workspace_root=workspace,
        text="keep archived",
    )

    with pytest.raises(RecoveryError, match="resolves outside its workspace"):
        journal.export_quarantined(
            record,
            destination=workspace / "linked" / "escaped.md",
        )

    assert not (outside / "escaped.md").exists()
    assert record.journal_path.exists()


def test_export_quarantined_rejects_corrupt_record(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    corrupt_path = quarantine_root / f"{'d' * 64}.json"
    corrupt_path.write_bytes(b"not JSON")
    (record,) = journal.list_quarantined()

    with pytest.raises(RecoveryError, match="Cannot export a corrupt recovery entry"):
        journal.export_quarantined(record, destination=tmp_path / "recovered.md")

    assert corrupt_path.read_bytes() == b"not JSON"


def test_retention_cleanup_requires_a_timezone_aware_cutoff(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")

    with pytest.raises(RecoveryError, match="cutoff must include a timezone"):
        journal.cleanup_quarantined(before=datetime(2026, 7, 12))


def test_retention_cleanup_selects_only_valid_old_workspace_entries(
    tmp_path: Path,
) -> None:
    now = datetime.now(UTC)
    cutoff = now - timedelta(days=30)
    workspace = tmp_path / "workspace"
    other_workspace = tmp_path / "other"
    workspace.mkdir()
    other_workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    old_record = _quarantine_draft(
        journal,
        document_path=workspace / "old.md",
        workspace_root=workspace,
        text="old",
        updated_at=now - timedelta(days=90),
    )
    recent_record = _quarantine_draft(
        journal,
        document_path=workspace / "recent.md",
        workspace_root=workspace,
        text="recent",
        updated_at=cutoff,
    )
    other_record = _quarantine_draft(
        journal,
        document_path=other_workspace / "other.md",
        workspace_root=other_workspace,
        text="other",
        updated_at=now - timedelta(days=90),
    )
    corrupt_path = journal.state_root / "quarantine" / f"{'f' * 64}.json"
    corrupt_path.write_bytes(b"corrupt quarantine")

    result = journal.cleanup_quarantined(before=cutoff, workspace_root=workspace)

    assert result.cutoff == cutoff
    assert result.selected_count == 1
    assert result.deleted_count == 1
    assert result.failed_count == 0
    assert len(result.outcomes) == 1
    assert result.outcomes[0].document_path == workspace / "old.md"
    assert result.outcomes[0].deleted
    assert result.outcomes[0].error is None
    assert not old_record.journal_path.exists()
    assert recent_record.journal_path.exists()
    assert other_record.journal_path.exists()
    assert corrupt_path.read_bytes() == b"corrupt quarantine"


def test_retention_cleanup_reports_partial_failures_and_continues(
    tmp_path: Path,
) -> None:
    now = datetime.now(UTC)
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    first = _quarantine_draft(
        journal,
        document_path=workspace / "first.md",
        workspace_root=workspace,
        text="first",
        updated_at=now - timedelta(days=90),
    )
    second = _quarantine_draft(
        journal,
        document_path=workspace / "second.md",
        workspace_root=workspace,
        text="second",
        updated_at=now - timedelta(days=60),
    )
    original_delete = journal.delete_quarantined

    def delete_with_one_failure(record: RecoveryRecord) -> None:
        if record.journal_path == first.journal_path:
            raise RecoveryError("injected retention failure")
        original_delete(record)

    with patch.object(journal, "delete_quarantined", side_effect=delete_with_one_failure):
        result = journal.cleanup_quarantined(
            before=now - timedelta(days=30),
            workspace_root=workspace,
        )

    outcomes = {outcome.document_path: outcome for outcome in result.outcomes}
    assert result.selected_count == 2
    assert result.deleted_count == 1
    assert result.failed_count == 1
    assert not outcomes[workspace / "first.md"].deleted
    assert outcomes[workspace / "first.md"].error == "injected retention failure"
    assert outcomes[workspace / "second.md"].deleted
    assert outcomes[workspace / "second.md"].error is None
    assert first.journal_path.exists()
    assert not second.journal_path.exists()


def test_retention_cleanup_deletes_only_the_confirmed_inventory(tmp_path: Path) -> None:
    now = datetime.now(UTC)
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    confirmed = _quarantine_draft(
        journal,
        document_path=workspace / "confirmed.md",
        workspace_root=workspace,
        text="confirmed",
        updated_at=now - timedelta(days=90),
    )
    unconfirmed = _quarantine_draft(
        journal,
        document_path=workspace / "unconfirmed.md",
        workspace_root=workspace,
        text="unconfirmed",
        updated_at=now - timedelta(days=90),
    )

    result = journal.cleanup_quarantined(
        before=now - timedelta(days=30),
        workspace_root=workspace,
        records=(confirmed,),
    )

    assert result.deleted_count == 1
    assert not confirmed.journal_path.exists()
    assert unconfirmed.journal_path.exists()


def test_retarget_preserves_draft_and_timestamp(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    old_path = workspace / "old.md"
    new_path = workspace / "renamed.md"
    saved = journal.save(
        document_path=old_path,
        workspace_root=workspace,
        text="draft without newline",
        encoding="utf-8-sig",
        base_snapshot=_BASE_SNAPSHOT,
    )
    (record,) = journal.list_entries()

    moved = journal.retarget(
        record,
        document_path=new_path,
        workspace_root=workspace,
    )

    assert moved.document_path == new_path
    assert moved.text == saved.text
    assert moved.updated_at == saved.updated_at
    assert journal.load(old_path) is None
    assert journal.load(new_path) == moved


def test_retarget_lock_preserves_source_replacement_started_after_verification(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    old_path = workspace / "old.md"
    new_path = workspace / "renamed.md"
    journal = RecoveryJournal(tmp_path / "state")
    writer = RecoveryJournal(journal.state_root)
    journal.save(
        document_path=old_path,
        workspace_root=workspace,
        text="older unsaved draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (record,) = journal.list_entries(workspace)
    replacement_started = Event()
    replacement_finished = Event()

    def replace_source() -> None:
        replacement_started.set()
        writer.save(
            document_path=old_path,
            workspace_root=workspace,
            text="newer unsaved draft",
            encoding="utf-8",
            base_snapshot=FileSnapshot.missing(),
        )
        replacement_finished.set()

    replacement = Thread(target=replace_source)
    original_verify = journal._verify_record
    verification_count = 0

    def verify_then_replace(item: RecoveryRecord) -> RecoveryRecord:
        nonlocal verification_count
        current = original_verify(item)
        verification_count += 1
        if verification_count == 2:
            replacement.start()
            assert replacement_started.wait(1)
            assert not replacement_finished.wait(0.05)
        return current

    with patch.object(journal, "_verify_record", side_effect=verify_then_replace):
        moved = journal.retarget(
            record,
            document_path=new_path,
            workspace_root=workspace,
        )

    replacement.join(timeout=2)
    assert not replacement.is_alive()
    active_source = journal.load(old_path)
    assert active_source is not None
    assert active_source.text == "newer unsaved draft"
    assert moved.text == "older unsaved draft"
    assert journal.load(new_path) == moved


def test_retarget_collision_never_overwrites_either_draft(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    old_path = workspace / "old.md"
    occupied_path = workspace / "occupied.md"
    journal.save(
        document_path=old_path,
        workspace_root=workspace,
        text="draft from old path",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    journal.save(
        document_path=occupied_path,
        workspace_root=workspace,
        text="existing destination draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    record = next(
        item
        for item in journal.list_entries()
        if item.entry is not None and item.entry.document_path == old_path
    )

    with pytest.raises(RecoveryError, match="already exists"):
        journal.retarget(
            record,
            document_path=occupied_path,
            workspace_root=workspace,
        )

    assert journal.load(old_path) is not None
    assert journal.load(old_path).text == "draft from old path"  # type: ignore[union-attr]
    assert journal.load(occupied_path) is not None
    assert journal.load(occupied_path).text == "existing destination draft"  # type: ignore[union-attr]


def test_retarget_rejects_path_outside_workspace_and_keeps_source(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    old_path = workspace / "old.md"
    journal.save(
        document_path=old_path,
        workspace_root=workspace,
        text="keep this draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    (record,) = journal.list_entries()

    with pytest.raises(RecoveryError, match="outside its workspace"):
        journal.retarget(
            record,
            document_path=tmp_path / "outside.md",
            workspace_root=workspace,
        )

    assert journal.load(old_path) is not None
    assert journal.load(old_path).text == "keep this draft"  # type: ignore[union-attr]


def test_stale_listing_requires_explicit_guarded_deletion(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    other_workspace = tmp_path / "other"
    workspace.mkdir()
    other_workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    old_path = workspace / "old.md"
    recent_path = workspace / "recent.md"
    other_path = other_workspace / "other.md"
    for document_path, root in (
        (old_path, workspace),
        (recent_path, workspace),
        (other_path, other_workspace),
    ):
        journal.save(
            document_path=document_path,
            workspace_root=root,
            text=document_path.stem,
            encoding="utf-8",
            base_snapshot=_BASE_SNAPSHOT,
        )
    old_journal = journal.path_for(old_path)
    payload = json.loads(old_journal.read_text(encoding="utf-8"))
    payload["updated_at"] = (datetime.now(UTC) - timedelta(days=90)).isoformat()
    old_journal.write_text(json.dumps(payload), encoding="utf-8")

    stale = journal.list_stale(
        before=datetime.now(UTC) - timedelta(days=30),
        workspace_root=workspace,
    )

    assert len(stale) == 1
    assert stale[0].entry is not None
    assert stale[0].entry.document_path == old_path
    assert journal.load(old_path) is not None

    journal.delete_record(stale[0])

    assert journal.load(old_path) is None
    assert journal.load(recent_path) is not None
    assert journal.load(other_path) is not None


def test_guarded_delete_refuses_entry_changed_after_listing(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    document = workspace / "note.md"
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="first draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )
    (record,) = journal.list_entries()
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="newer draft",
        encoding="utf-8",
        base_snapshot=_BASE_SNAPSHOT,
    )

    with pytest.raises(RecoveryError, match="changed after it was listed"):
        journal.delete_record(record)

    assert journal.load(document) is not None
    assert journal.load(document).text == "newer draft"  # type: ignore[union-attr]


def test_guarded_delete_lock_preserves_replacement_started_after_verification(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    journal = RecoveryJournal(tmp_path / "state")
    writer = RecoveryJournal(journal.state_root)
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="older draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (record,) = journal.list_entries(workspace)
    replacement_started = Event()
    replacement_finished = Event()

    def replace_journal() -> None:
        replacement_started.set()
        writer.save(
            document_path=document,
            workspace_root=workspace,
            text="newer draft",
            encoding="utf-8",
            base_snapshot=FileSnapshot.missing(),
        )
        replacement_finished.set()

    replacement = Thread(target=replace_journal)
    original_verify = journal._verify_record

    def verify_then_replace(item: RecoveryRecord) -> RecoveryRecord:
        current = original_verify(item)
        replacement.start()
        assert replacement_started.wait(1)
        assert not replacement_finished.wait(0.05)
        return current

    with patch.object(journal, "_verify_record", side_effect=verify_then_replace):
        journal.delete_record(record)

    replacement.join(timeout=2)
    assert not replacement.is_alive()
    active = journal.load(document)
    assert active is not None
    assert active.text == "newer draft"


def test_recovery_paths_cannot_escape_workspace(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")

    with pytest.raises(RecoveryError, match="outside its workspace"):
        journal.save(
            document_path=tmp_path / "outside.md",
            workspace_root=workspace,
            text="draft",
            encoding="utf-8",
            base_snapshot=_BASE_SNAPSHOT,
        )


def test_recovery_rejects_path_through_symlink_outside_workspace(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    outside = tmp_path / "outside"
    workspace.mkdir()
    outside.mkdir()
    (workspace / "escape").symlink_to(outside, target_is_directory=True)
    journal = RecoveryJournal(tmp_path / "state")

    with pytest.raises(RecoveryError, match="resolves outside its workspace"):
        journal.save(
            document_path=workspace / "escape" / "draft.md",
            workspace_root=workspace,
            text="draft",
            encoding="utf-8",
            base_snapshot=_BASE_SNAPSHOT,
        )


def test_loading_rejects_document_path_that_now_resolves_outside_workspace(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    outside = tmp_path / "outside"
    workspace.mkdir()
    outside.mkdir()
    notes = workspace / "notes"
    notes.mkdir()
    document = notes / "draft.md"
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="unsaved draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    notes.rename(workspace / "old-notes")
    notes.symlink_to(outside, target_is_directory=True)

    with pytest.raises(RecoveryError, match="resolves outside its workspace"):
        journal.load(document)

    (record,) = journal.list_entries(workspace)
    assert record.is_corrupt
    assert record.entry is None
    assert "resolves outside its workspace" in (record.error or "")


def test_delete_record_rejects_path_outside_recovery_root(tmp_path: Path) -> None:
    journal = RecoveryJournal(tmp_path / "state")
    outside = tmp_path / "do-not-delete.json"
    outside.write_bytes(b"important")
    forged = RecoveryRecord(
        journal_path=outside,
        fingerprint="not trusted",
        error="forged",
    )

    with pytest.raises(RecoveryError, match="outside the recovery directory"):
        journal.delete_record(forged)

    assert outside.read_bytes() == b"important"
