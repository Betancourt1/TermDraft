"""Rendered Markdown preview that never launches document links."""

from textual.widgets import Markdown


class MarkdownPreview(Markdown):
    """A Markdown view with an observable source for coordination and tests."""

    def __init__(self) -> None:
        self.source_text = "Select a Markdown file to begin."
        super().__init__(self.source_text, id="markdown-preview", open_links=False)

    async def render_source(self, source: str) -> None:
        """Replace the rendered document and wait for its blocks to mount."""
        await self.update(source)
        self.source_text = source
