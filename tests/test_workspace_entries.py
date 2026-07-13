"""File and folder management tests across the service and Textual coordinator."""

from __future__ import annotations

from pathlib import Path

import pytest
from textual.widgets import Input, Static

from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace
from termwriter.screens.dialogs import (
    ConflictDialog,
    RemoveWorkspaceEntryDialog,
    WorkspaceEntryDialog,
)
from termwriter.services.recovery import RecoveryJournal
from termwriter.services.session import SessionStore
from termwriter.services.workspace_entries import (
    WorkspaceEntryError,
    create_folder,
    create_markdown_file,
    move_entry,
    remove_entry,
    rename_entry,
)


def _app(target: Path, state_root: Path) -> TermWriterApp:
    return TermWriterApp(
        Workspace.from_target(target),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(state_root / "recovery"),
        session_store=SessionStore(state_root / "sessions"),
    )


def test_create_file_and_folder_use_explicit_workspace_locations(tmp_path: Path) -> None:
    workspace = Workspace.from_target(tmp_path)

    folder = create_folder(workspace, tmp_path, "notes")
    document = create_markdown_file(workspace, folder, "idea")

    assert folder == tmp_path / "notes"
    assert document == folder / "idea.md"
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


def test_remove_folder_includes_contents_hidden_by_the_explorer(tmp_path: Path) -> None:
    workspace = Workspace.from_target(tmp_path)
    folder = tmp_path / "notes"
    folder.mkdir()
    (folder / "visible.md").write_text("visible", encoding="utf-8")
    (folder / "hidden.txt").write_text("hidden", encoding="utf-8")

    removed = remove_entry(workspace, folder)

    assert removed == folder
    assert not folder.exists()


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


async def test_create_file_dialog_opens_the_new_document(tmp_path: Path) -> None:
    workspace_root = tmp_path / "workspace"
    workspace_root.mkdir()
    app = _app(workspace_root, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_create_file()
        await pilot.pause()
        dialog = app.screen
        assert isinstance(dialog, WorkspaceEntryDialog)
        dialog.query_one("#workspace-entry-input", Input).value = "new-note"
        await pilot.press("enter")
        for _ in range(200):
            if app.document is not None and app.document.path.name == "new-note.md":
                break
            await pilot.pause(0.01)

        path = workspace_root / "new-note.md"
        assert path.is_file()
        assert app.document is not None
        assert app.document.path == path
        assert app.editor.text == ""


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

    monkeypatch.setattr("termwriter.app.rename_entry", change_then_rename)

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


async def test_remove_folder_warns_about_hidden_contents_and_removes_after_confirmation(
    tmp_path: Path,
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
            RemoveWorkspaceEntryDialog(folder, workspace_root),
            lambda confirmed: app._handle_remove_entry_confirmation(folder, confirmed),
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


async def test_remove_refuses_to_delete_an_open_document(tmp_path: Path) -> None:
    path = tmp_path / "note.md"
    path.write_text("content", encoding="utf-8")
    app = _app(path, tmp_path / "state")

    async with app.run_test(size=(100, 30)) as pilot:
        app.action_remove_entry()
        await pilot.pause()

        assert not isinstance(app.screen, RemoveWorkspaceEntryDialog)
        assert path.read_text(encoding="utf-8") == "content"
        assert app.document is not None
        assert app.document.path == path
