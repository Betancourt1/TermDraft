"""Focused interaction tests for reusable modal behavior."""

from textual import on
from textual.app import App
from textual.widgets import Button, Input, Static

from termwriter.screens.dialogs import SaveAsDialog


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
