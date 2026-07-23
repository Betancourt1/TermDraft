"""Tests for content-free workspace session storage."""

from __future__ import annotations

import json
import os
import stat
from pathlib import Path

import pytest

from termdraft.services.session import (
    MAX_SESSION_BYTES,
    MAX_SESSION_DOCUMENTS,
    DocumentViewState,
    SessionError,
    SessionState,
    SessionStore,
    default_session_root,
)


def _state(workspace: Path) -> SessionState:
    first = workspace / "notes" / "café.md"
    second = workspace / "todo.markdown"
    return SessionState(
        workspace_root=workspace,
        active_path=second,
        documents=(
            DocumentViewState(
                first,
                line=3,
                column=8,
                scroll_x=1.5,
                scroll_y=12.25,
                preview_scroll_x=2.0,
                preview_scroll_y=18.0,
            ),
            DocumentViewState(
                second,
                line=9,
                column=2,
                scroll_y=40.0,
                preview_scroll_y=52.0,
            ),
        ),
        open_paths=(first, second),
    )


def test_session_round_trip_preserves_multiple_document_views(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    expected = _state(workspace)

    store.save(expected)
    result = store.load(workspace)

    assert result.state == expected
    assert result.warning is None
    assert result.state is not None
    assert result.state.view_for(workspace / "todo.markdown") == expected.documents[1]
    assert store.path_for(workspace).parent == tmp_path / "state"
    assert len(store.path_for(workspace).stem) == 64
    assert stat.S_IMODE(store.state_root.stat().st_mode) == 0o700
    assert stat.S_IMODE(store.path_for(workspace).stat().st_mode) == 0o600


def test_serialized_session_contains_paths_and_positions_but_no_markdown_source(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(_state(workspace))

    payload = json.loads(store.path_for(workspace).read_text(encoding="utf-8"))

    assert payload == {
        "version": 3,
        "workspace_root": str(workspace),
        "active_path": "todo.markdown",
        "open_paths": ["notes/café.md", "todo.markdown"],
        "documents": [
            {
                "path": "notes/café.md",
                "line": 3,
                "column": 8,
                "scroll_x": 1.5,
                "scroll_y": 12.25,
                "preview_scroll_x": 2.0,
                "preview_scroll_y": 18.0,
            },
            {
                "path": "todo.markdown",
                "line": 9,
                "column": 2,
                "scroll_x": 0.0,
                "scroll_y": 40.0,
                "preview_scroll_x": 0.0,
                "preview_scroll_y": 52.0,
            },
        ],
    }
    assert "text" not in payload
    assert "content" not in payload


def test_version_one_session_migrates_only_its_active_document(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(_state(workspace))
    state_path = store.path_for(workspace)
    payload = json.loads(state_path.read_text(encoding="utf-8"))
    payload["version"] = 1
    del payload["open_paths"]
    for document in payload["documents"]:
        del document["preview_scroll_x"]
        del document["preview_scroll_y"]
    state_path.write_text(json.dumps(payload), encoding="utf-8")

    state = store.load(workspace).state

    assert state is not None
    assert state.open_paths == (workspace / "todo.markdown",)


def test_open_tab_order_is_independent_from_recent_document_order(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    first = workspace / "first.md"
    second = workspace / "second.md"
    state = SessionState(
        workspace,
        second,
        (DocumentViewState(second), DocumentViewState(first)),
        (first, second),
    )
    store = SessionStore(tmp_path / "state")

    store.save(state)

    assert store.load(workspace).state == state


def test_missing_session_is_silent_and_does_not_create_state_directory(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "missing-state")

    result = store.load(workspace)

    assert result.state is None
    assert result.warning is None
    assert not store.state_root.exists()


def test_corrupt_session_is_preserved_and_ignored(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    state_path = store.path_for(workspace)
    state_path.parent.mkdir()
    corrupt = b"not UTF-8 JSON: \xff\x00"
    state_path.write_bytes(corrupt)

    result = store.load(workspace)

    assert result.state is None
    assert "Ignoring invalid session state" in (result.warning or "")
    assert state_path.read_bytes() == corrupt


def test_oversized_session_is_preserved_and_ignored(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    state_path = store.path_for(workspace)
    state_path.parent.mkdir()
    oversized = b"{" + b" " * MAX_SESSION_BYTES + b"}"
    state_path.write_bytes(oversized)

    result = store.load(workspace)

    assert result.state is None
    assert f"exceeds {MAX_SESSION_BYTES} bytes" in (result.warning or "")
    assert state_path.read_bytes() == oversized


def test_session_document_limit_is_enforced_on_save_and_load(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    views = tuple(
        DocumentViewState(workspace / f"note-{index}.md") for index in range(MAX_SESSION_DOCUMENTS)
    )
    store.save(SessionState(workspace, views[0].path, views))
    assert store.load(workspace).state is not None

    too_many = (*views, DocumentViewState(workspace / "overflow.md"))
    with pytest.raises(SessionError, match=f"more than {MAX_SESSION_DOCUMENTS}"):
        store.save(SessionState(workspace, too_many[0].path, too_many))

    payload = json.loads(store.path_for(workspace).read_text(encoding="utf-8"))
    payload["documents"].append(
        {
            "path": "overflow.md",
            "line": 0,
            "column": 0,
            "scroll_x": 0.0,
            "scroll_y": 0.0,
            "preview_scroll_x": 0.0,
            "preview_scroll_y": 0.0,
        }
    )
    store.path_for(workspace).write_text(json.dumps(payload), encoding="utf-8")
    result = store.load(workspace)
    assert result.state is None
    assert f"more than {MAX_SESSION_DOCUMENTS}" in (result.warning or "")


@pytest.mark.parametrize(
    "change",
    [
        {"version": True},
        {"version": 99},
        {"extra": "field"},
        {"documents": {}},
        {"active_path": 7},
    ],
)
def test_invalid_top_level_schema_is_ignored(
    tmp_path: Path,
    change: dict[str, object],
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(_state(workspace))
    payload = json.loads(store.path_for(workspace).read_text(encoding="utf-8"))
    payload.update(change)
    store.path_for(workspace).write_text(json.dumps(payload), encoding="utf-8")

    result = store.load(workspace)

    assert result.state is None
    assert result.warning is not None


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("line", True),
        ("line", -1),
        ("column", 1.5),
        ("scroll_x", -0.1),
        ("scroll_y", float("nan")),
        ("scroll_y", "10"),
    ],
)
def test_invalid_view_coordinates_are_ignored(
    tmp_path: Path,
    field: str,
    value: object,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(_state(workspace))
    payload = json.loads(store.path_for(workspace).read_text(encoding="utf-8"))
    payload["documents"][0][field] = value
    store.path_for(workspace).write_text(json.dumps(payload), encoding="utf-8")

    assert store.load(workspace).state is None


def test_huge_scroll_integer_is_ignored_without_crashing(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(_state(workspace))
    state_path = store.path_for(workspace)
    payload = json.loads(state_path.read_text(encoding="utf-8"))
    payload["documents"][0]["scroll_y"] = 10**400
    state_path.write_text(json.dumps(payload), encoding="utf-8")

    result = store.load(workspace)

    assert result.state is None
    assert "finite non-negative number" in (result.warning or "")


def test_integer_over_json_digit_limit_is_ignored_without_crashing(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    state_path = store.path_for(workspace)
    state_path.parent.mkdir()
    state_path.write_text('{"version":' + "1" * 5000 + "}", encoding="utf-8")

    result = store.load(workspace)

    assert result.state is None
    assert "Ignoring invalid session state" in (result.warning or "")


def test_deeply_nested_json_is_ignored_without_crashing(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    state_path = store.path_for(workspace)
    state_path.parent.mkdir()
    nesting = 20_000
    state_path.write_text("[" * nesting + "0" + "]" * nesting, encoding="utf-8")

    result = store.load(workspace)

    assert result.state is None
    assert "Ignoring invalid session state" in (result.warning or "")


@pytest.mark.parametrize("relative_path", ["../escape.md", "/outside.md", "note.txt"])
def test_unsafe_or_unsupported_paths_are_ignored(
    tmp_path: Path,
    relative_path: str,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(_state(workspace))
    payload = json.loads(store.path_for(workspace).read_text(encoding="utf-8"))
    payload["documents"][0]["path"] = relative_path
    store.path_for(workspace).write_text(json.dumps(payload), encoding="utf-8")

    result = store.load(workspace)

    assert result.state is None
    assert result.warning is not None


def test_resolved_path_escape_is_rejected_on_save(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    outside = tmp_path / "outside"
    workspace.mkdir()
    outside.mkdir()
    (workspace / "linked").symlink_to(outside, target_is_directory=True)
    state = SessionState(
        workspace,
        workspace / "linked" / "note.md",
        (DocumentViewState(workspace / "linked" / "note.md"),),
    )

    with pytest.raises(SessionError, match="outside its workspace"):
        SessionStore(tmp_path / "state").save(state)


@pytest.mark.parametrize(
    "document",
    [Path("../outside.md"), Path("inside.rtf")],
)
def test_save_rejects_lexical_escape_and_unsupported_suffix(
    tmp_path: Path,
    document: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    path = workspace / document
    state = SessionState(workspace, path, (DocumentViewState(path),))

    with pytest.raises(
        SessionError,
        match=r"absolute and normalized|outside its workspace|not an editable text file",
    ):
        SessionStore(tmp_path / "state").save(state)


def test_active_document_must_have_a_stored_view(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    state = SessionState(
        workspace,
        workspace / "active.md",
        (DocumentViewState(workspace / "other.md"),),
    )

    with pytest.raises(SessionError, match="active document must have a stored view"):
        SessionStore(tmp_path / "state").save(state)


def test_open_documents_must_be_unique_and_have_stored_views(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    first = workspace / "first.md"
    second = workspace / "second.md"

    with pytest.raises(SessionError, match="duplicate open document path"):
        SessionStore(tmp_path / "state").save(
            SessionState(
                workspace,
                first,
                (DocumentViewState(first),),
                (first, first),
            )
        )

    with pytest.raises(SessionError, match="every open document"):
        SessionStore(tmp_path / "state").save(
            SessionState(
                workspace,
                first,
                (DocumentViewState(first),),
                (first, second),
            )
        )


def test_active_document_must_be_in_open_tab_set(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    first = workspace / "first.md"
    second = workspace / "second.md"
    state = SessionState(
        workspace,
        first,
        (DocumentViewState(first), DocumentViewState(second)),
        (second,),
    )

    with pytest.raises(SessionError, match="active document must be open"):
        SessionStore(tmp_path / "state").save(state)


def test_duplicate_document_paths_are_rejected(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    document = workspace / "note.md"
    state = SessionState(
        workspace,
        document,
        (DocumentViewState(document), DocumentViewState(document, line=2)),
    )

    with pytest.raises(SessionError, match="duplicate document path"):
        SessionStore(tmp_path / "state").save(state)


def test_state_for_another_workspace_is_ignored(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    store = SessionStore(tmp_path / "state")
    store.save(SessionState(first, first / "note.md", (DocumentViewState(first / "note.md"),)))
    store.state_root.mkdir(exist_ok=True)
    store.path_for(second).write_bytes(store.path_for(first).read_bytes())

    result = store.load(second)

    assert result.state is None
    assert "different workspace" in (result.warning or "")


def test_successful_save_replaces_from_same_directory_and_syncs(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    real_replace = os.replace
    real_fsync = os.fsync
    replacements: list[tuple[Path, Path]] = []
    syncs: list[int] = []

    def tracking_replace(
        source: str | os.PathLike[str], destination: str | os.PathLike[str]
    ) -> None:
        source_path = Path(source)
        destination_path = Path(destination)
        replacements.append((source_path, destination_path))
        assert source_path.parent == destination_path.parent == store.state_root
        real_replace(source, destination)

    def tracking_fsync(descriptor: int) -> None:
        syncs.append(descriptor)
        real_fsync(descriptor)

    monkeypatch.setattr("termdraft.services.session.os.replace", tracking_replace)
    monkeypatch.setattr("termdraft.services.session.os.fsync", tracking_fsync)

    store.save(_state(workspace))

    assert len(replacements) == 1
    assert len(syncs) == 2
    assert list(store.state_root.glob("*.tmp")) == []


def test_failed_replace_preserves_previous_session_and_cleans_temporary(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    store = SessionStore(tmp_path / "state")
    original = _state(workspace)
    store.save(original)
    updated = SessionState(
        workspace,
        original.active_path,
        (original.documents[0], DocumentViewState(workspace / "todo.markdown", line=99)),
    )

    def broken_replace(source: object, destination: object) -> None:
        del source, destination
        raise OSError("injected replacement failure")

    monkeypatch.setattr("termdraft.services.session.os.replace", broken_replace)

    with pytest.raises(SessionError, match="Cannot save session state"):
        store.save(updated)

    assert store.load(workspace).state == original
    assert list(store.state_root.glob("*.tmp")) == []


def test_default_session_root_honors_xdg_state_home(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))

    assert default_session_root() == tmp_path / "termdraft" / "sessions"


def test_default_session_root_uses_existing_legacy_leaf(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
    legacy = tmp_path / "termwriter" / "sessions"
    legacy.mkdir(parents=True)

    assert default_session_root() == legacy


def test_default_session_root_prefers_existing_new_leaf(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
    legacy = tmp_path / "termwriter" / "sessions"
    canonical = tmp_path / "termdraft" / "sessions"
    legacy.mkdir(parents=True)
    canonical.mkdir(parents=True)

    assert default_session_root() == canonical
