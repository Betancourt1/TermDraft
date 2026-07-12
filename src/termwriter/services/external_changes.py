"""Classification of disk changes relative to an open document."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum, auto

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


def detect_external_change(document: Document) -> ExternalChange:
    """Hash the disk file and classify changes without mutating the document."""
    try:
        current = snapshot_file(document.path)
    except (OSError, PersistenceError) as error:
        return ExternalChange(ExternalChangeKind.INACCESSIBLE, None, str(error))

    if current.has_same_content(document.snapshot):
        if current.exists and not current.has_same_origin(document.snapshot):
            kind = ExternalChangeKind.CONFLICT if document.dirty else ExternalChangeKind.MODIFIED
            return ExternalChange(kind, current)
        return ExternalChange(ExternalChangeKind.UNCHANGED, current)
    if document.dirty:
        return ExternalChange(ExternalChangeKind.CONFLICT, current)
    if not current.exists:
        return ExternalChange(ExternalChangeKind.DELETED, current)
    return ExternalChange(ExternalChangeKind.MODIFIED, current)
