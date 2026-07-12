"""Central key bindings and user-facing command help."""

from __future__ import annotations

from collections.abc import Mapping

from textual.binding import Binding, BindingType

from termwriter.config import (
    BINDING_ID_COMMAND_PALETTE,
    BINDING_ID_FIND_FILE,
    BINDING_ID_QUIT,
    BINDING_ID_REDO,
    BINDING_ID_SAVE,
    BINDING_ID_SEARCH_TEXT,
    BINDING_ID_SHOW_HELP,
    BINDING_ID_TOGGLE_EXPLORER,
    BINDING_ID_TOGGLE_PREVIEW,
    BINDING_ID_UNDO,
    DEFAULT_KEYBINDINGS,
)

APP_BINDINGS: list[BindingType] = [
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_SAVE],
        "save",
        "Save",
        priority=True,
        id=BINDING_ID_SAVE,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_QUIT],
        "request_quit",
        "Quit",
        priority=True,
        id=BINDING_ID_QUIT,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_TOGGLE_EXPLORER],
        "toggle_explorer",
        "Files",
        priority=True,
        id=BINDING_ID_TOGGLE_EXPLORER,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_FIND_FILE],
        "find_file",
        "Find file",
        priority=True,
        id=BINDING_ID_FIND_FILE,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_SEARCH_TEXT],
        "search_text",
        "Search text",
        priority=True,
        id=BINDING_ID_SEARCH_TEXT,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_TOGGLE_PREVIEW],
        "toggle_preview",
        "Preview",
        priority=True,
        id=BINDING_ID_TOGGLE_PREVIEW,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_SHOW_HELP],
        "show_help",
        "Help",
        priority=True,
        id=BINDING_ID_SHOW_HELP,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_COMMAND_PALETTE],
        "command_palette",
        "Commands",
        show=False,
        priority=True,
        id=BINDING_ID_COMMAND_PALETTE,
    ),
]

EDITOR_BINDINGS: list[BindingType] = [
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_UNDO],
        "undo",
        "Undo",
        show=False,
        id=BINDING_ID_UNDO,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_REDO],
        "redo",
        "Redo",
        show=False,
        id=BINDING_ID_REDO,
    ),
]

_SHORTCUTS = (
    (BINDING_ID_SAVE, "Save the current document"),
    (BINDING_ID_QUIT, "Quit safely"),
    (BINDING_ID_TOGGLE_EXPLORER, "Show or hide files"),
    (BINDING_ID_FIND_FILE, "Find a Markdown file"),
    (BINDING_ID_SEARCH_TEXT, "Search workspace text (literal / word / regex)"),
    (BINDING_ID_TOGGLE_PREVIEW, "Show or hide preview; switch pane when narrow"),
    (BINDING_ID_UNDO, "Undo"),
    (BINDING_ID_REDO, "Redo"),
    (BINDING_ID_COMMAND_PALETTE, "Open the command palette"),
    (BINDING_ID_SHOW_HELP, "Show shortcut help"),
)

MARKDOWN_SYNTAX_HELP = """\
Headings           # H1 through ###### H6
Emphasis           *italic*, **bold**, ~~strikethrough~~
Bullets            - item   (also * or +)
Numbered lists     1. item
Nested lists       Indent the nested marker by at least three spaces
Tasks              - [ ] pending   - [x] done
Quotes             > quoted text
Alerts             > [!NOTE] then > body (NOTE/TIP/IMPORTANT/WARNING/CAUTION)
Links              [label](https://example.com)
Images             ![alt](path) (shown as a terminal placeholder)
Code               `inline` or fenced ``` blocks
Tables             | A | B | with a | --- | --- | separator row
Footnote ref       Text[^note]
Footnote body      [^note]: source (on its own later line)
Definitions        Term followed on the next line by : Definition
Rules              ---

Enter continues bullets, numbered lists, tasks, and blockquotes. Press Enter on
an empty marker to end the list. Preview footnote labels and their ↩ arrows navigate internally.
Raw HTML is displayed as text and never run.

Not rendered yet: math, underline, subscript, and superscript.
Markdown has no portable __underline__ syntax; double underscores mean bold.
A nested ordered item is "   1. item", not "1.1.".
"""


def format_shortcut_help(
    keybindings: Mapping[str, str],
    *,
    auto_continue_lists: bool = True,
) -> str:
    """Render help from the effective keymap so remapped keys stay truthful."""
    rows = [
        (_display_keys(keybindings[binding_id]), description)
        for binding_id, description in _SHORTCUTS
    ]
    rows.append(("Tab / Shift+Tab", "Move focus"))
    if auto_continue_lists:
        rows.append(("Enter in a list", "Continue it; an empty marker ends it"))
    width = max(len(keys) for keys, _description in rows) + 3
    return "\n".join(f"{keys:<{width}}{description}" for keys, description in rows)


def format_command_help(
    keybindings: Mapping[str, str],
    *,
    auto_continue_lists: bool = True,
) -> str:
    """Return terminal-friendly help for the CLI command listing."""
    palette = _display_keys(keybindings[BINDING_ID_COMMAND_PALETTE])
    return (
        "TermWriter commands\n\n"
        + format_shortcut_help(
            keybindings,
            auto_continue_lists=auto_continue_lists,
        )
        + "\n\n"
        + f"Press {palette} in the TUI to search all commands, including:\n"
        + "  Save, find file, search workspace text, toggle files, toggle preview,\n"
        + "  undo, redo,\n"
        + "  reload configuration, shortcut help, Markdown syntax help, and safe quit."
    )


def _display_keys(keys: str) -> str:
    return " / ".join(_display_key(key.strip()) for key in keys.split(","))


def _display_key(key: str) -> str:
    names = {
        "alt": "Alt",
        "backslash": "\\",
        "ctrl": "Ctrl",
        "escape": "Esc",
        "shift": "Shift",
        "super": "Super",
    }
    displayed: list[str] = []
    for part in key.split("+"):
        normalized = part.lower()
        if normalized in names:
            displayed.append(names[normalized])
        elif len(part) == 1 or (normalized.startswith("f") and normalized[1:].isdigit()):
            displayed.append(part.upper())
        else:
            displayed.append(part.replace("_", " ").title())
    return "+".join(displayed)
