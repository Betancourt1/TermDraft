"""Compact grouped rendering for Textual's command palette."""

from __future__ import annotations

from rich.text import Text
from textual import events
from textual.app import ComposeResult
from textual.command import (
    Command,
    CommandInput,
    CommandList,
    CommandPalette,
    ProviderSource,
    SearchIcon,
)
from textual.containers import Grid, Horizontal, Vertical, VerticalScroll
from textual.widgets import LoadingIndicator, OptionList, Static

_COMMAND_GROUPS = (
    (
        "document",
        "DOCUMENT",
        (
            ("save", "Save"),
            ("save_as", "Save as"),
            ("duplicate_document", "Duplicate"),
            ("find_file", "Find file"),
            ("open_recent", "Recent documents"),
            ("close_tab", "Close tab"),
        ),
    ),
    (
        "navigate",
        "NAVIGATE",
        (
            ("next_tab", "Next tab"),
            ("previous_tab", "Previous tab"),
            ("search_text", "Search workspace"),
            ("find_replace", "Find and replace"),
            ("document_outline", "Outline"),
            ("toggle_explorer", "Explorer"),
        ),
    ),
    (
        "files",
        "FILES",
        (
            ("create_entry", "Create"),
            ("copy_entry", "Copy"),
            ("cut_entry", "Cut"),
            ("paste_entry", "Paste"),
            ("rename_entry", "Rename"),
            ("move_entry", "Move"),
            ("trash_entry", "Trash"),
        ),
    ),
    (
        "mode",
        "MODE",
        (
            ("enter_write_mode", "Write mode"),
            ("enter_command_mode", "Command mode"),
        ),
    ),
    (
        "edit",
        "EDIT",
        (
            ("editor_undo", "Undo"),
            ("editor_redo", "Redo"),
            ("reload_config", "Reload config"),
            ("inspect_semantic_blocks", "Inspect blocks"),
            ("read_semantic_blocks", "Read blocks"),
        ),
    ),
    (
        "view",
        "VIEW",
        (
            ("toggle_preview", "Preview"),
            ("manage_recovery", "Recovery drafts"),
            ("show_help", "Shortcut help"),
            ("show_markdown_help", "Markdown help"),
            ("inspect_cursor_coordinates", "Cursor coordinates"),
            ("request_quit", "Quit"),
        ),
    ),
)


class _CommandGroupList(OptionList, can_focus=False):
    """A visible palette group that leaves keyboard focus in search."""


class GroupedCommandPalette(CommandPalette):
    """Show command discovery as a compact two-column cheatsheet."""

    _COMPACT_WIDTH = 58

    def __init__(
        self,
        providers: ProviderSource | None = None,
        *,
        placeholder: str = "Search commands…",
        name: str | None = None,
        id: str | None = None,
        classes: str | None = None,
    ) -> None:
        super().__init__(
            providers,
            placeholder=placeholder,
            name=name,
            id=id,
            classes=classes,
        )
        self._source_actions: list[str] = []
        self._locations: dict[str, tuple[str, int]] = {}
        self._descriptions: dict[str, str] = {}

    def compose(self) -> ComposeResult:
        """Compose the search engine and its grouped visual projection."""
        with Vertical(id="--container"):
            with Horizontal(id="--input"):
                yield SearchIcon()
                yield CommandInput(placeholder=self._placeholder, select_on_focus=False)
            with VerticalScroll(id="--results"):
                yield CommandList(id="--command-source")
                with Grid(id="--command-groups"):
                    for group_id, title, _ in _COMMAND_GROUPS:
                        with Vertical(
                            id=f"--command-{group_id}-group",
                            classes="command-group",
                        ):
                            yield Static(title, classes="command-group-title")
                            yield _CommandGroupList(
                                id=f"--command-{group_id}",
                                classes="command-group-list",
                                compact=True,
                            )
                yield Static("No matching commands", id="--command-empty")
                yield LoadingIndicator()
            yield Static(id="--command-description")

    def _on_mount(self, event: events.Mount) -> None:
        self._set_compact(self.size.width)
        super()._on_mount(event)

    async def _on_resize(self, event: events.Resize) -> None:
        self._set_compact(event.size.width)
        await super()._on_resize(event)

    def _set_compact(self, width: int) -> None:
        self.set_class(width < self._COMPACT_WIDTH, "-compact")

    def _refresh_command_list(
        self,
        command_list: CommandList,
        commands: list[Command],
        clear_current: bool,
    ) -> None:
        """Project Textual's matched commands into the six visual groups."""
        del clear_current
        by_action = {
            action: command for command in commands if (action := self._action_name(command))
        }
        ordered_commands: list[Command] = []
        self._source_actions = []
        self._locations.clear()
        self._descriptions.clear()

        for group_id, _, specs in _COMMAND_GROUPS:
            group_list = self.query_one(f"#--command-{group_id}", OptionList)
            group_list.clear_options()
            group_options: list[Command] = []
            for action, label in specs:
                command = by_action.get(action)
                if command is None:
                    continue
                shortcut, description = self._shortcut_and_description(command)
                prompt = Text()
                prompt.append(f"{shortcut}  ", style="bold")
                prompt.append(label)
                group_options.append(Command(prompt, command.hit, id=action))
                self._locations[action] = (group_id, len(group_options) - 1)
                self._descriptions[action] = description
                self._source_actions.append(action)
                ordered_commands.append(command)
            group_list.add_options(group_options)
            group_list.highlighted = None
            self.query_one(f"#--command-{group_id}-group").display = bool(group_options)

        command_list.clear_options().add_options(ordered_commands)
        command_list.highlighted = 0 if ordered_commands else None
        self._list_visible = bool(ordered_commands)
        self._hit_count = len(ordered_commands)
        self.call_after_refresh(self._sync_visible_highlight)

    @staticmethod
    def _action_name(command: Command) -> str:
        callback_name = getattr(command.hit.command, "__name__", "")
        return callback_name.removeprefix("action_")

    @staticmethod
    def _shortcut_and_description(command: Command) -> tuple[str, str]:
        help_text = command.hit.help or ""
        key_text, separator, description = help_text.partition("  ·  ")
        shortcut = key_text.removeprefix("Keys: ").strip()
        return shortcut, description if separator else help_text

    def _sync_visible_highlight(self) -> None:
        if not self.is_mounted:
            return
        source = self.query_one("#--command-source", CommandList)
        source_index = source.highlighted
        if source_index is None or source_index >= len(self._source_actions):
            return
        action = self._source_actions[source_index]
        with self.prevent(OptionList.OptionHighlighted):
            for group_id, _, _ in _COMMAND_GROUPS:
                self.query_one(f"#--command-{group_id}", OptionList).highlighted = None
            group_id, option_index = self._locations[action]
            self.query_one(f"#--command-{group_id}", OptionList).highlighted = option_index
        self.query_one("#--command-description", Static).update(self._descriptions[action])

    def _action_command_list(self, action: str) -> None:
        super()._action_command_list(action)
        if action != "select":
            self.call_after_refresh(self._sync_visible_highlight)

    def _action_cursor_down(self) -> None:
        super()._action_cursor_down()
        self.call_after_refresh(self._sync_visible_highlight)

    def _watch__list_visible(self) -> None:
        super()._watch__list_visible()
        if not self.is_mounted:
            return
        has_commands = bool(self._source_actions)
        self.query_one("#--command-groups").display = self._list_visible and has_commands
        self.query_one("#--command-description").display = self._list_visible and has_commands
        self.query_one("#--command-empty").display = self._list_visible and not has_commands
