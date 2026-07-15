"""File and folder management tests across the service and Textual coordinator."""

from __future__ import annotations

from pathlib import Path

import pytest
from textual.widgets import Input, Static

import termdraft.services.workspace_entries as workspace_entries
from termdraft.app import TermDraftApp
from termdraft.models.workspace import Workspace
from termdraft.screens.dialogs import (
    ConflictDialog,
    TrashWorkspaceEntryDialog,
    WorkspaceEntryDialog,
    WorkspaceEntryOperation,
)
from termdraft.services.external_changes import DiskProbe, probe_file
from termdraft.services.recovery import RecoveryJournal
from termdraft.services.session import DocumentViewState, SessionState, SessionStore
from termdraft.services.workspace_entries import (
    WorkspaceEntryError,
    copy_entry,
    create_file,
    create_folder,
    move_entry,
    move_to_trash,
    rename_entry,
)
from termdraft.widgets.dialog import TerminalDialog


def _app(target: Path, state_root: Path) -> TermDraftApp:
    return TermDraftApp(
        Workspace.from_target(target),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(state_root / "recovery"),
        session_store=SessionStore(state_root / "sessions"),
    )


def _select_root_entry(app: TermDraftApp, path: Path) -> None:
    tree = app.explorer.directory_tree
    node = next(
        child for child in tree.root.children if child.data is not None and child.data.path == path
    )
    tree.move_cursor(node)
    tree.focus()


def test_create_file_and_folder_use_explicit_workspace_locations(tmp_path: Path) -> None:
    workspace = Workspace.from_target(tmp_path)

    folder = create_folder(workspace, tmp_path, "notes")
    document = create_file(workspace, folder, "idea.txt")

    assert folder == tmp_path / "notes"
    assert document == folder / "idea.txt"
    assert document.read_bytes() == b""


def test_rename_and_move_preserve_file_contents(tmp_path: Path) -> None:
    workspace = Workspace.from_target(tmp_path)
    archive = create_folder(workspace, tmp_path, "archive")
    source = tmp_path / "draft.md"
    source.write_text("# Draft\n", encoding="utf-8")

    renamed = rename_entry(workspace, source, "essay.md")
    moved = move_entry(workspace, renamed, archive / "essay.md")

    assert not source.exists()
    assert not renamed.exists()
    assert moved.read_text(encoding="utf-8") == "# Draft\n"


def test_copy_file_and_folder_preserves_contents_without_moving_source(tmp_path: Path) -> None:
    workspace = Workspace.from_target(tmp_path)
    source_file = tmp_path / "draft.md"
    source_file.write_text("draft", encoding="utf-8")
    source_folder = tmp_path / "notes"
    source_folder.mkdir()
    (source_folder / "visible.md").write_text("visible", encoding="utf-8")
    (source_folder / ".hidden.txt").write_text("hidden", encoding="utf-8")

    copied_file = copy_entry(workspace, source_file, tmp_path / "draft-copy.md")
    copied_folder = copy_entry(workspace, source_folder, tmp_path / "notes-copy")

    assert source_file.read_text(encoding="utf-8") == "draft"
    assert copied_file.read_text(encoding="utf-8") == "draft"
    assert (source_folder / "visible.md").is_file()
    assert (copied_folder / "visible.md").read_text(encoding="utf-8") == "visible"
    assert (copied_folder / ".hidden.txt").read_text(encoding="utf-8") == "hidden"


def test_copy_rejects_existing_destination_and_folder_self_copy(tmp_path: Path) -> None:
    workspace = Workspace.from_target(tmp_path)
    source = tmp_path / "source.md"
    target = tmp_path / "target.md"
    source.write_text("source", encoding="utf-8")
    target.write_text("target", encoding="utf-8")
    folder = tmp_path / "folder"
    folder.mkdir()

    with pytest.raises(WorkspaceEntryError, match="already exists"):
        copy_entry(workspace, source, target)
    with pytest.raises(WorkspaceEntryError, match="inside itself"):
        copy_entry(workspace, folder, folder / "copy")

    assert source.read_text(encoding="utf-8") == "source"
    assert target.read_text(encoding="utf-8") == "target"


@pytest.fixture
def fake_trash(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    trash = tmp_path.parent / f"{tmp_path.name}-trash"
    trash.mkdir()

    def move(source: Path) -> None:
        Path(source).rename(trash / Path(source).name)

    monkeypatch.setattr(workspace_entries, "send2trash", move)
    return trash


def test_trash_folder_includes_contents_hidden_by_the_explorer(
    tmp_path: Path,
    fake_trash: Path,
) -> None:
    workspace = Workspace.from_target(tmp_path)
    folder = tmp_path / "notes"
    folder.mkdir()
    (folder / "visible.md").write_text("visible", encoding="utf-8")
    (folder / "hidden.txt").write_text("hidden", encoding="utf-8")

    removed = move_to_trash(workspace, folder)

    assert removed == folder
    assert not folder.exists()
    assert (fake_trash / "notes" / "visible.md").read_text(encoding="utf-8") == "visible"
    assert (fake_trash / "notes" / "hidden.txt").read_text(encoding="utf-8") == "hidden"


def test_trash_failure_preserves_the_source(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    workspace = Workspace.from_target(tmp_path)
    source = tmp_path / "note.md"
    source.write_text("source", encoding="utf-8")

    def fail(_source: Path) -> None:
        raise OSError("trash unavailable")

    monkeypatch.setattr(workspace_entries, "send2trash", fail)

    with pytest.raises(WorkspaceEntryError, match=r"Cannot move .* to Trash"):
        move_to_trash(workspace, source)

    assert source.read_text(encoding="utf-8") == "source"


def test_workspace_operations_reject_replacement_escape_and_self_move(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    workspace = Workspace.from_target(workspace_root)
    source = workspace_root / "source.md"
    existing = workspace_root / "existing.md"
    source.write_text("source", encoding="utf-8")
    existing.write_text("existing", encoding="utf-8")
    folder = workspace_root / "folder"
    folder.mkdir()

    with pytest.raises(WorkspaceEntryError, match="already exists"):
        move_entry(workspace, source, existing)
    with pytest.raises(WorkspaceEntryError, match="outside the workspace"):
        move_entry(workspace, source, tmp_path / "outside.md")
    with pytest.raises(WorkspaceEntryError, match="inside itself"):
        move_entry(workspace, folder, folder / "nested")

    assert source.read_text(encoding="utf-8") == "source"
    assert existing.read_text(encoding="utf-8") == "existing"


@pytest.mark.parametrize("source_is_directory", [False, True])
def test_move_never_replaces_a_destination_created_after_validation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    source_is_directory: bool,
) -> None:
    workspace = Workspace.from_target(tmp_path)
    source = tmp_path / ("source" if source_is_directory else "source.md")
    target = tmp_path / ("target" if source_is_directory else "target.md")
    if source_is_directory:
        source.mkdir()
        (source / "source.md").write_text("source", encoding="utf-8")
    else:
        source.write_text("source", encoding="utf-8")
    original_rename = workspace_entries._rename_no_replace

    def create_racer_then_rename(racing_source: Path, racing_target: Path) -> None:
        if source_is_directory:
            racing_target.mkdir()
            (racing_target / "racer.md").write_text("racer", encoding="utf-8")
        else:
            racing_target.write_text("racer", encoding="utf-8")
        original_rename(racing_source, racing_target)

    monkeypatch.setattr(workspace_entries, "_rename_no_replace", create_racer_then_rename)

    with pytest.raises(WorkspaceEntryError, match="already exists"):
        move_entry(workspace, source, target)

    assert source.exists()
    if source_is_directory:
        assert (source / "source.md").read_text(encoding="utf-8") == "source"
        assert (target / "racer.md").read_text(encoding="utf-8") == "racer"
    else:
        assert source.read_text(encoding="utf-8") == "source"
        assert target.read_text(encoding="utf-8") == "racer"


async def test_create_entry_dialog_opens_a_new_text_document(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    app = _app(workspace_root, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_create_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        frame = dialog.query_one(TerminalDialog)
        assert "file or folder" in str(frame.border_title)
        dialog.query_one("#workspace-entry-input", Input).value = "new-note.txt"
        await pilot.press("enter")
        for _ in range(200):
            if app.document is not None and app.document.path.name == "new-note.txt":
                break
            await pilot.pause(0.01)

        path = workspace_root / "new-note.txt"
        assert path.is_file()
        assert app.document is not None
        assert app.document.path == path
        assert app.editor.text == ""


async def test_explorer_keys_create_copy_cut_and_paste_entries(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    source = workspace_root / "draft.md"
    source.write_text("draft", encoding="utf-8")
    copied_destination = workspace_root / "copies"
    copied_destination.mkdir()
    moved_destination = workspace_root / "archive"
    moved_destination.mkdir()
    app = _app(source, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(100):
            if len(app.explorer.directory_tree.root.children) == 3:
                break
            await pilot.pause(0.01)

        _select_root_entry(app, source)
        await pilot.press("c")
        assert app.document is not None
        assert app.document.path == source

        _select_root_entry(app, copied_destination)
        await pilot.press("p")
        copied = copied_destination / source.name
        for _ in range(100):
            if copied.exists() and not app._critical_io:
                break
            await pilot.pause(0.01)

        assert copied.read_text(encoding="utf-8") == "draft"
        assert source.is_file()

        _select_root_entry(app, source)
        await pilot.press("x")
        _select_root_entry(app, moved_destination)
        await pilot.press("p")
        moved = moved_destination / source.name
        for _ in range(100):
            if moved.exists() and not app._critical_io:
                break
            await pilot.pause(0.01)

        assert not source.exists()
        assert moved.read_text(encoding="utf-8") == "draft"
        assert app.document is not None
        assert app.document.path == moved
        assert app._workspace_clipboard is None

        app.explorer.directory_tree.focus()
        await pilot.press("a")
        assert isinstance(app.screen, WorkspaceEntryDialog)


async def test_explorer_cut_keeps_a_dirty_document_at_its_original_path(tmp_path: Path) -> None:
    source = tmp_path / "draft.md"
    source.write_text("draft", encoding="utf-8")
    destination = tmp_path / "archive"
    destination.mkdir()
    app = _app(source, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x", "escape")
        tree = app.explorer.directory_tree
        for _ in range(100):
            if len(tree.root.children) == 2:
                break
            await pilot.pause(0.01)

        _select_root_entry(app, source)
        await pilot.press("x")
        _select_root_entry(app, destination)
        await pilot.press("p")
        await pilot.pause()

        assert source.read_text(encoding="utf-8") == "draft"
        assert not (destination / source.name).exists()
        assert app.document is not None
        assert app.document.path == source
        assert app.document.dirty
        assert app._workspace_clipboard is not None


async def test_explorer_keys_rename_and_trash_the_selected_entry(tmp_path: Path) -> None:
    source = tmp_path / "draft.md"
    source.write_text("draft", encoding="utf-8")
    app = _app(tmp_path, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        tree = app.explorer.directory_tree
        for _ in range(100):
            if tree.root.children:
                break
            await pilot.pause(0.01)

        _select_root_entry(app, source)
        await pilot.press("r")
        assert isinstance(app.screen, WorkspaceEntryDialog)
        assert app.screen.operation is WorkspaceEntryOperation.RENAME
        await pilot.press("escape")

        _select_root_entry(app, source)
        await pilot.press("d")
        assert isinstance(app.screen, TrashWorkspaceEntryDialog)


async def test_create_entry_uses_trailing_slash_for_folder_and_only_warns_about_weird_name(
    tmp_path: Path,
) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    (workspace_root / "some.txt.md").mkdir()
    app = _app(workspace_root, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_create_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "some.txt.md/.md/"
        await pilot.pause()

        feedback = dialog.query_one("#workspace-entry-feedback", Static)
        assert "It will still be created" in str(feedback.render())

        await pilot.press("enter")
        target = workspace_root / "some.txt.md" / ".md"
        for _ in range(200):
            if target.is_dir():
                break
            await pilot.pause(0.01)

        assert target.is_dir()
        assert app.screen is not dialog


async def test_rename_keeps_a_clean_open_document_attached(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("content", encoding="utf-8")
    app = _app(path, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_rename_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "renamed.md"
        await pilot.press("enter")
        for _ in range(200):
            if app.document is not None and app.document.path.name == "renamed.md":
                break
            await pilot.pause(0.01)

        renamed = tmp_path / "renamed.md"
        assert not path.exists()
        assert renamed.read_text(encoding="utf-8") == "content"
        assert app.document is not None
        assert app.document.path == renamed
        assert not app.document.dirty


async def test_rename_retargets_a_deferred_restored_tab(tmp_path: Path) -> None:
    first = tmp_path / "first.md"
    second = tmp_path / "second.md"
    first.write_text("first", encoding="utf-8")
    second.write_text("second", encoding="utf-8")
    state_root = tmp_path / "state"
    store = SessionStore(state_root / "sessions")
    store.save(
        SessionState(
            tmp_path,
            first,
            (DocumentViewState(first), DocumentViewState(second)),
            (first, second),
        )
    )
    app = _app(tmp_path, state_root)

    async with app.run_test(size=(100, 30)) as pilot:
        for _ in range(200):
            if app.document is not None and not app._restoring_session_tabs:
                break
            await pilot.pause(0.01)

        assert len(app._materialized_open_documents()) == 1
        app.push_screen(
            WorkspaceEntryDialog(
                WorkspaceEntryOperation.RENAME,
                tmp_path,
                source=second,
            )
        )
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "renamed.md"
        await pilot.press("enter")
        renamed = tmp_path / "renamed.md"
        for _ in range(200):
            if renamed.exists() and not app._critical_io:
                break
            await pilot.pause(0.01)

        assert [app._tab_path(opened) for opened in app._open_documents] == [first, renamed]
        assert len(app._materialized_open_documents()) == 1

        app._request_open(renamed)
        for _ in range(200):
            if app.document is not None and app.document.path == renamed:
                break
            await pilot.pause(0.01)

        assert app.editor.text == "second"
        assert len(app._materialized_open_documents()) == 2


async def test_rename_refuses_an_open_document_changed_on_disk(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        path.write_text("external", encoding="utf-8")
        app.action_rename_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "renamed.md"
        await pilot.press("enter")
        for _ in range(200):
            if dialog.error is not None:
                break
            await pilot.pause(0.01)

        assert dialog.error is not None
        assert "changed on disk" in dialog.error
        assert path.read_text(encoding="utf-8") == "external"
        assert not (tmp_path / "renamed.md").exists()
        assert app.document is not None
        assert app.document.path == path
        assert app.document.text == "base"
        assert app.document.conflict


async def test_rename_marks_a_change_during_the_operation_as_a_conflict(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    app = _app(path, tmp_path / "state")
    original_rename = rename_entry

    def change_then_rename(workspace: Workspace, source: Path, name: str) -> Path:
        source.write_text("external", encoding="utf-8")
        return original_rename(workspace, source, name)

    monkeypatch.setattr("termdraft.app.rename_entry", change_then_rename)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_rename_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "renamed.md"
        await pilot.press("enter")
        renamed = tmp_path / "renamed.md"
        for _ in range(200):
            if app.document is not None and app.document.path == renamed:
                break
            await pilot.pause(0.01)

        assert app.document is not None
        assert app.document.path == renamed
        assert app.document.text == "base"
        assert app.document.conflict
        assert renamed.read_text(encoding="utf-8") == "external"

        await pilot.press("i", "x", "ctrl+s")
        for _ in range(200):
            if isinstance(app.screen, ConflictDialog):
                break
            await pilot.pause(0.01)

        assert isinstance(app.screen, ConflictDialog)
        assert renamed.read_text(encoding="utf-8") == "external"


async def test_rename_retargets_open_document_when_post_move_probe_fails(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    renamed = tmp_path / "renamed.md"
    app = _app(path, tmp_path / "state")
    original_probe = probe_file

    def fail_renamed_probe(probed_path: Path) -> DiskProbe:
        if probed_path == renamed:
            raise OSError("injected post-move probe failure")
        return original_probe(probed_path)

    monkeypatch.setattr("termdraft.app.probe_file", fail_renamed_probe)

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_rename_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "renamed.md"
        await pilot.press("enter")
        for _ in range(200):
            if app.document is not None and app.document.path == renamed:
                break
            await pilot.pause(0.01)

        assert not path.exists()
        assert renamed.read_text(encoding="utf-8") == "base"
        assert app.document is not None
        assert app.document.path == renamed
        assert app.document.conflict
        assert app.document.last_save_status == "File unavailable"
        assert [app._tab_path(opened) for opened in app._open_documents] == [renamed]
        assert app.screen is not dialog


async def test_move_keeps_dirty_document_and_disk_at_the_original_path(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("base", encoding="utf-8")
    (tmp_path / "archive").mkdir()
    app = _app(path, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        await pilot.press("i", "x")
        app.action_move_entry()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "archive/note.md"
        await pilot.press("enter")
        await pilot.pause()

        assert dialog.error is not None
        assert "Save or close" in dialog.error
        assert app.screen is dialog
        assert path.read_text(encoding="utf-8") == "base"
        assert not (tmp_path / "archive" / "note.md").exists()
        assert app.document is not None
        assert app.document.path == path
        assert app.document.text == "xbase"


async def test_trash_folder_warns_about_hidden_contents_and_moves_after_confirmation(
    tmp_path: Path,
    fake_trash: Path,
) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    folder = workspace_root / "notes"
    folder.mkdir()
    (folder / "note.md").write_text("note", encoding="utf-8")
    (folder / "hidden.txt").write_text("hidden", encoding="utf-8")
    app = _app(workspace_root, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.push_screen(
            TrashWorkspaceEntryDialog(folder, workspace_root),
            lambda confirmed: app._handle_trash_entry_confirmation(folder, confirmed),
        )
        await pilot.pause()

        message = app.screen.query_one(".dialog-message", Static)
        assert "including files hidden by the explorer" in str(message.render())

        await pilot.click("#remove-workspace-entry-confirm")
        for _ in range(200):
            if not folder.exists():
                break
            await pilot.pause(0.01)

        assert not folder.exists()
        assert (fake_trash / "notes" / "note.md").read_text(encoding="utf-8") == "note"


async def test_trash_refuses_to_move_an_open_document(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("content", encoding="utf-8")
    app = _app(path, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_trash_entry()
        await pilot.pause()

        assert not isinstance(app.screen, TrashWorkspaceEntryDialog)
        assert path.read_text(encoding="utf-8") == "content"
        assert app.document is not None
        assert app.document.path == path
