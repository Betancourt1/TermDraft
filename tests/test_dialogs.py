"""Focused interaction tests for reusable modal behavior."""

from datetime import UTC, datetime
from pathlib import Path

from textual import on
from textual.app import App
from textual.widgets import Button, Input, OptionList, Static

from termwriter.models.document import FileSnapshot
from termwriter.screens.dialogs import (
    RecoveryDeleteDialog,
    RecoveryManagerAction,
    RecoveryManagerDialog,
    RecoveryManagerRequest,
    RecoveryRetentionDialog,
    RecoveryRetentionRequest,
    SaveAsDialog,
)
from termwriter.services.recovery import RecoveryJournal


class SaveAsHarness(App[None]):
    """Mount a save-as dialog and record submissions without app coordination."""

    def __init__(self, dialog: SaveAsDialog) -> None:
        self.dialog = dialog
        self.submissions: list[str] = []
        super().__init__()

    def on_mount(self) -> None:
        self.push_screen(self.dialog)

    @on(SaveAsDialog.Submitted)
    def record_submission(self, event: SaveAsDialog.Submitted) -> None:
        self.submissions.append(event.value)


class RecoveryManagerHarness(App[None]):
    """Mount the inventory dialog and retain its typed result."""

    def __init__(self, dialog: RecoveryManagerDialog) -> None:
        self.dialog = dialog
        self.result: RecoveryManagerRequest | None = None
        super().__init__(css_path=Path(__file__).parents[1] / "src" / "termwriter" / "default.tcss")

    def on_mount(self) -> None:
        self.push_screen(self.dialog, self._store_result)

    def _store_result(
        self,
        result: RecoveryManagerRequest | RecoveryRetentionRequest | None,
    ) -> None:
        self.result = result if isinstance(result, RecoveryManagerRequest) else None


class RecoveryDeleteFlowHarness(App[None]):
    """Exercise the inventory-to-deletion-confirmation keyboard flow."""

    def __init__(self, dialog: RecoveryManagerDialog) -> None:
        self.dialog = dialog
        self.confirmed: bool | None = None
        super().__init__(css_path=Path(__file__).parents[1] / "src" / "termwriter" / "default.tcss")

    def on_mount(self) -> None:
        self.push_screen(self.dialog, self._show_confirmation)

    def _show_confirmation(
        self,
        request: RecoveryManagerRequest | RecoveryRetentionRequest | None,
    ) -> None:
        if isinstance(request, RecoveryManagerRequest):
            self.push_screen(RecoveryDeleteDialog(request.record), self._store_confirmation)

    def _store_confirmation(self, confirmed: bool | None) -> None:
        self.confirmed = confirmed


class RecoveryRetentionHarness(App[None]):
    """Mount the irreversible retention confirmation with production CSS."""

    def __init__(self, dialog: RecoveryRetentionDialog) -> None:
        self.dialog = dialog
        super().__init__(css_path=Path(__file__).parents[1] / "src" / "termwriter" / "default.tcss")

    def on_mount(self) -> None:
        self.push_screen(self.dialog)


async def test_save_as_busy_state_blocks_edit_submit_cancel_and_escape() -> None:
    dialog = SaveAsDialog("note-local.md")
    app = SaveAsHarness(dialog)

    async with app.run_test(size=(80, 24)) as pilot:
        await pilot.pause()
        dialog.set_busy(True)
        await pilot.pause()

        input_widget = dialog.query_one("#save-as-input", Input)
        assert input_widget.disabled
        assert dialog.query_one("#save-as-confirm", Button).disabled
        assert dialog.query_one("#save-as-cancel", Button).disabled

        await pilot.press("x", "enter", "escape")
        await pilot.click("#save-as-confirm")
        await pilot.click("#save-as-cancel")
        await pilot.pause()

        assert input_widget.value == "note-local.md"
        assert app.submissions == []
        assert app.screen is dialog


async def test_save_as_error_restores_editable_focused_state() -> None:
    dialog = SaveAsDialog("note-local.md")
    app = SaveAsHarness(dialog)

    async with app.run_test(size=(80, 24)) as pilot:
        await pilot.pause()
        dialog.set_busy(True)
        dialog.show_error("Publication failed")
        await pilot.pause()

        input_widget = dialog.query_one("#save-as-input", Input)
        assert not input_widget.disabled
        assert not dialog.query_one("#save-as-confirm", Button).disabled
        assert not dialog.query_one("#save-as-cancel", Button).disabled
        assert dialog.error == "Publication failed"
        assert "Publication failed" in str(dialog.query_one("#save-as-error", Static).render())
        assert app.focused is input_widget

        await pilot.press("enter")
        await pilot.pause()
        assert app.submissions == ["note-local.md"]

        await pilot.press("escape")
        await pilot.pause()
        assert app.screen is not dialog


async def test_recovery_manager_protects_active_draft_and_can_archive_corrupt_entry(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    active = workspace / "active.md"
    journal.save(
        document_path=active,
        workspace_root=workspace,
        text="active draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    corrupt_path = journal.state_root / f"{'f' * 64}.json"
    corrupt_path.write_bytes(b"not json")
    records = journal.list_entries(workspace)
    dialog = RecoveryManagerDialog(
        records,
        workspace,
        protected_journal_path=journal.path_for(active),
    )
    app = RecoveryManagerHarness(dialog)

    async with app.run_test(size=(90, 32)) as pilot:
        await pilot.pause()
        options = dialog.query_one("#recovery-manager-records", OptionList)
        valid_index = next(
            index for index, record in enumerate(records) if record.entry is not None
        )
        corrupt_index = next(index for index, record in enumerate(records) if record.entry is None)

        options.highlighted = valid_index
        await pilot.pause()
        assert dialog.query_one("#recovery-manager-open", Button).disabled
        assert dialog.query_one("#recovery-manager-retarget", Button).disabled
        assert dialog.query_one("#recovery-manager-archive", Button).disabled

        options.highlighted = corrupt_index
        await pilot.pause()
        assert dialog.query_one("#recovery-manager-open", Button).disabled
        assert dialog.query_one("#recovery-manager-retarget", Button).disabled
        assert not dialog.query_one("#recovery-manager-archive", Button).disabled

        await pilot.click("#recovery-manager-archive")
        await pilot.pause()

        assert app.result is not None
        assert app.result.action is RecoveryManagerAction.QUARANTINE
        assert app.result.record.entry is None


async def test_recovery_manager_actions_fit_and_scroll_into_a_narrow_terminal(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    journal.save(
        document_path=workspace / "draft.md",
        workspace_root=workspace,
        text="draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    dialog = RecoveryManagerDialog(journal.list_entries(workspace), workspace)
    app = RecoveryManagerHarness(dialog)

    async with app.run_test(size=(24, 20)) as pilot:
        await pilot.pause()
        for selector in (
            "#recovery-manager-open",
            "#recovery-manager-retarget",
            "#recovery-manager-archive",
            "#recovery-manager-retention",
            "#recovery-manager-close",
        ):
            button = dialog.query_one(selector, Button).focus()
            await pilot.pause()
            assert button.region.x >= 0
            assert button.region.right <= app.size.width
            assert button.region.y >= 0
            assert button.region.bottom <= app.size.height


async def test_recovery_manager_returns_explicit_retarget_request(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    old_path = workspace / "old.md"
    journal.save(
        document_path=old_path,
        workspace_root=workspace,
        text="draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    records = journal.list_entries(workspace)
    dialog = RecoveryManagerDialog(records, workspace)
    app = RecoveryManagerHarness(dialog)

    async with app.run_test(size=(90, 32)) as pilot:
        await pilot.pause()
        dialog.query_one("#recovery-manager-target", Input).value = "renamed.md"
        await pilot.click("#recovery-manager-retarget")
        await pilot.pause()

        assert app.result is not None
        assert app.result.action is RecoveryManagerAction.RETARGET
        assert app.result.target == "renamed.md"
        assert app.result.record.entry is not None
        assert app.result.record.entry.document_path == old_path


async def test_recovery_manager_restores_a_valid_quarantined_entry(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    document = workspace / "draft.md"
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    journal.quarantine(active_record)
    dialog = RecoveryManagerDialog(journal.list_quarantined(workspace), workspace)
    app = RecoveryManagerHarness(dialog)

    async with app.run_test(size=(90, 36)) as pilot:
        await pilot.pause()

        assert "Restore" in str(dialog.query_one("#recovery-manager-open", Button).label)
        assert "Delete forever" in str(dialog.query_one("#recovery-manager-retarget", Button).label)
        assert not dialog.query_one("#recovery-manager-target", Input).disabled
        assert "Export copy" in str(dialog.query_one("#recovery-manager-archive", Button).label)
        assert not dialog.query_one("#recovery-manager-archive", Button).disabled
        await pilot.click("#recovery-manager-open")
        await pilot.pause()

        assert app.result is not None
        assert app.result.action is RecoveryManagerAction.RESTORE_QUARANTINED
        assert app.result.record.quarantined


async def test_recovery_manager_returns_explicit_quarantine_export_request(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    document = workspace / "draft.md"
    journal.save(
        document_path=document,
        workspace_root=workspace,
        text="draft",
        encoding="utf-8",
        base_snapshot=FileSnapshot.missing(),
    )
    (active_record,) = journal.list_entries(workspace)
    journal.quarantine(active_record)
    dialog = RecoveryManagerDialog(journal.list_quarantined(workspace), workspace)
    app = RecoveryManagerHarness(dialog)

    async with app.run_test(size=(90, 36)) as pilot:
        await pilot.pause()
        dialog.query_one("#recovery-manager-target", Input).value = "exports/draft.md"
        await pilot.click("#recovery-manager-archive")
        await pilot.pause()

        assert app.result is not None
        assert app.result.action is RecoveryManagerAction.EXPORT_QUARANTINED
        assert app.result.target == "exports/draft.md"
        assert app.result.record.quarantined


async def test_recovery_manager_returns_explicit_permanent_delete_request(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    (quarantine_root / f"{'f' * 64}.json").write_bytes(b"corrupt archive")
    dialog = RecoveryManagerDialog(journal.list_quarantined(workspace), workspace)
    app = RecoveryManagerHarness(dialog)

    async with app.run_test(size=(90, 36)) as pilot:
        await pilot.pause()
        assert dialog.query_one("#recovery-manager-open", Button).disabled

        await pilot.click("#recovery-manager-retarget")
        await pilot.pause()
        assert app.result is not None
        assert app.result.action is RecoveryManagerAction.DELETE_QUARANTINED


async def test_permanent_delete_double_enter_defaults_to_cancel(tmp_path: Path) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    quarantine_root = journal.state_root / "quarantine"
    quarantine_root.mkdir(parents=True)
    (quarantine_root / f"{'f' * 64}.json").write_bytes(b"corrupt archive")
    dialog = RecoveryManagerDialog(journal.list_quarantined(workspace), workspace)
    app = RecoveryDeleteFlowHarness(dialog)

    async with app.run_test(size=(90, 36)) as pilot:
        await pilot.pause()
        dialog.query_one("#recovery-manager-retarget", Button).focus()

        await pilot.press("enter", "enter")
        await pilot.pause()

        assert app.confirmed is False


async def test_retention_confirmation_lists_files_and_fits_narrow_terminal(
    tmp_path: Path,
) -> None:
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    journal = RecoveryJournal(tmp_path / "state")
    for name in ("first.md", "second.md"):
        document = workspace / name
        journal.save(
            document_path=document,
            workspace_root=workspace,
            text=f"{name} draft",
            encoding="utf-8",
            base_snapshot=FileSnapshot.missing(),
        )
        record = next(
            item
            for item in journal.list_entries(workspace)
            if item.entry is not None and item.entry.document_path == document
        )
        journal.quarantine(record)
    records = journal.list_quarantined(workspace)
    dialog = RecoveryRetentionDialog(RecoveryRetentionRequest(datetime.now(UTC), records, 30))
    app = RecoveryRetentionHarness(dialog)

    async with app.run_test(size=(24, 20)) as pilot:
        await pilot.pause()
        options = dialog.query_one("#recovery-retention-records", OptionList)
        labels = [str(options.get_option_at_index(index).prompt) for index in range(len(records))]
        assert any("first.md" in label for label in labels)
        assert any("second.md" in label for label in labels)

        for selector in ("#recovery-retention-confirm", "#recovery-retention-cancel"):
            button = dialog.query_one(selector, Button)
            button.focus()
            await pilot.pause()
            assert button.region.x >= 0
            assert button.region.right <= app.size.width
