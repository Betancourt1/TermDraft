"""Discoverability coverage for workspaces without an open document."""

from pathlib import Path

from termdraft.app import TermDraftApp
from termdraft.models.workspace import Workspace
from termdraft.services.recovery import RecoveryJournal


async def test_empty_workspace_explains_create_open_and_help_actions(tmp_path: Path) -> None:
    app = TermDraftApp(
        Workspace.from_target(tmp_path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
    )

    async with app.run_test(size=(100, 30)):
        assert isinstance(app.editor.placeholder, str)
        guidance = app.editor.placeholder
        assert app.document is None
        assert app.preview.source_text == guidance
        assert "COMMAND mode" in guidance
        assert "press :" in guidance
        assert "Create file or folder" in guidance
        assert "Ctrl+P" in guidance
        assert "? shows help" in guidance
