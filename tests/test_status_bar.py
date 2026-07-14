"""Status-bar priority coverage for narrow terminals."""

from pathlib import Path

from termwriter.app import TermWriterApp
from termwriter.models.document import Document, FileSnapshot
from termwriter.models.workspace import Workspace
from termwriter.services.recovery import RecoveryJournal
from termwriter.widgets.status_bar import TermWriterStatusBar


async def test_persistent_safety_markers_precede_a_long_path(tmp_path: Path) -> None:
    path = tmp_path / ("deep-note-" * 12 + "draft.md")
    path.write_text("saved", encoding="utf-8")
    document = Document(
        path,
        "local\r\ndraft\n",
        "saved",
        FileSnapshot.missing(),
        conflict=True,
        recovery_saved=True,
    )
    app = TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
    )

    async with app.run_test(size=(100, 20)):
        status = app.query_one(TermWriterStatusBar)
        status.show_document(document, root=tmp_path, mode="COMMAND")

        rendered = str(status.render())
        prioritized = (
            "CONFLICT",
            "● modified",
            "RECOVERY STORED",
            "MIXED→CRLF",
            path.name,
        )
        assert [rendered.index(label) for label in prioritized] == sorted(
            rendered.index(label) for label in prioritized
        )


async def test_narrow_terminal_keeps_safety_markers_visible(tmp_path: Path) -> None:
    path = tmp_path / ("very-long-" * 12 + "draft.md")
    path.write_text("saved", encoding="utf-8")
    app = TermWriterApp(
        Workspace.from_target(path),
        preview_debounce=0.01,
        recovery_journal=RecoveryJournal(tmp_path / "recovery"),
    )

    async with app.run_test(size=(70, 20)) as pilot:
        await pilot.pause(0.03)
        assert app.document is not None
        app.document.text = "local draft"
        app.document.conflict = True
        app.document.recovery_saved = True
        app._refresh_status()
        await pilot.pause()

        visible_status = app.query_one(TermWriterStatusBar).render_line(0).text
        assert "CONFLICT" in visible_status
        assert "● modified" in visible_status
        assert "RECOVERY STORED" in visible_status
        assert path.name not in visible_status
