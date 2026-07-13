"""Markdown source editor configured for prose."""

from typing import ClassVar

from textual import events
from textual.binding import BindingType
from textual.widgets import TextArea

from termwriter.bindings import EDITOR_BINDINGS
from termwriter.services.markdown_continuation import continuation_edit

_COMMAND_NAVIGATION_KEYS = frozenset({"up", "down", "left", "right"})


class MarkdownEditor(TextArea):
    """A soft-wrapped TextArea that always edits Markdown source."""

    BINDINGS: ClassVar[list[BindingType]] = EDITOR_BINDINGS

    def __init__(
        self,
        text: str = "",
        *,
        auto_continue_lists: bool = True,
        soft_wrap: bool = True,
        show_line_numbers: bool = True,
        read_only: bool = True,
        id: str | None = "markdown-editor",
        classes: str | None = None,
    ) -> None:
        self.auto_continue_lists = auto_continue_lists
        self.write_mode = True
        super().__init__(
            text,
            language="markdown",
            soft_wrap=soft_wrap,
            tab_behavior="indent",
            show_line_numbers=show_line_numbers,
            max_checkpoints=100,
            id=id,
            classes=classes,
            placeholder="Select a Markdown file from the explorer or press Ctrl+P.",
        )
        self.read_only = read_only

    def undo(self) -> None:
        """Keep history immutable while a background writer owns the source."""
        if not self.read_only:
            super().undo()

    def redo(self) -> None:
        """Keep history immutable while a background writer owns the source."""
        if not self.read_only:
            super().redo()

    def check_consume_key(self, key: str, character: str | None = None) -> bool:
        """Let application command bindings own printable keys outside WRITE mode."""
        if not self.write_mode:
            return False
        return super().check_consume_key(key, character)

    async def _on_key(self, event: events.Key) -> None:
        """Apply predictable Markdown continuation before TextArea handles Enter."""
        if not self.write_mode and event.key not in _COMMAND_NAVIGATION_KEYS:
            event.stop()
            event.prevent_default()
            return
        if (
            event.key == "enter"
            and self.auto_continue_lists
            and not self.read_only
            and self.selection.start == self.selection.end
        ):
            line, column = self.cursor_location
            edit = continuation_edit(self.text, line, column)
            if edit is not None:
                event.stop()
                event.prevent_default()
                self.replace(
                    edit.text,
                    (edit.start_line, edit.start_column),
                    (edit.end_line, edit.end_column),
                    maintain_selection_offset=False,
                )
                self.move_cursor((edit.cursor_line, edit.cursor_column))
                return
        await super()._on_key(event)
