"""Tests for the separate crash-recovery journal."""

from __future__ import annotations

import json
import os
import stat
from datetime import UTC, datetime, timedelta
from pathlib import Path

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
