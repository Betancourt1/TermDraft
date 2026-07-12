"""Tests for exact loading and guarded atomic persistence."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from termwriter.models.document import FileSnapshot
from termwriter.services import persistence
from termwriter.services.persistence import (
    ExternalModificationError,
    InvalidEncodingError,
    PersistenceError,
    UnsafeFileError,
    atomic_save,
    load_file,
    snapshot_file,
)


@pytest.mark.parametrize(
    "source",
    [
        b"",
        "Unicode: café 東京\n".encode(),
        b"first\nsecond",
        b"first\r\nsecond\r\n",
        b"no final newline",
        b"\xef\xbb\xbfBOM text\r\n",
    ],
)
def test_utf8_load_and_save_preserve_exact_content(tmp_path: Path, source: bytes) -> None:
    path = tmp_path / "exact.md"
    path.write_bytes(source)
    loaded = load_file(path)

    result = atomic_save(
        path,
        loaded.text,
        encoding=loaded.encoding,
        expected=loaded.snapshot,
    )

    assert path.read_bytes() == source
    assert result.snapshot.digest == loaded.snapshot.digest


def test_atomic_save_uses_same_directory_replacement(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("before", encoding="utf-8")
    loaded = load_file(path)
    real_replace = os.replace
    calls: list[tuple[Path, Path]] = []

    def tracking_replace(
        source: str | os.PathLike[str],
        destination: str | os.PathLike[str],
        *,
        src_dir_fd: int | None = None,
        dst_dir_fd: int | None = None,
    ) -> None:
        source_path = tmp_path / Path(source)
        destination_path = tmp_path / Path(destination)
        calls.append((source_path, destination_path))
        assert source_path.parent == destination_path.parent == tmp_path
        assert source_path.exists()
        real_replace(
            source,
            destination,
            src_dir_fd=src_dir_fd,
            dst_dir_fd=dst_dir_fd,
        )

    monkeypatch.setattr("termwriter.services.persistence.os.replace", tracking_replace)

    atomic_save(path, "after", encoding="utf-8", expected=loaded.snapshot)

    assert path.read_text(encoding="utf-8") == "after"
    assert len(calls) == 1


def test_failed_atomic_replace_does_not_corrupt_original(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_bytes(b"original")
    loaded = load_file(path)

    def broken_replace(
        source: object,
        destination: object,
        *,
        src_dir_fd: int | None = None,
        dst_dir_fd: int | None = None,
    ) -> None:
        del source, destination, src_dir_fd, dst_dir_fd
        raise OSError("injected replacement failure")

    monkeypatch.setattr("termwriter.services.persistence.os.replace", broken_replace)

    with pytest.raises(PersistenceError, match="atomically publish"):
        atomic_save(path, "new text", encoding="utf-8", expected=loaded.snapshot)

    assert path.read_bytes() == b"original"
    assert list(tmp_path.glob(".note.md.*.tmp")) == []


def test_failed_temporary_write_does_not_corrupt_original(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_bytes(b"original")
    loaded = load_file(path)

    def broken_write(descriptor: int, data: bytes, mode: int | None) -> None:
        del data, mode
        os.write(descriptor, b"partial")
        os.close(descriptor)
        raise OSError("injected write failure")

    monkeypatch.setattr(persistence, "_write_temporary", broken_write)

    with pytest.raises(PersistenceError, match="temporary file"):
        atomic_save(path, "new text", encoding="utf-8", expected=loaded.snapshot)

    assert path.read_bytes() == b"original"
    assert list(tmp_path.glob(".note.md.*.tmp")) == []


def test_atomic_save_preserves_permission_bits(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("before", encoding="utf-8")
    path.chmod(0o640)
    loaded = load_file(path)

    atomic_save(path, "after", encoding="utf-8", expected=loaded.snapshot)

    assert path.stat().st_mode & 0o777 == 0o640


def test_atomic_save_rejects_concurrent_permission_tightening(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("before", encoding="utf-8")
    path.chmod(0o644)
    loaded = load_file(path)
    real_write = persistence._write_temporary

    def write_then_tighten_mode(descriptor: int, data: bytes, mode: int | None) -> None:
        real_write(descriptor, data, mode)
        path.chmod(0o600)

    monkeypatch.setattr(persistence, "_write_temporary", write_then_tighten_mode)

    with pytest.raises(ExternalModificationError):
        atomic_save(path, "after", encoding="utf-8", expected=loaded.snapshot)

    assert path.stat().st_mode & 0o777 == 0o600
    assert path.read_text(encoding="utf-8") == "before"
    assert list(tmp_path.glob(".note.md.*.tmp")) == []


def test_atomic_save_preserves_special_permission_bits(tmp_path: Path) -> None:
    path = tmp_path / "script.md"
    path.write_text("before", encoding="utf-8")
    path.chmod(0o4755)
    if path.stat().st_mode & 0o7777 != 0o4755:
        pytest.skip("filesystem does not permit a user-owned setuid test file")
    loaded = load_file(path)

    atomic_save(path, "after", encoding="utf-8", expected=loaded.snapshot)

    assert path.stat().st_mode & 0o7777 == 0o4755


def test_atomic_save_rejects_changed_baseline(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("before", encoding="utf-8")
    loaded = load_file(path)
    path.write_text("external", encoding="utf-8")

    with pytest.raises(ExternalModificationError):
        atomic_save(path, "local", encoding="utf-8", expected=loaded.snapshot)

    assert path.read_text(encoding="utf-8") == "external"


def test_new_file_save_does_not_overwrite_a_racing_creator(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "copy.md"
    expected = snapshot_file(path)

    def racing_link(
        source: str | os.PathLike[str],
        destination: str | os.PathLike[str],
        *,
        src_dir_fd: int | None = None,
        dst_dir_fd: int | None = None,
        follow_symlinks: bool = True,
    ) -> None:
        del source, destination, src_dir_fd, dst_dir_fd, follow_symlinks
        path.write_text("racing version", encoding="utf-8")
        raise FileExistsError

    monkeypatch.setattr("termwriter.services.persistence.os.link", racing_link)

    with pytest.raises(ExternalModificationError):
        atomic_save(path, "local", encoding="utf-8", expected=expected)

    assert path.read_text(encoding="utf-8") == "racing version"


def test_new_file_save_requires_parent_bound_snapshot(tmp_path: Path) -> None:
    path = tmp_path / "copy.md"

    with pytest.raises(PersistenceError, match="parent-bound snapshot"):
        atomic_save(path, "local", encoding="utf-8", expected=FileSnapshot.missing())

    assert not path.exists()


def test_invalid_utf8_is_rejected(tmp_path: Path) -> None:
    path = tmp_path / "invalid.md"
    path.write_bytes(b"\xff\xfe")

    with pytest.raises(InvalidEncodingError):
        load_file(path)


def test_loading_missing_file_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        load_file(tmp_path / "missing.md")


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="FIFO creation is POSIX-only")
def test_loading_fifo_rejects_without_waiting_for_a_writer(tmp_path: Path) -> None:
    path = tmp_path / "pipe.md"
    os.mkfifo(path)

    with pytest.raises(UnsafeFileError, match="regular file"):
        load_file(path)


def test_atomic_save_stops_if_destination_directory_is_replaced(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    parent = tmp_path / "parent"
    moved_parent = tmp_path / "parent-moved"
    parent.mkdir()
    path = parent / "note.md"
    path.write_text("original", encoding="utf-8")
    loaded = load_file(path)
    real_write = persistence._write_temporary

    def write_then_replace_parent(descriptor: int, data: bytes, mode: int | None) -> None:
        real_write(descriptor, data, mode)
        parent.rename(moved_parent)
        parent.mkdir()

    monkeypatch.setattr(persistence, "_write_temporary", write_then_replace_parent)

    with pytest.raises(PersistenceError, match="Destination directory changed"):
        atomic_save(path, "local", encoding="utf-8", expected=loaded.snapshot)

    assert (moved_parent / "note.md").read_text(encoding="utf-8") == "original"
    assert not path.exists()
    assert list(moved_parent.glob(".note.md.*.tmp")) == []


def test_atomic_save_rejects_nested_ancestor_symlink_redirect(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    moved_workspace = tmp_path / "workspace-moved"
    outside = tmp_path / "outside"
    original_parent = workspace / "notes"
    outside_parent = outside / "notes"
    original_parent.mkdir(parents=True)
    outside_parent.mkdir(parents=True)
    path = original_parent / "note.md"
    outside_path = outside_parent / "note.md"
    path.write_text("base", encoding="utf-8")
    outside_path.write_text("base", encoding="utf-8")
    loaded = load_file(path)

    workspace.rename(moved_workspace)
    workspace.symlink_to(outside, target_is_directory=True)

    with pytest.raises(ExternalModificationError):
        atomic_save(path, "local", encoding="utf-8", expected=loaded.snapshot)

    assert outside_path.read_text(encoding="utf-8") == "base"
    assert (moved_workspace / "notes" / "note.md").read_text(encoding="utf-8") == "base"
