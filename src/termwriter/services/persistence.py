"""Exact UTF-8 loading and guarded same-directory atomic persistence."""

from __future__ import annotations

import codecs
import hashlib
import os
import stat
import tempfile
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType

from termwriter.models.document import FileSnapshot


class PersistenceError(Exception):
    """Base class for persistence failures suitable for UI reporting."""


class InvalidEncodingError(PersistenceError):
    """Raised when a source file is not valid UTF-8."""


class UnsafeFileError(PersistenceError):
    """Raised when a file is a symlink or not a regular file."""


class FileChangedDuringReadError(PersistenceError):
    """Raised when a file cannot be read from a stable disk state."""


class ExternalModificationError(PersistenceError):
    """Raised when the disk baseline differs before an atomic replacement."""

    def __init__(self, path: Path, current: FileSnapshot) -> None:
        super().__init__(f"{path} changed on disk; the local version was not written")
        self.path = path
        self.current = current


class SaveVerificationError(PersistenceError):
    """Raised when the newly visible file does not match the bytes just written."""


@dataclass(frozen=True, slots=True)
class LoadedFile:
    """Decoded source plus the exact disk state it came from."""

    text: str
    encoding: str
    snapshot: FileSnapshot


@dataclass(frozen=True, slots=True)
class SaveResult:
    """The verified saved state and an optional durability warning."""

    snapshot: FileSnapshot
    warning: str | None = None


class _TemporaryPath:
    """Ensure a same-directory temporary file is removed unless consumed."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self.consumed = False

    def __enter__(self) -> _TemporaryPath:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        del exc_type, exc_value, traceback
        if not self.consumed:
            try:
                self.path.unlink()
            except FileNotFoundError:
                pass


def _snapshot_from_data(data: bytes, file_stat: os.stat_result) -> FileSnapshot:
    return FileSnapshot(
        exists=True,
        digest=hashlib.sha256(data).hexdigest(),
        size=len(data),
        mtime_ns=file_stat.st_mtime_ns,
        mode=stat.S_IMODE(file_stat.st_mode),
        device=file_stat.st_dev,
        inode=file_stat.st_ino,
    )


def _stable_read(path: Path, *, attempts: int = 2) -> tuple[bytes, FileSnapshot]:
    """Read a regular file without following its final symlink component."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    for _ in range(attempts):
        try:
            descriptor = os.open(path, flags)
        except FileNotFoundError:
            raise
        except OSError as error:
            if path.is_symlink():
                raise UnsafeFileError(f"Symbolic links are not supported: {path}") from error
            raise

        try:
            before = os.fstat(descriptor)
            if not stat.S_ISREG(before.st_mode):
                raise UnsafeFileError(f"Not a regular file: {path}")
            with os.fdopen(descriptor, "rb", closefd=False) as source:
                data = source.read()
                after = os.fstat(descriptor)
        finally:
            os.close(descriptor)

        identity_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
        if identity_before == identity_after and len(data) == after.st_size:
            return data, _snapshot_from_data(data, after)
    raise FileChangedDuringReadError(f"File changed while it was being read: {path}")


def snapshot_file(path: Path) -> FileSnapshot:
    """Hash the current file contents, or return a missing snapshot."""
    try:
        _, snapshot = _stable_read(path)
    except FileNotFoundError:
        return FileSnapshot.missing()
    return snapshot


def load_file(path: Path) -> LoadedFile:
    """Load exact UTF-8 source without universal-newline conversion."""
    data, snapshot = _stable_read(path)
    encoding = "utf-8-sig" if data.startswith(codecs.BOM_UTF8) else "utf-8"
    try:
        text = data.decode(encoding)
    except UnicodeDecodeError as error:
        raise InvalidEncodingError(f"File is not valid UTF-8: {path}") from error
    return LoadedFile(text=text, encoding=encoding, snapshot=snapshot)


def _encode_text(text: str, encoding: str) -> bytes:
    if encoding not in {"utf-8", "utf-8-sig"}:
        raise InvalidEncodingError(f"Unsupported document encoding: {encoding}")
    return text.encode(encoding)


def _write_temporary(descriptor: int, data: bytes, mode: int | None) -> None:
    """Write, flush, and sync a temporary file before it becomes visible."""
    with os.fdopen(descriptor, "wb") as destination:
        if mode is not None:
            os.fchmod(destination.fileno(), mode)
        destination.write(data)
        destination.flush()
        os.fsync(destination.fileno())


def _sync_directory(directory: Path) -> bool:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    try:
        descriptor = os.open(directory, flags)
    except OSError:
        return False
    try:
        os.fsync(descriptor)
    except OSError:
        return False
    finally:
        os.close(descriptor)
    return True


def atomic_save(
    path: Path,
    text: str,
    *,
    encoding: str,
    expected: FileSnapshot,
) -> SaveResult:
    """Persist source without truncation and reject a changed disk baseline.

    Existing files use ``os.replace`` after a fully synced temporary write. New files use a
    hard-link publication step so a racing creator cannot be overwritten by Save As.
    """
    data = _encode_text(text, encoding)
    current = snapshot_file(path)
    if not current.has_same_content(expected):
        raise ExternalModificationError(path, current)

    try:
        descriptor, temporary_name = tempfile.mkstemp(
            prefix=f".{path.name}.",
            suffix=".tmp",
            dir=path.parent,
        )
    except OSError as error:
        raise PersistenceError(f"Cannot create a temporary file beside {path}: {error}") from error

    temporary = Path(temporary_name)
    with _TemporaryPath(temporary) as guard:
        try:
            _write_temporary(descriptor, data, current.mode)
        except OSError as error:
            raise PersistenceError(
                f"Could not write the temporary file for {path}: {error}"
            ) from error

        latest = snapshot_file(path)
        if not latest.has_same_content(expected):
            raise ExternalModificationError(path, latest)

        try:
            if expected.exists:
                os.replace(temporary, path)
                guard.consumed = True
            else:
                os.link(temporary, path, follow_symlinks=False)
                temporary.unlink()
                guard.consumed = True
        except FileExistsError as error:
            raise ExternalModificationError(path, snapshot_file(path)) from error
        except OSError as error:
            raise PersistenceError(f"Could not atomically publish {path}: {error}") from error

    directory_synced = _sync_directory(path.parent)
    saved = snapshot_file(path)
    expected_digest = hashlib.sha256(data).hexdigest()
    if saved.digest != expected_digest:
        raise SaveVerificationError(f"Saved bytes could not be verified for {path}")

    warning = None
    if not directory_synced:
        warning = "Saved, but the directory could not be synced for crash durability"
    return SaveResult(snapshot=saved, warning=warning)
