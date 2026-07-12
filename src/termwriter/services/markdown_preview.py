"""Safe Markdown parser configuration for the rendered preview."""

from markdown_it import MarkdownIt
from markdown_it.rules_core import StateCore
from markdown_it.token import Token


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


def preview_parser() -> MarkdownIt:
    """Build the GFM-like parser used only for safe preview rendering."""
    parser = MarkdownIt("gfm-like2", {"alerts": False, "html": False})
    parser.core.ruler.after("inline", "termwriter_task_symbols", _add_task_symbols)
    return parser
