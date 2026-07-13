"""Tests for TermWriter's monochrome terminal icon set."""

import regex

from termwriter.icons import FOLDER_ICON, IMAGE_ICON, MARKDOWN_ICON, OPEN_FOLDER_ICON, SEARCH_ICON
from termwriter.widgets.file_tree import MarkdownDirectoryTree


def test_interface_uses_yazi_compatible_icons_instead_of_emoji() -> None:
    icons = (MARKDOWN_ICON, FOLDER_ICON, OPEN_FOLDER_ICON, SEARCH_ICON, IMAGE_ICON)

    assert MarkdownDirectoryTree.ICON_FILE == MARKDOWN_ICON
    assert MarkdownDirectoryTree.ICON_NODE == FOLDER_ICON
    assert MarkdownDirectoryTree.ICON_NODE_EXPANDED == OPEN_FOLDER_ICON
    assert icons == (" ", " ", " ", "", " ")
    assert not any(regex.search(r"\p{Emoji}", icon) for icon in icons)
