"""Document state shared by the editor, preview, and persistence layer."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path

_WORD_PATTERN = re.compile(r"[^\W_]+(?:['\u2019][^\W_]+)*", re.UNICODE)
_LINE_ENDING_PATTERN = re.compile(r"\r\n|\r|\n")


class LineEndingStyle(Enum):
    """Line-ending forms that matter to exact-source editing."""

    NONE = "NONE"
    LF = "LF"
    CRLF = "CRLF"
    CR = "CR"
    MIXED = "MIXED"


def analyze_line_endings(text: str) -> tuple[LineEndingStyle, str | None]:
    """Return the source style and the separator Textual will retain."""
    endings = _LINE_ENDING_PATTERN.findall(text)
    if not endings:
        return LineEndingStyle.NONE, None
    if "\r\n" in endings:
        preferred = "\r\n"
    elif "\n" in endings:
        preferred = "\n"
    else:
        preferred = "\r"
    if len(set(endings)) > 1:
        return LineEndingStyle.MIXED, preferred
    styles = {
        "\n": LineEndingStyle.LF,
        "\r\n": LineEndingStyle.CRLF,
        "\r": LineEndingStyle.CR,
    }
    return styles[preferred], preferred


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
    read_only: bool = False
    last_save_status: str = "Loaded"
    recovery_saved: bool = False
    recovery_conflict: bool = False
    recovery_base_snapshot: FileSnapshot | None = None
    line_ending_style: LineEndingStyle = field(init=False)
    preferred_line_ending: str | None = field(init=False)

    def __post_init__(self) -> None:
        self._refresh_line_ending_metadata()

    @property
    def dirty(self) -> bool:
        """Whether the current source differs from the last saved source."""
        return self.text != self.saved_text or self.recovery_conflict

    @property
    def word_count(self) -> int:
        """Count Unicode word-like runs in the Markdown source."""
        return len(_WORD_PATTERN.findall(self.text))

    @property
    def has_mixed_line_endings(self) -> bool:
        """Whether editing will normalize separators to the first observed form."""
        return self.line_ending_style is LineEndingStyle.MIXED

    @property
    def line_ending_label(self) -> str:
        """Return a compact status label, including mixed-file normalization."""
        if self.line_ending_style is not LineEndingStyle.MIXED:
            return str(self.line_ending_style.value)
        if self.preferred_line_ending == "\n":
            preferred = "LF"
        elif self.preferred_line_ending == "\r\n":
            preferred = "CRLF"
        elif self.preferred_line_ending == "\r":
            preferred = "CR"
        else:
            preferred = "UNKNOWN"
        return f"MIXED→{preferred}"

    def update_text(self, text: str) -> None:
        """Replace the current in-memory source without changing its baseline."""
        self.text = text
        if self.line_ending_style is LineEndingStyle.MIXED or text == self.saved_text:
            self._refresh_line_ending_metadata()

    def restore_recovery(
        self,
        text: str,
        encoding: str,
        base_snapshot: FileSnapshot,
    ) -> None:
        """Install an explicitly selected journal draft over the current disk baseline."""
        self.text = text
        self.encoding = encoding
        self.recovery_saved = True
        self.recovery_base_snapshot = base_snapshot
        self.recovery_conflict = not (
            base_snapshot.has_same_content(self.snapshot)
            and base_snapshot.has_same_origin(self.snapshot)
        )
        self.conflict = self.recovery_conflict
        self.last_save_status = (
            "Recovered conflict" if self.recovery_conflict else "Recovered draft"
        )
        self._refresh_line_ending_metadata()

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
        self.recovery_saved = False
        self.recovery_conflict = False
        self.recovery_base_snapshot = None
        self.last_save_status = status
        self._refresh_line_ending_metadata()

    def discard_changes(self) -> None:
        """Return to the saved source after an explicit discard decision."""
        recovered_conflict = self.recovery_conflict
        self.text = self.saved_text
        self.recovery_saved = False
        self.recovery_conflict = False
        self.recovery_base_snapshot = None
        if recovered_conflict:
            self.conflict = False
        if not self.conflict:
            self.last_save_status = "Changes discarded"
        self._refresh_line_ending_metadata()

    def accept_unchanged_snapshot(self, snapshot: FileSnapshot) -> None:
        """Refresh disk metadata and clear a conflict whose content returned to baseline."""
        self.snapshot = snapshot
        if self.recovery_conflict:
            return
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
        self.recovery_saved = False
        self.recovery_conflict = False
        self.recovery_base_snapshot = None
        self.last_save_status = "Reloaded"
        self._refresh_line_ending_metadata()

    def retarget(self, path: Path) -> None:
        """Move this document identity after a successful Save As operation."""
        self.path = path

    def _refresh_line_ending_metadata(self) -> None:
        self.line_ending_style, self.preferred_line_ending = analyze_line_endings(self.text)
