"""Strict, non-executable user configuration for TermWriter."""

from __future__ import annotations

import os
import stat
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import cast

CONFIG_HOME_ENV = "TERMWRITER_CONFIG_HOME"
CONFIG_DIRECTORY_NAME = ".termwriter"
CONFIG_FILE_NAME = "config.toml"
THEME_FILE_NAME = "theme.tcss"

BINDING_ID_SAVE = "save"
BINDING_ID_QUIT = "quit"
BINDING_ID_TOGGLE_EXPLORER = "toggle_explorer"
BINDING_ID_FIND_FILE = "find_file"
BINDING_ID_TOGGLE_PREVIEW = "toggle_preview"
BINDING_ID_UNDO = "undo"
BINDING_ID_REDO = "redo"
BINDING_ID_SHOW_HELP = "show_help"
BINDING_ID_COMMAND_PALETTE = "command_palette"

DEFAULT_KEYBINDINGS: Mapping[str, str] = MappingProxyType(
    {
        BINDING_ID_SAVE: "ctrl+s",
        BINDING_ID_QUIT: "ctrl+q",
        BINDING_ID_TOGGLE_EXPLORER: "ctrl+b",
        BINDING_ID_FIND_FILE: "ctrl+p",
        BINDING_ID_TOGGLE_PREVIEW: "ctrl+e",
        BINDING_ID_UNDO: "ctrl+z,super+z",
        BINDING_ID_REDO: "ctrl+y,super+y,ctrl+shift+z",
        BINDING_ID_SHOW_HELP: "f1",
        BINDING_ID_COMMAND_PALETTE: "ctrl+backslash",
    }
)
KNOWN_BINDING_IDS = frozenset(DEFAULT_KEYBINDINGS)

CONFIG_TEMPLATE = """\
# TermWriter configuration. Unknown options are rejected instead of ignored.

[editor]
auto_continue_lists = true
soft_wrap = true
show_line_numbers = true

[keybindings]
# Bindings override keys only. They cannot define actions or commands.
# save = "ctrl+s"
# quit = "ctrl+q"
# toggle_explorer = "ctrl+b"
# find_file = "ctrl+p"
# toggle_preview = "ctrl+e"
# undo = "ctrl+z,super+z"
# redo = "ctrl+y,super+y,ctrl+shift+z"
# show_help = "f1"
# command_palette = "ctrl+backslash"
"""

THEME_TEMPLATE = """\
/* TermWriter user theme overrides.

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


@dataclass(frozen=True, slots=True)
class TermWriterConfig:
    """Fully resolved configuration, including defaults."""

    root: Path
    editor: EditorConfig = field(default_factory=EditorConfig)
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
    """Resolve the configuration root using explicit, environment, then home paths."""
    if explicit_root is not None:
        return explicit_root.expanduser()

    environment = os.environ if environ is None else environ
    environment_root = environment.get(CONFIG_HOME_ENV)
    if environment_root is not None:
        if not environment_root.strip():
            raise ConfigError(f"{CONFIG_HOME_ENV} must not be empty")
        return Path(environment_root).expanduser()

    return Path.home() / CONFIG_DIRECTORY_NAME


def load_config(
    explicit_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
) -> TermWriterConfig:
    """Load strict TOML configuration and apply all defaults."""
    root = get_config_root(explicit_root, environ=environ)
    config_path = root / CONFIG_FILE_NAME
    try:
        with config_path.open("rb") as config_file:
            raw_config: object = tomllib.load(config_file)
    except FileNotFoundError:
        return TermWriterConfig(root=root)
    except (OSError, tomllib.TOMLDecodeError, UnicodeDecodeError) as error:
        raise ConfigError(f"cannot read {config_path}: {error}") from error

    config = _as_table(raw_config, "configuration root")
    unknown_sections = set(config) - {"editor", "keybindings"}
    if unknown_sections:
        raise ConfigError(
            f"unknown configuration section or key: {_format_names(unknown_sections)}"
        )

    editor = _parse_editor(config.get("editor", {}))
    keybindings = _parse_keybindings(config.get("keybindings", {}))
    return TermWriterConfig(root=root, editor=editor, keybindings=keybindings)


def initialize_config(
    explicit_root: Path | None = None,
    *,
    environ: Mapping[str, str] | None = None,
) -> TermWriterConfig:
    """Create missing configuration files without replacing existing content."""
    root = get_config_root(explicit_root, environ=environ)
    _ensure_config_directory(root)
    _create_file_exclusively(root / CONFIG_FILE_NAME, CONFIG_TEMPLATE)
    _create_file_exclusively(root / THEME_FILE_NAME, THEME_TEMPLATE)
    return load_config(root)


def _parse_editor(raw_editor: object) -> EditorConfig:
    editor = _as_table(raw_editor, "editor")
    allowed_keys = {"auto_continue_lists", "soft_wrap", "show_line_numbers"}
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

    return EditorConfig(
        auto_continue_lists=values["auto_continue_lists"],
        soft_wrap=values["soft_wrap"],
        show_line_numbers=values["show_line_numbers"],
    )


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
            previous_id = used_tokens.get(token)
            if previous_id is not None:
                raise ConfigError(
                    f"key {token!r} is assigned to both {previous_id!r} and {binding_id!r}"
                )
            used_tokens[token] = binding_id

    return MappingProxyType(effective)


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
