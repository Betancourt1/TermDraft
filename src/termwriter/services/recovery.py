"""Crash-recovery journal storage kept separate from Markdown documents."""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from stat import S_ISDIR, S_ISREG
from typing import Any

if sys.platform == "win32":  # pragma: no cover - exercised on Windows
    import msvcrt
else:  # pragma: no cover - the branch is platform-specific
    import fcntl

from termwriter.models.document import FileSnapshot
from termwriter.services.persistence import (
    PersistenceError,
    SaveResult,
    atomic_save,
    snapshot_file,
)

_SCHEMA_VERSION = 2
_SUPPORTED_ENCODINGS = {"utf-8", "utf-8-sig"}


class RecoveryError(Exception):
    """Raised when a recovery journal cannot be stored or trusted."""


@dataclass(frozen=True, slots=True)
class RecoveryEntry:
    """A recoverable in-memory document state and its saved baseline."""

    document_path: Path
    workspace_root: Path
    text: str
    encoding: str
    base_snapshot: FileSnapshot
    updated_at: datetime


@dataclass(frozen=True, slots=True)
class RecoveryScan:
    """Validated workspace entries plus journals that could not be trusted."""

    entries: tuple[RecoveryEntry, ...]
    warnings: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class RecoveryRecord:
    """One journal observed during an inventory scan."""

    journal_path: Path
    fingerprint: str
    entry: RecoveryEntry | None = None
    error: str | None = None
    quarantined: bool = False
    has_content_fingerprint: bool = False

    @property
    def is_corrupt(self) -> bool:
        """Return whether the journal could not be validated."""
        return self.entry is None


@dataclass(frozen=True, slots=True)
class RecoveryRetentionOutcome:
    """The independently reported result of one retention deletion."""

    journal_path: Path
    document_path: Path
    updated_at: datetime
    deleted: bool
    error: str | None = None


@dataclass(frozen=True, slots=True)
class RecoveryRetentionResult:
    """Aggregate and per-entry results from an explicit retention cleanup."""

    cutoff: datetime
    outcomes: tuple[RecoveryRetentionOutcome, ...]

    @property
    def selected_count(self) -> int:
        """Return the number of quarantined entries selected by the cutoff."""
        return len(self.outcomes)

    @property
    def deleted_count(self) -> int:
        """Return the number of entries deleted without an error."""
        return sum(outcome.deleted for outcome in self.outcomes)

    @property
    def failed_count(self) -> int:
        """Return the number of selected entries that could not be deleted."""
        return self.selected_count - self.deleted_count


def default_recovery_root() -> Path:
    """Return a platform-appropriate per-user state directory."""
    if state_home := os.environ.get("XDG_STATE_HOME"):
        return Path(state_home).expanduser() / "termwriter" / "recovery"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "TermWriter" / "recovery"
    if os.name == "nt" and (local_app_data := os.environ.get("LOCALAPPDATA")):
        return Path(local_app_data) / "TermWriter" / "recovery"
    return Path.home() / ".local" / "state" / "termwriter" / "recovery"


class RecoveryJournal:
    """Persist one atomic JSON recovery entry per document path."""

    def __init__(self, state_root: Path | None = None) -> None:
        root = default_recovery_root() if state_root is None else state_root
        self.state_root = root.expanduser().absolute()

    def path_for(self, document_path: Path) -> Path:
        """Return the opaque journal path assigned to a document."""
        normalized_path = _absolute_path(document_path)
        identity = hashlib.sha256(os.fsencode(normalized_path)).hexdigest()
        return self.state_root / f"{identity}.json"

    def save(
        self,
        *,
        document_path: Path,
        workspace_root: Path,
        text: str,
        encoding: str,
        base_snapshot: FileSnapshot,
    ) -> RecoveryEntry:
        """Atomically record the current source without touching the Markdown file."""
        record = self.publish(
            document_path=document_path,
            workspace_root=workspace_root,
            text=text,
            encoding=encoding,
            base_snapshot=base_snapshot,
        )
        assert record.entry is not None
        return record.entry

    def publish(
        self,
        *,
        document_path: Path,
        workspace_root: Path,
        text: str,
        encoding: str,
        base_snapshot: FileSnapshot,
    ) -> RecoveryRecord:
        """Atomically publish source and return its exact deletion token."""
        entry = RecoveryEntry(
            document_path=_absolute_path(document_path),
            workspace_root=_absolute_path(workspace_root),
            text=text,
            encoding=encoding,
            base_snapshot=base_snapshot,
            updated_at=datetime.now(UTC),
        )
        _validate_entry(entry)
        data = _serialize(entry)
        destination = self.path_for(entry.document_path)
        with self._journal_locks(destination):
            temporary: Path | None = None
            descriptor = -1
            try:
                descriptor, temporary_name = tempfile.mkstemp(
                    prefix=f".{destination.stem}.",
                    suffix=".tmp",
                    dir=self.state_root,
                )
                temporary = Path(temporary_name)
                os.fchmod(descriptor, 0o600)
                with os.fdopen(descriptor, "wb") as stream:
                    descriptor = -1
                    stream.write(data)
                    stream.flush()
                    os.fsync(stream.fileno())
                os.replace(temporary, destination)
                temporary = None
                _sync_directory(self.state_root)
            except OSError as error:
                raise RecoveryError(
                    f"Cannot save recovery journal for {entry.document_path}: {error}"
                ) from error
            finally:
                if descriptor >= 0:
                    try:
                        os.close(descriptor)
                    except OSError:
                        pass
                if temporary is not None:
                    try:
                        temporary.unlink()
                    except FileNotFoundError:
                        pass
                    except OSError:
                        pass
        return RecoveryRecord(
            destination,
            _fingerprint(data),
            entry=entry,
            has_content_fingerprint=True,
        )

    def load(self, document_path: Path) -> RecoveryEntry | None:
        """Load and validate a document journal, returning ``None`` when absent."""
        expected_path = _absolute_path(document_path)
        journal_path = self.path_for(expected_path)
        try:
            entry = _read_entry(journal_path)
        except FileNotFoundError:
            return None
        except OSError as error:
            raise RecoveryError(
                f"Cannot read recovery journal for {expected_path}: {error}"
            ) from error
        if entry.document_path != expected_path:
            raise RecoveryError(
                f"Recovery journal path mismatch: expected {expected_path}, "
                f"found {entry.document_path}"
            )
        return entry

    def record_for(self, document_path: Path) -> RecoveryRecord | None:
        """Return the exact valid journal version currently stored for a document."""
        expected_path = _absolute_path(document_path)
        journal_path = self.path_for(expected_path)
        with self._journal_locks(journal_path):
            try:
                journal_path.lstat()
            except FileNotFoundError:
                return None
            except OSError as error:
                raise RecoveryError(
                    f"Cannot inspect recovery journal for {expected_path}: {error}"
                ) from error
            record = self._inspect(journal_path)
            if record.entry is None:
                raise RecoveryError(
                    record.error or f"Cannot validate recovery journal for {expected_path}"
                )
            if record.entry.document_path != expected_path:
                raise RecoveryError(
                    f"Recovery journal path mismatch: expected {expected_path}, "
                    f"found {record.entry.document_path}"
                )
            return record

    def scan_workspace(self, workspace_root: Path) -> RecoveryScan:
        """Find trusted journals for a workspace, including entries whose file vanished."""
        expected_root = _absolute_path(workspace_root)
        try:
            records = self.list_entries(expected_root)
        except RecoveryError as error:
            return RecoveryScan((), (f"Cannot scan recovery directory: {error}",))

        entries: list[RecoveryEntry] = []
        warnings: list[str] = []
        for record in records:
            if record.entry is None:
                warnings.append(
                    f"Cannot use recovery entry {record.journal_path.name}: {record.error}"
                )
                continue
            if record.entry.workspace_root == expected_root:
                entries.append(record.entry)
        entries.sort(key=lambda entry: entry.updated_at, reverse=True)
        return RecoveryScan(tuple(entries), tuple(warnings))

    def list_entries(
        self,
        workspace_root: Path | None = None,
    ) -> tuple[RecoveryRecord, ...]:
        """List journals, always retaining corrupt records when filtering a workspace."""
        try:
            journal_paths = sorted(self.state_root.glob("*.json"))
        except OSError as error:
            raise RecoveryError(f"Cannot scan recovery directory: {error}") from error
        records = tuple(self._inspect(journal_path) for journal_path in journal_paths)
        if workspace_root is None:
            return records
        expected_root = _absolute_path(workspace_root)
        return tuple(
            record
            for record in records
            if record.entry is None or record.entry.workspace_root == expected_root
        )

    def list_stale(
        self,
        *,
        before: datetime,
        workspace_root: Path | None = None,
    ) -> tuple[RecoveryRecord, ...]:
        """List valid entries older than an explicit timezone-aware cutoff."""
        if before.tzinfo is None or before.utcoffset() is None:
            raise RecoveryError("Recovery stale cutoff must include a timezone")
        expected_root = None if workspace_root is None else _absolute_path(workspace_root)
        return tuple(
            record
            for record in self.list_entries(workspace_root)
            if record.entry is not None
            and record.entry.updated_at < before
            and (expected_root is None or record.entry.workspace_root == expected_root)
        )

    def list_quarantined(
        self,
        workspace_root: Path | None = None,
    ) -> tuple[RecoveryRecord, ...]:
        """List quarantined journals, including corrupt entries."""
        quarantine_root = self._validated_quarantine_root(create=False)
        if quarantine_root is None:
            return ()
        try:
            journal_paths = sorted(quarantine_root.glob("*.json"))
        except OSError as error:
            raise RecoveryError(f"Cannot scan recovery quarantine: {error}") from error
        records = tuple(
            self._inspect(journal_path, quarantined=True) for journal_path in journal_paths
        )
        if workspace_root is None:
            return records
        expected_root = _absolute_path(workspace_root)
        return tuple(
            record
            for record in records
            if record.entry is None or record.entry.workspace_root == expected_root
        )

    def delete_record(self, record: RecoveryRecord) -> None:
        """Delete exactly the journal version represented by ``record``."""
        if record.quarantined:
            raise RecoveryError("Cannot delete a quarantined entry without permanent deletion")
        with self._journal_locks(record.journal_path):
            self._verify_record(record)
            self._unlink_record(record)

    def retarget(
        self,
        record: RecoveryRecord,
        *,
        document_path: Path,
        workspace_root: Path,
    ) -> RecoveryEntry:
        """Move a trusted draft to a new Markdown path without replacing another draft."""
        target_path = _absolute_path(document_path)
        target_root = _absolute_path(workspace_root)
        destination = self.path_for(target_path)
        with self._journal_locks(record.journal_path, destination):
            current = self._verify_record(record)
            if current.entry is None:
                raise RecoveryError("Cannot retarget a corrupt recovery entry")

            entry = RecoveryEntry(
                document_path=target_path,
                workspace_root=target_root,
                text=current.entry.text,
                encoding=current.entry.encoding,
                base_snapshot=current.entry.base_snapshot,
                updated_at=current.entry.updated_at,
            )
            _validate_entry(entry)
            if destination == record.journal_path:
                if entry.workspace_root == current.entry.workspace_root:
                    return current.entry
                raise RecoveryError("Retarget destination already contains this recovery entry")

            self._publish_new(destination, _serialize(entry))
            self._verify_record(record)
            self._unlink_record(record)
            return entry

    def quarantine(self, record: RecoveryRecord) -> Path:
        """Move an unchanged journal aside while preserving its exact bytes."""
        if record.quarantined:
            raise RecoveryError("Recovery entry is already quarantined")
        with self._journal_locks(record.journal_path):
            self._verify_record(record)
            quarantine_root = self._validated_quarantine_root(create=True)
            assert quarantine_root is not None
            destination = quarantine_root / record.journal_path.name
            try:
                os.link(record.journal_path, destination, follow_symlinks=False)
                _sync_directory(quarantine_root)
            except FileExistsError as error:
                raise RecoveryError(
                    f"Recovery quarantine already contains {destination.name}"
                ) from error
            except OSError as error:
                raise RecoveryError(
                    f"Cannot quarantine recovery entry {record.journal_path.name}: {error}"
                ) from error

            try:
                self._verify_record(record)
                record.journal_path.unlink()
                _sync_directory(self.state_root)
            except (OSError, RecoveryError) as error:
                if isinstance(error, RecoveryError):
                    raise
                raise RecoveryError(
                    f"Cannot finish quarantining {record.journal_path.name}: {error}"
                ) from error
            return destination

    def restore_quarantined(self, record: RecoveryRecord) -> RecoveryEntry:
        """Restore an unchanged trusted journal without replacing an active draft."""
        if not record.quarantined:
            raise RecoveryError("Recovery entry is not quarantined")
        with self._journal_locks(record.journal_path):
            current = self._verify_record(record)
            if current.entry is None:
                raise RecoveryError("Cannot restore a corrupt recovery entry")

            destination = self.path_for(current.entry.document_path)
            try:
                os.link(record.journal_path, destination, follow_symlinks=False)
                _sync_directory(self.state_root)
            except FileExistsError as error:
                raise RecoveryError(
                    f"An active recovery entry already exists for {destination.name}"
                ) from error
            except OSError as error:
                raise RecoveryError(
                    f"Cannot restore quarantined recovery entry {record.journal_path.name}: {error}"
                ) from error

            quarantine_root = self._validated_quarantine_root(create=False)
            assert quarantine_root is not None
            try:
                self._verify_record(record)
                record.journal_path.unlink()
                _sync_directory(quarantine_root)
            except (OSError, RecoveryError) as error:
                if isinstance(error, RecoveryError):
                    raise
                raise RecoveryError(
                    f"Recovery entry was restored, but its quarantine copy could not be "
                    f"removed: {error}"
                ) from error
            return current.entry

    def delete_quarantined(self, record: RecoveryRecord) -> None:
        """Permanently delete exactly the quarantined version represented by ``record``."""
        if not record.quarantined:
            raise RecoveryError("Recovery entry is not quarantined")
        with self._journal_locks(record.journal_path):
            current = self._verify_record(record)
            if not record.has_content_fingerprint or not current.has_content_fingerprint:
                raise RecoveryError(
                    "Cannot permanently delete recovery bytes that could not be fingerprinted"
                )
            self._unlink_record(record)

    def export_quarantined(
        self,
        record: RecoveryRecord,
        *,
        destination: Path,
    ) -> SaveResult:
        """Export a trusted quarantined draft without replacing files or removing the draft."""
        if not record.quarantined:
            raise RecoveryError("Recovery entry is not quarantined")
        with self._journal_locks(record.journal_path):
            current = self._verify_record(record)
            if current.entry is None:
                raise RecoveryError("Cannot export a corrupt recovery entry")

            target = _absolute_path(destination)
            workspace_root = current.entry.workspace_root
            _validate_workspace_path(target, workspace_root)
            _validate_target_path(target, workspace_root)
            try:
                expected = snapshot_file(target)
                if expected.exists:
                    raise RecoveryError(f"Recovery export destination already exists: {target}")
                return atomic_save(
                    target,
                    current.entry.text,
                    encoding=current.entry.encoding,
                    expected=expected,
                )
            except (OSError, PersistenceError) as error:
                raise RecoveryError(f"Cannot export recovery draft to {target}: {error}") from error

    def cleanup_quarantined(
        self,
        *,
        before: datetime,
        workspace_root: Path | None = None,
        records: tuple[RecoveryRecord, ...] | None = None,
    ) -> RecoveryRetentionResult:
        """Explicitly delete valid quarantined entries older than an aware cutoff."""
        if before.tzinfo is None or before.utcoffset() is None:
            raise RecoveryError("Recovery retention cutoff must include a timezone")

        expected_root = None if workspace_root is None else _absolute_path(workspace_root)
        inventory = self.list_quarantined(workspace_root) if records is None else records
        selected = tuple(
            record
            for record in inventory
            if record.quarantined
            and record.entry is not None
            and record.entry.updated_at < before
            and (expected_root is None or record.entry.workspace_root == expected_root)
        )
        outcomes: list[RecoveryRetentionOutcome] = []
        for record in selected:
            assert record.entry is not None
            try:
                self.delete_quarantined(record)
            except RecoveryError as error:
                outcomes.append(
                    RecoveryRetentionOutcome(
                        journal_path=record.journal_path,
                        document_path=record.entry.document_path,
                        updated_at=record.entry.updated_at,
                        deleted=False,
                        error=str(error),
                    )
                )
            else:
                outcomes.append(
                    RecoveryRetentionOutcome(
                        journal_path=record.journal_path,
                        document_path=record.entry.document_path,
                        updated_at=record.entry.updated_at,
                        deleted=True,
                    )
                )
        return RecoveryRetentionResult(cutoff=before, outcomes=tuple(outcomes))

    def delete(self, document_path: Path) -> None:
        """Delete a recovery journal after a successful save or explicit discard."""
        journal_path = self.path_for(document_path)
        with self._journal_locks(journal_path):
            try:
                journal_path.unlink()
            except FileNotFoundError:
                return
            except OSError as error:
                raise RecoveryError(
                    f"Cannot delete recovery journal for {_absolute_path(document_path)}: {error}"
                ) from error
            try:
                _sync_directory(self.state_root)
            except OSError as error:
                raise RecoveryError(
                    f"Recovery journal was deleted, but its directory could not be synced: {error}"
                ) from error

    def delete_expected(
        self,
        document_path: Path,
        *,
        fingerprint: str | None,
    ) -> None:
        """Delete only the exact observed version, or confirm expected absence."""
        expected_path = _absolute_path(document_path)
        journal_path = self.path_for(expected_path)
        with self._journal_locks(journal_path):
            try:
                journal_path.lstat()
            except FileNotFoundError:
                return
            except OSError as error:
                raise RecoveryError(
                    f"Cannot inspect recovery journal for {expected_path}: {error}"
                ) from error
            if fingerprint is None:
                raise RecoveryError("Recovery entry appeared after cleanup was requested")
            current = self._inspect(journal_path)
            if not current.has_content_fingerprint or current.fingerprint != fingerprint:
                raise RecoveryError("Recovery entry changed after cleanup was requested")
            self._unlink_record(current)

    def _inspect(
        self,
        journal_path: Path,
        *,
        quarantined: bool = False,
    ) -> RecoveryRecord:
        try:
            if journal_path.is_symlink() or not journal_path.is_file():
                raise RecoveryError("Recovery entry is not a regular file")
            data = journal_path.read_bytes()
        except (OSError, RecoveryError) as error:
            return RecoveryRecord(
                journal_path,
                _path_fingerprint(journal_path),
                error=str(error),
                quarantined=quarantined,
            )

        fingerprint = _fingerprint(data)
        try:
            entry = _entry_from_bytes(data, journal_path.name)
            expected_path = self.path_for(entry.document_path)
            path_matches = (
                expected_path.name == journal_path.name
                if quarantined
                else expected_path == journal_path
            )
            if not path_matches:
                raise RecoveryError("Recovery journal filename does not match its document")
            return RecoveryRecord(
                journal_path,
                fingerprint,
                entry=entry,
                quarantined=quarantined,
                has_content_fingerprint=True,
            )
        except RecoveryError as error:
            return RecoveryRecord(
                journal_path,
                fingerprint,
                error=str(error),
                quarantined=quarantined,
                has_content_fingerprint=True,
            )

    def _verify_record(self, record: RecoveryRecord) -> RecoveryRecord:
        expected_parent = self.state_root
        if record.quarantined:
            expected_parent = self.state_root / "quarantine"
            self._validated_quarantine_root(create=False)
        if record.journal_path.parent != expected_parent:
            location = "recovery quarantine" if record.quarantined else "recovery directory"
            raise RecoveryError(f"Recovery entry is outside the {location}")
        current = self._inspect(
            record.journal_path,
            quarantined=record.quarantined,
        )
        if current.fingerprint != record.fingerprint:
            raise RecoveryError("Recovery entry changed after it was listed")
        return current

    def _unlink_record(self, record: RecoveryRecord) -> None:
        try:
            record.journal_path.unlink()
            _sync_directory(record.journal_path.parent)
        except FileNotFoundError as error:
            raise RecoveryError("Recovery entry disappeared after it was listed") from error
        except OSError as error:
            raise RecoveryError(
                f"Cannot delete recovery entry {record.journal_path.name}: {error}"
            ) from error

    def _validated_quarantine_root(self, *, create: bool) -> Path | None:
        quarantine_root = self.state_root / "quarantine"
        if create:
            try:
                quarantine_root.mkdir(mode=0o700, parents=True, exist_ok=True)
            except OSError as error:
                raise RecoveryError(f"Cannot create recovery quarantine: {error}") from error
        try:
            metadata = quarantine_root.lstat()
        except FileNotFoundError:
            return None
        except OSError as error:
            raise RecoveryError(f"Cannot inspect recovery quarantine: {error}") from error
        if not S_ISDIR(metadata.st_mode):
            raise RecoveryError("Recovery quarantine is not a real directory")
        return quarantine_root

    def _publish_new(self, destination: Path, data: bytes) -> None:
        try:
            self.state_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        except OSError as error:
            raise RecoveryError(
                f"Cannot create recovery directory {self.state_root}: {error}"
            ) from error

        temporary: Path | None = None
        descriptor = -1
        try:
            descriptor, temporary_name = tempfile.mkstemp(
                prefix=f".{destination.stem}.",
                suffix=".tmp",
                dir=self.state_root,
            )
            temporary = Path(temporary_name)
            os.fchmod(descriptor, 0o600)
            with os.fdopen(descriptor, "wb") as stream:
                descriptor = -1
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            os.link(temporary, destination, follow_symlinks=False)
            _sync_directory(self.state_root)
        except FileExistsError as error:
            raise RecoveryError(f"Recovery entry already exists for {destination.name}") from error
        except OSError as error:
            raise RecoveryError(f"Cannot create retargeted recovery entry: {error}") from error
        finally:
            if descriptor >= 0:
                try:
                    os.close(descriptor)
                except OSError:
                    pass
            if temporary is not None:
                try:
                    temporary.unlink()
                except OSError:
                    pass

    @contextmanager
    def _journal_locks(self, *journal_paths: Path) -> Iterator[None]:
        """Lock journal identities across threads and cooperating processes."""
        try:
            self.state_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        except OSError as error:
            raise RecoveryError(
                f"Cannot create recovery directory {self.state_root}: {error}"
            ) from error

        ordered_paths = sorted({_absolute_path(path) for path in journal_paths}, key=os.fsencode)
        descriptors: list[int] = []
        try:
            for journal_path in ordered_paths:
                lock_path = self.state_root / f".{journal_path.name}.lock"
                descriptor = -1
                try:
                    flags = os.O_CREAT | os.O_RDWR | getattr(os, "O_NOFOLLOW", 0)
                    descriptor = os.open(lock_path, flags, 0o600)
                    if not S_ISREG(os.fstat(descriptor).st_mode):
                        raise OSError(f"Lock path is not a regular file: {lock_path}")
                    _acquire_file_lock(descriptor)
                except OSError as error:
                    if descriptor >= 0:
                        os.close(descriptor)
                    raise RecoveryError(
                        f"Cannot lock recovery journal {journal_path.name}: {error}"
                    ) from error
                descriptors.append(descriptor)
            yield
        finally:
            for descriptor in reversed(descriptors):
                try:
                    _release_file_lock(descriptor)
                except OSError:
                    pass
                finally:
                    os.close(descriptor)


def _absolute_path(path: Path) -> Path:
    return path.expanduser().absolute()


def _serialize(entry: RecoveryEntry) -> bytes:
    payload = {
        "version": _SCHEMA_VERSION,
        "document_path": str(entry.document_path),
        "workspace_root": str(entry.workspace_root),
        "text": entry.text,
        "encoding": entry.encoding,
        "base_snapshot": _snapshot_payload(entry.base_snapshot),
        "updated_at": entry.updated_at.isoformat(),
    }
    try:
        serialized = json.dumps(
            payload,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        return (serialized + "\n").encode("utf-8")
    except (TypeError, UnicodeEncodeError) as error:
        raise RecoveryError(f"Recovery entry cannot be encoded as UTF-8 JSON: {error}") from error


def _read_entry(journal_path: Path) -> RecoveryEntry:
    data = journal_path.read_bytes()
    return _entry_from_bytes(data, journal_path.name)


def _entry_from_bytes(data: bytes, name: str) -> RecoveryEntry:
    try:
        payload = json.loads(data.decode("utf-8"))
        return _entry_from_payload(payload)
    except (UnicodeDecodeError, json.JSONDecodeError, RecoveryError) as error:
        if isinstance(error, RecoveryError):
            raise
        raise RecoveryError(f"Invalid recovery journal {name}: {error}") from error


def _entry_from_payload(payload: Any) -> RecoveryEntry:
    if not isinstance(payload, dict):
        raise RecoveryError("Invalid recovery journal: expected a JSON object")
    version = payload.get("version")
    if not isinstance(version, int) or isinstance(version, bool) or version not in {1, 2}:
        raise RecoveryError("Unsupported recovery journal version")

    required_types: dict[str, type[object]] = {
        "document_path": str,
        "workspace_root": str,
        "text": str,
        "encoding": str,
        "updated_at": str,
    }
    for key, expected_type in required_types.items():
        if not isinstance(payload.get(key), expected_type):
            raise RecoveryError(f"Invalid recovery journal field: {key}")
    if version == 1:
        base_digest = payload.get("base_digest")
        if base_digest is not None and not isinstance(base_digest, str):
            raise RecoveryError("Invalid recovery journal field: base_digest")
        base_snapshot = FileSnapshot(
            exists=base_digest is not None,
            digest=base_digest,
        )
    else:
        base_snapshot = _snapshot_from_payload(payload.get("base_snapshot"))

    try:
        updated_at = datetime.fromisoformat(payload["updated_at"])
    except ValueError as error:
        raise RecoveryError("Invalid recovery journal field: updated_at") from error
    entry = RecoveryEntry(
        document_path=Path(payload["document_path"]),
        workspace_root=Path(payload["workspace_root"]),
        text=payload["text"],
        encoding=payload["encoding"],
        base_snapshot=base_snapshot,
        updated_at=updated_at,
    )
    _validate_entry(entry)
    return entry


def _validate_entry(entry: RecoveryEntry) -> None:
    if not entry.document_path.is_absolute():
        raise RecoveryError("Recovery document path must be absolute")
    if not entry.workspace_root.is_absolute():
        raise RecoveryError("Recovery workspace root must be absolute")
    if entry.encoding not in _SUPPORTED_ENCODINGS:
        raise RecoveryError(f"Unsupported recovery encoding: {entry.encoding}")
    _validate_workspace_path(entry.document_path, entry.workspace_root)
    _validate_target_path(entry.document_path, entry.workspace_root)
    _validate_snapshot(entry.base_snapshot)
    if entry.updated_at.tzinfo is None or entry.updated_at.utcoffset() is None:
        raise RecoveryError("Recovery updated timestamp must include a timezone")


def _snapshot_payload(snapshot: FileSnapshot) -> dict[str, object]:
    return {
        "exists": snapshot.exists,
        "digest": snapshot.digest,
        "size": snapshot.size,
        "mtime_ns": snapshot.mtime_ns,
        "mode": snapshot.mode,
        "device": snapshot.device,
        "inode": snapshot.inode,
        "parent_device": snapshot.parent_device,
        "parent_inode": snapshot.parent_inode,
    }


def _snapshot_from_payload(payload: Any) -> FileSnapshot:
    if not isinstance(payload, dict) or not isinstance(payload.get("exists"), bool):
        raise RecoveryError("Invalid recovery journal field: base_snapshot")
    optional_fields = (
        "size",
        "mtime_ns",
        "mode",
        "device",
        "inode",
        "parent_device",
        "parent_inode",
    )
    values: dict[str, int | None] = {}
    for field_name in optional_fields:
        value = payload.get(field_name)
        if value is not None and (not isinstance(value, int) or isinstance(value, bool)):
            raise RecoveryError(f"Invalid recovery snapshot field: {field_name}")
        values[field_name] = value
    digest = payload.get("digest")
    if digest is not None and not isinstance(digest, str):
        raise RecoveryError("Invalid recovery snapshot field: digest")
    snapshot = FileSnapshot(
        exists=payload["exists"],
        digest=digest,
        **values,
    )
    _validate_snapshot(snapshot)
    return snapshot


def _validate_snapshot(snapshot: FileSnapshot) -> None:
    digest = snapshot.digest
    if snapshot.exists and (
        digest is None
        or len(digest) != 64
        or any(character not in "0123456789abcdef" for character in digest)
    ):
        raise RecoveryError("Recovery base digest must be a lowercase SHA-256 digest")
    if not snapshot.exists and digest is not None:
        raise RecoveryError("A missing recovery baseline cannot have a digest")


def _validate_workspace_path(document_path: Path, workspace_root: Path) -> None:
    if document_path.suffix.casefold() not in {".md", ".markdown"}:
        raise RecoveryError("Recovery document is not Markdown")
    try:
        document_path.relative_to(workspace_root)
    except ValueError as error:
        raise RecoveryError("Recovery document is outside its workspace") from error


def _validate_target_path(document_path: Path, workspace_root: Path) -> None:
    try:
        resolved_root = workspace_root.resolve(strict=False)
        document_path.resolve(strict=False).relative_to(resolved_root)
    except (OSError, RuntimeError, ValueError) as error:
        raise RecoveryError("Recovery document resolves outside its workspace") from error


def _fingerprint(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _path_fingerprint(path: Path) -> str:
    try:
        if path.is_symlink():
            data = b"symlink\0" + os.fsencode(os.readlink(path))
        else:
            metadata = path.lstat()
            data = (f"special\0{metadata.st_mode}\0{metadata.st_dev}\0{metadata.st_ino}").encode()
    except OSError as error:
        data = f"unreadable\0{type(error).__name__}\0{error}".encode()
    return _fingerprint(data)


def _acquire_file_lock(descriptor: int) -> None:
    if sys.platform == "win32":  # pragma: no cover - exercised on Windows
        if os.fstat(descriptor).st_size == 0:
            os.write(descriptor, b"\0")
        os.lseek(descriptor, 0, os.SEEK_SET)
        msvcrt.locking(descriptor, msvcrt.LK_LOCK, 1)
    else:  # pragma: no branch - exactly one platform branch runs
        fcntl.flock(descriptor, fcntl.LOCK_EX)


def _release_file_lock(descriptor: int) -> None:
    if sys.platform == "win32":  # pragma: no cover - exercised on Windows
        os.lseek(descriptor, 0, os.SEEK_SET)
        msvcrt.locking(descriptor, msvcrt.LK_UNLCK, 1)
    else:  # pragma: no branch - exactly one platform branch runs
        fcntl.flock(descriptor, fcntl.LOCK_UN)


def _sync_directory(directory: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    descriptor = os.open(directory, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
