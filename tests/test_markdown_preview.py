"""Tests for the safe Markdown preview parser."""

from pathlib import Path

from markdown_it.token import Token
from textual.app import App, ComposeResult
from textual.content import Content
from textual.widgets import Button
from textual.widgets.markdown import MarkdownBlock

from termwriter.services.markdown_preview import (
    FOOTNOTE_BACKREF_PREFIX,
    FOOTNOTE_DEFINITION_PREFIX,
    FOOTNOTE_LABEL_TOKEN,
    preview_parser,
)
from termwriter.widgets.preview import MarkdownPreview


def _token_types(tokens: list[Token]) -> list[str]:
    return [token.type for token in tokens]


def _inline_text(tokens: list[Token]) -> list[str]:
    return [
        "".join(child.content for child in token.children or [] if child.type == "text")
        for token in tokens
        if token.type == "inline"
    ]


def _all_token_types(tokens: list[Token]) -> list[str]:
    def descendants(token: Token) -> list[str]:
        return [
            token.type,
            *(token_type for child in token.children or [] for token_type in descendants(child)),
        ]

    return [token_type for token in tokens for token_type in descendants(token)]


def _footnote_labels(tokens: list[Token]) -> list[Token]:
    return [token for token in tokens if token.type == FOOTNOTE_LABEL_TOKEN]


def _assert_extensions_are_normalized(tokens: list[Token]) -> None:
    unsupported = {"dl_open", "dl_close", "dt_open", "dt_close", "dd_open", "dd_close"}
    assert not unsupported.intersection(_all_token_types(tokens))
    assert not any(token_type.startswith("footnote_") for token_type in _all_token_types(tokens))
    assert not any(token_type.startswith("alert") for token_type in _all_token_types(tokens))


def test_task_lists_render_visible_symbols_at_each_nesting_level() -> None:
    tokens = preview_parser().parse(
        "- [ ] outer\n  - [x] nested complete\n  - [ ] nested pending\n- [x] complete\n"
    )

    assert _inline_text(tokens) == [
        "☐ outer",
        "☑ nested complete",
        "☐ nested pending",
        "☑ complete",
    ]


def test_tables_and_strikethrough_remain_enabled() -> None:
    parser = preview_parser()
    table_tokens = parser.parse("| Name | Done |\n| --- | --- |\n| Draft | no |\n")
    strike_tokens = parser.parse("~~removed~~ and ~also removed~\n")

    assert "table_open" in _token_types(table_tokens)
    inline = next(token for token in strike_tokens if token.type == "inline")
    assert inline.children is not None
    assert [token.type for token in inline.children].count("s_open") == 2
    assert [token.type for token in inline.children].count("s_close") == 2


def test_raw_html_is_literal_text() -> None:
    tokens = preview_parser().parse("<script>alert('no')</script>\n")

    assert "html_block" not in _token_types(tokens)
    assert "html_inline" not in _token_types(tokens)
    assert _inline_text(tokens) == ["<script>alert('no')</script>"]


def test_supported_gfm_alerts_become_safe_titled_blockquotes() -> None:
    source = "\n\n".join(
        f"> [!{kind}]\n> {kind.title()} body with **bold** text."
        for kind in ("NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION")
    )

    tokens = preview_parser().parse(source)

    _assert_extensions_are_normalized(tokens)
    token_types = _token_types(tokens)
    assert token_types.count("blockquote_open") == 5
    assert token_types.count("blockquote_close") == 5
    assert _inline_text(tokens) == [
        "Note",
        "Note body with bold text.",
        "Tip",
        "Tip body with bold text.",
        "Important",
        "Important body with bold text.",
        "Warning",
        "Warning body with bold text.",
        "Caution",
        "Caution body with bold text.",
    ]
    titles = [token for token in tokens if token.type == "inline" and token.content.istitle()]
    assert len(titles) == 5
    assert all(
        title.children is not None
        and title.children[0].type == "strong_open"
        and title.children[-1].type == "strong_close"
        for title in titles
    )


def test_unknown_alert_and_raw_html_remain_literal_text() -> None:
    tokens = preview_parser().parse(
        "> [!DANGER]\n> <script>alert('no')</script>\n\n"
        "> [!WARNING]\n> <img src=x onerror=alert('no')>\n"
    )

    _assert_extensions_are_normalized(tokens)
    assert "html_block" not in _token_types(tokens)
    assert "html_inline" not in _all_token_types(tokens)
    assert "[!DANGER]<script>alert('no')</script>" in _inline_text(tokens)
    assert "<img src=x onerror=alert('no')>" in _inline_text(tokens)


def test_footnotes_become_visible_references_and_supported_list_blocks() -> None:
    tokens = preview_parser().parse(
        "See the named note[^details] and an inline note.^[Inline *detail*.]\n\n"
        "[^details]: First paragraph with `code`.\n\n"
        "    Second paragraph.\n"
    )

    _assert_extensions_are_normalized(tokens)
    assert "See the named note[1] and an inline note.[2]" in _inline_text(tokens)
    labels = _footnote_labels(tokens)
    assert [token.content for token in labels] == ["[1] ↩", "[2] ↩"]
    assert [token.attrs["id"] for token in labels] == [
        f"{FOOTNOTE_DEFINITION_PREFIX}1",
        f"{FOOTNOTE_DEFINITION_PREFIX}2",
    ]
    assert [
        child.attrs["href"]
        for token in labels
        for child in token.children or []
        if child.type == "link_open"
    ] == [f"{FOOTNOTE_BACKREF_PREFIX}1", f"{FOOTNOTE_BACKREF_PREFIX}2"]
    assert any(token.content == "First paragraph with `code`." for token in tokens)
    assert "Second paragraph." in _inline_text(tokens)
    assert _token_types(tokens).count("bullet_list_open") == 1
    assert _token_types(tokens).count("list_item_open") == 2


def test_definition_lists_become_nested_supported_list_blocks() -> None:
    tokens = preview_parser().parse(
        "Outer *term*\n"
        ": First paragraph.\n\n"
        "  Inner term\n"
        "  : Nested definition.\n\n"
        "  - ordinary nested item\n\n"
        "Second term\n"
        "~ Another definition.\n"
    )

    _assert_extensions_are_normalized(tokens)
    token_types = _token_types(tokens)
    assert token_types.count("bullet_list_open") == 3
    assert token_types.count("list_item_open") == 4
    assert token_types.count("blockquote_open") == 3
    assert token_types.count("blockquote_close") == 3
    assert {"Outer term", "Inner term", "Second term"} <= set(_inline_text(tokens))
    term_tokens = [token for token in tokens if token.type == "inline" and "term" in token.content]
    assert term_tokens
    assert all(
        token.children is not None
        and token.children[0].type == "strong_open"
        and token.children[-1].type == "strong_close"
        for token in term_tokens
    )


def test_extension_syntax_inside_fenced_code_is_not_transformed() -> None:
    source = "```markdown\nTerm\n: definition\nReference[^note]\n[^note]: hidden\n```\n"

    tokens = preview_parser().parse(source)

    assert _token_types(tokens) == ["fence"]
    assert tokens[0].content == "Term\n: definition\nReference[^note]\n[^note]: hidden\n"


def test_malformed_extension_syntax_remains_literal_markdown() -> None:
    tokens = preview_parser().parse(
        "Undefined[^missing]\n\n[^not-a-definition]\n\nTerm\n:No required space\n"
    )

    _assert_extensions_are_normalized(tokens)
    assert _inline_text(tokens) == [
        "Undefined[^missing]",
        "[^not-a-definition]",
        "Term:No required space",
    ]
    assert "bullet_list_open" not in _token_types(tokens)


def test_nested_footnote_blocks_keep_supported_markdown_children() -> None:
    tokens = preview_parser().parse(
        "Reference[^nested].\n\n"
        "[^nested]: A note with nested content.\n\n"
        "    - list item\n\n"
        "    ```text\n"
        "    literal [^reference]\n"
        "    ```\n"
    )

    _assert_extensions_are_normalized(tokens)
    assert "fence" in _token_types(tokens)
    fence = next(token for token in tokens if token.type == "fence")
    assert fence.content == "literal [^reference]\n"
    assert _token_types(tokens).count("bullet_list_open") == 2


def test_footnote_references_nested_in_image_alt_text_are_normalized() -> None:
    tokens = preview_parser().parse("![Diagram[^source]](diagram.png)\n\n[^source]: Source note.\n")

    _assert_extensions_are_normalized(tokens)
    image = next(
        child for token in tokens for child in token.children or [] if child.type == "image"
    )
    assert image.children is not None
    assert [(child.type, child.content) for child in image.children] == [
        ("text", "Diagram"),
        ("text", "[1]"),
    ]


def test_named_and_inline_footnotes_receive_distinct_visible_numbers() -> None:
    tokens = preview_parser().parse("Named[^2], inline^[second].\n\n[^2]: Named definition.\n")

    inline_text = _inline_text(tokens)
    assert "Named[1], inline[2]." in inline_text
    assert [token.content for token in _footnote_labels(tokens)] == ["[1] ↩", "[2] ↩"]


async def test_textual_mounts_normalized_extensions_without_unhandled_blocks() -> None:
    class PreviewApp(App[None]):
        def compose(self) -> ComposeResult:
            yield MarkdownPreview()

    app = PreviewApp()
    async with app.run_test() as pilot:
        preview = app.query_one(MarkdownPreview)
        await preview.render_source(
            "> [!WARNING]\n> Alert with **bold** text.\n\n"
            "Term\n: Definition with note[^n].\n\n[^n]: Note body.\n"
        )
        await pilot.pause()
        assert preview.children
        assert preview.query_one(f"#{FOOTNOTE_DEFINITION_PREFIX}1")


async def test_markdown_gallery_mounts_without_changing_its_source() -> None:
    source = (Path(__file__).parents[1] / "docs" / "markdown-gallery.md").read_text(
        encoding="utf-8"
    )

    class GalleryApp(App[None]):
        def compose(self) -> ComposeResult:
            yield MarkdownPreview()

    app = GalleryApp()
    async with app.run_test(size=(100, 40)) as pilot:
        gallery = app.query_one(MarkdownPreview)
        await gallery.render_source(source)
        await pilot.pause()
        assert gallery.source == source
        assert gallery.source_text == source
        assert gallery.children


async def test_footnote_links_scroll_within_preview_and_never_open_urls() -> None:
    source = (
        "Start with a reference[^note] and an [external link](https://example.com).\n\n"
        + "\n\n".join(f"Filler paragraph {number}." for number in range(30))
        + "\n\n[^note]: The internal definition.\n"
    )

    class PreviewApp(App[None]):
        CSS = "MarkdownPreview { height: 8; overflow-y: auto; }"

        def __init__(self) -> None:
            super().__init__()
            self.opened_urls: list[str] = []

        def compose(self) -> ComposeResult:
            yield MarkdownPreview()

        def open_url(self, url: str, *, new_tab: bool = True) -> None:
            self.opened_urls.append(url)

    app = PreviewApp()
    async with app.run_test(size=(60, 12)) as pilot:
        preview = app.query_one(MarkdownPreview)
        await preview.render_source(source)
        await pilot.pause()

        origin = preview.children[0]
        assert isinstance(origin, MarkdownBlock)
        definition = preview.query_one(f"#{FOOTNOTE_DEFINITION_PREFIX}1", MarkdownBlock)

        await origin.action_link(f"#{FOOTNOTE_DEFINITION_PREFIX}1")
        await pilot.pause()
        assert preview.scroll_y > 0

        await definition.action_link(f"{FOOTNOTE_BACKREF_PREFIX}1")
        await pilot.pause()
        assert preview.scroll_y == 0

        await origin.action_link("https://example.com")
        await pilot.pause()
        assert app.opened_urls == []


async def test_keyboard_selects_and_activates_footnote_links() -> None:
    source = (
        "Start with a reference[^note].\n\n"
        + "\n\n".join(f"Filler paragraph {number}." for number in range(30))
        + "\n\n[^note]: The internal definition.\n"
    )

    class PreviewApp(App[None]):
        CSS = "MarkdownPreview { height: 8; overflow-y: auto; }"

        def compose(self) -> ComposeResult:
            yield MarkdownPreview()

    app = PreviewApp()
    async with app.run_test(size=(60, 12)) as pilot:
        preview = app.query_one(MarkdownPreview)
        await preview.render_source(source)
        preview.focus()
        await pilot.pause()

        await pilot.press("tab")
        selected = preview.query_one(".keyboard-link-selected", MarkdownBlock)
        selected_content = selected.render()
        assert isinstance(selected_content, Content)
        assert "Start with a reference" in selected_content.plain

        await pilot.press("enter")
        await pilot.pause()
        assert preview.scroll_y > 0
        selected = preview.query_one(".keyboard-link-selected", MarkdownBlock)
        assert selected.id == f"{FOOTNOTE_DEFINITION_PREFIX}1"

        await pilot.press("enter")
        await pilot.pause()
        assert preview.scroll_y == 0
        selected = preview.query_one(".keyboard-link-selected", MarkdownBlock)
        selected_content = selected.render()
        assert isinstance(selected_content, Content)
        assert "Start with a reference" in selected_content.plain


async def test_keyboard_link_navigation_leaves_preview_at_each_boundary() -> None:
    class PreviewApp(App[None]):
        def __init__(self) -> None:
            super().__init__()
            self.opened_urls: list[str] = []

        def compose(self) -> ComposeResult:
            yield Button("Before", id="before")
            yield MarkdownPreview()
            yield Button("After", id="after")

        def open_url(self, url: str, *, new_tab: bool = True) -> None:
            self.opened_urls.append(url)

    app = PreviewApp()
    async with app.run_test(size=(60, 16)) as pilot:
        preview = app.query_one(MarkdownPreview)
        await preview.render_source(
            "[First](https://one.example) and [second](https://two.example).\n"
        )
        preview.focus()
        await pilot.pause()

        await pilot.press("tab")
        assert preview.query_one(".keyboard-link-selected", MarkdownBlock)
        await pilot.press("enter")
        await pilot.pause()
        assert app.opened_urls == []

        await pilot.press("tab")
        assert preview.query_one(".keyboard-link-selected", MarkdownBlock)
        await pilot.press("tab")
        assert app.focused is app.query_one("#after", Button)
        assert not preview.query(".keyboard-link-selected")

        preview.focus()
        await pilot.press("shift+tab")
        assert preview.query_one(".keyboard-link-selected", MarkdownBlock)
        await pilot.press("shift+tab")
        assert preview.query_one(".keyboard-link-selected", MarkdownBlock)
        await pilot.press("shift+tab")
        assert app.focused is app.query_one("#before", Button)
        assert not preview.query(".keyboard-link-selected")


async def test_malformed_reserved_fragment_stays_inert() -> None:
    class PreviewApp(App[None]):
        def __init__(self) -> None:
            super().__init__()
            self.opened_urls: list[str] = []

        def compose(self) -> ComposeResult:
            yield MarkdownPreview()

        def open_url(self, url: str, *, new_tab: bool = True) -> None:
            self.opened_urls.append(url)

    app = PreviewApp()
    async with app.run_test() as pilot:
        preview = app.query_one(MarkdownPreview)
        await preview.render_source("[boom](#termwriter-footnote-%5B)\n")
        preview.focus()

        await pilot.press("tab", "enter")
        await pilot.pause()

        assert app.opened_urls == []
        assert preview.query(".keyboard-link-selected")
