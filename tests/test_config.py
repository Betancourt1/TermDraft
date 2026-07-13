"""Tests for strict, safe user configuration."""

from __future__ import annotations

import os
import stat
import tomllib
from pathlib import Path

import pytest

from termwriter.config import (
    CONFIG_FILE_NAME,
    CONFIG_HOME_ENV,
    CONFIG_TEMPLATE,
    DEFAULT_KEYBINDINGS,
    KNOWN_BINDING_IDS,
    THEME_FILE_NAME,
    THEME_TEMPLATE,
    ConfigError,
    EditorConfig,
    RecoveryConfig,
    get_config_root,
    initialize_config,
    load_config,
)


def test_config_root_defaults_to_dot_directory() -> None:
    assert get_config_root(environ={}) == Path.home() / ".termwriter"


def test_config_root_honors_environment_and_explicit_path(tmp_path: Path) -> None:
    environment_root = tmp_path / "environment"
    explicit_root = tmp_path / "explicit"
    environment = {CONFIG_HOME_ENV: str(environment_root)}

    assert get_config_root(environ=environment) == environment_root
    assert get_config_root(explicit_root, environ=environment) == explicit_root


def test_relative_config_roots_become_absolute(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.chdir(tmp_path)

    assert get_config_root(Path("explicit"), environ={}) == tmp_path / "explicit"
    assert get_config_root(environ={CONFIG_HOME_ENV: "environment"}) == tmp_path / "environment"


def test_empty_environment_root_is_rejected() -> None:
    with pytest.raises(ConfigError, match="must not be empty"):
        get_config_root(environ={CONFIG_HOME_ENV: "  "})


def test_missing_config_returns_effective_defaults(tmp_path: Path) -> None:
    config = load_config(tmp_path / "missing")

    assert config.editor == EditorConfig()
    assert config.recovery == RecoveryConfig()
    assert config.keybindings == DEFAULT_KEYBINDINGS
    assert set(config.keybindings) == KNOWN_BINDING_IDS
    assert config.config_path == tmp_path / "missing" / CONFIG_FILE_NAME
    assert config.theme_path == tmp_path / "missing" / THEME_FILE_NAME


def test_config_applies_editor_and_keybinding_overrides(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text(
        """\
[editor]
auto_continue_lists = false
show_line_numbers = false
startup_mode = "write"

[recovery]
retention_days = 45

[keybindings]
save = "ctrl+shift+s"
search_text = "ctrl+g"
preview_next_heading = "ctrl+n"
redo = "ctrl+r, ctrl+shift+r"
command_cursor_left = "a"
""",
        encoding="utf-8",
    )

    config = load_config(root)

    assert config.editor == EditorConfig(
        auto_continue_lists=False,
        soft_wrap=True,
        show_line_numbers=False,
        startup_mode="write",
    )
    assert config.recovery == RecoveryConfig(retention_days=45)
    assert config.keybindings["save"] == "ctrl+shift+s"
    assert config.keybindings["search_text"] == "ctrl+g"
    assert config.keybindings["preview_next_heading"] == "ctrl+n"
    assert config.keybindings["redo"] == "ctrl+r,ctrl+shift+r"
    assert config.keybindings["command_cursor_left"] == "a"
    assert config.keybindings["quit"] == "ctrl+q"


@pytest.mark.parametrize(
    ("content", "message"),
    [
        ("unknown = true\n", "unknown configuration section or key"),
        ("[other]\nvalue = true\n", "unknown configuration section or key"),
        ("editor = true\n", "editor must be a TOML table"),
        ("[editor]\nunknown = true\n", "unknown editor option"),
        ("[editor]\nsoft_wrap = 1\n", "editor.soft_wrap must be true or false"),
        ('[editor]\nstartup_mode = "insert"\n', "editor.startup_mode"),
        ("[editor]\nstartup_mode = false\n", "editor.startup_mode"),
        ("recovery = true\n", "recovery must be a TOML table"),
        ("[recovery]\nunknown = 1\n", "unknown recovery option"),
        ("[recovery]\nretention_days = 0\n", "must be a positive integer"),
        ("[recovery]\nretention_days = true\n", "must be a positive integer"),
        ("[recovery]\nretention_days = 999999999\n", "too large for the current date"),
        ("keybindings = false\n", "keybindings must be a TOML table"),
        ('[keybindings]\nunknown = "ctrl+x"\n', "unknown keybinding id"),
        ("[keybindings]\nsave = true\n", "keybindings.save must be a string"),
        ('[keybindings]\nsave = ""\n', "must contain non-empty key names"),
        ('[keybindings]\nsave = "ctrl+x, "\n', "must contain non-empty key names"),
    ],
)
def test_invalid_config_values_are_rejected(tmp_path: Path, content: str, message: str) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text(content, encoding="utf-8")

    with pytest.raises(ConfigError, match=message):
        load_config(root)


@pytest.mark.parametrize(
    "binding",
    ["ctrl+x,ctrl+x", "ctrl+x, ctrl+x"],
)
def test_duplicate_tokens_within_one_binding_are_rejected(tmp_path: Path, binding: str) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text(f'[keybindings]\nsave = "{binding}"\n', encoding="utf-8")

    with pytest.raises(ConfigError, match="assigned to both 'save' and 'save'"):
        load_config(root)


def test_duplicate_tokens_across_effective_bindings_are_rejected(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text('[keybindings]\nsave = "ctrl+q"\n', encoding="utf-8")

    with pytest.raises(ConfigError, match="assigned to both 'save' and 'quit'"):
        load_config(root)


def test_command_navigation_key_collisions_are_rejected(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text(
        '[keybindings]\ncommand_cursor_left = "l"\n',
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match=r"command_cursor_left.*command_cursor_right"):
        load_config(root)


def test_duplicate_character_aliases_are_rejected(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text(
        '[keybindings]\nsave = "?"\nquit = "question_mark"\n',
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="assigned to both 'save' and 'quit'"):
        load_config(root)


@pytest.mark.parametrize("binding", ["tab", "shift+tab", "enter", "TAB"])
def test_preview_link_control_keys_are_reserved(tmp_path: Path, binding: str) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text(
        f'[keybindings]\npreview_next_heading = "{binding}"\n',
        encoding="utf-8",
    )

    with pytest.raises(ConfigError, match="reserved for preview link controls"):
        load_config(root)


def test_invalid_toml_and_unreadable_shape_raise_clear_errors(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).write_text("[editor\n", encoding="utf-8")

    with pytest.raises(ConfigError, match="cannot read"):
        load_config(root)

    (root / CONFIG_FILE_NAME).unlink()
    (root / CONFIG_FILE_NAME).mkdir()
    with pytest.raises(ConfigError, match="cannot read"):
        load_config(root)


def test_templates_are_valid_and_initialization_is_private(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"

    config = initialize_config(root)

    assert config.editor == EditorConfig()
    assert config.recovery == RecoveryConfig()
    assert (root / CONFIG_FILE_NAME).read_text(encoding="utf-8") == CONFIG_TEMPLATE
    assert (root / THEME_FILE_NAME).read_text(encoding="utf-8") == THEME_TEMPLATE
    assert stat.S_IMODE(root.stat().st_mode) == 0o700
    assert stat.S_IMODE((root / CONFIG_FILE_NAME).stat().st_mode) == 0o600
    assert stat.S_IMODE((root / THEME_FILE_NAME).stat().st_mode) == 0o600
    assert tomllib.loads(CONFIG_TEMPLATE)["editor"]["soft_wrap"] is True
    assert tomllib.loads(CONFIG_TEMPLATE)["recovery"]["retention_days"] == 30


def test_initialization_does_not_replace_existing_files(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir(mode=0o750)
    config_path = root / CONFIG_FILE_NAME
    theme_path = root / THEME_FILE_NAME
    config_content = "[editor]\nsoft_wrap = false\n"
    theme_content = "#status-bar { color: red; }\n"
    config_path.write_text(config_content, encoding="utf-8")
    theme_path.write_text(theme_content, encoding="utf-8")
    config_path.chmod(0o640)
    theme_path.chmod(0o644)

    config = initialize_config(root)

    assert config.editor.soft_wrap is False
    assert config_path.read_text(encoding="utf-8") == config_content
    assert theme_path.read_text(encoding="utf-8") == theme_content
    assert stat.S_IMODE(config_path.stat().st_mode) == 0o640
    assert stat.S_IMODE(theme_path.stat().st_mode) == 0o644


def test_initialization_fills_only_missing_file(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    config_path = root / CONFIG_FILE_NAME
    config_path.write_text("[editor]\nsoft_wrap = false\n", encoding="utf-8")

    initialize_config(root)

    assert config_path.read_text(encoding="utf-8") == "[editor]\nsoft_wrap = false\n"
    assert (root / THEME_FILE_NAME).read_text(encoding="utf-8") == THEME_TEMPLATE


def test_initialization_rejects_non_directory_root(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.write_text("not a directory", encoding="utf-8")

    with pytest.raises(ConfigError, match="not a directory"):
        initialize_config(root)


def test_initialization_rejects_non_file_destination(tmp_path: Path) -> None:
    root = tmp_path / ".termwriter"
    root.mkdir()
    (root / CONFIG_FILE_NAME).mkdir()

    with pytest.raises(ConfigError, match="not a regular file"):
        initialize_config(root)


def test_failed_initial_write_removes_partial_file(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = tmp_path / ".termwriter"

    def fail_write(_descriptor: int, _content: object) -> int:
        raise OSError("interrupted write")

    monkeypatch.setattr(os, "write", fail_write)

    with pytest.raises(ConfigError, match="interrupted write"):
        initialize_config(root)

    assert not (root / CONFIG_FILE_NAME).exists()
