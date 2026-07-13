"""Filtered Markdown workspace explorer."""

from __future__ import annotations

from collections.abc import Iterable
from pathlib import Path

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Vertical
from textual.widgets import DirectoryTree, Static

from termwriter.icons import FILE_ICON, FOLDER_ICON, OPEN_FOLDER_ICON
from termwriter.models.workspace import IGNORED_DIRECTORIES, MARKDOWN_SUFFIXES, Workspace


class MarkdownDirectoryTree(DirectoryTree):
    """Directory tree that omits unsafe paths and non-Markdown files."""

    ICON_FILE = FILE_ICON
    ICON_NODE = FOLDER_ICON
    ICON_NODE_EXPANDED = OPEN_FOLDER_ICON

    def __init__(self, workspace: Workspace) -> None:
        self.workspace = workspace
        super().__init__(workspace.root, id="file-tree")
        self.show_root = False

    def filter_paths(self, paths: Iterable[Path]) -> Iterable[Path]:
        filtered: list[Path] = []
        for path in paths:
            try:
                if path.is_symlink() or not self.workspace.contains(path):
                    continue
                if path.is_dir():
                    if path.name not in IGNORED_DIRECTORIES:
                        filtered.append(path)
                elif path.suffix.casefold() in MARKDOWN_SUFFIXES:
                    filtered.append(path)
            except OSError:
                continue
        return filtered


class FileExplorer(Vertical):
    """Explorer panel with a plain-text active-file indicator."""

    def __init__(self, workspace: Workspace) -> None:
        self.workspace = workspace
        super().__init__(id="file-explorer")

    def compose(self) -> ComposeResult:
        yield Static("Files", id="explorer-title", markup=False)
        yield MarkdownDirectoryTree(self.workspace)

    @property
    def directory_tree(self) -> MarkdownDirectoryTree:
        return self.query_one(MarkdownDirectoryTree)

    def set_active(self, path: Path | None) -> None:
        title = Text("Files")
        if path is not None:
            title.append(" · ", style="dim")
            title.append(path.relative_to(self.workspace.root).as_posix())
        self.query_one("#explorer-title", Static).update(title)
