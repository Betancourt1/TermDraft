"""Small, content-free workspace session storage."""

from __future__ import annotations

import hashlib
import json
import math
import os
import stat
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from termwriter.models.workspace import MARKDOWN_SUFFIXES

_SCHEMA_VERSION = 2
MAX_SESSION_BYTES = 512 * 1024
MAX_SESSION_DOCUMENTS = 100


class SessionError(Exception):
    """Raised when session state cannot be validated or stored."""


@dataclass(frozen=True, slots=True)
class DocumentViewState:
    """Cursor and scroll coordinates for one Markdown document."""

    path: Path
    line: int = 0
    column: int = 0
    scroll_x: float = 0.0
    scroll_y: float = 0.0


@dataclass(frozen=True, slots=True)
class SessionState:
    """The active tab, open tab order, and recent content-free views."""

    workspace_root: Path
    active_path: Path | None
    documents: tuple[DocumentViewState, ...] = ()
    open_paths: tuple[Path, ...] = ()

    def __post_init__(self) -> None:
        """Treat older callers with only an active path as one open tab."""
        if self.active_path is not None and not self.open_paths:
            object.__setattr__(self, "open_paths", (self.active_path,))

    def view_for(self, path: Path) -> DocumentViewState | None:
        """Return the stored view for an exact document path."""
        return next((view for view in self.documents if view.path == path), None)


@dataclass(frozen=True, slots=True)
class SessionLoadResult:
    """A safe load outcome; missing state has neither state nor warning."""

    state: SessionState | None = None
    warning: str | None = None


def default_session_root() -> Path:
    """Return a platform-appropriate directory outside user workspaces."""
    if state_home := os.environ.get("XDG_STATE_HOME"):
        return Path(state_home).expanduser() / "termwriter" / "sessions"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "TermWriter" / "sessions"
    if os.name == "nt" and (local_app_data := os.environ.get("LOCALAPPDATA")):
        return Path(local_app_data) / "TermWriter" / "sessions"
    return Path.home() / ".local" / "state" / "termwriter" / "sessions"


class SessionStore:
    """Atomically store one JSON session per canonical workspace path."""

    def __init__(self, state_root: Path | None = None) -> None:
        root = default_session_root() if state_root is None else state_root
        self.state_root = root.expanduser().absolute()

    def path_for(self, workspace_root: Path) -> Path:
        """Return the opaque state path assigned to a workspace."""
        root = _normalize_workspace(workspace_root)
        identity = hashlib.sha256(os.fsencode(root)).hexdigest()
        return self.state_root / f"{identity}.json"

    def load(self, workspace_root: Path) -> SessionLoadResult:
        """Load trusted state while treating missing or invalid state as non-fatal."""
        expected_root = _normalize_workspace(workspace_root)
        state_path = self.path_for(expected_root)
        try:
            metadata = state_path.lstat()
        except FileNotFoundError:
            return SessionLoadResult()
        except OSError as error:
            return SessionLoadResult(warning=f"Cannot read session state: {error}")

        try:
            if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
                raise SessionError("session path is not a regular file")
            if metadata.st_size > MAX_SESSION_BYTES:
                raise SessionError(f"session file exceeds {MAX_SESSION_BYTES} bytes")
            with state_path.open("rb") as state_file:
                data = state_file.read(MAX_SESSION_BYTES + 1)
            if len(data) > MAX_SESSION_BYTES:
                raise SessionError(f"session file exceeds {MAX_SESSION_BYTES} bytes")
        except OSError as error:
            return SessionLoadResult(warning=f"Cannot read session state: {error}")
        except SessionError as error:
            return SessionLoadResult(warning=f"Ignoring invalid session state: {error}")

        try:
            state = _state_from_bytes(data)
            if state.workspace_root != expected_root:
                raise SessionError("session belongs to a different workspace")
        except SessionError as error:
            return SessionLoadResult(warning=f"Ignoring invalid session state: {error}")
        return SessionLoadResult(state=state)

    def save(self, state: SessionState) -> None:
        """Validate and atomically replace a workspace session file."""
        _validate_state(state)
        data = _serialize(state)
        if len(data) > MAX_SESSION_BYTES:
            raise SessionError(f"session state exceeds {MAX_SESSION_BYTES} bytes")
        destination = self.path_for(state.workspace_root)
        temporary: Path | None = None
        descriptor = -1
        try:
            self.state_root.mkdir(mode=0o700, parents=True, exist_ok=True)
            descriptor, temporary_name = tempfile.mkstemp(
                prefix=f".{destination.stem}.",
                suffix=".tmp",
                dir=self.state_root,
            )
            temporary = Path(temporary_name)
            os.fchmod(descriptor, 0o600)
            with os.fdopen(descriptor, "wb") as stream:
                descriptor = -1
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, destination)
            temporary = None
            _sync_directory(self.state_root)
        except OSError as error:
            raise SessionError(f"Cannot save session state: {error}") from error
        finally:
            if descriptor >= 0:
                try:
                    os.close(descriptor)
                except OSError:
                    pass
            if temporary is not None:
                try:
                    temporary.unlink()
                except OSError:
                    pass


def _serialize(state: SessionState) -> bytes:
    root = state.workspace_root
    payload = {
        "version": _SCHEMA_VERSION,
        "workspace_root": str(root),
        "active_path": _relative_path(state.active_path, root),
        "open_paths": [_relative_path(path, root) for path in state.open_paths],
        "documents": [
            {
                "path": _relative_path(view.path, root),
                "line": view.line,
                "column": view.column,
                "scroll_x": view.scroll_x,
                "scroll_y": view.scroll_y,
            }
            for view in state.documents
        ],
    }
    try:
        serialized = json.dumps(
            payload,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        return (serialized + "\n").encode("utf-8")
    except (TypeError, UnicodeEncodeError, ValueError) as error:
        raise SessionError(f"Session state cannot be encoded as UTF-8 JSON: {error}") from error


def _state_from_bytes(data: bytes) -> SessionState:
    try:
        payload = json.loads(data.decode("utf-8"))
    except (RecursionError, ValueError) as error:
        raise SessionError(f"invalid UTF-8 JSON: {error}") from error
    return _state_from_payload(payload)


def _state_from_payload(payload: Any) -> SessionState:
    if not isinstance(payload, dict):
        raise SessionError("expected a JSON object")
    version = payload.get("version")
    if not isinstance(version, int) or isinstance(version, bool) or version not in {1, 2}:
        raise SessionError("unsupported session version")
    expected_keys = {"version", "workspace_root", "active_path", "documents"}
    if version == 2:
        expected_keys.add("open_paths")
    _require_keys(payload, expected_keys, "session")
    workspace_value = payload["workspace_root"]
    if not isinstance(workspace_value, str):
        raise SessionError("workspace_root must be a string")
    workspace_root = Path(workspace_value)
    if not workspace_root.is_absolute() or workspace_root != _normalize_workspace(workspace_root):
        raise SessionError("workspace_root must be an absolute canonical path")

    documents_value = payload["documents"]
    if not isinstance(documents_value, list):
        raise SessionError("documents must be a list")
    if len(documents_value) > MAX_SESSION_DOCUMENTS:
        raise SessionError(f"session has more than {MAX_SESSION_DOCUMENTS} documents")
    documents = tuple(
        _view_from_payload(item, workspace_root, index)
        for index, item in enumerate(documents_value)
    )

    active_value = payload["active_path"]
    if active_value is not None and not isinstance(active_value, str):
        raise SessionError("active_path must be a string or null")
    active_path = (
        None
        if active_value is None
        else _document_path_from_relative(active_value, workspace_root, "active_path")
    )
    open_paths: tuple[Path, ...]
    if version == 1:
        open_paths = () if active_path is None else (active_path,)
    else:
        open_values = payload["open_paths"]
        if not isinstance(open_values, list):
            raise SessionError("open_paths must be a list")
        if len(open_values) > MAX_SESSION_DOCUMENTS:
            raise SessionError(f"session has more than {MAX_SESSION_DOCUMENTS} open documents")
        open_paths = tuple(
            _open_path_from_payload(value, workspace_root, index)
            for index, value in enumerate(open_values)
        )
    state = SessionState(workspace_root, active_path, documents, open_paths)
    _validate_state(state)
    return state


def _open_path_from_payload(value: Any, root: Path, index: int) -> Path:
    if not isinstance(value, str):
        raise SessionError(f"open_paths[{index}] must be a string")
    return _document_path_from_relative(value, root, f"open_paths[{index}]")


def _view_from_payload(payload: Any, root: Path, index: int) -> DocumentViewState:
    if not isinstance(payload, dict):
        raise SessionError(f"documents[{index}] must be an object")
    _require_keys(
        payload,
        {"path", "line", "column", "scroll_x", "scroll_y"},
        f"documents[{index}]",
    )
    path_value = payload["path"]
    if not isinstance(path_value, str):
        raise SessionError(f"documents[{index}].path must be a string")
    return DocumentViewState(
        path=_document_path_from_relative(path_value, root, f"documents[{index}].path"),
        line=_nonnegative_int(payload["line"], f"documents[{index}].line"),
        column=_nonnegative_int(payload["column"], f"documents[{index}].column"),
        scroll_x=_nonnegative_float(payload["scroll_x"], f"documents[{index}].scroll_x"),
        scroll_y=_nonnegative_float(payload["scroll_y"], f"documents[{index}].scroll_y"),
    )


def _validate_state(state: SessionState) -> None:
    root = state.workspace_root
    if not root.is_absolute() or root != _normalize_workspace(root):
        raise SessionError("workspace root must be an absolute canonical path")
    if len(state.documents) > MAX_SESSION_DOCUMENTS:
        raise SessionError(f"session has more than {MAX_SESSION_DOCUMENTS} documents")
    if len(state.open_paths) > MAX_SESSION_DOCUMENTS:
        raise SessionError(f"session has more than {MAX_SESSION_DOCUMENTS} open documents")

    paths: set[Path] = set()
    for view in state.documents:
        _validate_document_path(view.path, root)
        _nonnegative_int(view.line, "line")
        _nonnegative_int(view.column, "column")
        _nonnegative_float(view.scroll_x, "scroll_x")
        _nonnegative_float(view.scroll_y, "scroll_y")
        if view.path in paths:
            raise SessionError(f"duplicate document path: {view.path}")
        paths.add(view.path)

    if state.active_path is not None:
        _validate_document_path(state.active_path, root)
        if state.active_path not in paths:
            raise SessionError("active document must have a stored view")

    open_paths: set[Path] = set()
    for path in state.open_paths:
        _validate_document_path(path, root)
        if path in open_paths:
            raise SessionError(f"duplicate open document path: {path}")
        if path not in paths:
            raise SessionError("every open document must have a stored view")
        open_paths.add(path)

    if state.active_path is None:
        if open_paths:
            raise SessionError("open documents require an active document")
    else:
        if state.active_path not in open_paths:
            raise SessionError("active document must be open")


def _validate_document_path(path: Path, root: Path) -> None:
    if not path.is_absolute() or path != Path(os.path.abspath(path)):
        raise SessionError("document paths must be absolute and normalized")
    if path.suffix.casefold() not in MARKDOWN_SUFFIXES:
        raise SessionError(f"session document is not Markdown: {path}")
    try:
        path.relative_to(root)
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
    except (OSError, RuntimeError, ValueError) as error:
        raise SessionError(f"session document is outside its workspace: {path}") from error


def _document_path_from_relative(value: str, root: Path, field: str) -> Path:
    relative = Path(value)
    if not value or relative.is_absolute() or ".." in relative.parts:
        raise SessionError(f"{field} must be a safe relative path")
    path = root / relative
    _validate_document_path(path, root)
    return path


def _relative_path(path: Path | None, root: Path) -> str | None:
    if path is None:
        return None
    return path.relative_to(root).as_posix()


def _normalize_workspace(path: Path) -> Path:
    try:
        return path.expanduser().resolve(strict=False)
    except (OSError, RuntimeError) as error:
        raise SessionError(f"Cannot normalize workspace path {path}: {error}") from error


def _require_keys(payload: dict[Any, Any], expected: set[str], context: str) -> None:
    keys = set(payload)
    if keys != expected:
        raise SessionError(f"{context} fields do not match the supported schema")


def _nonnegative_int(value: Any, field: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise SessionError(f"{field} must be a non-negative integer")
    return value


def _nonnegative_float(value: Any, field: str) -> float:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        raise SessionError(f"{field} must be a finite non-negative number")
    try:
        normalized = float(value)
    except OverflowError as error:
        raise SessionError(f"{field} must be a finite non-negative number") from error
    if not math.isfinite(normalized) or normalized < 0:
        raise SessionError(f"{field} must be a finite non-negative number")
    return normalized


def _sync_directory(directory: Path) -> None:
    flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
    descriptor = os.open(directory, flags)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
