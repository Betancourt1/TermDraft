"""Tests for the safe Markdown preview parser."""

from markdown_it.token import Token

from termwriter.services.markdown_preview import preview_parser


def _token_types(tokens: list[Token]) -> list[str]:
    return [token.type for token in tokens]


def _inline_text(tokens: list[Token]) -> list[str]:
    return [
        "".join(child.content for child in token.children or [] if child.type == "text")
        for token in tokens
        if token.type == "inline"
    ]


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
