"""Crash-recovery journal storage kept separate from Markdown documents."""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from termwriter.models.document import FileSnapshot

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

        try:
            self.state_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        except OSError as error:
            raise RecoveryError(
                f"Cannot create recovery directory {self.state_root}: {error}"
            ) from error

        destination = self.path_for(entry.document_path)
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
        return entry

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

    def scan_workspace(self, workspace_root: Path) -> RecoveryScan:
        """Find trusted journals for a workspace, including entries whose file vanished."""
        expected_root = _absolute_path(workspace_root)
        try:
            journal_paths = sorted(self.state_root.glob("*.json"))
        except OSError as error:
            return RecoveryScan((), (f"Cannot scan recovery directory: {error}",))

        entries: list[RecoveryEntry] = []
        warnings: list[str] = []
        for journal_path in journal_paths:
            try:
                if journal_path.is_symlink() or not journal_path.is_file():
                    raise RecoveryError("Recovery entry is not a regular file")
                entry = _read_entry(journal_path)
                if self.path_for(entry.document_path) != journal_path:
                    raise RecoveryError("Recovery journal filename does not match its document")
                if entry.workspace_root != expected_root:
                    continue
                entry.document_path.relative_to(expected_root)
                if entry.document_path.suffix.casefold() not in {".md", ".markdown"}:
                    raise RecoveryError("Recovery document is not Markdown")
            except (OSError, RecoveryError, ValueError) as error:
                warnings.append(f"Cannot use recovery entry {journal_path.name}: {error}")
                continue
            entries.append(entry)
        entries.sort(key=lambda entry: entry.updated_at, reverse=True)
        return RecoveryScan(tuple(entries), tuple(warnings))

    def delete(self, document_path: Path) -> None:
        """Delete a recovery journal after a successful save or explicit discard."""
        journal_path = self.path_for(document_path)
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
    try:
        payload = json.loads(data.decode("utf-8"))
        return _entry_from_payload(payload)
    except (UnicodeDecodeError, json.JSONDecodeError, RecoveryError) as error:
        if isinstance(error, RecoveryError):
            raise
        raise RecoveryError(f"Invalid recovery journal {journal_path.name}: {error}") from error


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


def _sync_directory(directory: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    descriptor = os.open(directory, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
