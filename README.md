# TermWriter

TermWriter is a local-first Markdown editor for the terminal. It edits ordinary `.md` and
`.markdown` files directly: there is no database, project format, or import step.

The current release is a functional MVP focused on a dependable writing loop:

- browse a Markdown workspace;
- edit source with soft wrapping, Unicode, syntax highlighting, undo, and redo;
- read a rendered preview without leaving the terminal;
- find files quickly;
- save through a same-directory temporary file;
- detect external changes before saves and guarded transitions;
- require an explicit decision before leaving unsaved work.

Future WYSIWYM block editing is designed in
[`docs/semantic-editing.md`](docs/semantic-editing.md), but it is intentionally not implemented.

## Interface

```text
┌ TermWriter · ~/notes ────────────────────────────────────────────────────────┐
│ Files                    │ Markdown source            │ Rendered preview     │
│  journal/                │ # Friday                   │ Friday               │
│   2026-07-11.md          │                            │                      │
│  projects/               │ Today I learned…           │ Today I learned…     │
│   termwriter.md          │                            │                      │
├──────────────────────────┴────────────────────────────┴──────────────────────┤
│ EDIT | journal/2026-07-11.md ● modified | 36 words | Ln 4, Col 8 | Loaded │
└─────────────────────────────────────────────────────────────────────────────┘
```

At widths below 100 columns, Ctrl+E switches between the editor and preview instead of squeezing
both panes into an unusable layout. Ctrl+B can reclaim the explorer width at any terminal size.

## Requirements

- Python 3.12 or newer;
- a terminal supported by [Textual](https://textual.textualize.io/);
- a filesystem on which the selected workspace is readable and its files are writable when saving.

TermWriter currently targets Textual 8.x and installs its Markdown syntax-highlighting extra.

## Installation

From the repository:

```bash
python3.12 -m venv .venv
source .venv/bin/activate
pip install -e .
```

For development tools:

```bash
pip install -e ".[dev]"
```

## Running

Open the current directory:

```bash
termwriter .
```

Open a different workspace or one Markdown file:

```bash
termwriter ~/Documents/notes
termwriter essay.md
```

When a file is passed, it opens initially and its parent directory becomes the workspace, so sibling
Markdown files remain available in the explorer and search.

The CLI rejects missing paths, non-Markdown file targets, and Markdown file symlinks before the TUI
starts. The explorer omits `.git`, `.venv`, `node_modules`, `__pycache__`, and all symlinks.

## Shortcuts

| Key | Action |
| --- | --- |
| Ctrl+S | Save |
| Ctrl+Q | Quit through the unsaved-change guard |
| Ctrl+B | Show or hide the file explorer |
| Ctrl+P | Find and open a workspace Markdown file |
| Ctrl+E | Show/hide preview, or switch editor/preview when narrow |
| Ctrl+Z | Undo |
| Ctrl+Y or Ctrl+Shift+Z | Redo |
| F1 | Shortcut help |

Tab and Shift+Tab move focus where the focused control does not use Tab for indentation. F1 is the
help key so a literal `?` remains editable Markdown. Some terminals do not distinguish
Ctrl+Shift+Z from Ctrl+Z; Ctrl+Y remains the portable redo binding.

## Data-safety behavior

`Document` is the in-memory source of truth for the active file. The preview never writes back to
the editor or model.

An existing file save follows this sequence:

1. hash the current disk bytes and compare them with the last loaded/saved fingerprint;
2. create a temporary file in the destination directory;
3. encode the current source using its detected UTF-8 or UTF-8-with-BOM encoding;
4. write, flush, and `fsync` the temporary file;
5. copy the original POSIX permission bits;
6. hash the destination again to narrow the concurrent-change window;
7. publish with `os.replace`;
8. attempt to `fsync` the parent directory and verify the visible bytes;
9. only then update the document's saved/dirty state.

Save As publishes a fully written temporary file with a no-clobber hard-link step. If the target
appears concurrently, TermWriter reports a conflict instead of replacing it.

If both local and disk content changed, the only choices are:

- save the local version under a new workspace-relative Markdown name;
- explicitly reload the disk version;
- cancel.

Changing files and quitting use a separate Save / Discard / Cancel guard. Save failures keep the
document dirty and stop the requested transition.

## Tests and quality checks

```bash
pytest
ruff format --check .
ruff check .
mypy
```

The suite covers the document model, UTF-8/BOM/LF/CRLF preservation, empty files, missing final
newlines, atomic-save failures, permission bits, external conflicts, deleted files, workspace
filtering, symlinks, file search, CLI validation, and Textual Pilot workflows.

## Known limitations

- One document is active at a time; there are no tabs or session restoration yet.
- There is no autosave, journal, backup, or crash-recovery file. Normal Ctrl+Q is guarded, but forced
  termination and power loss before a durable save cannot be recovered by TermWriter.
- External changes are checked on save, guarded transitions, and supported terminal focus events.
  There is no permanent filesystem watcher.
- Files must be valid UTF-8, with or without a UTF-8 BOM.
- Uniform LF and CRLF sources round-trip through edits. Textual uses the first detected separator;
  after an edit, a file with deliberately mixed line endings is normalized to that separator.
  An untouched mixed-ending file is not rewritten by Ctrl+S.
- Atomic replacement means the destination name never points to a partially written temporary file
  on normal local filesystems that honor same-filesystem `os.replace` semantics. It is not a
  universal guarantee for every network or unusual filesystem, and power-loss durability still
  depends on the filesystem despite the file and directory `fsync` attempts.
- Mode bits are preserved. Ownership, ACLs, extended attributes, Finder metadata, and hard-link
  identity are not preserved by replacement.
- Conflict Save As depends on hard-link support in the destination filesystem and fails cleanly when
  that publication mechanism is unavailable. It is currently available only from conflict recovery.
- A second hash check narrows but cannot eliminate the race between that check and `os.replace` if
  another process writes at exactly that moment. Cooperative file locking would not protect against
  editors that ignore the lock.
- Path validation rejects symlinks and resolved workspace escapes, but it is not hardened against a
  hostile process swapping intermediate path components between validation and I/O.
- Cursor and scroll coordinates are recorded for the active document, but there is no multi-document
  cache or restart restoration yet.
- Preview links are deliberately non-opening. Raw document HTML, JavaScript, and shell text are not
  executed.
- Global full-text search is not part of this MVP.

## Near-term roadmap

1. Add a portable filesystem watcher and an in-app reload indicator without weakening save checks.
2. Add an explicit mixed-line-ending warning and normalization choice before the first edit/save.
3. Preserve more filesystem metadata where the host platform exposes a safe, testable mechanism.
4. Add optional in-process workspace text search.
5. Prototype read-only semantic block mapping before attempting hybrid block editing.

Implementation boundaries and tradeoffs are documented in
[`docs/architecture.md`](docs/architecture.md).
