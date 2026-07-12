"""Classification of disk changes relative to an open document."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum, auto
from pathlib import Path

from termwriter.models.document import Document, FileSnapshot
from termwriter.services.persistence import PersistenceError, snapshot_file


class ExternalChangeKind(Enum):
    """Possible relationships between the open source and the disk file."""

    UNCHANGED = auto()
    MODIFIED = auto()
    DELETED = auto()
    CONFLICT = auto()
    INACCESSIBLE = auto()


@dataclass(frozen=True, slots=True)
class ExternalChange:
    """The detected state, current fingerprint, and optional error detail."""

    kind: ExternalChangeKind
    snapshot: FileSnapshot | None
    detail: str | None = None


@dataclass(frozen=True, slots=True)
class DiskProbe:
    """An immutable result from inspecting one path on disk."""

    path: Path
    snapshot: FileSnapshot | None
    error: str | None = None


def probe_file(path: Path) -> DiskProbe:
    """Hash a path into a snapshot without raising expected access errors."""
    try:
        snapshot = snapshot_file(path)
    except (OSError, PersistenceError) as error:
        return DiskProbe(path=path, snapshot=None, error=str(error))
    return DiskProbe(path=path, snapshot=snapshot)


def classify_external_change(
    baseline: FileSnapshot,
    *,
    dirty: bool,
    probe: DiskProbe,
) -> ExternalChange:
    """Classify a completed probe against an immutable document baseline."""
    current = probe.snapshot
    if current is None:
        return ExternalChange(ExternalChangeKind.INACCESSIBLE, None, probe.error)

    if current.has_same_content(baseline):
        if current.exists and not current.has_same_origin(baseline):
            kind = ExternalChangeKind.CONFLICT if dirty else ExternalChangeKind.MODIFIED
            return ExternalChange(kind, current)
        return ExternalChange(ExternalChangeKind.UNCHANGED, current)
    if dirty:
        return ExternalChange(ExternalChangeKind.CONFLICT, current)
    if not current.exists:
        return ExternalChange(ExternalChangeKind.DELETED, current)
    return ExternalChange(ExternalChangeKind.MODIFIED, current)


def detect_external_change(document: Document) -> ExternalChange:
    """Synchronously probe and classify a document for compatibility."""
    return classify_external_change(
        document.snapshot,
        dirty=document.dirty,
        probe=probe_file(document.path),
    )
