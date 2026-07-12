"""Rendered Markdown preview that never launches document links."""

from markdown_it.token import Token
from textual.css.query import NoMatches
from textual.widgets import Markdown
from textual.widgets.markdown import MarkdownBlock

from termwriter.services.markdown_preview import (
    FOOTNOTE_BACKREF_PREFIX,
    FOOTNOTE_DEFINITION_PREFIX,
    FOOTNOTE_LABEL_TOKEN,
    preview_parser,
)


class _FootnoteLabel(MarkdownBlock):
    """An anchored definition label understood by Textual's Markdown widget."""

    def __init__(self, markdown: Markdown, token: Token) -> None:
        super().__init__(markdown, token, id=str(token.attrs["id"]))
        self.build_from_token(token)


class MarkdownPreview(Markdown):
    """A Markdown view with an observable source for coordination and tests."""

    def __init__(self) -> None:
        self.source_text = "Select a Markdown file to begin."
        self._footnote_origins: dict[str, float] = {}
        super().__init__(
            self.source_text,
            id="markdown-preview",
            open_links=False,
            parser_factory=preview_parser,
        )

    async def render_source(self, source: str) -> None:
        """Replace the rendered document and wait for its blocks to mount."""
        self._footnote_origins.clear()
        await self.update(source)
        self.source_text = source

    def unhandled_token(self, token: Token) -> MarkdownBlock | None:
        """Mount the one custom block used as a footnote definition target."""
        if token.type == FOOTNOTE_LABEL_TOKEN:
            return _FootnoteLabel(self, token)
        return super().unhandled_token(token)

    def on_markdown_link_clicked(self, event: Markdown.LinkClicked) -> None:
        """Handle footnotes inside the preview and keep every other link inert."""
        event.stop()
        href = event.href

        if href.startswith(FOOTNOTE_BACKREF_PREFIX):
            label = href.removeprefix(FOOTNOTE_BACKREF_PREFIX)
            origin = self._footnote_origins.get(label)
            if origin is not None:
                self.scroll_to(y=origin, animate=False)
            return

        target_prefix = f"#{FOOTNOTE_DEFINITION_PREFIX}"
        if not href.startswith(target_prefix):
            return

        label = href.removeprefix(target_prefix)
        self._footnote_origins[label] = self.scroll_y
        try:
            target = self.query_one(f"#{FOOTNOTE_DEFINITION_PREFIX}{label}")
        except NoMatches:
            return
        target.scroll_visible(top=True)
