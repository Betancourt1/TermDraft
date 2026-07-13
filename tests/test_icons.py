"""Tests for TermWriter's monochrome terminal icon set."""

import regex

from termwriter.icons import FILE_ICON, FOLDER_ICON, IMAGE_ICON, OPEN_FOLDER_ICON, SEARCH_ICON
from termwriter.widgets.file_tree import MarkdownDirectoryTree


def test_interface_uses_monochrome_symbols_instead_of_emoji() -> None:
    icons = (FILE_ICON, FOLDER_ICON, OPEN_FOLDER_ICON, SEARCH_ICON, IMAGE_ICON)

    assert MarkdownDirectoryTree.ICON_FILE == FILE_ICON
    assert MarkdownDirectoryTree.ICON_NODE == FOLDER_ICON
    assert MarkdownDirectoryTree.ICON_NODE_EXPANDED == OPEN_FOLDER_ICON
    assert not any(regex.search(r"\p{Emoji}", icon) for icon in icons)
