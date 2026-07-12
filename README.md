# TermWriter

TermWriter is a local-first Markdown editor for the terminal. It edits ordinary `.md` and
`.markdown` files directly: there is no database, project format, or import step.

The current release is a functional MVP focused on a dependable writing loop:

- browse a Markdown workspace;
- edit source with soft wrapping, Unicode, syntax highlighting, undo, redo, and list continuation;
- read a safe GFM-style rendered preview without leaving the terminal;
- find files quickly;
- search source text across the workspace without an external command;
- search commands from a palette;
- customize editor options, keybindings, and Textual CSS without reinstalling;
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
- a filesystem where workspace files are readable and their parent directories are writable and
  searchable when saving.

TermWriter currently targets Textual 8.x, installs its Markdown syntax-highlighting extra, uses
`markdown-it-py` plus `mdit-py-plugins` for the preview parser, and uses the small `regex` package for
Unicode-aware whole-word matching and time-limited regular expressions.

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

Show every CLI option or the effective in-app commands without starting the TUI:

```bash
termwriter --help
termwriter --commands
```

## Markdown support

The preview supports headings H1-H6, paragraphs, emphasis, bold, strikethrough, blockquotes,
horizontal rules, links, image placeholders, inline and fenced code, nested ordered/unordered lists,
tables, task lists, footnotes, definition lists, and the five standard GFM alerts. Task state is
rendered as `☐` or `☑`; alert titles are bold blockquotes; footnote references and definitions
remain visible and navigate inside the preview; definition terms are bold with their definitions
indented as quotes. The Markdown source remains unchanged. Raw HTML is displayed literally and is
never executed.

```markdown
Claim with a source.[^source]

[^source]: Footnote text.

Term
: Definition text.

> [!WARNING]
> Review before saving.
```

Nested ordered items use an indented normal marker:

```markdown
1. First
   1. Nested
   2. Nested
2. Second
```

`1.1. Nested` is ordinary text in CommonMark, not a nested-list marker. Likewise, `__text__` means
bold; portable Markdown has no underline syntax. F1 opens the effective shortcut list, and the
command palette includes a compact Markdown syntax reference.

The repository also includes a complete [Markdown syntax gallery](docs/markdown-gallery.md). Open it
with `termwriter docs/markdown-gallery.md` to compare its editable syntax and preview side by side.

Clicking a footnote label scrolls to its definition; `↩` returns to the most recently followed
reference for that note. Unreferenced definitions are omitted by the footnote parser, and definition
lists use bold terms plus quoted bodies rather than a dedicated `<dl>` layout. Alerts use a
conservative titled-blockquote presentation rather than GitHub's color and icon treatment. Math,
underline, subscript, superscript, and rendered raw HTML are not supported. Preview rendering never
writes back to the source editor.

## Configuration

Create editable, no-clobber templates:

```bash
termwriter --init-config
termwriter --config-path
```

The default files are `~/.termwriter/config.toml` and `~/.termwriter/theme.tcss`. Override the
directory with `TERMWRITER_CONFIG_HOME` or `--config-dir PATH`. The TOML file accepts only the
documented editor booleans and known binding IDs; it cannot define actions, commands, or executable
hooks.

```toml
[editor]
auto_continue_lists = true
soft_wrap = true
show_line_numbers = true

[keybindings]
save = "ctrl+s"
quit = "ctrl+q"
toggle_explorer = "ctrl+b"
find_file = "ctrl+p"
search_text = "ctrl+shift+f"
toggle_preview = "ctrl+e"
undo = "ctrl+z,super+z"
redo = "ctrl+y,super+y,ctrl+shift+z"
show_help = "f1"
command_palette = "ctrl+backslash"
```

Use **Reload configuration** from the command palette after editing `config.toml`. Help is generated
from the effective map, so it reflects remapped keys. Duplicate keys, unknown IDs/options, invalid
TOML, and non-boolean editor options are rejected with a clear error.

`theme.tcss` is [Textual CSS](https://textual.textualize.io/guide/CSS/), not browser CSS. It loads
after TermWriter's bundled stylesheet, so matching selectors override the defaults:

```css
#title-bar {
    background: $primary-darken-2;
}

#markdown-preview {
    padding: 1 4;
}
```

An existing `theme.tcss` is watched and reapplied when saved. If the theme file is created while
TermWriter is already running, restart once so it can be added to the watched stylesheet list.

## Shortcuts

| Key | Action |
| --- | --- |
| Ctrl+S | Save |
| Ctrl+Q | Quit through the unsaved-change guard |
| Ctrl+B | Show or hide the file explorer |
| Ctrl+P | Find and open a workspace Markdown file |
| Ctrl+Shift+F | Search workspace Markdown source |
| Ctrl+E | Show/hide preview, or switch editor/preview when narrow |
| Ctrl+Z | Undo |
| Ctrl+Y or Ctrl+Shift+Z | Redo |
| Ctrl+\ | Open the searchable command palette |
| F1 | Shortcut help |

Tab and Shift+Tab move focus where the focused control does not use Tab for indentation. F1 is the
help key so a literal `?` remains editable Markdown. Some terminals do not distinguish
Ctrl+Shift+Z from Ctrl+Z; Ctrl+Y remains the portable redo binding. On a bullet, numbered item,
task, or blockquote, Enter inserts the next marker; Enter on an empty marker ends that structure.

Workspace text search runs only after Enter is pressed in its dialog. Choose literal, whole-word, or
regular-expression matching; optionally match case and restrict paths with one workspace-relative
glob such as `notes/**/*.md`. Results are capped at 100 matching lines and include the active
document's unsaved source. Selecting another file still passes through Save / Discard / Cancel before
leaving dirty work.

## Data-safety behavior

`Document` is the in-memory source of truth for the active file. The preview never writes back to
the editor or model.

The first dirty edit schedules a recovery write for 500 ms later; continued typing updates the
pending payload without postponing that deadline. Each JSON entry is mode 0600, written through
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

Stable document reads, disk probes, all content hashing, atomic publication, Save As, and orphan
source validation run in Textual thread workers. A completed probe is classified on the UI thread
against the latest dirty state, so an edit made during a watcher or transition check cannot be
silently reloaded or left behind. During actual publication the editor is temporarily read-only;
quit, switching, duplicate saves, and Save As dismissal wait until the non-cancellable writer has
finished. Stale worker results are rejected with a document-generation ticket.

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
filtering, symlinks, file and workspace-text search, CLI validation, and Textual Pilot workflows.
Customization tests also exercise remapped keys, runtime TOML reload, user-TCSS precedence, command
discovery, task continuation, termination, and undo grouping. Race-focused worker tests block probes,
loads, saves, and Save As publication to verify UI responsiveness, stale-result rejection, conflict
preservation, and non-cancellable writer locking.

## Known limitations

- One document is active at a time; there are no tabs or session restoration yet.
- Recovery has a nominal 500 ms first-write delay. Termination before the timer runs, a blocked event
  loop, or a failed journal write can lose the latest unsaved keystrokes. It is not version history,
  a backup, or an autosave of the Markdown path. Recovery entries contain the draft's plaintext
  source in a private per-user state directory.
- The watcher polls only the active file every two seconds. It is not an operating-system event
  watcher. Hashing runs in a worker, so a completed check can arrive after the disk changed again;
  save and transition checks remain authoritative.
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
- Alerts use titled blockquotes without GitHub's colors or icons. Preview intentionally omits math,
  underline, subscript, and superscript. A repeated footnote's back arrow returns to the most recently
  followed reference, definition lists use quoted bodies, and images do not render terminal graphics.
- Smart Enter handles common list/task/blockquote prefixes, thematic breaks, fenced code, and the
  distinction between indented code and genuine nested lists. Ambiguous indentation parses the
  source prefix through the cursor, so Enter may have a small delay in extremely large files.
- A malformed `theme.tcss` can prevent startup until the file is corrected. Only a theme present at
  launch is watched; create the templates with `--init-config` before opening the TUI.
- Workspace text search is not fuzzy, accepts one include glob rather than a compound filter
  expression, returns one match per source line, and caps results at 100. Regexes are limited to 500
  characters and 50 ms per source line; a timed-out expression returns an error instead of results.
- Thread workers cannot stop an in-progress operating-system read or write. TermWriter ignores stale
  read/probe results; an atomic writer is deliberately allowed to finish while the UI stays locked.
- Recovery-journal publication and workspace index refreshes remain synchronous. They can briefly
  pause input for an unusually large dirty document or workspace even though Markdown-file hashing
  and publication now run outside the UI thread.

## Near-term roadmap

1. Add internal footnote navigation and improve definition-list presentation.
2. Add recovery-entry management for renamed files and manual cleanup of corrupt/stale entries.
3. Add fuzzy path/text ranking and compound include/exclude search filters.
4. Add a multi-document cache and restart restoration for cursor/scroll state.
5. Prototype read-only semantic block mapping before attempting hybrid block editing.

Implementation boundaries and tradeoffs are documented in
[`docs/architecture.md`](docs/architecture.md).
