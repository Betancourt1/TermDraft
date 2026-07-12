"""Workspace validation and safe Markdown discovery."""

from __future__ import annotations

import os
import stat
import unicodedata
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

MARKDOWN_SUFFIXES = frozenset({".md", ".markdown"})
IGNORED_DIRECTORIES = frozenset({".git", ".venv", "node_modules", "__pycache__"})


def path_spelling_key(path: Path) -> str:
    """Normalize spelling aliases without treating arbitrary hardlinks as one path."""
    return unicodedata.normalize("NFC", path.as_posix()).casefold()


def paths_are_spelling_aliases(left: Path, right: Path) -> bool:
    """Return true for one directory entry reached through equivalent spellings."""
    if left == right:
        return True
    if path_spelling_key(left) != path_spelling_key(right):
        return False
    try:
        if not left.parent.samefile(right.parent):
            return False
        with os.scandir(left.parent) as entries:
            names = {entry.name for entry in entries}
        if left.name != right.name and left.name in names and right.name in names:
            return False
        return left.samefile(right)
    except OSError:
        return False


class WorkspaceError(Exception):
    """Base class for user-facing workspace errors."""


class WorkspaceNotFoundError(WorkspaceError):
    """Raised when the requested workspace target does not exist."""


class WorkspaceAccessError(WorkspaceError):
    """Raised when the workspace root cannot be read."""


class UnsafePathError(WorkspaceError):
    """Raised when a path escapes the workspace or traverses a symlink."""


class UnsupportedFileError(WorkspaceError):
    """Raised when a path is not a supported Markdown file."""


@dataclass(frozen=True, slots=True)
class ScanResult:
    """Markdown files plus non-fatal directory errors found during scanning."""

    files: tuple[Path, ...]
    warnings: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class Workspace:
    """A canonical workspace root and optional file requested by the CLI."""

    root: Path
    initial_file: Path | None = None

    @classmethod
    def from_target(cls, target: Path) -> Workspace:
        """Build a workspace from an existing directory or Markdown file."""
        requested = target.expanduser()
        if not requested.is_absolute():
            requested = Path.cwd() / requested
        requested = Path(os.path.abspath(requested))

        try:
            requested_stat = requested.stat()
        except FileNotFoundError:
            raise WorkspaceNotFoundError(f"Path does not exist: {requested}") from None
        except OSError as error:
            raise WorkspaceAccessError(f"Cannot inspect path {requested}: {error}") from error

        if requested.is_symlink() and not stat.S_ISDIR(requested_stat.st_mode):
            raise UnsafePathError(f"Symbolic-link files are not supported: {requested}")

        if stat.S_ISDIR(requested_stat.st_mode):
            try:
                root = requested.resolve(strict=True)
            except OSError as error:
                raise WorkspaceAccessError(
                    f"Cannot resolve workspace {requested}: {error}"
                ) from error
            cls._verify_root_access(root)
            return cls(root=root)

        if not stat.S_ISREG(requested_stat.st_mode):
            raise UnsupportedFileError(f"Not a regular file or directory: {requested}")
        if requested.suffix.casefold() not in MARKDOWN_SUFFIXES:
            raise UnsupportedFileError(f"Not a Markdown file: {requested}")

        try:
            root = requested.parent.resolve(strict=True)
        except OSError as error:
            raise WorkspaceAccessError(
                f"Cannot resolve workspace {requested.parent}: {error}"
            ) from error
        cls._verify_root_access(root)
        workspace = cls(root=root)
        initial_file = workspace.validate_document_path(requested)
        return cls(root=root, initial_file=initial_file)

    @staticmethod
    def _verify_root_access(root: Path) -> None:
        try:
            with os.scandir(root):
                pass
        except OSError as error:
            raise WorkspaceAccessError(f"Cannot read workspace {root}: {error}") from error

    def contains(self, path: Path) -> bool:
        """Return whether a resolved path remains under the workspace root."""
        try:
            path.resolve(strict=False).relative_to(self.root)
        except (OSError, ValueError):
            return False
        return True

    def validate_document_path(self, path: Path, *, must_exist: bool = True) -> Path:
        """Validate a Markdown file path without allowing symlink traversal."""
        candidate = path.expanduser()
        if not candidate.is_absolute():
            candidate = self.root / candidate
        candidate = Path(os.path.abspath(candidate))

        if candidate.is_symlink():
            raise UnsafePathError(f"Symbolic links are not supported: {candidate}")
        try:
            candidate = candidate.parent.resolve(strict=True) / candidate.name
        except OSError as error:
            raise WorkspaceAccessError(
                f"Cannot access parent directory: {candidate.parent}"
            ) from error

        try:
            relative = candidate.relative_to(self.root)
        except ValueError as error:
            raise UnsafePathError(f"Path is outside the workspace: {candidate}") from error

        if candidate.suffix.casefold() not in MARKDOWN_SUFFIXES:
            raise UnsupportedFileError(f"Not a Markdown file: {candidate}")

        current = self.root
        for part in relative.parts:
            current /= part
            try:
                if current.is_symlink():
                    raise UnsafePathError(f"Symbolic links are not supported: {current}")
            except OSError as error:
                raise WorkspaceAccessError(f"Cannot inspect path {current}: {error}") from error

        try:
            resolved_parent = candidate.parent.resolve(strict=True)
        except OSError as error:
            raise WorkspaceAccessError(
                f"Cannot access parent directory: {candidate.parent}"
            ) from error
        try:
            resolved_parent.relative_to(self.root)
        except ValueError as error:
            raise UnsafePathError(f"Path is outside the workspace: {candidate}") from error

        if must_exist:
            try:
                file_stat = candidate.lstat()
            except FileNotFoundError as error:
                raise WorkspaceNotFoundError(f"File no longer exists: {candidate}") from error
            except OSError as error:
                raise WorkspaceAccessError(f"Cannot inspect file {candidate}: {error}") from error
            if not stat.S_ISREG(file_stat.st_mode):
                raise UnsupportedFileError(f"Not a regular Markdown file: {candidate}")
        return candidate

    def scan(
        self,
        *,
        should_cancel: Callable[[], bool] | None = None,
    ) -> ScanResult:
        """Find Markdown files while treating unreadable directories as warnings."""
        pending = [self.root]
        files: list[Path] = []
        warnings: list[str] = []

        while pending:
            if should_cancel is not None and should_cancel():
                break
            directory = pending.pop()
            try:
                with os.scandir(directory) as iterator:
                    entries = sorted(iterator, key=lambda entry: entry.name.casefold())
            except OSError as error:
                warnings.append(f"Cannot read {directory}: {error}")
                continue

            for entry in entries:
                if should_cancel is not None and should_cancel():
                    break
                path = Path(entry.path)
                try:
                    if entry.is_symlink():
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        if entry.name not in IGNORED_DIRECTORIES:
                            pending.append(path)
                    elif (
                        entry.is_file(follow_symlinks=False)
                        and path.suffix.casefold() in MARKDOWN_SUFFIXES
                    ):
                        files.append(path)
                except OSError as error:
                    warnings.append(f"Cannot inspect {path}: {error}")

        files.sort(key=lambda path: path.relative_to(self.root).as_posix().casefold())
        return ScanResult(tuple(files), tuple(warnings))
