"""Strict, non-executable user configuration for TermDraft."""

from __future__ import annotations

import os
import stat
import tomllib
import unicodedata
from collections.abc import Mapping
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from types import MappingProxyType
from typing import Literal, cast

CONFIG_HOME_ENV = "TERMDRAFT_CONFIG_HOME"
LEGACY_CONFIG_HOME_ENV = "TERMWRITER_CONFIG_HOME"
CONFIG_DIRECTORY_NAME = ".termdraft"
LEGACY_CONFIG_DIRECTORY_NAME = ".termwriter"
CONFIG_FILE_NAME = "config.toml"
THEME_FILE_NAME = "theme.tcss"

BINDING_ID_SAVE = "save"
BINDING_ID_SAVE_AS = "save_as"
BINDING_ID_QUIT = "quit"
BINDING_ID_TOGGLE_EXPLORER = "toggle_explorer"
BINDING_ID_FIND_FILE = "find_file"
BINDING_ID_RECENT_DOCUMENTS = "recent_documents"
BINDING_ID_NEXT_TAB = "next_tab"
BINDING_ID_PREVIOUS_TAB = "previous_tab"
BINDING_ID_CLOSE_TAB = "close_tab"
BINDING_ID_FIND_REPLACE = "find_replace"
BINDING_ID_SEARCH_TEXT = "search_text"
BINDING_ID_DOCUMENT_OUTLINE = "document_outline"
BINDING_ID_TOGGLE_PREVIEW = "toggle_preview"
BINDING_ID_PREVIEW_NEXT_HEADING = "preview_next_heading"
BINDING_ID_PREVIEW_PREVIOUS_HEADING = "preview_previous_heading"
BINDING_ID_UNDO = "undo"
BINDING_ID_REDO = "redo"
BINDING_ID_SHOW_HELP = "show_help"
BINDING_ID_COMMAND_PALETTE = "command_palette"
BINDING_ID_COMMAND_WRITE_MODE = "command_write_mode"
BINDING_ID_COMMAND_SAVE = "command_save"
BINDING_ID_COMMAND_SAVE_AS = "command_save_as"
BINDING_ID_COMMAND_DUPLICATE_DOCUMENT = "command_duplicate_document"
BINDING_ID_COMMAND_QUIT = "command_quit"
BINDING_ID_COMMAND_TOGGLE_EXPLORER = "command_toggle_explorer"
BINDING_ID_COMMAND_FIND_FILE = "command_find_file"
BINDING_ID_COMMAND_RECENT_DOCUMENTS = "command_recent_documents"
BINDING_ID_COMMAND_NEXT_TAB = "command_next_tab"
BINDING_ID_COMMAND_PREVIOUS_TAB = "command_previous_tab"
BINDING_ID_COMMAND_CLOSE_TAB = "command_close_tab"
BINDING_ID_COMMAND_SEARCH_TEXT = "command_search_text"
BINDING_ID_COMMAND_FIND_REPLACE = "command_find_replace"
BINDING_ID_COMMAND_DOCUMENT_OUTLINE = "command_document_outline"
BINDING_ID_COMMAND_TOGGLE_PREVIEW = "command_toggle_preview"
BINDING_ID_COMMAND_UNDO = "command_undo"
BINDING_ID_COMMAND_REDO = "command_redo"
BINDING_ID_COMMAND_RELOAD_CONFIG = "command_reload_config"
BINDING_ID_COMMAND_MANAGE_RECOVERY = "command_manage_recovery"
BINDING_ID_COMMAND_MARKDOWN_HELP = "command_markdown_help"
BINDING_ID_COMMAND_INSPECT_SEMANTIC_BLOCKS = "command_inspect_semantic_blocks"
BINDING_ID_COMMAND_READ_SEMANTIC_BLOCKS = "command_read_semantic_blocks"
BINDING_ID_COMMAND_INSPECT_CURSOR_COORDINATES = "command_inspect_cursor_coordinates"
BINDING_ID_COMMAND_OPEN_PALETTE = "command_open_palette"
BINDING_ID_COMMAND_SHOW_HELP = "command_show_help"
BINDING_ID_COMMAND_CURSOR_LEFT = "command_cursor_left"
BINDING_ID_COMMAND_CURSOR_DOWN = "command_cursor_down"
BINDING_ID_COMMAND_CURSOR_UP = "command_cursor_up"
BINDING_ID_COMMAND_CURSOR_RIGHT = "command_cursor_right"
BINDING_ID_COMMAND_LINE_START = "command_line_start"
BINDING_ID_COMMAND_LINE_END = "command_line_end"
BINDING_ID_COMMAND_DOCUMENT_START = "command_document_start"
BINDING_ID_COMMAND_DOCUMENT_END = "command_document_end"

DEFAULT_KEYBINDINGS: Mapping[str, str] = MappingProxyType(
    {
        BINDING_ID_SAVE: "ctrl+s",
        BINDING_ID_SAVE_AS: "ctrl+shift+s",
        BINDING_ID_QUIT: "ctrl+q",
        BINDING_ID_TOGGLE_EXPLORER: "ctrl+b",
        BINDING_ID_FIND_FILE: "ctrl+p",
        BINDING_ID_RECENT_DOCUMENTS: "ctrl+o",
        BINDING_ID_NEXT_TAB: "ctrl+pagedown",
        BINDING_ID_PREVIOUS_TAB: "ctrl+pageup",
        BINDING_ID_CLOSE_TAB: "ctrl+f4",
        BINDING_ID_FIND_REPLACE: "ctrl+f",
        BINDING_ID_SEARCH_TEXT: "ctrl+shift+f",
        BINDING_ID_DOCUMENT_OUTLINE: "ctrl+shift+o",
        BINDING_ID_TOGGLE_PREVIEW: "ctrl+e",
        BINDING_ID_PREVIEW_NEXT_HEADING: "alt+down",
        BINDING_ID_PREVIEW_PREVIOUS_HEADING: "alt+up",
        BINDING_ID_UNDO: "ctrl+z,super+z",
        BINDING_ID_REDO: "ctrl+y,super+y,ctrl+shift+z",
        BINDING_ID_SHOW_HELP: "f1",
        BINDING_ID_COMMAND_PALETTE: "ctrl+backslash",
        BINDING_ID_COMMAND_WRITE_MODE: "i",
        BINDING_ID_COMMAND_SAVE: "w",
        BINDING_ID_COMMAND_SAVE_AS: "W",
        BINDING_ID_COMMAND_DUPLICATE_DOCUMENT: "D",
        BINDING_ID_COMMAND_QUIT: "q",
        BINDING_ID_COMMAND_TOGGLE_EXPLORER: "e",
        BINDING_ID_COMMAND_FIND_FILE: "f",
        BINDING_ID_COMMAND_RECENT_DOCUMENTS: "o",
        BINDING_ID_COMMAND_NEXT_TAB: "]",
        BINDING_ID_COMMAND_PREVIOUS_TAB: "[",
        BINDING_ID_COMMAND_CLOSE_TAB: "C",
        BINDING_ID_COMMAND_SEARCH_TEXT: "slash",
        BINDING_ID_COMMAND_FIND_REPLACE: "s",
        BINDING_ID_COMMAND_DOCUMENT_OUTLINE: "S",
        BINDING_ID_COMMAND_TOGGLE_PREVIEW: "v",
        BINDING_ID_COMMAND_UNDO: "u",
        BINDING_ID_COMMAND_REDO: "U",
        BINDING_ID_COMMAND_RELOAD_CONFIG: "R",
        BINDING_ID_COMMAND_MANAGE_RECOVERY: "M",
        BINDING_ID_COMMAND_MARKDOWN_HELP: "K",
        BINDING_ID_COMMAND_INSPECT_SEMANTIC_BLOCKS: "b",
        BINDING_ID_COMMAND_READ_SEMANTIC_BLOCKS: "B",
        BINDING_ID_COMMAND_INSPECT_CURSOR_COORDINATES: "I",
        BINDING_ID_COMMAND_OPEN_PALETTE: "colon",
        BINDING_ID_COMMAND_SHOW_HELP: "question_mark",
        BINDING_ID_COMMAND_CURSOR_LEFT: "h",
        BINDING_ID_COMMAND_CURSOR_DOWN: "j",
        BINDING_ID_COMMAND_CURSOR_UP: "k",
        BINDING_ID_COMMAND_CURSOR_RIGHT: "l",
        BINDING_ID_COMMAND_LINE_START: "0",
        BINDING_ID_COMMAND_LINE_END: "dollar_sign",
        BINDING_ID_COMMAND_DOCUMENT_START: "g",
        BINDING_ID_COMMAND_DOCUMENT_END: "G",
    }
)
KNOWN_BINDING_IDS = frozenset(DEFAULT_KEYBINDINGS)
RESERVED_PREVIEW_KEYS = frozenset({"tab", "shift+tab", "enter"})

_CHARACTER_KEY_REPLACEMENTS = {
    "solidus": "slash",
    "reverse_solidus": "backslash",
    "commercial_at": "at",
    "hyphen_minus": "minus",
    "plus_sign": "plus",
    "low_line": "underscore",
}

CONFIG_TEMPLATE = """\
# TermDraft configuration. Unknown options are rejected instead of ignored.

[editor]
auto_continue_lists = true
soft_wrap = true
show_line_numbers = true
# Applied on the next launch: "command" or "write".
startup_mode = "command"
# "inline" previews every line except the cursor line. Use "split" for two panes.
view_mode = "inline"

[recovery]
# Used only when you explicitly choose age-based cleanup in Recovery Manager.
retention_days = 30

[keybindings]
# Bindings override keys only. They cannot define actions or commands.
# save = "ctrl+s"
# save_as = "ctrl+shift+s"
# quit = "ctrl+q"
# toggle_explorer = "ctrl+b"
# find_file = "ctrl+p"
# recent_documents = "ctrl+o"
# next_tab = "ctrl+pagedown"
# previous_tab = "ctrl+pageup"
# close_tab = "ctrl+f4"
# find_replace = "ctrl+f"
# search_text = "ctrl+shift+f"
# document_outline = "ctrl+shift+o"
# toggle_preview = "ctrl+e"
# preview_next_heading = "alt+down"
# preview_previous_heading = "alt+up"
# Tab, Shift+Tab, and Enter remain reserved for preview link controls.
# undo = "ctrl+z,super+z"
# redo = "ctrl+y,super+y,ctrl+shift+z"
# show_help = "f1"
# command_palette = "ctrl+backslash"
# Single-key COMMAND bindings are remappable too.
# command_write_mode = "i"
# command_save = "w"
# command_save_as = "W"
# command_duplicate_document = "D"
# command_quit = "q"
# command_toggle_explorer = "e"
# command_find_file = "f"
# command_recent_documents = "o"
# command_next_tab = "]"
# command_previous_tab = "["
# command_close_tab = "C"
# command_search_text = "slash"
# command_find_replace = "s"
# command_document_outline = "S"
# command_toggle_preview = "v"
# command_undo = "u"
# command_redo = "U"
# command_reload_config = "R"
# command_manage_recovery = "M"
# command_markdown_help = "K"
# command_inspect_semantic_blocks = "b"
# command_read_semantic_blocks = "B"
# command_inspect_cursor_coordinates = "I"
# command_open_palette = "colon"
# command_show_help = "question_mark"
# command_cursor_left = "h"
# command_cursor_down = "j"
# command_cursor_up = "k"
# command_cursor_right = "l"
# command_line_start = "0"
# command_line_end = "dollar_sign"
# command_document_start = "g"
# command_document_end = "G"
"""

THEME_TEMPLATE = """\
/* TermDraft user theme overrides.

   This file uses Textual CSS (TCSS), not browser CSS. For example:

   #status-bar {
       background: $primary-darken-2;
   }
*/
"""


class ConfigError(Exception):
    """A configuration path, file, or value is invalid."""


@dataclass(frozen=True, slots=True)
class EditorConfig:
    """Editor behavior controlled by safe boolean options."""

    auto_continue_lists: bool = True
    soft_wrap: bool = True
    show_line_numbers: bool = True
    startup_mode: Literal["command", "write"] = "command"
    view_mode: Literal["inline", "split"] = "inline"


@dataclass(frozen=True, slots=True)
class RecoveryConfig:
    """Manual recovery-retention behavior."""

    retention_days: int = 30


@dataclass(frozen=True, slots=True)
class TermDraftConfig:
    """Fully resolved configuration, including defaults."""

    root: Path
    editor: EditorConfig = field(default_factory=EditorConfig)
    recovery: RecoveryConfig = field(default_factory=RecoveryConfig)
    keybindings: Mapping[str, str] = field(
        default_factory=lambda: MappingProxyType(dict(DEFAULT_KEYBINDINGS))
    )

    @property
    def config_path(self) -> Path:
        """Path to the TOML configuration file."""
        return self.root / CONFIG_FILE_NAME

    @property
    def theme_path(self) -> Path:
        """Fixed path to user Textual CSS overrides."""
        return self.root / THEME_FILE_NAME


def get_config_root(
    explicit_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
) -> Path:
    """Resolve the configuration root, including the pre-1.0 legacy locations."""
    if explicit_root is not None:
        return explicit_root.expanduser().absolute()

    environment = os.environ if environ is None else environ
    for environment_name in (CONFIG_HOME_ENV, LEGACY_CONFIG_HOME_ENV):
        environment_root = environment.get(environment_name)
        if environment_root is None:
            continue
        if not environment_root.strip():
            raise ConfigError(f"{environment_name} must not be empty")
        return Path(environment_root).expanduser().absolute()

    canonical_root = Path.home() / CONFIG_DIRECTORY_NAME
    legacy_root = Path.home() / LEGACY_CONFIG_DIRECTORY_NAME
    if canonical_root.exists():
        return canonical_root.absolute()
    if legacy_root.exists():
        return legacy_root.absolute()
    return canonical_root.absolute()


def load_config(
    explicit_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
) -> TermDraftConfig:
    """Load strict TOML configuration and apply all defaults."""
    root = get_config_root(explicit_root, environ=environ)
    config_path = root / CONFIG_FILE_NAME
    try:
        with config_path.open("rb") as config_file:
            raw_config: object = tomllib.load(config_file)
    except FileNotFoundError:
        return TermDraftConfig(root=root)
    except (OSError, tomllib.TOMLDecodeError, UnicodeDecodeError) as error:
        raise ConfigError(f"cannot read {config_path}: {error}") from error

    config = _as_table(raw_config, "configuration root")
    unknown_sections = set(config) - {"editor", "recovery", "keybindings"}
    if unknown_sections:
        raise ConfigError(
            f"unknown configuration section or key: {_format_names(unknown_sections)}"
        )

    editor = _parse_editor(config.get("editor", {}))
    recovery = _parse_recovery(config.get("recovery", {}))
    keybindings = _parse_keybindings(config.get("keybindings", {}))
    return TermDraftConfig(
        root=root,
        editor=editor,
        recovery=recovery,
        keybindings=keybindings,
    )


def initialize_config(
    explicit_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
) -> TermDraftConfig:
    """Create missing configuration files without replacing existing content."""
    root = get_config_root(explicit_root, environ=environ)
    _ensure_config_directory(root)
    _create_file_exclusively(root / CONFIG_FILE_NAME, CONFIG_TEMPLATE)
    _create_file_exclusively(root / THEME_FILE_NAME, THEME_TEMPLATE)
    return load_config(root)


def _parse_editor(raw_editor: object) -> EditorConfig:
    editor = _as_table(raw_editor, "editor")
    allowed_keys = {
        "auto_continue_lists",
        "soft_wrap",
        "show_line_numbers",
        "startup_mode",
        "view_mode",
    }
    unknown_keys = set(editor) - allowed_keys
    if unknown_keys:
        raise ConfigError(f"unknown editor option: {_format_names(unknown_keys)}")

    values = {
        "auto_continue_lists": True,
        "soft_wrap": True,
        "show_line_numbers": True,
    }
    for name in values:
        if name not in editor:
            continue
        value = editor[name]
        if not isinstance(value, bool):
            raise ConfigError(f"editor.{name} must be true or false")
        values[name] = value

    startup_mode = editor.get("startup_mode", "command")
    if not isinstance(startup_mode, str) or startup_mode not in {"command", "write"}:
        raise ConfigError('editor.startup_mode must be "command" or "write"')

    view_mode = editor.get("view_mode", "inline")
    if not isinstance(view_mode, str) or view_mode not in {"inline", "split"}:
        raise ConfigError('editor.view_mode must be "inline" or "split"')

    return EditorConfig(
        auto_continue_lists=values["auto_continue_lists"],
        soft_wrap=values["soft_wrap"],
        show_line_numbers=values["show_line_numbers"],
        startup_mode=cast(Literal["command", "write"], startup_mode),
        view_mode=cast(Literal["inline", "split"], view_mode),
    )


def _parse_recovery(raw_recovery: object) -> RecoveryConfig:
    recovery = _as_table(raw_recovery, "recovery")
    unknown_keys = set(recovery) - {"retention_days"}
    if unknown_keys:
        raise ConfigError(f"unknown recovery option: {_format_names(unknown_keys)}")
    retention_days = recovery.get("retention_days", 30)
    if (
        not isinstance(retention_days, int)
        or isinstance(retention_days, bool)
        or retention_days < 1
    ):
        raise ConfigError("recovery.retention_days must be a positive integer")
    maximum_days = (datetime.now(UTC) - datetime.min.replace(tzinfo=UTC)).days
    if retention_days > maximum_days:
        raise ConfigError("recovery.retention_days is too large for the current date")
    return RecoveryConfig(retention_days=retention_days)


def _parse_keybindings(raw_keybindings: object) -> Mapping[str, str]:
    overrides = _as_table(raw_keybindings, "keybindings")
    unknown_ids = set(overrides) - KNOWN_BINDING_IDS
    if unknown_ids:
        raise ConfigError(f"unknown keybinding id: {_format_names(unknown_ids)}")

    effective = dict(DEFAULT_KEYBINDINGS)
    for binding_id, raw_binding in overrides.items():
        if not isinstance(raw_binding, str):
            raise ConfigError(f"keybindings.{binding_id} must be a string")
        tokens = [token.strip() for token in raw_binding.split(",")]
        if not tokens or any(not token for token in tokens):
            raise ConfigError(f"keybindings.{binding_id} must contain non-empty key names")
        effective[binding_id] = ",".join(tokens)

    used_tokens: dict[str, str] = {}
    for binding_id, binding in effective.items():
        for token in binding.split(","):
            collision_token = _normalized_character_key(token)
            if collision_token.casefold() in RESERVED_PREVIEW_KEYS:
                raise ConfigError(f"key {token!r} is reserved for preview link controls")
            previous_id = used_tokens.get(collision_token)
            if previous_id is not None:
                raise ConfigError(
                    f"key {token!r} is assigned to both {previous_id!r} and {binding_id!r}"
                )
            used_tokens[collision_token] = binding_id

    return MappingProxyType(effective)


def _normalized_character_key(token: str) -> str:
    """Match Textual's canonical names for single printable character keys."""
    if len(token) != 1 or token.isalnum():
        return token
    try:
        name = unicodedata.name(token).lower().replace("-", "_").replace(" ", "_")
    except ValueError:
        return token
    return _CHARACTER_KEY_REPLACEMENTS.get(name, name)


def _as_table(value: object, name: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ConfigError(f"{name} must be a TOML table")
    return cast(dict[str, object], value)


def _format_names(names: set[str]) -> str:
    return ", ".join(repr(name) for name in sorted(names))


def _ensure_config_directory(root: Path) -> None:
    try:
        root.mkdir(mode=0o700, parents=True, exist_ok=False)
    except FileExistsError:
        if not root.is_dir():
            raise ConfigError(f"configuration root is not a directory: {root}") from None
    except OSError as error:
        raise ConfigError(f"cannot create configuration directory {root}: {error}") from error
    else:
        try:
            root.chmod(0o700)
        except OSError as error:
            raise ConfigError(f"cannot secure configuration directory {root}: {error}") from error


def _create_file_exclusively(path: Path, content: str) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_CLOEXEC"):
        flags |= os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW

    try:
        descriptor = os.open(path, flags, 0o600)
    except FileExistsError:
        if not path.is_file():
            raise ConfigError(f"configuration path is not a regular file: {path}") from None
        return
    except OSError as error:
        raise ConfigError(f"cannot create configuration file {path}: {error}") from error

    created_stat: os.stat_result | None = None
    write_error: OSError | None = None
    try:
        created_stat = os.fstat(descriptor)
        os.fchmod(descriptor, 0o600)
        remaining = memoryview(content.encode("utf-8"))
        while remaining:
            written = os.write(descriptor, remaining)
            if written == 0:
                raise OSError("write returned zero bytes")
            remaining = remaining[written:]
        os.fsync(descriptor)
    except OSError as error:
        write_error = error
    finally:
        os.close(descriptor)

    if write_error is not None:
        if created_stat is not None:
            _remove_created_file(path, created_stat)
        raise ConfigError(f"cannot write configuration file {path}: {write_error}") from write_error

    assert created_stat is not None

    try:
        written_stat = path.stat(follow_symlinks=False)
    except OSError as error:
        raise ConfigError(f"cannot verify configuration file {path}: {error}") from error
    if not stat.S_ISREG(written_stat.st_mode) or (
        written_stat.st_dev,
        written_stat.st_ino,
    ) != (created_stat.st_dev, created_stat.st_ino):
        raise ConfigError(f"configuration file changed while it was being created: {path}")


def _remove_created_file(path: Path, created_stat: os.stat_result) -> None:
    """Best-effort cleanup without unlinking a path another process replaced."""
    try:
        current_stat = path.stat(follow_symlinks=False)
        if (current_stat.st_dev, current_stat.st_ino) == (
            created_stat.st_dev,
            created_stat.st_ino,
        ):
            path.unlink()
    except OSError:
        pass
