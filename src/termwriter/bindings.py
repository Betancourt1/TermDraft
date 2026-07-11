"""Central key bindings and the text shown by the shortcut help dialog."""

from textual.binding import Binding, BindingType

APP_BINDINGS: list[BindingType] = [
    Binding("ctrl+s", "save", "Save", priority=True),
    Binding("ctrl+q", "request_quit", "Quit", priority=True),
    Binding("ctrl+b", "toggle_explorer", "Files", priority=True),
    Binding("ctrl+p", "find_file", "Find file", priority=True),
    Binding("ctrl+e", "toggle_preview", "Preview", priority=True),
    Binding("ctrl+z", "editor_undo", "Undo", show=False),
    Binding("ctrl+y,ctrl+shift+z", "editor_redo", "Redo", show=False),
    Binding("f1", "show_help", "Help", priority=True),
]

SHORTCUT_HELP = """\
Ctrl+S            Save
Ctrl+Q            Quit safely
Ctrl+B            Show or hide files
Ctrl+P            Find a Markdown file
Ctrl+E            Show or hide preview; switch pane when narrow
Ctrl+Z            Undo
Ctrl+Y            Redo
Ctrl+Shift+Z      Redo
F1                Show this help
Tab / Shift+Tab   Move focus
"""
