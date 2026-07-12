"""Read-only top-level Markdown source mapping."""

from __future__ import annotations

import re
from dataclasses import dataclass

from markdown_it import MarkdownIt
from markdown_it.token import Token
from mdit_py_plugins.deflist import deflist_plugin
from mdit_py_plugins.footnote import footnote_plugin

_LINE_ENDING = re.compile(r"\r\n|\r|\n")
_KINDS = {
    "heading_open": "heading",
    "paragraph_open": "paragraph",
    "bullet_list_open": "bullet list",
    "ordered_list_open": "ordered list",
    "blockquote_open": "quote",
    "alert_open": "alert",
    "table_open": "table",
    "dl_open": "definition list",
    "fence": "fenced code",
    "code_block": "indented code",
    "hr": "thematic break",
    "footnote_reference_open": "footnote definition",
}


@dataclass(frozen=True, slots=True)
class SemanticBlock:
    """One exact source slice described by a parser line map."""

    kind: str
    start_line: int
    end_line: int
    start_offset: int
    end_offset: int
    source: str
    detail: str | None = None


@dataclass(frozen=True, slots=True)
class SemanticBlockMap:
    """Mapped top-level blocks and the exact source gaps between them."""

    blocks: tuple[SemanticBlock, ...]
    gaps: tuple[SemanticBlock, ...]

    @property
    def segments(self) -> tuple[SemanticBlock, ...]:
        """Return blocks and gaps in source order."""
        return tuple(sorted((*self.blocks, *self.gaps), key=lambda block: block.start_offset))


def map_semantic_blocks(source: str) -> SemanticBlockMap:
    """Map conservative top-level parser tokens without changing source bytes."""
    offsets = _line_offsets(source)
    candidates: list[SemanticBlock] = []
    for token in _semantic_parser().parse(source):
        block = _block_from_token(token, source, offsets)
        if block is not None:
            candidates.append(block)

    blocks: list[SemanticBlock] = []
    gaps: list[SemanticBlock] = []
    cursor_offset = 0
    cursor_line = 0
    for block in sorted(candidates, key=lambda item: (item.start_offset, -item.end_offset)):
        if block.start_offset < cursor_offset:
            continue
        if cursor_offset < block.start_offset:
            gaps.append(
                _gap(source, cursor_offset, block.start_offset, cursor_line, block.start_line)
            )
        blocks.append(block)
        cursor_offset = block.end_offset
        cursor_line = block.end_line
    if cursor_offset < len(source):
        gaps.append(
            _gap(
                source,
                cursor_offset,
                len(source),
                cursor_line,
                len(offsets) - 1,
            )
        )
    return SemanticBlockMap(tuple(blocks), tuple(gaps))


def _semantic_parser() -> MarkdownIt:
    parser = MarkdownIt("gfm-like2", {"alerts": True, "html": False})
    parser.use(deflist_plugin)
    parser.use(
        footnote_plugin,
        inline=True,
        move_to_end=False,
        always_match_refs=False,
    )
    return parser


def _line_offsets(source: str) -> tuple[int, ...]:
    offsets = [0, *(match.end() for match in _LINE_ENDING.finditer(source))]
    if offsets[-1] != len(source):
        offsets.append(len(source))
    return tuple(offsets)


def _block_from_token(
    token: Token,
    source: str,
    offsets: tuple[int, ...],
) -> SemanticBlock | None:
    source_map = token.map
    if token.level != 0 or token.nesting < 0 or source_map is None:
        return None
    start_line, end_line = source_map
    if (
        start_line < 0
        or end_line <= start_line
        or start_line >= len(offsets)
        or end_line >= len(offsets)
    ):
        return None
    start_offset = offsets[start_line]
    end_offset = offsets[end_line]
    kind = _KINDS.get(token.type, token.type.removesuffix("_open").replace("_", " "))
    return SemanticBlock(
        kind=kind,
        start_line=start_line,
        end_line=end_line,
        start_offset=start_offset,
        end_offset=end_offset,
        source=source[start_offset:end_offset],
        detail=_token_detail(token),
    )


def _token_detail(token: Token) -> str | None:
    if token.type == "heading_open":
        return token.tag.upper()
    if token.type == "alert_open":
        kind = token.meta.get("kind")
        return kind if isinstance(kind, str) else None
    if token.type == "fence":
        return token.info.strip() or None
    if token.type == "ordered_list_open":
        start = token.attrGet("start")
        return None if start is None else f"starts at {start}"
    return None


def _gap(
    source: str,
    start_offset: int,
    end_offset: int,
    start_line: int,
    end_line: int,
) -> SemanticBlock:
    gap_source = source[start_offset:end_offset]
    kind = "separator" if not gap_source.strip() else "unmapped source"
    return SemanticBlock(
        kind,
        start_line,
        end_line,
        start_offset,
        end_offset,
        gap_source,
    )
