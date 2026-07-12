"""Tests for the safe Markdown preview parser."""

from markdown_it.token import Token
from textual.app import App, ComposeResult
from textual.widgets import Markdown

from termwriter.services.markdown_preview import preview_parser


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


def _assert_extensions_are_normalized(tokens: list[Token]) -> None:
    unsupported = {"dl_open", "dl_close", "dt_open", "dt_close", "dd_open", "dd_close"}
    assert not unsupported.intersection(_all_token_types(tokens))
    assert not any(token_type.startswith("footnote_") for token_type in _all_token_types(tokens))


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


def test_gfm_alert_syntax_stays_a_safe_blockquote() -> None:
    tokens = preview_parser().parse("> [!NOTE]\n> Preview only.\n")
    token_types = _token_types(tokens)

    assert "blockquote_open" in token_types
    assert not any(token_type.startswith("alert") for token_type in token_types)
    assert _inline_text(tokens) == ["[!NOTE]Preview only."]


def test_footnotes_become_visible_references_and_supported_list_blocks() -> None:
    tokens = preview_parser().parse(
        "See the named note[^details] and an inline note.^[Inline *detail*.]\n\n"
        "[^details]: First paragraph with `code`.\n\n"
        "    Second paragraph.\n"
    )

    _assert_extensions_are_normalized(tokens)
    assert "See the named note[details] and an inline note.[2]" in _inline_text(tokens)
    assert "[details]" in _inline_text(tokens)
    assert "[2]" in _inline_text(tokens)
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
        ("text", "[source]"),
    ]


async def test_textual_mounts_normalized_extensions_without_unhandled_blocks() -> None:
    class PreviewApp(App[None]):
        def compose(self) -> ComposeResult:
            yield Markdown(
                "Term\n: Definition with note[^n].\n\n[^n]: Note body.\n",
                parser_factory=preview_parser,
            )

    app = PreviewApp()
    async with app.run_test() as pilot:
        await pilot.pause()
        assert app.query_one(Markdown).children
