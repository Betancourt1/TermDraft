"""Small, explicit file and folder operations inside one workspace."""

from __future__ import annotations

import ctypes
import errno
import os
import shutil
import sys
from collections.abc import Callable
from pathlib import Path
from typing import cast

from termwriter.models.workspace import (
    IGNORED_DIRECTORIES,
    MARKDOWN_SUFFIXES,
    Workspace,
    WorkspaceError,
)
from termwriter.services.persistence import PersistenceError, atomic_save, snapshot_file


class WorkspaceEntryError(Exception):
    """A file-management failure suitable for display in the UI."""


def _rename_no_replace(source: Path, target: Path) -> None:
    """Atomically rename one entry without replacing a racing destination."""
    if sys.platform == "darwin":
        function_name = "renameatx_np"
        exclusive_flag = 0x00000004
    elif sys.platform.startswith("linux"):
        function_name = "renameat2"
        exclusive_flag = 0x00000001
    else:
        raise WorkspaceEntryError("Exclusive workspace moves are unavailable on this platform.")

    library = ctypes.CDLL(None, use_errno=True)
    try:
        raw_rename = getattr(library, function_name)
    except AttributeError as error:
        raise WorkspaceEntryError(
            "Exclusive workspace moves are unavailable on this system."
        ) from error
    rename = cast(Callable[[int, bytes, int, bytes, int], int], raw_rename)

    directory_flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    source_directory = os.open(source.parent, directory_flags)
    try:
        target_directory = os.open(target.parent, directory_flags)
        try:
            result = rename(
                source_directory,
                os.fsencode(source.name),
                target_directory,
                os.fsencode(target.name),
                exclusive_flag,
            )
            error_number = ctypes.get_errno()
        finally:
            os.close(target_directory)
    finally:
        os.close(source_directory)

    if result == 0:
        return
    if error_number in {errno.EEXIST, errno.ENOTEMPTY}:
        raise WorkspaceEntryError(f"An entry already exists at {target}.")
    if error_number in {errno.ENOSYS, errno.ENOTSUP, errno.EOPNOTSUPP}:
        raise WorkspaceEntryError("This filesystem does not support exclusive workspace moves.")
    raise OSError(error_number, os.strerror(error_number), target)


def _validate_name(name: str) -> str:
    if not name or name in {".", ".."} or Path(name).name != name:
        raise WorkspaceEntryError("Enter one file or folder name, without a path.")
    return name


def _validate_visible_path(workspace: Workspace, path: Path, *, must_exist: bool) -> Path:
    try:
        safe_path = workspace.validate_entry_path(path, must_exist=must_exist)
    except WorkspaceError as error:
        raise WorkspaceEntryError(str(error)) from error
    relative = safe_path.relative_to(workspace.root)
    if any(part in IGNORED_DIRECTORIES for part in relative.parts):
        raise WorkspaceEntryError("TermWriter cannot manage entries inside an ignored folder.")
    return safe_path


def _validate_parent(workspace: Workspace, path: Path) -> Path:
    try:
        parent = workspace.validate_entry_path(path, must_exist=True, allow_root=True)
    except WorkspaceError as error:
        raise WorkspaceEntryError(str(error)) from error
    relative = parent.relative_to(workspace.root)
    if any(part in IGNORED_DIRECTORIES for part in relative.parts):
        raise WorkspaceEntryError("TermWriter cannot manage entries inside an ignored folder.")
    if not parent.is_dir():
        raise WorkspaceEntryError(f"Not a folder: {parent}")
    return parent


def create_markdown_file(workspace: Workspace, parent: Path, name: str) -> Path:
    """Create one empty Markdown file without replacing an existing entry."""
    safe_parent = _validate_parent(workspace, parent)
    filename = _validate_name(name)
    if Path(filename).suffix.casefold() not in MARKDOWN_SUFFIXES:
        filename += ".md"
    try:
        target = workspace.validate_document_path(safe_parent / filename, must_exist=False)
        expected = snapshot_file(target)
        if expected.exists or target.is_symlink():
            raise WorkspaceEntryError(f"An entry already exists at {target.name}.")
        atomic_save(target, "", encoding="utf-8", expected=expected)
    except WorkspaceEntryError:
        raise
    except (OSError, PersistenceError, WorkspaceError) as error:
        raise WorkspaceEntryError(str(error)) from error
    return target


def create_folder(workspace: Workspace, parent: Path, name: str) -> Path:
    """Create one folder without creating implicit ancestor folders."""
    safe_parent = _validate_parent(workspace, parent)
    target = _validate_visible_path(
        workspace,
        safe_parent / _validate_name(name),
        must_exist=False,
    )
    if target.exists() or target.is_symlink():
        raise WorkspaceEntryError(f"An entry already exists at {target.name}.")
    try:
        target.mkdir()
    except OSError as error:
        raise WorkspaceEntryError(f"Cannot create folder {target}: {error}") from error
    return target


def rename_entry(workspace: Workspace, source: Path, name: str) -> Path:
    """Rename one existing file or folder in place."""
    safe_source = _validate_visible_path(workspace, source, must_exist=True)
    return move_entry(workspace, safe_source, safe_source.with_name(_validate_name(name)))


def move_entry(workspace: Workspace, source: Path, target: Path) -> Path:
    """Move one existing file or folder to an explicit workspace-relative path."""
    safe_source = _validate_visible_path(workspace, source, must_exist=True)
    source_is_directory = safe_source.is_dir()
    if source_is_directory:
        safe_target = _validate_visible_path(workspace, target, must_exist=False)
        try:
            safe_target.relative_to(safe_source)
        except ValueError:
            pass
        else:
            raise WorkspaceEntryError("A folder cannot be moved inside itself.")
    else:
        try:
            safe_target = workspace.validate_document_path(target, must_exist=False)
        except WorkspaceError as error:
            raise WorkspaceEntryError(str(error)) from error
        relative = safe_target.relative_to(workspace.root)
        if any(part in IGNORED_DIRECTORIES for part in relative.parts):
            raise WorkspaceEntryError("TermWriter cannot manage entries inside an ignored folder.")

    _validate_parent(workspace, safe_target.parent)
    if safe_target.exists() or safe_target.is_symlink():
        raise WorkspaceEntryError(f"An entry already exists at {safe_target}.")
    try:
        _rename_no_replace(safe_source, safe_target)
    except WorkspaceEntryError:
        raise
    except OSError as error:
        raise WorkspaceEntryError(f"Cannot move {safe_source}: {error}") from error
    return safe_target


def remove_entry(workspace: Workspace, source: Path) -> Path:
    """Permanently remove one file or folder tree after UI confirmation."""
    safe_source = _validate_visible_path(workspace, source, must_exist=True)
    try:
        if safe_source.is_dir():
            shutil.rmtree(safe_source)
        else:
            safe_source.unlink()
    except OSError as error:
        raise WorkspaceEntryError(f"Cannot remove {safe_source}: {error}") from error
    return safe_source
