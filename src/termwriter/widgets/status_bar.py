"""Compact status information for the active document."""

from __future__ import annotations

from pathlib import Path

from rich.text import Text
from textual.widgets import Static

from termwriter.models.document import Document


class TermWriterStatusBar(Static):
    """Render mode, safety state, path, and ordinary document metadata."""

    def __init__(self) -> None:
        super().__init__("WRITE · FILES | No file open", id="status-bar", markup=False)

    def show_document(
        self,
        document: Document | None,
        *,
        root: Path,
        mode: str,
        announcement: str | None = None,
    ) -> None:
        status = Text(mode, style="bold")
        if document is None:
            status.append("  |  ", style="dim")
            status.append("No file open", style="dim")
            self.update(status)
            return

        if document.conflict:
            status.append("  |  CONFLICT", style="bold red")
        if document.dirty:
            status.append("  |  ● modified", style="bold yellow")
        if document.recovery_saved:
            status.append("  |  RECOVERY STORED", style="bold cyan")
        if document.has_mixed_line_endings:
            status.append(f"  |  {document.line_ending_label}", style="bold magenta")
        status.append("  |  ", style="dim")
        status.append(document.path.relative_to(root).as_posix())
        if announcement is not None:
            status.append("  |  ", style="dim")
            status.append(announcement, style="bold cyan")
            self.update(status)
            return
        status.append(f"  |  {document.word_count} words")
        status.append(f"  |  Ln {document.cursor.line + 1}, Col {document.cursor.column + 1}")
        status.append(f"  |  {document.last_save_status}")
        self.update(status)
