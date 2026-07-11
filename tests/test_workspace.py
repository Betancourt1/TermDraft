"""Tests for workspace boundaries and Markdown discovery."""

from __future__ import annotations

from pathlib import Path

import pytest

from termwriter.models.workspace import (
    UnsafePathError,
    Workspace,
    WorkspaceNotFoundError,
)


def test_workspace_filters_supported_files_and_ignored_directories(tmp_path: Path) -> None:
    (tmp_path / "notes").mkdir()
    (tmp_path / "notes" / "one.md").write_text("one", encoding="utf-8")
    (tmp_path / "notes" / "DOS.MARKDOWN").write_text("two", encoding="utf-8")
    (tmp_path / "notes" / "ignore.txt").write_text("no", encoding="utf-8")
    for ignored in (".git", ".venv", "node_modules", "__pycache__"):
        directory = tmp_path / ignored
        directory.mkdir()
        (directory / "hidden.md").write_text("hidden", encoding="utf-8")

    result = Workspace.from_target(tmp_path).scan()

    assert [path.name for path in result.files] == ["DOS.MARKDOWN", "one.md"]
    assert result.warnings == ()


def test_workspace_accepts_unicode_filenames(tmp_path: Path) -> None:
    path = tmp_path / "日本語-café.md"
    path.write_text("hello", encoding="utf-8")

    result = Workspace.from_target(tmp_path).scan()

    assert result.files == (path,)


def test_workspace_rejects_missing_target(tmp_path: Path) -> None:
    with pytest.raises(WorkspaceNotFoundError):
        Workspace.from_target(tmp_path / "missing")


def test_workspace_rejects_relative_escape(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    outside = tmp_path / "outside.md"
    outside.write_text("outside", encoding="utf-8")
    workspace = Workspace.from_target(workspace_root)

    with pytest.raises(UnsafePathError):
        workspace.validate_document_path(Path("../outside.md"))


def test_workspace_does_not_follow_file_or_directory_symlinks(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    outside_file = tmp_path / "outside.md"
    outside_file.write_text("outside", encoding="utf-8")
    outside_directory = tmp_path / "outside"
    outside_directory.mkdir()
    (outside_directory / "nested.md").write_text("nested", encoding="utf-8")
    file_link = workspace_root / "linked.md"
    directory_link = workspace_root / "linked-directory"
    file_link.symlink_to(outside_file)
    directory_link.symlink_to(outside_directory, target_is_directory=True)
    workspace = Workspace.from_target(workspace_root)

    assert workspace.scan().files == ()
    with pytest.raises(UnsafePathError):
        workspace.validate_document_path(file_link)


def test_individual_markdown_file_becomes_initial_file(tmp_path: Path) -> None:
    path = tmp_path / "essay.md"
    path.write_text("essay", encoding="utf-8")

    workspace = Workspace.from_target(path)

    assert workspace.root == tmp_path.resolve()
    assert workspace.initial_file == path
