"""Tests for Markdown continuation edits."""

from __future__ import annotations

import pytest

from termdraft.services.markdown_continuation import (
    MarkdownContinuationEdit,
    continuation_edit,
)


@pytest.mark.parametrize(
    ("source", "expected_text", "expected_column"),
    [
        ("- item", "\n- ", 2),
        ("  * item", "\n  * ", 4),
        ("   + item", "\n   + ", 5),
        ("1. item", "\n2. ", 3),
        ("9) item", "\n10) ", 4),
        ("009. item", "\n010. ", 5),
        ("- [ ] todo", "\n- [ ] ", 6),
        ("  + [x] done", "\n  + [ ] ", 8),
        ("* [X] done", "\n* [ ] ", 6),
        ("> quote", "\n> ", 2),
        ("> > nested", "\n> > ", 4),
        ("> 1. item", "\n> 2. ", 5),
        (">    - indented item", "\n>    - ", 7),
        ("> >    * nested item", "\n> >    * ", 9),
        (">\t- tab-indented item", "\n>\t- ", 4),
        ("> \t1. tab-indented item", "\n> \t2. ", 6),
        ("   > - indented quote item", "\n   > - ", 7),
    ],
)
def test_continues_common_markdown_prefixes(
    source: str,
    expected_text: str,
    expected_column: int,
) -> None:
    edit = continuation_edit(source, 0, len(source))

    assert edit == MarkdownContinuationEdit(
        start_line=0,
        start_column=len(source),
        end_line=0,
        end_column=len(source),
        text=expected_text,
        cursor_line=1,
        cursor_column=expected_column,
    )


def test_continuation_splits_content_at_cursor() -> None:
    edit = continuation_edit("- first second", 0, 7)

    assert edit is not None
    assert edit.text == "\n- "
    assert edit.start_column == 7
    assert edit.end_column == 7


@pytest.mark.parametrize("source", ["- ", "  *   ", "1. ", "- [ ]", "- [x] ", "> "])
def test_empty_marker_terminates_the_construct(source: str) -> None:
    edit = continuation_edit(source, 0, len(source))

    assert edit == MarkdownContinuationEdit(
        start_line=0,
        start_column=0,
        end_line=0,
        end_column=len(source),
        text="\n",
        cursor_line=1,
        cursor_column=0,
    )


def test_uses_current_crlf_ending() -> None:
    source = "intro\r\n- item\r\nnext"

    edit = continuation_edit(source, 1, len("- item"))

    assert edit is not None
    assert edit.text == "\r\n- "


def test_last_line_reuses_preceding_crlf_ending() -> None:
    source = "intro\r\n- item"

    edit = continuation_edit(source, 1, len("- item"))

    assert edit is not None
    assert edit.text == "\r\n- "


def test_current_line_ending_wins_in_mixed_document() -> None:
    source = "intro\r\n- item\nnext"

    edit = continuation_edit(source, 1, len("- item"))

    assert edit is not None
    assert edit.text == "\n- "


@pytest.mark.parametrize(
    ("source", "line"),
    [
        ("```\n- code\n```", 1),
        ("~~~python\n1. code\n~~~~", 1),
        ("   ```\n> code\n   ```", 1),
    ],
)
def test_does_not_continue_inside_fenced_code(source: str, line: int) -> None:
    current_line = source.splitlines()[line]

    assert continuation_edit(source, line, len(current_line)) is None


def test_does_not_continue_inside_blockquoted_fenced_code() -> None:
    source = "> ```\n> - literal\n> ```"

    assert continuation_edit(source, 1, len("> - literal")) is None


def test_continues_after_fenced_code_closes() -> None:
    source = "```\n- code\n```\n- prose"

    edit = continuation_edit(source, 3, len("- prose"))

    assert edit is not None
    assert edit.text == "\n- "


@pytest.mark.parametrize("source", ["* * *", "- - -", "***", "___", "  *  *  *  "])
def test_thematic_breaks_are_not_treated_as_list_items(source: str) -> None:
    assert continuation_edit(source, 0, len(source)) is None


@pytest.mark.parametrize(
    ("source", "line"),
    [
        ("    - indented code", 0),
        ("\t* indented code", 0),
        (" \t1. indented code", 0),
        ("paragraph\n    - indented code", 1),
        ("- old list\n\noutside paragraph\n\n    - indented code", 4),
        ("100. parent\n    - paragraph continuation", 1),
        (">     - quoted indented code", 0),
        (">\t  - quoted indented code", 0),
        (">   \t1. quoted indented code", 0),
        ("    > - indented code", 0),
        ("\t> 1. indented code", 0),
        ("> - old list\n>\n> outside paragraph\n>\n>     - indented code", 4),
        ("> 100. parent\n>     - paragraph continuation", 1),
    ],
)
def test_indented_code_is_not_treated_as_a_list(source: str, line: int) -> None:
    current_line = source.splitlines()[line]

    assert continuation_edit(source, line, len(current_line)) is None


@pytest.mark.parametrize(
    ("source", "line", "expected_text"),
    [
        ("- parent\n    - child", 1, "\n    - "),
        ("- parent\n\t* child", 1, "\n\t* "),
        ("10. parent\n    1. child", 1, "\n    2. "),
        ("100. parent\n     - child", 1, "\n     - "),
        ("- parent\n    - first\n    - second", 2, "\n    - "),
        ("- parent\n  continued text\n    - child", 2, "\n    - "),
        ("- parent\n\n    - child", 2, "\n    - "),
        ("- parent\n    > - quoted child", 1, "\n    > - "),
        ("10. parent\n    > 1. quoted child", 1, "\n    > 2. "),
        ("> - parent\n>     - child", 1, "\n>     - "),
        ("> 10. parent\n>     1) child", 1, "\n>     2) "),
        ("> 100. parent\n>      - child", 1, "\n>      - "),
    ],
)
def test_indented_lists_continue_under_an_open_list_parent(
    source: str,
    line: int,
    expected_text: str,
) -> None:
    current_line = source.splitlines()[line]

    edit = continuation_edit(source, line, len(current_line))

    assert edit is not None
    assert edit.text == expected_text


@pytest.mark.parametrize("source", ["    - ", "\t1. ", ">     - [ ]"])
def test_empty_indented_code_marker_does_not_delete_the_line(source: str) -> None:
    assert continuation_edit(source, 0, len(source)) is None


def test_empty_nested_list_marker_still_terminates_the_list() -> None:
    source = "- parent\n    - "

    edit = continuation_edit(source, 1, len("    - "))

    assert edit is not None
    assert edit.start_line == 1
    assert edit.start_column == 0
    assert edit.end_column == len("    - ")
    assert edit.text == "\n"


@pytest.mark.parametrize(
    ("source", "line", "column"),
    [
        ("plain text", 0, 10),
        ("- item", -1, 0),
        ("- item", 1, 0),
        ("- item", 0, 99),
        ("- item", 0, 1),
        ("", 0, 0),
    ],
)
def test_returns_none_when_no_continuation_applies(
    source: str,
    line: int,
    column: int,
) -> None:
    assert continuation_edit(source, line, column) is None
