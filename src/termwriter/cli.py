"""Command-line entry point for TermWriter."""

from __future__ import annotations

import argparse
import signal
import sys
from collections.abc import Callable, Sequence
from pathlib import Path
from types import FrameType

from termwriter import __version__
from termwriter.app import TermWriterApp
from termwriter.bindings import format_command_help
from termwriter.config import (
    ConfigError,
    get_config_root,
    initialize_config,
    load_config,
)
from termwriter.models.workspace import Workspace, WorkspaceError

SignalHandler = Callable[[int, FrameType | None], object] | int | signal.Handlers | None


def _run_with_shutdown_signals(app: TermWriterApp) -> None:
    """Forward cooperative process shutdown requests and restore prior handlers."""
    previous_handlers: list[tuple[int, SignalHandler]] = []

    def request_shutdown(signal_number: int, frame: FrameType | None) -> None:
        del frame
        app.request_orderly_shutdown(signal_number)

    shutdown_signals: list[int] = [signal.SIGTERM]
    hangup_signal = getattr(signal, "SIGHUP", None)
    if hangup_signal is not None:
        shutdown_signals.append(hangup_signal)
    try:
        for shutdown_signal in shutdown_signals:
            try:
                previous = signal.signal(shutdown_signal, request_shutdown)
            except (OSError, ValueError):
                continue
            previous_handlers.append((shutdown_signal, previous))
        app.run()
    finally:
        for installed_signal, previous in previous_handlers:
            try:
                signal.signal(installed_signal, previous)
            except (OSError, ValueError):
                pass


def build_parser() -> argparse.ArgumentParser:
    """Create the small, stable command-line interface."""
    parser = argparse.ArgumentParser(
        prog="termwriter",
        description="Edit Markdown and plain-text files in a local terminal workspace.",
    )
    parser.add_argument(
        "target", nargs="?", default=".", help="workspace directory or editable text file"
    )
    parser.add_argument(
        "--config-dir",
        type=Path,
        help="configuration directory (default: ~/.termwriter)",
    )
    parser.add_argument(
        "--safe-mode",
        action="store_true",
        help="ignore theme.tcss for this launch",
    )
    utilities = parser.add_mutually_exclusive_group()
    utilities.add_argument(
        "--init-config",
        action="store_true",
        help="create no-clobber config.toml and theme.tcss templates, then exit",
    )
    utilities.add_argument(
        "--config-path",
        action="store_true",
        help="print the resolved configuration paths, then exit",
    )
    utilities.add_argument(
        "--commands",
        action="store_true",
        help="show effective shortcuts and command-palette actions, then exit",
    )
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate the target, launch the TUI, and return a shell status."""
    arguments = build_parser().parse_args(argv)
    try:
        config_root = get_config_root(arguments.config_dir)
        if arguments.init_config:
            config = initialize_config(config_root)
            print(f"Configuration: {config.config_path}")
            print(f"Theme:         {config.theme_path}")
            return 0
        if arguments.config_path:
            print(config_root / "config.toml")
            print(config_root / "theme.tcss")
            return 0
        config = load_config(config_root)
    except ConfigError as error:
        print(f"termwriter: configuration error: {error}", file=sys.stderr)
        return 2

    if arguments.commands:
        print(
            format_command_help(
                config.keybindings,
                auto_continue_lists=config.editor.auto_continue_lists,
            )
        )
        return 0

    try:
        workspace = Workspace.from_target(Path(str(arguments.target)))
    except WorkspaceError as error:
        print(f"termwriter: error: {error}", file=sys.stderr)
        return 2

    _run_with_shutdown_signals(
        TermWriterApp(workspace, config=config, use_user_theme=not arguments.safe_mode)
    )
    return 0
