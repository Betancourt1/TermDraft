"""Rendered Markdown preview that never launches document links."""

from dataclasses import dataclass
from typing import ClassVar

from markdown_it.token import Token
from textual.actions import ActionError
from textual.actions import parse as parse_action
from textual.binding import Binding, BindingType
from textual.content import Content
from textual.css.query import NoMatches
from textual.message import Message
from textual.widgets import Markdown
from textual.widgets.markdown import MarkdownBlock

from termwriter.services.markdown_preview import (
    FOOTNOTE_BACKREF_PREFIX,
    FOOTNOTE_DEFINITION_PREFIX,
    FOOTNOTE_LABEL_TOKEN,
    preview_parser,
)


@dataclass(frozen=True, slots=True)
class _PreviewLink:
    """One rendered Markdown link and the block that contains it."""

    block: MarkdownBlock
    content: Content
    href: str
    start: int
    end: int


@dataclass(frozen=True, slots=True)
class _PreviewHeading:
    """One rendered Markdown heading exposed by Textual's table of contents."""

    block: MarkdownBlock
    label: str
    level: int


class _FootnoteLabel(MarkdownBlock):
    """An anchored definition label understood by Textual's Markdown widget."""

    def __init__(self, markdown: Markdown, token: Token) -> None:
        super().__init__(markdown, token, id=str(token.attrs["id"]))
        self.build_from_token(token)


class MarkdownPreview(Markdown):
    """A Markdown view with an observable source for coordination and tests."""

    can_focus = True
    BINDINGS: ClassVar[list[BindingType]] = [
        Binding("tab", "select_next_link", "Next preview link", show=False),
        Binding("shift+tab", "select_previous_link", "Previous preview link", show=False),
        Binding("enter", "activate_selected_link", "Open preview link", show=False),
        Binding(
            "alt+down",
            "select_next_heading",
            "Next preview heading",
            show=False,
            id="preview_next_heading",
        ),
        Binding(
            "alt+up",
            "select_previous_heading",
            "Previous preview heading",
            show=False,
            id="preview_previous_heading",
        ),
    ]
    DEFAULT_CSS = """
    MarkdownPreview .keyboard-link-selected {
        background: $accent 18%;
    }
    MarkdownPreview .keyboard-heading-selected {
        background: $accent 18%;
        outline-left: thick $accent;
    }
    """

    class HeadingFocused(Message):
        """A rendered heading selected through preview keyboard navigation."""

        def __init__(
            self,
            preview: "MarkdownPreview",
            label: str,
            level: int,
            position: int,
            total: int,
        ) -> None:
            super().__init__()
            self.preview = preview
            self.label = label
            self.level = level
            self.position = position
            self.total = total

        @property
        def control(self) -> "MarkdownPreview":
            """Return the preview associated with this message."""
            return self.preview

    def __init__(self) -> None:
        self.source_text = "Select a Markdown file to begin."
        self._footnote_origins: dict[str, float] = {}
        self._footnote_link_origins: dict[str, int] = {}
        self._links: list[_PreviewLink] = []
        self._selected_link: int | None = None
        self._headings: list[_PreviewHeading] = []
        self._selected_heading: int | None = None
        super().__init__(
            self.source_text,
            id="markdown-preview",
            open_links=False,
            parser_factory=preview_parser,
        )

    async def render_source(self, source: str) -> None:
        """Replace the rendered document and wait for its blocks to mount."""
        self._clear_link_selection()
        self._clear_heading_selection()
        self._links.clear()
        self._headings.clear()
        self._footnote_origins.clear()
        self._footnote_link_origins.clear()
        await self.update(source)
        self.source_text = source
        self._index_links()
        self._index_headings()

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
            if not label.isascii() or not label.isdecimal():
                return
            origin = self._footnote_origins.get(label)
            if origin is not None:
                self.scroll_to(y=origin, animate=False)
            selected = self._footnote_link_origins.get(label)
            if self.has_focus and selected is not None:
                self._select_link(selected, scroll=False)
            return

        target_prefix = f"#{FOOTNOTE_DEFINITION_PREFIX}"
        if not href.startswith(target_prefix):
            return

        label = href.removeprefix(target_prefix)
        if not label.isascii() or not label.isdecimal():
            return
        self._footnote_origins[label] = self.scroll_y
        if self._selected_link is not None and self._links[self._selected_link].href == href:
            self._footnote_link_origins[label] = self._selected_link
        try:
            target = self.query_one(f"#{FOOTNOTE_DEFINITION_PREFIX}{label}")
        except NoMatches:
            return
        target.scroll_visible(top=True)
        if self.has_focus:
            self._select_href(f"{FOOTNOTE_BACKREF_PREFIX}{label}", scroll=False)

    def action_select_next_link(self) -> None:
        """Select the next rendered link or leave the preview at the end."""
        if not self._links:
            self.screen.focus_next()
        elif self._selected_link is None:
            self._select_link(0)
        elif self._selected_link == len(self._links) - 1:
            self._clear_link_selection()
            self.screen.focus_next()
        else:
            self._select_link(self._selected_link + 1)

    def action_select_previous_link(self) -> None:
        """Select the previous rendered link or leave the preview at the start."""
        if not self._links:
            self.screen.focus_previous()
        elif self._selected_link is None:
            self._select_link(len(self._links) - 1)
        elif self._selected_link == 0:
            self._clear_link_selection()
            self.screen.focus_previous()
        else:
            self._select_link(self._selected_link - 1)

    async def action_activate_selected_link(self) -> None:
        """Activate the selected link through Textual's normal Markdown message."""
        if self._selected_link is None:
            return
        selected = self._links[self._selected_link]
        await selected.block.action_link(selected.href)

    def action_select_next_heading(self) -> None:
        """Select the next rendered heading, stopping at the final heading."""
        if not self._headings:
            return
        index = 0 if self._selected_heading is None else self._selected_heading + 1
        self._select_heading(min(index, len(self._headings) - 1))

    def action_select_previous_heading(self) -> None:
        """Select the previous rendered heading, stopping at the first heading."""
        if not self._headings:
            return
        index = (
            len(self._headings) - 1
            if self._selected_heading is None
            else self._selected_heading - 1
        )
        self._select_heading(max(index, 0))

    def on_blur(self) -> None:
        """Remove keyboard selection when focus leaves the preview."""
        self._clear_link_selection()
        self._clear_heading_selection()

    def _index_links(self) -> None:
        """Index Textual link actions from the mounted Markdown blocks."""
        links: list[_PreviewLink] = []
        for block in self.query(MarkdownBlock):
            content = block.render()
            if not isinstance(content, Content):
                continue
            for span in content.spans:
                style = span.style
                if isinstance(style, str):
                    continue
                click = style.meta.get("@click")
                if not isinstance(click, str):
                    continue
                try:
                    namespace, action, arguments = parse_action(click)
                except ActionError:
                    continue
                if (
                    namespace
                    or action != "link"
                    or len(arguments) != 1
                    or not isinstance(arguments[0], str)
                ):
                    continue
                links.append(
                    _PreviewLink(
                        block=block,
                        content=content,
                        href=arguments[0],
                        start=span.start,
                        end=span.end,
                    )
                )
        self._links = links

    def _index_headings(self) -> None:
        """Index mounted heading blocks through Textual's public table of contents."""
        headings: list[_PreviewHeading] = []
        for level, label, block_id in self.table_of_contents:
            if block_id is None:
                continue
            try:
                block = self.query_one(f"#{block_id}", MarkdownBlock)
            except NoMatches:
                continue
            headings.append(_PreviewHeading(block=block, label=label, level=level))
        self._headings = headings

    def _select_href(self, href: str, *, scroll: bool) -> None:
        for index, link in enumerate(self._links):
            if link.href == href:
                self._select_link(index, scroll=scroll)
                return

    def _select_link(self, index: int, *, scroll: bool = True) -> None:
        self._clear_link_selection()
        self._clear_heading_selection()
        self._selected_link = index
        selected = self._links[index]
        selected.block.add_class("keyboard-link-selected")
        selected.block.update(
            selected.content.stylize("reverse bold", selected.start, selected.end),
            layout=False,
        )
        if scroll:
            selected.block.scroll_visible()

    def _select_heading(self, index: int) -> None:
        self._clear_heading_selection()
        self._clear_link_selection()
        self._selected_heading = index
        selected = self._headings[index]
        selected.block.add_class("keyboard-heading-selected")
        selected.block.scroll_visible(animate=False, top=True, immediate=True)
        self.post_message(
            self.HeadingFocused(
                self,
                selected.label,
                selected.level,
                index + 1,
                len(self._headings),
            )
        )

    def _clear_link_selection(self) -> None:
        if self._selected_link is None:
            return
        selected = self._links[self._selected_link]
        selected.block.remove_class("keyboard-link-selected")
        selected.block.update(selected.content, layout=False)
        self._selected_link = None

    def _clear_heading_selection(self) -> None:
        if self._selected_heading is None:
            return
        selected = self._headings[self._selected_heading]
        selected.block.remove_class("keyboard-heading-selected")
        self._selected_heading = None
