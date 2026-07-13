"""Tests for TermWriter's monochrome terminal icon set."""

import regex
from textual.color import Color

from termwriter.icons import (
    FOLDER_ICON,
    FOLDER_ICON_COLOR,
    IMAGE_ICON,
    IMAGE_ICON_COLOR,
    MARKDOWN_ICON,
    MARKDOWN_ICON_COLOR,
    OPEN_FOLDER_ICON,
    SEARCH_ICON,
    SEARCH_ICON_COLOR,
)
from termwriter.widgets.file_tree import MarkdownDirectoryTree


def test_interface_uses_yazi_compatible_icons_instead_of_emoji() -> None:
    icons = (MARKDOWN_ICON, FOLDER_ICON, OPEN_FOLDER_ICON, SEARCH_ICON, IMAGE_ICON)

    assert MarkdownDirectoryTree.ICON_FILE == MARKDOWN_ICON
    assert MarkdownDirectoryTree.ICON_NODE == FOLDER_ICON
    assert MarkdownDirectoryTree.ICON_NODE_EXPANDED == OPEN_FOLDER_ICON
    assert icons == (" ", " ", " ", "", " ")
    assert not any(regex.search(r"\p{Emoji}", icon) for icon in icons)


def test_icon_colors_are_grayscale() -> None:
    colors = (
        MARKDOWN_ICON_COLOR,
        FOLDER_ICON_COLOR,
        SEARCH_ICON_COLOR,
        IMAGE_ICON_COLOR,
    )

    assert all(color.r == color.g == color.b for color in map(Color.parse, colors))
