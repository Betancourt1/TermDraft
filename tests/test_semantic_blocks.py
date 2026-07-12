"""Tests for conservative, exact semantic source maps."""

from termwriter.services.semantic_blocks import map_semantic_blocks


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


def test_reference_definition_is_reported_as_unmapped_source() -> None:
    source = "Text [reference].\n\n[reference]: https://example.com\n"

    mapping = map_semantic_blocks(source)

    assert any(
        gap.kind == "unmapped source" and "[reference]:" in gap.source for gap in mapping.gaps
    )
    assert "".join(segment.source for segment in mapping.segments) == source


def test_unterminated_fence_stays_inside_source_bounds() -> None:
    source = "```python\nprint('open')"

    mapping = map_semantic_blocks(source)

    assert len(mapping.blocks) == 1
    assert mapping.blocks[0].kind == "fenced code"
    assert mapping.blocks[0].end_offset == len(source)
