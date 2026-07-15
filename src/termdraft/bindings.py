"""Central key bindings and user-facing command help."""

from __future__ import annotations

from collections.abc import Mapping

from textual.binding import Binding, BindingType

from termdraft.config import (
    BINDING_ID_CLOSE_TAB,
    BINDING_ID_COMMAND_CLOSE_TAB,
    BINDING_ID_COMMAND_CURSOR_DOWN,
    BINDING_ID_COMMAND_CURSOR_LEFT,
    BINDING_ID_COMMAND_CURSOR_RIGHT,
    BINDING_ID_COMMAND_CURSOR_UP,
    BINDING_ID_COMMAND_DOCUMENT_END,
    BINDING_ID_COMMAND_DOCUMENT_START,
    BINDING_ID_COMMAND_FIND_FILE,
    BINDING_ID_COMMAND_LINE_END,
    BINDING_ID_COMMAND_LINE_START,
    BINDING_ID_COMMAND_NEXT_TAB,
    BINDING_ID_COMMAND_OPEN_PALETTE,
    BINDING_ID_COMMAND_PALETTE,
    BINDING_ID_COMMAND_PREVIOUS_TAB,
    BINDING_ID_COMMAND_QUIT,
    BINDING_ID_COMMAND_RECENT_DOCUMENTS,
    BINDING_ID_COMMAND_REDO,
    BINDING_ID_COMMAND_SAVE,
    BINDING_ID_COMMAND_SEARCH_TEXT,
    BINDING_ID_COMMAND_SHOW_HELP,
    BINDING_ID_COMMAND_TOGGLE_EXPLORER,
    BINDING_ID_COMMAND_TOGGLE_PREVIEW,
    BINDING_ID_COMMAND_UNDO,
    BINDING_ID_COMMAND_WRITE_MODE,
    BINDING_ID_DOCUMENT_OUTLINE,
    BINDING_ID_FIND_FILE,
    BINDING_ID_FIND_REPLACE,
    BINDING_ID_NEXT_TAB,
    BINDING_ID_PREVIEW_NEXT_HEADING,
    BINDING_ID_PREVIEW_PREVIOUS_HEADING,
    BINDING_ID_PREVIOUS_TAB,
    BINDING_ID_QUIT,
    BINDING_ID_RECENT_DOCUMENTS,
    BINDING_ID_REDO,
    BINDING_ID_SAVE,
    BINDING_ID_SAVE_AS,
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
        DEFAULT_KEYBINDINGS[BINDING_ID_SAVE_AS],
        "save_as",
        "Save as",
        priority=True,
        id=BINDING_ID_SAVE_AS,
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
        DEFAULT_KEYBINDINGS[BINDING_ID_RECENT_DOCUMENTS],
        "open_recent",
        "Recent",
        priority=True,
        id=BINDING_ID_RECENT_DOCUMENTS,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_NEXT_TAB],
        "next_tab",
        "Next tab",
        priority=True,
        id=BINDING_ID_NEXT_TAB,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_PREVIOUS_TAB],
        "previous_tab",
        "Previous tab",
        priority=True,
        id=BINDING_ID_PREVIOUS_TAB,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_CLOSE_TAB],
        "close_tab",
        "Close tab",
        priority=True,
        id=BINDING_ID_CLOSE_TAB,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_SEARCH_TEXT],
        "search_text",
        "Search text",
        priority=True,
        id=BINDING_ID_SEARCH_TEXT,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_FIND_REPLACE],
        "find_replace",
        "Find",
        priority=True,
        id=BINDING_ID_FIND_REPLACE,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_DOCUMENT_OUTLINE],
        "document_outline",
        "Outline",
        priority=True,
        id=BINDING_ID_DOCUMENT_OUTLINE,
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
    Binding(
        "escape",
        "enter_command_mode",
        "Command mode",
        show=False,
        priority=True,
        id="enter_command_mode",
    ),
]

COMMAND_MODE_SHORTCUTS = (
    (BINDING_ID_COMMAND_WRITE_MODE, "enter_write_mode", "Enter WRITE mode"),
    (BINDING_ID_COMMAND_SAVE, "save", "Save the current document"),
    (BINDING_ID_COMMAND_QUIT, "request_quit", "Quit safely"),
    (BINDING_ID_COMMAND_TOGGLE_EXPLORER, "toggle_explorer", "Show or hide files"),
    (BINDING_ID_COMMAND_FIND_FILE, "find_file", "Find a text file"),
    (BINDING_ID_COMMAND_RECENT_DOCUMENTS, "open_recent", "Open a recent document"),
    (BINDING_ID_COMMAND_NEXT_TAB, "next_tab", "Activate the next open document tab"),
    (
        BINDING_ID_COMMAND_PREVIOUS_TAB,
        "previous_tab",
        "Activate the previous open document tab",
    ),
    (BINDING_ID_COMMAND_CLOSE_TAB, "close_tab", "Close the active tab safely"),
    (BINDING_ID_COMMAND_SEARCH_TEXT, "search_text", "Search workspace text"),
    (
        BINDING_ID_COMMAND_TOGGLE_PREVIEW,
        "toggle_preview",
        "Show, hide, or focus the preview",
    ),
    (BINDING_ID_COMMAND_UNDO, "editor_undo", "Undo"),
    (BINDING_ID_COMMAND_REDO, "editor_redo", "Redo"),
    (BINDING_ID_COMMAND_OPEN_PALETTE, "command_palette", "Open the command palette"),
    (BINDING_ID_COMMAND_SHOW_HELP, "show_help", "Show shortcut help"),
    (BINDING_ID_COMMAND_CURSOR_LEFT, "cursor_left", "Move left"),
    (BINDING_ID_COMMAND_CURSOR_DOWN, "cursor_down", "Move down"),
    (BINDING_ID_COMMAND_CURSOR_UP, "cursor_up", "Move up"),
    (BINDING_ID_COMMAND_CURSOR_RIGHT, "cursor_right", "Move right"),
    (BINDING_ID_COMMAND_LINE_START, "line_start", "Move to source-line start"),
    (BINDING_ID_COMMAND_LINE_END, "line_end", "Move to source-line end"),
    (BINDING_ID_COMMAND_DOCUMENT_START, "document_start", "Move to document start"),
    (BINDING_ID_COMMAND_DOCUMENT_END, "document_end", "Move to document end"),
)

COMMAND_NAVIGATION_ACTIONS = frozenset(
    {
        "cursor_left",
        "cursor_down",
        "cursor_up",
        "cursor_right",
        "line_start",
        "line_end",
        "document_start",
        "document_end",
    }
)

FILES_SHORTCUTS = (
    ("a", "Create a file or folder"),
    ("c", "Copy the selected file or folder"),
    ("x", "Cut the selected file or folder"),
    ("p", "Paste into the selected folder or beside the selected file"),
    ("r", "Rename the selected file or folder"),
    ("d", "Move the selected file or folder to Trash"),
)

_COMMAND_MODE_BINDING_IDS = {
    action: binding_id for binding_id, action, _description in COMMAND_MODE_SHORTCUTS
}

APP_BINDINGS.extend(
    Binding(
        DEFAULT_KEYBINDINGS[binding_id],
        f"command_mode_key('{action}')",
        description,
        show=False,
        priority=True,
        id=binding_id,
    )
    for binding_id, action, description in COMMAND_MODE_SHORTCUTS
)

EDITOR_BINDINGS: list[BindingType] = [
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_UNDO],
        "undo",
        "Undo",
        show=False,
        priority=True,
        id=BINDING_ID_UNDO,
    ),
    Binding(
        DEFAULT_KEYBINDINGS[BINDING_ID_REDO],
        "redo",
        "Redo",
        show=False,
        priority=True,
        id=BINDING_ID_REDO,
    ),
]

_SHORTCUTS = (
    (BINDING_ID_SAVE, "Save the current document"),
    (BINDING_ID_SAVE_AS, "Save the active document under a new path"),
    (BINDING_ID_QUIT, "Quit safely"),
    (BINDING_ID_TOGGLE_EXPLORER, "Show or hide files"),
    (BINDING_ID_FIND_FILE, "Find a text file"),
    (BINDING_ID_RECENT_DOCUMENTS, "Open a recent document"),
    (BINDING_ID_NEXT_TAB, "Activate the next open document tab"),
    (BINDING_ID_PREVIOUS_TAB, "Activate the previous open document tab"),
    (BINDING_ID_CLOSE_TAB, "Close the active tab safely"),
    (BINDING_ID_FIND_REPLACE, "Find and replace in the active document"),
    (BINDING_ID_SEARCH_TEXT, "Search workspace text (literal / fuzzy / word / regex)"),
    (BINDING_ID_DOCUMENT_OUTLINE, "Search headings in the active document"),
    (BINDING_ID_TOGGLE_PREVIEW, "Show or hide preview; switch pane when narrow"),
    (BINDING_ID_PREVIEW_NEXT_HEADING, "Select the next heading in the focused preview"),
    (
        BINDING_ID_PREVIEW_PREVIOUS_HEADING,
        "Select the previous heading in the focused preview",
    ),
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
an empty marker to end the list. Press Esc for COMMAND mode and i to return to WRITE mode. In the
focused preview, Tab and Shift+Tab select links and Enter activates the selection.
Alt+Down and Alt+Up move between rendered headings and show the current heading position.
Footnotes navigate internally; external URLs remain inert. Raw HTML is displayed as text and never
run.

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
    command_rows = [("Esc", "Enter COMMAND mode")]
    command_rows.extend(
        (_display_command_keys(keybindings[binding_id]), description)
        for binding_id, _action, description in COMMAND_MODE_SHORTCUTS
    )
    configured_rows = [
        (_display_keys(keybindings[binding_id]), description)
        for binding_id, description in _SHORTCUTS
    ]
    files_rows = list(FILES_SHORTCUTS)
    extra_rows = [
        ("Tab / Shift+Tab in preview", "Select links or leave the preview"),
        ("Enter in preview", "Activate the selected preview link"),
    ]
    if auto_continue_lists:
        extra_rows.append(("Enter in a list", "Continue it; an empty marker ends it"))
    rows = [*command_rows, *configured_rows, *extra_rows]
    width = max(len(keys) for keys, _description in rows) + 3

    def format_rows(section: list[tuple[str, str]]) -> str:
        return "\n".join(f"{keys:<{width}}{description}" for keys, description in section)

    return (
        "Modes and COMMAND keys\n"
        + format_rows(command_rows)
        + "\n\nFocused Files keys\n"
        + format_rows(files_rows)
        + "\n\nConfigured shortcuts (available in both modes)\n"
        + format_rows(configured_rows)
        + "\n\nEditor and preview controls\n"
        + format_rows(extra_rows)
    )


def format_command_help(
    keybindings: Mapping[str, str],
    *,
    auto_continue_lists: bool = True,
) -> str:
    """Return terminal-friendly help for the CLI command listing."""
    command_palette = format_action_shortcuts("command_palette", keybindings)
    palette = _display_keys(keybindings[BINDING_ID_COMMAND_PALETTE])
    return (
        "TermDraft commands\n\n"
        + format_shortcut_help(
            keybindings,
            auto_continue_lists=auto_continue_lists,
        )
        + "\n\n"
        + f"Press {command_palette} in COMMAND mode or {palette} in either mode to search all "
        + "commands, "
        + "including:\n"
        + "  Save, find file, open recent,\n"
        + "  next/previous/close tab, search workspace text,\n"
        + "  toggle files, toggle preview,\n"
        + "  undo, redo,\n"
        + "  reload configuration, manage recovery drafts, inspect semantic blocks,\n"
        + "  read semantic blocks experimentally, inspect cursor coordinates,\n"
        + "  shortcut help,\n"
        + "  Markdown syntax help, and safe quit.\n\n"
        + "Focus Files to create, copy, cut, paste, rename, or trash workspace entries."
    )


def format_action_shortcuts(action: str, keybindings: Mapping[str, str]) -> str:
    """Display the single-key COMMAND shortcut for a palette action."""
    binding_id = _COMMAND_MODE_BINDING_IDS.get(action)
    return (
        _display_command_keys(keybindings[binding_id]) if binding_id is not None else "Palette only"
    )


def _display_keys(keys: str) -> str:
    return " / ".join(_display_key(key.strip()) for key in keys.split(","))


def _display_command_keys(keys: str) -> str:
    return " / ".join(
        key if len(key) == 1 else _display_key(key) for key in map(str.strip, keys.split(","))
    )


def _display_key(key: str) -> str:
    names = {
        "alt": "Alt",
        "backslash": "\\",
        "colon": ":",
        "ctrl": "Ctrl",
        "dollar_sign": "$",
        "escape": "Esc",
        "question_mark": "?",
        "shift": "Shift",
        "slash": "/",
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
