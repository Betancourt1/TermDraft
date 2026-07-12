"""Exact UTF-8 loading and guarded same-directory atomic persistence."""

from __future__ import annotations

import codecs
import hashlib
import os
import secrets
import stat
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


class _TemporaryEntry:
    """Remove a temporary entry through its open parent directory when safe."""

    def __init__(
        self,
        directory_descriptor: int,
        name: str,
        identity: tuple[int, int],
    ) -> None:
        self.directory_descriptor = directory_descriptor
        self.name = name
        self.identity = identity
        self.consumed = False

    def __enter__(self) -> _TemporaryEntry:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        del exc_type, exc_value, traceback
        if self.consumed:
            return
        try:
            file_stat = os.stat(
                self.name,
                dir_fd=self.directory_descriptor,
                follow_symlinks=False,
            )
        except OSError:
            return
        if (file_stat.st_dev, file_stat.st_ino) != self.identity:
            return
        try:
            os.unlink(self.name, dir_fd=self.directory_descriptor)
        except FileNotFoundError:
            pass


def _open_directory(directory: Path) -> tuple[int, os.stat_result]:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(directory, flags)
    except OSError as error:
        raise PersistenceError(f"Cannot open destination directory {directory}: {error}") from error
    directory_stat = os.fstat(descriptor)
    if not stat.S_ISDIR(directory_stat.st_mode):
        os.close(descriptor)
        raise UnsafeFileError(f"Not a regular directory: {directory}")
    return descriptor, directory_stat


def _path_directory_identity(directory: Path) -> tuple[int, int]:
    try:
        directory_stat = directory.lstat()
    except OSError as error:
        raise PersistenceError(
            f"Cannot inspect destination directory {directory}: {error}"
        ) from error
    if stat.S_ISLNK(directory_stat.st_mode) or not stat.S_ISDIR(directory_stat.st_mode):
        raise UnsafeFileError(f"Destination parent is not a regular directory: {directory}")
    return directory_stat.st_dev, directory_stat.st_ino


def _snapshot_from_data(
    data: bytes,
    file_stat: os.stat_result,
    parent_stat: os.stat_result,
) -> FileSnapshot:
    return FileSnapshot(
        exists=True,
        digest=hashlib.sha256(data).hexdigest(),
        size=len(data),
        mtime_ns=file_stat.st_mtime_ns,
        mode=stat.S_IMODE(file_stat.st_mode),
        device=file_stat.st_dev,
        inode=file_stat.st_ino,
        parent_device=parent_stat.st_dev,
        parent_inode=parent_stat.st_ino,
    )


def _stable_read_at(
    directory_descriptor: int,
    name: str,
    parent_stat: os.stat_result,
    *,
    attempts: int = 2,
) -> tuple[bytes, FileSnapshot]:
    """Read one regular entry relative to an already-open directory."""
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    for _ in range(attempts):
        try:
            descriptor = os.open(name, flags, dir_fd=directory_descriptor)
        except FileNotFoundError:
            raise
        except OSError as error:
            try:
                entry_stat = os.stat(
                    name,
                    dir_fd=directory_descriptor,
                    follow_symlinks=False,
                )
            except OSError:
                raise error from None
            if stat.S_ISLNK(entry_stat.st_mode):
                raise UnsafeFileError(f"Symbolic links are not supported: {name}") from error
            raise

        try:
            before = os.fstat(descriptor)
            if not stat.S_ISREG(before.st_mode):
                raise UnsafeFileError(f"Not a regular file: {name}")
            with os.fdopen(descriptor, "rb", closefd=False) as source:
                data = source.read()
                after = os.fstat(descriptor)
        finally:
            os.close(descriptor)

        identity_before = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
        identity_after = (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
        if identity_before == identity_after and len(data) == after.st_size:
            return data, _snapshot_from_data(data, after, parent_stat)
    raise FileChangedDuringReadError(f"File changed while it was being read: {name}")


def _stable_read(path: Path, *, attempts: int = 2) -> tuple[bytes, FileSnapshot]:
    directory_descriptor, parent_stat = _open_directory(path.parent)
    try:
        return _stable_read_at(
            directory_descriptor,
            path.name,
            parent_stat,
            attempts=attempts,
        )
    finally:
        os.close(directory_descriptor)


def _snapshot_at(
    directory_descriptor: int,
    name: str,
    parent_stat: os.stat_result,
) -> FileSnapshot:
    try:
        _, snapshot = _stable_read_at(directory_descriptor, name, parent_stat)
    except FileNotFoundError:
        return FileSnapshot.missing(
            parent_device=parent_stat.st_dev,
            parent_inode=parent_stat.st_ino,
        )
    return snapshot


def snapshot_file(path: Path) -> FileSnapshot:
    """Hash the current file contents, or return a missing snapshot."""
    try:
        directory_descriptor, parent_stat = _open_directory(path.parent)
    except PersistenceError as error:
        if isinstance(error.__cause__, FileNotFoundError):
            return FileSnapshot.missing()
        raise
    try:
        return _snapshot_at(directory_descriptor, path.name, parent_stat)
    finally:
        os.close(directory_descriptor)


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
    """Write and sync a private temporary file, leaving its descriptor open."""
    del mode
    with os.fdopen(descriptor, "wb", closefd=False) as destination:
        destination.write(data)
        destination.flush()
        os.fsync(destination.fileno())


def _create_temporary(
    directory_descriptor: int,
    destination_name: str,
) -> tuple[int, str, tuple[int, int]]:
    flags = (
        os.O_WRONLY
        | os.O_CREAT
        | os.O_EXCL
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_CLOEXEC", 0)
    )
    safe_stem = destination_name[:40]
    for _ in range(100):
        name = f".{safe_stem}.{secrets.token_hex(8)}.tmp"
        try:
            descriptor = os.open(name, flags, 0o600, dir_fd=directory_descriptor)
        except FileExistsError:
            continue
        file_stat = os.fstat(descriptor)
        return descriptor, name, (file_stat.st_dev, file_stat.st_ino)
    raise PersistenceError(f"Cannot allocate a temporary file for {destination_name}")


def _sync_directory(directory_descriptor: int) -> bool:
    try:
        os.fsync(directory_descriptor)
    except OSError:
        return False
    return True


def _matches_expected(current: FileSnapshot, expected: FileSnapshot) -> bool:
    if not current.has_same_content(expected):
        return False
    if expected.exists and not current.has_same_origin(expected):
        return False
    if expected.parent_device is not None:
        return (
            current.parent_device,
            current.parent_inode,
        ) == (
            expected.parent_device,
            expected.parent_inode,
        )
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
    hard-link publication step so a racing creator cannot be overwritten by Save As. All temporary
    and publication operations stay attached to an open parent-directory descriptor.
    """
    data = _encode_text(text, encoding)
    directory_descriptor, parent_stat = _open_directory(path.parent)
    directory_identity = (parent_stat.st_dev, parent_stat.st_ino)
    temporary_descriptor = -1

    try:
        current = _snapshot_at(directory_descriptor, path.name, parent_stat)
        if not _matches_expected(current, expected):
            raise ExternalModificationError(path, current)

        try:
            temporary_descriptor, temporary_name, temporary_identity = _create_temporary(
                directory_descriptor,
                path.name,
            )
        except OSError as error:
            raise PersistenceError(
                f"Cannot create a temporary file beside {path}: {error}"
            ) from error

        with _TemporaryEntry(
            directory_descriptor,
            temporary_name,
            temporary_identity,
        ) as guard:
            try:
                _write_temporary(temporary_descriptor, data, current.mode)
            except OSError as error:
                raise PersistenceError(
                    f"Could not write the temporary file for {path}: {error}"
                ) from error

            if _path_directory_identity(path.parent) != directory_identity:
                raise PersistenceError(f"Destination directory changed while saving {path}")

            latest = _snapshot_at(directory_descriptor, path.name, parent_stat)
            if not _matches_expected(latest, expected):
                raise ExternalModificationError(path, latest)

            try:
                if current.mode is not None:
                    os.fchmod(temporary_descriptor, current.mode)
                os.fsync(temporary_descriptor)
            except OSError as error:
                raise PersistenceError(
                    f"Could not preserve permissions for {path}: {error}"
                ) from error
            finally:
                try:
                    os.close(temporary_descriptor)
                except OSError:
                    pass
                temporary_descriptor = -1

            try:
                if expected.exists:
                    os.replace(
                        temporary_name,
                        path.name,
                        src_dir_fd=directory_descriptor,
                        dst_dir_fd=directory_descriptor,
                    )
                    guard.consumed = True
                else:
                    os.link(
                        temporary_name,
                        path.name,
                        src_dir_fd=directory_descriptor,
                        dst_dir_fd=directory_descriptor,
                        follow_symlinks=False,
                    )
                    os.unlink(temporary_name, dir_fd=directory_descriptor)
                    guard.consumed = True
            except FileExistsError as error:
                raise ExternalModificationError(
                    path,
                    _snapshot_at(directory_descriptor, path.name, parent_stat),
                ) from error
            except OSError as error:
                raise PersistenceError(f"Could not atomically publish {path}: {error}") from error

        directory_synced = _sync_directory(directory_descriptor)
        saved = _snapshot_at(directory_descriptor, path.name, parent_stat)
        expected_digest = hashlib.sha256(data).hexdigest()
        if saved.digest != expected_digest:
            raise SaveVerificationError(f"Saved bytes could not be verified for {path}")
        if _path_directory_identity(path.parent) != directory_identity:
            raise SaveVerificationError(
                f"Destination directory moved after publishing {path}; saved state is uncertain"
            )

        warning = None
        if not directory_synced:
            warning = "Saved, but the directory could not be synced for crash durability"
        return SaveResult(snapshot=saved, warning=warning)
    finally:
        if temporary_descriptor >= 0:
            try:
                os.close(temporary_descriptor)
            except OSError:
                pass
        os.close(directory_descriptor)
