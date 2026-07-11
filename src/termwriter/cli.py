"""Command-line entry point for TermWriter."""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence
from pathlib import Path

from termwriter import __version__
from termwriter.app import TermWriterApp
from termwriter.models.workspace import Workspace, WorkspaceError


def build_parser() -> argparse.ArgumentParser:
    """Create the small, stable command-line interface."""
    parser = argparse.ArgumentParser(
        prog="termwriter",
        description="Edit Markdown files in a local terminal workspace.",
    )
    parser.add_argument(
        "target", nargs="?", default=".", help="workspace directory or Markdown file"
    )
    parser.add_argument("--version", action="version", version=f"%(prog)s {__version__}")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate the target, launch the TUI, and return a shell status."""
    arguments = build_parser().parse_args(argv)
    try:
        workspace = Workspace.from_target(Path(str(arguments.target)))
    except WorkspaceError as error:
        print(f"termwriter: error: {error}", file=sys.stderr)
        return 2

    TermWriterApp(workspace).run()
    return 0
