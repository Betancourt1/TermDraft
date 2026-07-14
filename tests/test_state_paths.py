"""Tests for independent canonical and legacy state-directory selection."""

from __future__ import annotations

from pathlib import Path

import pytest

from termdraft.services.recovery import default_recovery_root
from termdraft.services.session import default_session_root


def test_state_roots_select_existing_leaves_independently(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    first_root = tmp_path / "first"
    (first_root / "termdraft" / "recovery").mkdir(parents=True)
    (first_root / "termwriter" / "sessions").mkdir(parents=True)
    monkeypatch.setenv("XDG_STATE_HOME", str(first_root))

    assert default_recovery_root() == first_root / "termdraft" / "recovery"
    assert default_session_root() == first_root / "termwriter" / "sessions"

    second_root = tmp_path / "second"
    (second_root / "termwriter" / "recovery").mkdir(parents=True)
    (second_root / "termdraft" / "sessions").mkdir(parents=True)
    monkeypatch.setenv("XDG_STATE_HOME", str(second_root))

    assert default_recovery_root() == second_root / "termwriter" / "recovery"
    assert default_session_root() == second_root / "termdraft" / "sessions"


def test_darwin_state_roots_use_existing_legacy_leaves(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("XDG_STATE_HOME", raising=False)
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr("termdraft.services.recovery.sys.platform", "darwin")
    monkeypatch.setattr("termdraft.services.session.sys.platform", "darwin")
    application_support = tmp_path / "Library" / "Application Support"
    legacy_recovery = application_support / "TermWriter" / "recovery"
    legacy_sessions = application_support / "TermWriter" / "sessions"
    legacy_recovery.mkdir(parents=True)
    legacy_sessions.mkdir(parents=True)

    assert default_recovery_root() == legacy_recovery
    assert default_session_root() == legacy_sessions
