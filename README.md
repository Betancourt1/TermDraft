# TermWriter

TermWriter is a local-first Markdown editor for the terminal. It edits ordinary `.md` and
`.markdown` files directly: there is no database, project format, or import step.

The current release is a functional MVP focused on a dependable writing loop:

- browse a Markdown workspace;
- edit source with soft wrapping, Unicode, syntax highlighting, undo, and redo;
- read a rendered preview without leaving the terminal;
- find files quickly;
- save through a same-directory temporary file;
- keep an atomic per-user crash-recovery journal for dirty source;
- poll the active file for external changes while the application is running;
- require consent before editing a file with mixed line endings;
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
│ EDIT | journal/2026-07-11.md ● modified | RECOVERY STORED | 36 words | Ln… │
└─────────────────────────────────────────────────────────────────────────────┘
```

At widths below 100 columns, Ctrl+E switches between the editor and preview instead of squeezing
both panes into an unusable layout. Ctrl+B can reclaim the explorer width at any terminal size.

## Requirements

- Python 3.12 or newer;
- macOS or Linux with Python's POSIX directory-descriptor APIs;
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

The first dirty edit schedules a recovery write no more than 500 ms later; continued typing updates
the pending payload without postponing that deadline. Each JSON entry is mode 0600, written through
a same-directory temporary file, flushed, replaced, and followed by a directory `fsync`. On the next
open, TermWriter offers Restore draft / Use disk version / Cancel opening. A recovered draft whose
saved baseline no longer matches disk is marked as a conflict and cannot be written over the Markdown
path with Ctrl+S; it must be saved under another name or the disk version must be reloaded. Successful
saves and explicit discards remove the journal entry.
Opening a workspace directory also scans trusted entries for Markdown paths that disappeared or can
no longer be read safely; restoring one opens its draft in conflict state so it can only be kept
through Save As.

The journal is recovery state, not a document format or database. Markdown remains the source of
truth. The default recovery location is `~/Library/Application Support/TermWriter/recovery` on
macOS and `$XDG_STATE_HOME/termwriter/recovery` or `~/.local/state/termwriter/recovery` on Linux.

An existing file save follows this sequence:

1. hash the current disk bytes and compare them with the last loaded/saved fingerprint;
2. open the destination directory and create a private temporary entry relative to that descriptor;
3. encode the current source using its detected UTF-8 or UTF-8-with-BOM encoding;
4. write, flush, and `fsync` the temporary file;
5. hash the destination and permission mode again, aborting if either changed during the write;
6. attempt to copy the verified POSIX permission bits;
7. publish with descriptor-relative `os.replace` so a renamed ancestor cannot redirect the write;
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

If a clean open file disappears or becomes inaccessible, a guarded transition offers Save local as,
Continue without copy, or Cancel. Ctrl+S never recreates the missing original path silently.

The active file is also checked every two seconds. A clean external edit reloads safely and leaves a
visible `Reloaded externally` status. A dirty external edit, deletion, or inaccessible path keeps the
editor source intact, marks a persistent conflict, and shows one warning; only an explicit save or
transition opens the decision dialog. Checks pause while another modal workflow is active.

Mixed line endings are detected before the editor becomes active. TermWriter states the separator
Textual will use and requires an explicit Edit and normalize decision. Cancel leaves the current
document untouched. For a mixed file reloaded by the watcher, choosing Keep read-only requires
reopening the file to opt in to editing later. Merely opening or saving without an edit preserves its
exact bytes.

## Tests and quality checks

```bash
pytest
ruff format --check .
ruff check .
mypy
```

The suite covers the document model, UTF-8/BOM/LF/CRLF preservation, mixed-ending consent, empty
files, missing final newlines, recovery round trips and failures, restart recovery, atomic-save
failures, metadata and permission bits, watcher reload/conflict/deletion behavior, workspace
filtering, symlinks, file search, CLI validation, and Textual Pilot workflows.

## Known limitations

- One document is active at a time; there are no tabs or session restoration yet.
- Recovery has a maximum 500 ms first-write delay, so termination before the deadline or a failed
  journal write can lose the latest unsaved keystrokes. It is not version history, a backup, or an
  autosave of the Markdown path. Recovery entries contain the draft's plaintext source in a private
  per-user state directory.
- The watcher polls only the active file every two seconds. It is not an operating-system event
  watcher, and synchronous hashing can briefly pause input for unusually large files.
- Files must be valid UTF-8, with or without a UTF-8 BOM.
- Uniform LF and CRLF sources round-trip through edits. Textual prefers CRLF when it is present,
  otherwise LF and then CR. After explicit consent and an edit, a deliberately mixed file is
  normalized to that separator.
  An untouched mixed-ending file and an unedited recovered draft retain their exact separators.
- Atomic replacement means the destination name never points to a partially written temporary file
  on normal local filesystems that honor same-filesystem `os.replace` semantics. It is not a
  universal guarantee for every network or unusual filesystem, and power-loss durability still
  depends on the filesystem despite the file and directory `fsync` attempts.
- Ordinary POSIX permission bits are preserved where the filesystem permits. Special setuid/setgid
  bits are not guaranteed. Ownership, ACLs, extended attributes, Finder metadata, and hard-link
  identity are not preserved by replacement.
- Conflict Save As depends on hard-link support in the destination filesystem and fails cleanly when
  that publication mechanism is unavailable. It is currently available only from conflict recovery.
- A second hash check narrows but cannot eliminate the race between that check and `os.replace` if
  another process writes at exactly that moment. Cooperative file locking would not protect against
  editors that ignore the lock.
- Path validation rejects symlinks and resolved workspace escapes, but it is not hardened against a
  hostile process swapping intermediate path components during the initial open or a new Save As.
  Existing-file saves additionally compare file and parent identities and keep all temporary,
  cleanup, and replacement operations attached to the opened parent directory.
- Cursor and scroll coordinates are recorded for the active document, but there is no multi-document
  cache or restart restoration yet.
- Preview links are deliberately non-opening. Raw document HTML, JavaScript, and shell text are not
  executed.
- Global full-text search is not part of this MVP.

## Near-term roadmap

1. Add recovery-entry management for renamed files and manual cleanup of corrupt/stale entries.
2. Preserve more filesystem metadata where the host platform exposes a safe, testable mechanism.
3. Add optional in-process workspace text search.
4. Add a multi-document cache and restart restoration for cursor/scroll state.
5. Prototype read-only semantic block mapping before attempting hybrid block editing.

Implementation boundaries and tradeoffs are documented in
[`docs/architecture.md`](docs/architecture.md).
