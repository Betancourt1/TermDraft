"""Document state shared by the editor, preview, and persistence layer."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

_WORD_PATTERN = re.compile(r"[^\W_]+(?:['\u2019][^\W_]+)*", re.UNICODE)


@dataclass(frozen=True, slots=True)
class FileSnapshot:
    """A content fingerprint and useful metadata for one disk state."""

    exists: bool
    digest: str | None = None
    size: int | None = None
    mtime_ns: int | None = None
    mode: int | None = None
    device: int | None = None
    inode: int | None = None
    parent_device: int | None = None
    parent_inode: int | None = None

    @classmethod
    def missing(
        cls,
        *,
        parent_device: int | None = None,
        parent_inode: int | None = None,
    ) -> FileSnapshot:
        """Return the snapshot used for a path that does not exist."""
        return cls(
            exists=False,
            parent_device=parent_device,
            parent_inode=parent_inode,
        )

    def has_same_content(self, other: FileSnapshot) -> bool:
        """Compare content, ignoring metadata-only changes such as a touch."""
        if self.exists != other.exists:
            return False
        if not self.exists:
            return True
        return self.digest == other.digest

    def has_same_origin(self, other: FileSnapshot) -> bool:
        """Compare the file and parent directory identities when both exist."""
        if not self.exists or not other.exists:
            return self.exists == other.exists
        return (
            self.device,
            self.inode,
            self.parent_device,
            self.parent_inode,
        ) == (
            other.device,
            other.inode,
            other.parent_device,
            other.parent_inode,
        )


@dataclass(slots=True)
class CursorState:
    """Logical cursor and scroll positions retained while a document is open."""

    line: int = 0
    column: int = 0
    scroll_x: float = 0.0
    scroll_y: float = 0.0


@dataclass(slots=True)
class Document:
    """The source of truth for the currently open Markdown file."""

    path: Path
    text: str
    saved_text: str
    snapshot: FileSnapshot
    encoding: str = "utf-8"
    cursor: CursorState = field(default_factory=CursorState)
    conflict: bool = False
    last_save_status: str = "Loaded"

    @property
    def dirty(self) -> bool:
        """Whether the current source differs from the last saved source."""
        return self.text != self.saved_text

    @property
    def word_count(self) -> int:
        """Count Unicode word-like runs in the Markdown source."""
        return len(_WORD_PATTERN.findall(self.text))

    def update_text(self, text: str) -> None:
        """Replace the current in-memory source without changing its baseline."""
        self.text = text

    def update_cursor(
        self,
        line: int,
        column: int,
        *,
        scroll_x: float | None = None,
        scroll_y: float | None = None,
    ) -> None:
        """Store the editor position for status display and later restoration."""
        self.cursor.line = max(0, line)
        self.cursor.column = max(0, column)
        if scroll_x is not None:
            self.cursor.scroll_x = max(0.0, scroll_x)
        if scroll_y is not None:
            self.cursor.scroll_y = max(0.0, scroll_y)

    def mark_saved(self, snapshot: FileSnapshot, status: str = "Saved") -> None:
        """Advance the saved baseline after persistence has succeeded."""
        self.saved_text = self.text
        self.snapshot = snapshot
        self.conflict = False
        self.last_save_status = status

    def accept_unchanged_snapshot(self, snapshot: FileSnapshot) -> None:
        """Refresh disk metadata and clear a conflict whose content returned to baseline."""
        self.snapshot = snapshot
        if self.conflict:
            self.last_save_status = "Disk matches baseline"
        self.conflict = False

    def replace_from_disk(
        self,
        text: str,
        snapshot: FileSnapshot,
        encoding: str,
    ) -> None:
        """Replace local and saved state after an explicit or safe reload."""
        self.text = text
        self.saved_text = text
        self.snapshot = snapshot
        self.encoding = encoding
        self.cursor = CursorState()
        self.conflict = False
        self.last_save_status = "Reloaded"

    def retarget(self, path: Path) -> None:
        """Move this document identity after a successful Save As operation."""
        self.path = path
