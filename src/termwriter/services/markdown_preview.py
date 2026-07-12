"""Safe Markdown parser configuration for the rendered preview."""

from dataclasses import dataclass
from typing import Any, Literal

from markdown_it import MarkdownIt
from markdown_it.rules_core import StateCore
from markdown_it.token import Token
from mdit_py_plugins.deflist import deflist_plugin
from mdit_py_plugins.footnote import footnote_plugin


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


def _normalize_inline_child(token: Token) -> Token:
    """Normalize a footnote reference at any inline nesting depth."""
    if token.type == "footnote_ref":
        return _text_token(token, f"[{_label(token.meta)}]")
    if token.children is None:
        return token
    return token.copy(children=[_normalize_inline_child(child) for child in token.children])


def _normalize_inline(token: Token, *, bold: bool = False) -> Token:
    """Replace footnote references and optionally emphasize definition terms."""
    if token.children is None:
        return token

    children = [_normalize_inline_child(child) for child in token.children]
    if bold:
        children = [
            Token("strong_open", "strong", 1),
            *children,
            Token("strong_close", "strong", -1),
        ]
    return token.copy(children=children)


def _footnote_label_tokens(token: Token) -> list[Token]:
    """Build a small paragraph that labels one rendered footnote definition."""
    marker = f"[{_label(token.meta)}]"
    return [
        token.copy(type="paragraph_open", tag="p", nesting=1, children=None),
        Token(
            "inline",
            "",
            0,
            children=[
                Token("strong_open", "strong", 1),
                Token("text", "", 0, content=marker),
                Token("strong_close", "strong", -1),
            ],
            content=marker,
            block=True,
        ),
        token.copy(type="paragraph_close", tag="p", nesting=-1, children=None),
    ]


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
        elif token_type in {"dd_open", "dd_close"}:
            continue
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
            normalized.extend(_footnote_label_tokens(token))
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
