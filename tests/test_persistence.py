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
    atomic_save,
    load_file,
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
    ) -> None:
        source_path = Path(source)
        destination_path = Path(destination)
        calls.append((source_path, destination_path))
        assert source_path.parent == destination_path.parent == tmp_path
        assert source_path.exists()
        real_replace(source, destination)

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

    def broken_replace(source: object, destination: object) -> None:
        del source, destination
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

    def racing_link(
        source: str | os.PathLike[str],
        destination: str | os.PathLike[str],
        *,
        follow_symlinks: bool = True,
    ) -> None:
        del source, follow_symlinks
        Path(destination).write_text("racing version", encoding="utf-8")
        raise FileExistsError

    monkeypatch.setattr("termwriter.services.persistence.os.link", racing_link)

    with pytest.raises(ExternalModificationError):
        atomic_save(path, "local", encoding="utf-8", expected=FileSnapshot.missing())

    assert path.read_text(encoding="utf-8") == "racing version"


def test_invalid_utf8_is_rejected(tmp_path: Path) -> None:
    path = tmp_path / "invalid.md"
    path.write_bytes(b"\xff\xfe")

    with pytest.raises(InvalidEncodingError):
        load_file(path)


def test_loading_missing_file_raises(tmp_path: Path) -> None:
    with pytest.raises(FileNotFoundError):
        load_file(tmp_path / "missing.md")
