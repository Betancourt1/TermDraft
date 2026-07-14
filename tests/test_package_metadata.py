"""Tests for the installed package metadata contract."""

from __future__ import annotations

import tomllib
from pathlib import Path

from termdraft import __version__


def test_package_version_matches_pyproject() -> None:
    pyproject_path = Path(__file__).parents[1] / "pyproject.toml"
    pyproject = tomllib.loads(pyproject_path.read_text(encoding="utf-8"))

    assert pyproject["project"]["version"] == __version__
    assert pyproject["project"]["name"] == "termdraft"
    assert pyproject["project"]["scripts"] == {
        "termdraft": "termdraft.cli:main",
        "termdraft-benchmark": "termdraft.benchmark:main",
    }
