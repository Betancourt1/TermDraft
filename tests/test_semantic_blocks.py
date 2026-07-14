"""Tests for conservative, exact semantic source maps."""

from itertools import pairwise

import pytest

from termdraft.services.semantic_blocks import map_semantic_blocks

SEMANTIC_CORPUS = (
    pytest.param("", id="empty"),
    pytest.param(" \t\r\n\r\n", id="blank-only"),
    pytest.param("No final newline", id="no-final-newline"),
    pytest.param("# LF\n\nParagraph\n", id="lf"),
    pytest.param("# CRLF\r\n\r\nParagraph\r\n", id="crlf"),
    pytest.param("# CR\r\rParagraph\r", id="cr"),
    pytest.param("# Mixed\r\n\rParagraph\n", id="mixed-endings"),
    pytest.param("\ufeff# Cafe\u0301 日本語 👩🏽‍💻\n", id="unicode-and-bom"),
    pytest.param("Heading\n=======\n\nSubheading\n----------\n", id="setext-headings"),
    pytest.param(
        "Paragraph with **bold**, _emphasis_, ~~strike~~, `code`, and [a link](/x).\n",
        id="inline-markup",
    ),
    pytest.param(
        "1. outer\n   - child\n     1. grandchild\n2. next\n",
        id="nested-mixed-lists",
    ),
    pytest.param(
        "> quote\n>\n> - child\n>   ```python\n>   print('x')\n>   ```\n",
        id="quote-list-fence",
    ),
    pytest.param("- item\n  > nested quote\n  > continuation\n", id="list-quote"),
    pytest.param("> [!WARNING]\n> Be careful.\n", id="gfm-alert"),
    pytest.param("| A | B |\n|---|:--:|\n| 1 | 2 |\n", id="table"),
    pytest.param("Term\n: Definition\n  continuation\n", id="definition-list"),
    pytest.param(
        "Text[^note].\n\n[^note]: First line.\n\n    Second paragraph.\n",
        id="multiline-footnote",
    ),
    pytest.param(
        "```markdown\n[fake]: /inside-fence\n```\n",
        id="fence-with-fake-reference",
    ),
    pytest.param("    indented <tag> & text\n", id="indented-code"),
    pytest.param("<script>alert('inert')</script>\n", id="html-disabled"),
    pytest.param('Use [one].\n\n[one]: /target "Title"\n', id="link-reference"),
    pytest.param("[long\n label]: /target\n", id="multiline-reference-label"),
    pytest.param(
        '[multi]:\n  /target\n  "Multiline title"\n',
        id="multiline-reference-definition",
    ),
    pytest.param("[same]: /first\n[same]: /second\n", id="duplicate-reference"),
    pytest.param("[broken]: <> trailing garbage\n", id="malformed-reference"),
    pytest.param("```python\nprint('open')", id="unterminated-fence"),
)


def _independent_line_offsets(source: str) -> tuple[int, ...]:
    offsets = [0]
    index = 0
    while index < len(source):
        if source[index] == "\r" and index + 1 < len(source) and source[index + 1] == "\n":
            index += 2
            offsets.append(index)
        elif source[index] in "\r\n":
            index += 1
            offsets.append(index)
        else:
            index += 1
    if offsets[-1] != len(source):
        offsets.append(len(source))
    return tuple(offsets)


def test_maps_supported_top_level_markdown_blocks() -> None:
    source = """\
# Heading

Paragraph.

- item
  - nested

> quote

> [!NOTE]
> alert

| A | B |
|---|---|
| 1 | 2 |

Term
: Definition

```python
print(1)
```

    indented

---

Text[^note]

[^note]: Footnote
"""

    mapping = map_semantic_blocks(source)

    assert [block.kind for block in mapping.blocks] == [
        "heading",
        "paragraph",
        "bullet list",
        "quote",
        "alert",
        "table",
        "definition list",
        "fenced code",
        "indented code",
        "thematic break",
        "paragraph",
        "footnote definition",
    ]
    assert mapping.blocks[0].detail == "H1"
    assert mapping.blocks[4].detail == "NOTE"
    assert mapping.blocks[7].detail == "python"
    assert sum(block.kind == "bullet list" for block in mapping.blocks) == 1


def test_blocks_and_gaps_reconstruct_unicode_and_line_endings_exactly() -> None:
    for source in (
        "# Café ☕\r\n\r\nParagraph\r\n",
        "# 日本語\r\rText",
        "",
        "No final newline",
    ):
        mapping = map_semantic_blocks(source)

        assert "".join(segment.source for segment in mapping.segments) == source
        assert all(
            segment.source == source[segment.start_offset : segment.end_offset]
            for segment in mapping.segments
        )


def test_reference_definition_is_mapped_from_parser_environment() -> None:
    source = "Text [reference].\n\n[reference]: https://example.com\n"

    mapping = map_semantic_blocks(source)

    reference = next(block for block in mapping.blocks if block.kind == "link reference definition")
    assert reference.source == "[reference]: https://example.com\n"
    assert reference.detail == "[REFERENCE]"
    assert "".join(segment.source for segment in mapping.segments) == source


def test_unterminated_fence_stays_inside_source_bounds() -> None:
    source = "```python\nprint('open')"

    mapping = map_semantic_blocks(source)

    assert len(mapping.blocks) == 1
    assert mapping.blocks[0].kind == "fenced code"
    assert mapping.blocks[0].end_offset == len(source)


@pytest.mark.parametrize("source", SEMANTIC_CORPUS)
def test_semantic_corpus_is_a_lossless_non_overlapping_partition(source: str) -> None:
    mapping = map_semantic_blocks(source)
    segments = mapping.segments

    if not source:
        assert not segments
        return

    offsets = _independent_line_offsets(source)
    assert segments[0].start_offset == 0
    assert segments[-1].end_offset == len(source)
    assert all(left.end_offset == right.start_offset for left, right in pairwise(segments))
    assert all(segment.start_offset < segment.end_offset for segment in segments)
    assert all(
        segment.source == source[segment.start_offset : segment.end_offset] for segment in segments
    )
    assert all(offsets[segment.start_line] == segment.start_offset for segment in segments)
    assert all(offsets[segment.end_line] == segment.end_offset for segment in segments)
    assert "".join(segment.source for segment in segments) == source
    assert b"".join(segment.source.encode("utf-8") for segment in segments) == source.encode(
        "utf-8"
    )
    assert len(segments) == len(mapping.blocks) + len(mapping.gaps)
    assert all(left.end_offset <= right.start_offset for left, right in pairwise(mapping.blocks))


@pytest.mark.parametrize(
    ("source", "outer_kind"),
    (
        ("> quote\n> - item\n>   - nested\n", "quote"),
        ("- item\n  > quote\n  > continuation\n", "bullet list"),
        ("> [!NOTE]\n> - item\n>   - nested\n", "alert"),
        ("> [nested]: /inside\n>\n> text [nested]\n", "quote"),
    ),
)
def test_nested_container_uses_one_exact_outer_range(source: str, outer_kind: str) -> None:
    mapping = map_semantic_blocks(source)

    assert len(mapping.blocks) == 1
    assert mapping.blocks[0].kind == outer_kind
    assert mapping.blocks[0].source == source
    assert mapping.blocks[0].start_offset == 0
    assert mapping.blocks[0].end_offset == len(source)


def test_primary_duplicate_and_multiline_reference_ranges_are_exact() -> None:
    source = (
        '[one]: /first\n[one]: /second\n[multi]:\n  /target\n  "A title"\n\nText [one] [multi].\n'
    )

    mapping = map_semantic_blocks(source)
    references = [block for block in mapping.blocks if block.kind == "link reference definition"]

    assert [block.source for block in references] == [
        "[one]: /first\n",
        "[one]: /second\n",
        '[multi]:\n  /target\n  "A title"\n',
    ]
    assert [block.detail for block in references] == ["[ONE]", "[ONE]", "[MULTI]"]
    assert "".join(segment.source for segment in mapping.segments) == source


def test_malformed_reference_like_source_stays_ordinary_markdown() -> None:
    source = "[broken]: <> trailing garbage\n"

    mapping = map_semantic_blocks(source)

    assert all(block.kind != "link reference definition" for block in mapping.blocks)
    assert "".join(segment.source for segment in mapping.segments) == source
