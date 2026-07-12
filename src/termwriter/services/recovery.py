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

_SCHEMA_VERSION = 1
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
    base_digest: str | None
    updated_at: datetime


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
        base_digest: str | None,
    ) -> RecoveryEntry:
        """Atomically record the current source without touching the Markdown file."""
        entry = RecoveryEntry(
            document_path=_absolute_path(document_path),
            workspace_root=_absolute_path(workspace_root),
            text=text,
            encoding=encoding,
            base_digest=base_digest,
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
            data = journal_path.read_bytes()
        except FileNotFoundError:
            return None
        except OSError as error:
            raise RecoveryError(
                f"Cannot read recovery journal for {expected_path}: {error}"
            ) from error

        try:
            payload = json.loads(data.decode("utf-8"))
            entry = _entry_from_payload(payload)
        except (UnicodeDecodeError, json.JSONDecodeError, RecoveryError) as error:
            if isinstance(error, RecoveryError):
                raise
            raise RecoveryError(f"Invalid recovery journal for {expected_path}: {error}") from error
        if entry.document_path != expected_path:
            raise RecoveryError(
                f"Recovery journal path mismatch: expected {expected_path}, "
                f"found {entry.document_path}"
            )
        return entry

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
        "base_digest": entry.base_digest,
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


def _entry_from_payload(payload: Any) -> RecoveryEntry:
    if not isinstance(payload, dict):
        raise RecoveryError("Invalid recovery journal: expected a JSON object")
    version = payload.get("version")
    if not isinstance(version, int) or isinstance(version, bool) or version != _SCHEMA_VERSION:
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
    base_digest = payload.get("base_digest")
    if base_digest is not None and not isinstance(base_digest, str):
        raise RecoveryError("Invalid recovery journal field: base_digest")

    try:
        updated_at = datetime.fromisoformat(payload["updated_at"])
    except ValueError as error:
        raise RecoveryError("Invalid recovery journal field: updated_at") from error
    entry = RecoveryEntry(
        document_path=Path(payload["document_path"]),
        workspace_root=Path(payload["workspace_root"]),
        text=payload["text"],
        encoding=payload["encoding"],
        base_digest=base_digest,
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
    if entry.base_digest is not None and (
        len(entry.base_digest) != 64
        or any(character not in "0123456789abcdef" for character in entry.base_digest)
    ):
        raise RecoveryError("Recovery base digest must be a lowercase SHA-256 digest")
    if entry.updated_at.tzinfo is None or entry.updated_at.utcoffset() is None:
        raise RecoveryError("Recovery updated timestamp must include a timezone")


def _sync_directory(directory: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    descriptor = os.open(directory, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
