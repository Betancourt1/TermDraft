"""Safe Markdown parser configuration for the rendered preview."""

from dataclasses import dataclass
from typing import Any, Literal

from markdown_it import MarkdownIt
from markdown_it.rules_core import StateCore
from markdown_it.token import Token
from mdit_py_plugins.deflist import deflist_plugin
from mdit_py_plugins.footnote import footnote_plugin

FOOTNOTE_BACKREF_PREFIX = "#termwriter-footnote-back-"
FOOTNOTE_DEFINITION_PREFIX = "termwriter-footnote-"
FOOTNOTE_LABEL_TOKEN = "termwriter_footnote_label"


@dataclass(slots=True)
class _DefinitionList:
    """State for one definition list while its tokens are normalized."""

    item_open: bool = False
    term_open: bool = False


def _add_task_symbols(state: StateCore) -> None:
    """Expose GFM task state as terminal-friendly text."""
    pending_symbol: str | None = None

    for token in state.tokens:
        if token.type == "list_item_open":
            checked = token.meta.get("checked")
            pending_symbol = "☑ " if checked is True else "☐ " if checked is False else None
        elif token.type == "list_item_close":
            pending_symbol = None
        elif pending_symbol is not None and token.type == "inline":
            if token.children is not None:
                token.children.insert(0, Token("text", "", 0, content=pending_symbol))
            pending_symbol = None


def _retag(
    token: Token,
    token_type: str,
    tag: str,
    nesting: Literal[-1, 0, 1],
) -> Token:
    """Copy a source token with a Textual-supported block identity."""
    return token.copy(type=token_type, tag=tag, nesting=nesting)


def _label(meta: dict[Any, Any]) -> str:
    """Return the parser's unique sequential label for every footnote."""
    identifier = meta.get("id")
    if isinstance(identifier, int):
        return str(identifier + 1)
    label = meta.get("label")
    if isinstance(label, str) and label:
        return label
    return "?"


def _text_token(token: Token, content: str) -> Token:
    """Turn an unsupported inline token into literal preview text."""
    return token.copy(
        type="text",
        tag="",
        nesting=0,
        children=None,
        content=content,
        markup="",
    )


def _footnote_reference_tokens(token: Token) -> list[Token]:
    """Turn one footnote reference into a safe in-preview link."""
    label = _label(token.meta)
    href = f"#{FOOTNOTE_DEFINITION_PREFIX}{label}"
    return [
        Token("link_open", "a", 1, attrs={"href": href}),
        _text_token(token, f"[{label}]"),
        Token("link_close", "a", -1),
    ]


def _normalize_inline_children(tokens: list[Token], *, link_references: bool = True) -> list[Token]:
    """Normalize footnote references at any inline nesting depth."""
    normalized: list[Token] = []
    for token in tokens:
        if token.type == "footnote_ref":
            if link_references:
                normalized.extend(_footnote_reference_tokens(token))
            else:
                normalized.append(_text_token(token, f"[{_label(token.meta)}]"))
        elif token.children is None:
            normalized.append(token)
        else:
            normalized.append(
                token.copy(
                    children=_normalize_inline_children(
                        token.children,
                        link_references=link_references and token.type != "image",
                    )
                )
            )
    return normalized


def _normalize_inline(token: Token, *, bold: bool = False) -> Token:
    """Replace footnote references and optionally emphasize definition terms."""
    if token.children is None:
        return token

    children = _normalize_inline_children(token.children)
    if bold:
        children = [
            Token("strong_open", "strong", 1),
            *children,
            Token("strong_close", "strong", -1),
        ]
    return token.copy(children=children)


def _footnote_label_token(token: Token) -> Token:
    """Build an anchored label with a link back to the last used reference."""
    label = _label(token.meta)
    marker = f"[{label}]"
    return Token(
        FOOTNOTE_LABEL_TOKEN,
        "",
        0,
        attrs={"id": f"{FOOTNOTE_DEFINITION_PREFIX}{label}"},
        children=[
            Token("strong_open", "strong", 1),
            Token("text", "", 0, content=marker),
            Token("strong_close", "strong", -1),
            Token("text", "", 0, content=" "),
            Token(
                "link_open",
                "a",
                1,
                attrs={"href": f"{FOOTNOTE_BACKREF_PREFIX}{label}"},
            ),
            Token("text", "", 0, content="↩"),
            Token("link_close", "a", -1),
        ],
        content=f"{marker} ↩",
        meta=token.meta.copy(),
        block=True,
    )


def _normalize_extensions(state: StateCore) -> None:
    """Map plugin tokens to the conservative block vocabulary Textual renders."""
    normalized: list[Token] = []
    definition_lists: list[_DefinitionList] = []
    alert_title_open = False

    for token in state.tokens:
        token_type = token.type

        if token_type == "alert_open":
            normalized.append(_retag(token, "blockquote_open", "blockquote", 1))
        elif token_type == "alert_title_open":
            normalized.append(_retag(token, "paragraph_open", "p", 1))
            alert_title_open = True
        elif token_type == "alert_title_close":
            normalized.append(_retag(token, "paragraph_close", "p", -1))
            alert_title_open = False
        elif token_type == "alert_close":
            normalized.append(_retag(token, "blockquote_close", "blockquote", -1))
        elif token_type == "dl_open":
            normalized.append(_retag(token, "bullet_list_open", "ul", 1))
            definition_lists.append(_DefinitionList())
        elif token_type == "dt_open":
            if not definition_lists:
                continue
            current = definition_lists[-1]
            if current.item_open:
                normalized.append(_retag(token, "list_item_close", "li", -1))
            normalized.extend(
                [
                    _retag(token, "list_item_open", "li", 1),
                    _retag(token, "paragraph_open", "p", 1),
                ]
            )
            current.item_open = True
            current.term_open = True
        elif token_type == "dt_close":
            if definition_lists and definition_lists[-1].term_open:
                normalized.append(_retag(token, "paragraph_close", "p", -1))
                definition_lists[-1].term_open = False
        elif token_type == "dd_open":
            normalized.append(_retag(token, "blockquote_open", "blockquote", 1))
        elif token_type == "dd_close":
            normalized.append(_retag(token, "blockquote_close", "blockquote", -1))
        elif token_type == "dl_close":
            if not definition_lists:
                continue
            current = definition_lists.pop()
            if current.term_open:
                normalized.append(_retag(token, "paragraph_close", "p", -1))
            if current.item_open:
                normalized.append(_retag(token, "list_item_close", "li", -1))
            normalized.append(_retag(token, "bullet_list_close", "ul", -1))
        elif token_type == "footnote_block_open":
            normalized.append(_retag(token, "bullet_list_open", "ul", 1))
        elif token_type == "footnote_open":
            normalized.append(_retag(token, "list_item_open", "li", 1))
            normalized.append(_footnote_label_token(token))
        elif token_type == "footnote_anchor":
            continue
        elif token_type == "footnote_close":
            normalized.append(_retag(token, "list_item_close", "li", -1))
        elif token_type == "footnote_block_close":
            normalized.append(_retag(token, "bullet_list_close", "ul", -1))
        elif token_type == "inline":
            bold_inline = alert_title_open or bool(
                definition_lists and definition_lists[-1].term_open
            )
            normalized.append(_normalize_inline(token, bold=bold_inline))
        else:
            normalized.append(token)

    state.tokens = normalized
    _add_task_symbols(state)


def preview_parser() -> MarkdownIt:
    """Build the GFM-like parser used only for safe preview rendering."""
    parser = MarkdownIt("gfm-like2", {"alerts": True, "html": False})
    parser.use(deflist_plugin)
    parser.use(
        footnote_plugin,
        inline=True,
        move_to_end=True,
        always_match_refs=False,
    )
    parser.core.ruler.after(
        "footnote_tail",
        "termwriter_preview_extensions",
        _normalize_extensions,
    )
    return parser
