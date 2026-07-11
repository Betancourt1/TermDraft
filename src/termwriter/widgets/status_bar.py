"""Compact status information for the active document."""

from __future__ import annotations

from pathlib import Path

from rich.text import Text
from textual.widgets import Static

from termwriter.models.document import Document


class TermWriterStatusBar(Static):
    """Render mode, path, dirty state, words, cursor, save, and conflict."""

    def __init__(self) -> None:
        super().__init__("FILES | No file open", id="status-bar", markup=False)

    def show_document(self, document: Document | None, *, root: Path, mode: str) -> None:
        status = Text(mode, style="bold")
        status.append("  |  ", style="dim")
        if document is None:
            status.append("No file open", style="dim")
            self.update(status)
            return

        status.append(document.path.relative_to(root).as_posix())
        if document.dirty:
            status.append("  ● modified", style="bold yellow")
        status.append(f"  |  {document.word_count} words")
        status.append(f"  |  Ln {document.cursor.line + 1}, Col {document.cursor.column + 1}")
        status.append(f"  |  {document.last_save_status}")
        if document.conflict:
            status.append("  |  CONFLICT", style="bold red")
        self.update(status)
