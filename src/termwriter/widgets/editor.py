"""Markdown source editor configured for prose."""

from textual.widgets import TextArea


class MarkdownEditor(TextArea):
    """A soft-wrapped TextArea that always edits Markdown source."""

    def __init__(self) -> None:
        super().__init__(
            language="markdown",
            soft_wrap=True,
            tab_behavior="indent",
            show_line_numbers=True,
            max_checkpoints=100,
            id="markdown-editor",
            placeholder="Select a Markdown file from the explorer or press Ctrl+P.",
        )
        self.read_only = True
