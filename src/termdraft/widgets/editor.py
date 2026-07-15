"""Markdown source editor configured for prose."""

from typing import ClassVar

from textual import events
from textual.binding import BindingType
from textual.widget import Widget
from textual.widgets import Static, TextArea

from termdraft.bindings import EDITOR_BINDINGS
from termdraft.services.markdown_continuation import continuation_edit
from termdraft.widgets.scrollbar import use_thin_vertical_scrollbar

_COMMAND_NAVIGATION_KEYS = frozenset({"up", "down", "left", "right"})
WORKBENCH_MIN_PANE_WIDTH = 20


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
        super().__init__(
            text,
            language="markdown",
            soft_wrap=soft_wrap,
            tab_behavior="indent",
            show_line_numbers=show_line_numbers,
            max_checkpoints=100,
            id=id,
            classes=classes,
            placeholder=(
                "COMMAND mode: press : and choose Create file or folder. "
                "Ctrl+P opens existing files; ? shows help."
            ),
        )
        self.set_write_mode(True)
        self.read_only = read_only

    def on_mount(self) -> None:
        use_thin_vertical_scrollbar(self)

    def undo(self) -> None:
        """Keep history immutable while a background writer owns the source."""
        if not self.read_only:
            super().undo()

    def redo(self) -> None:
        """Keep history immutable while a background writer owns the source."""
        if not self.read_only:
            super().redo()

    def set_write_mode(self, enabled: bool) -> None:
        """Switch input ownership and the matching Vim-style cursor."""
        self.write_mode = enabled
        self.set_class(enabled, "write-mode")

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


class WorkbenchResizeHandle(Static):
    """Drag handle for resizing the raw editor and rendered preview."""

    def __init__(self) -> None:
        self._drag_start_x: int | None = None
        self._drag_start_editor_width = 0
        self._drag_start_preview_width = 0
        super().__init__(
            id="workbench-resize-handle",
            classes="horizontal-resize-handle",
        )
        self.tooltip = "Drag to resize raw and preview panes"

    def on_mouse_down(self, event: events.MouseDown) -> None:
        if event.button != 1:
            return
        editor, preview = self._panes()
        self._drag_start_x = int(event.screen_x)
        self._drag_start_editor_width = editor.region.width
        self._drag_start_preview_width = preview.region.width
        self.capture_mouse()
        event.stop()

    def on_mouse_move(self, event: events.MouseMove) -> None:
        if self._drag_start_x is None:
            return
        editor, preview = self._panes()
        total_width = self._drag_start_editor_width + self._drag_start_preview_width
        editor_width = min(
            max(
                self._drag_start_editor_width + int(event.screen_x) - self._drag_start_x,
                WORKBENCH_MIN_PANE_WIDTH,
            ),
            total_width - WORKBENCH_MIN_PANE_WIDTH,
        )
        editor.styles.width = f"{editor_width}fr"
        preview.styles.width = f"{total_width - editor_width}fr"
        event.stop()

    def on_mouse_up(self, event: events.MouseUp) -> None:
        if self._drag_start_x is None:
            return
        self._drag_start_x = None
        self.release_mouse()
        event.stop()

    def _panes(self) -> tuple[Widget, Widget]:
        return (
            self.screen.query_one("#markdown-editor", Widget),
            self.screen.query_one("#markdown-preview", Widget),
        )
