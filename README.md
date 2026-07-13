# TermWriter

TermWriter is a local-first Markdown editor for the terminal. It edits ordinary `.md` and
`.markdown` files directly: there is no database, project format, or import step.

The current release is a functional MVP focused on a dependable writing loop:

- browse a Markdown workspace;
- edit source with soft wrapping, Unicode, syntax highlighting, undo, redo, and list continuation;
- read a safe GFM-style rendered preview without leaving the terminal;
- create, rename, move, and trash Markdown files and workspace folders;
- find files quickly;
- search source text across the workspace without an external command;
- search commands from a palette;
- customize editor options, keybindings, and Textual CSS without reinstalling;
- keep several independently dirty documents open with independent runtime undo histories;
- restore the prior tab order and active view when reopening a workspace directory;
- export quarantined recovery drafts and explicitly clean confirmed entries past a configured age;
- traverse rendered headings from the keyboard with a visible level and position announcement;
- save through a same-directory temporary file;
- keep an atomic per-user crash-recovery journal for dirty source;
- drain exact dirty-tab recovery drafts on cooperative `SIGTERM` and `SIGHUP` shutdown;
- poll open files and visible workspace structure for external changes;
- require consent before editing a file with mixed line endings;
- require an explicit decision before closing, reloading, or replacing unsaved work.

Unsaved-document transitions use a compact keyboard prompt: `y` saves, `n` discards, and Esc
cancels. Enter and unrelated keys never choose a destructive action.

Future WYSIWYM block editing is designed in
[`docs/semantic-editing.md`](docs/semantic-editing.md). Hybrid editing is intentionally not
implemented; the command palette offers a source-range inspector and an opt-in read-only block
rendering experiment.

## Interface

```text
┌ TermWriter · ~/notes ────────────────────────────────────────────────────────┐
│ journal/2026-07-11.md │ ● projects/termwriter.md                            │
│ Files                    │ Markdown source            │ Rendered preview     │
│  journal/                │ # Friday                   │ Friday               │
│   2026-07-11.md          │                            │                      │
│  projects/               │ Today I learned…           │ Today I learned…     │
│   termwriter.md          │                            │                      │
├──────────────────────────┴────────────────────────────┴──────────────────────┤
│ COMMAND | journal/2026-07-11.md ● modified | RECOVERY STORED | 36 words |…│
└─────────────────────────────────────────────────────────────────────────────┘
```

At widths below 100 columns, `v` in COMMAND mode switches between the editor and preview instead of
squeezing both panes into an unusable layout. `e` can reclaim the explorer width at any terminal
size. The configured Ctrl shortcuts remain available in both modes.

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
Files, folders, search, and image placeholders use Yazi's Nerd Font icon language in grayscale. A
Nerd Font or a Symbols Nerd Font fallback is required to display them.

Show every CLI option or the effective in-app commands without starting the TUI:

```bash
termwriter --help
termwriter --commands
```

### File and folder management

Open the command palette with `:` in COMMAND mode or Ctrl+\ and choose **Create Markdown file**,
**Create folder**, **Rename selected file or folder**, **Move selected file or folder**, or
**Move selected file or folder to Trash**. Actions use the selected explorer entry; when the tree has no
selection, the active document is used. New files are created beside the selected file or inside the
selected folder, and a missing Markdown extension is added as `.md`.

Move destinations are workspace-relative paths such as `archive/essay.md`. Clean open documents
follow a rename or move without losing their tab, cursor, or contents. Save or close dirty documents
first. Rename and move publication never replaces a destination that appears concurrently. Open
documents cannot be trashed; close them through TermWriter's save guard before trying again. Moving a
folder to the operating system Trash includes every nested entry, including files hidden by the
Markdown-only explorer. A Trash failure leaves the original entry in place.

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
bold; portable Markdown has no underline syntax. `?` in COMMAND mode or F1 opens the effective
shortcut list, and the `:` command palette includes a compact Markdown syntax reference.

The repository also includes a complete [Markdown syntax gallery](docs/markdown-gallery.md). Open it
with `termwriter docs/markdown-gallery.md` to compare its editable syntax and preview side by side.

Clicking a footnote label scrolls to its definition; `↩` returns to the most recently followed
reference for that note. Esc enters COMMAND mode without moving the cursor; `i` returns to WRITE
mode and focuses the source editor. In COMMAND mode, `v` toggles the preview and focuses it when
shown. Once the preview has focus,
Tab/Shift+Tab select links and Enter activates the selection. Internal footnotes navigate; external
URLs remain inert. Alt+Down and Alt+Up move through rendered headings and show the selected
heading's level and position in the persistent status bar without stacking notifications.
Unreferenced definitions are omitted by the footnote parser, and definition lists
use bold terms plus quoted bodies rather than a dedicated `<dl>` layout. Alerts use a conservative
titled-blockquote presentation rather than GitHub's color and icon treatment. Math, underline,
subscript, superscript, and rendered raw HTML are not supported. Preview rendering never writes back
to the source editor.

## Configuration

Create editable, no-clobber templates:

```bash
termwriter --init-config
termwriter --config-path
```

The default files are `~/.termwriter/config.toml` and `~/.termwriter/theme.tcss`. Override the
directory with `TERMWRITER_CONFIG_HOME` or `--config-dir PATH`. The TOML file accepts only the
documented editor options, positive manual-retention age, and known binding IDs; it cannot define
actions, commands, or executable hooks.

```toml
[editor]
auto_continue_lists = true
soft_wrap = true
show_line_numbers = true
startup_mode = "command" # or "write"; applied on the next launch

[recovery]
# Applied only when Delete expired is explicitly confirmed.
retention_days = 30

[keybindings]
save = "ctrl+s"
quit = "ctrl+q"
toggle_explorer = "ctrl+b"
find_file = "ctrl+p"
recent_documents = "ctrl+o"
next_tab = "ctrl+pagedown"
previous_tab = "ctrl+pageup"
close_tab = "ctrl+f4"
search_text = "ctrl+shift+f"
toggle_preview = "ctrl+e"
preview_next_heading = "alt+down"
preview_previous_heading = "alt+up"
undo = "ctrl+z,super+z"
redo = "ctrl+y,super+y,ctrl+shift+z"
show_help = "f1"
command_palette = "ctrl+backslash"
command_cursor_left = "h"
command_cursor_down = "j"
command_cursor_up = "k"
command_cursor_right = "l"
command_line_start = "0"
command_line_end = "dollar_sign"
command_document_start = "g"
command_document_end = "G"
```

Tab, Shift+Tab, and Enter are reserved for preview link navigation and cannot be reassigned.
Every single-key COMMAND action has a `command_*` binding ID in the generated template. Configured
Ctrl/Alt shortcuts remain available in both modes and can also be remapped.

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

## Modes and shortcuts

TermWriter starts in COMMAND mode by default, like Vim; set `editor.startup_mode = "write"` to begin
ready for insertion instead. Source text is protected in COMMAND mode, so plain keys run commands
instead of being inserted. The arrow keys and `h`/`j`/`k`/`l` move without changing the document.
Press `i` to enter WRITE mode and Esc to return to COMMAND mode.

| COMMAND key | Action |
| --- | --- |
| `i` | Enter WRITE mode and focus the source editor |
| `w` | Save |
| `q` | Quit through the unsaved-change guard |
| `e` | Show or hide the file explorer |
| `f` | Find and open a workspace Markdown file |
| `o` | Open the recent-document switcher |
| `n` / `p` | Activate the next / previous open document tab |
| `c` | Close the active tab through the safety guard |
| `/` | Search workspace Markdown source |
| `v` | Show/hide preview, or switch editor/preview when narrow |
| `u` / `r` | Undo / redo |
| `:` | Open the searchable command palette |
| `?` | Shortcut help |
| `h` / `j` / `k` / `l` | Move left / down / up / right in the focused source editor |
| `0` / `$` | Move to the start / end of the source line |
| `g` / `G` | Move to the start / end of the document |

Configured shortcuts remain available in both modes:

| Key | Action |
| --- | --- |
| Ctrl+S | Save |
| Ctrl+Q | Quit through the unsaved-change guard |
| Ctrl+B | Show or hide the file explorer |
| Ctrl+P | Find and open a workspace Markdown file |
| Ctrl+O | Open the recent-document switcher |
| Ctrl+PageDown / Ctrl+PageUp | Activate next / previous open document tab |
| Ctrl+F4 | Close the active tab through the safety guard |
| Ctrl+Shift+F | Search workspace Markdown source |
| Ctrl+E | Show/hide preview, or switch editor/preview when narrow |
| Alt+Down / Alt+Up in preview | Select next / previous rendered heading |
| Ctrl+Z | Undo |
| Ctrl+Y or Ctrl+Shift+Z | Redo |
| Ctrl+\ | Open the searchable command palette |
| F1 | Shortcut help |

The command palette shows each command's effective single-key COMMAND-mode shortcut. Commands
without one are labeled `Palette only`; configured Ctrl/Alt shortcuts remain available but stay out
of the palette for a cleaner scan.

The editor keeps Tab and Shift+Tab for Markdown indentation. Inside the focused preview, Tab and
Shift+Tab select links, and Enter activates the selection; reaching either end returns to the normal
focus chain. `?` opens help only in COMMAND mode and remains editable Markdown in WRITE mode. Some
terminals do not distinguish
Ctrl+Shift+Z from Ctrl+Z; Ctrl+Y remains the portable redo binding. On a bullet, numbered item,
task, or blockquote, Enter inserts the next marker; Enter on an empty marker ends that structure.

Workspace text search runs only after Enter is pressed in its dialog. Choose literal, whole-word, or
regular-expression matching, or fuzzy subsequence ranking; optionally match case and restrict paths
with comma-separated workspace-relative includes and `!` exclusions such as
`notes/**/*.md, !notes/archive/**`. Results are capped at 100 matching lines and include unsaved
source from every open tab. Selecting a result activates an existing tab or opens a new one without
discarding the current buffer. Ctrl+P uses the same compound filter syntax and fuzzy-ranks
abbreviated path queries.

**Inspect semantic blocks** in the command palette parses the active source in a worker and lists
top-level block ranges plus uncovered separators or reference-definition source. Selecting a range
jumps to its first line. **Read semantic blocks (experimental)** uses the same immutable worker
snapshot, independently renders only top-level headings and paragraphs, and shows every unsupported
construct as exact Markdown source. Escape or **Return to source** immediately reveals the unchanged
full editor. Links stay inert. Neither command edits, saves, or splices source.

**Inspect cursor coordinates** reports the current cursor as a source-character offset, UTF-8 byte
offset, logical line/column, wrapped row/cell, and live screen position. It also flags cursors inside
an extended grapheme and very narrow wraps that split one. This is a read-only developer diagnostic
for future semantic editing, not a source transformation.

## Data-safety behavior

Each loaded file has one live `Document` that remains its in-memory source of truth and one mounted
Textual editor with its own runtime undo stack. Restored background tabs keep only their path until
first selected; once loaded, their editor remains mounted so runtime undo history survives tab
switches. One document/editor pair is active beside the shared preview; neither rendered output nor
tab widgets write source back to the model.

Workspace session JSON stores only ordered open paths, the active path, and per-document
cursor/scroll coordinates. It is limited to 100 document views and 512 KiB. Loading and serialized,
coalesced writes run in workers; clean quit waits for the newest queued snapshot. Opening a directory
restores the prior active tab immediately and defers loading the other readable tabs until selected,
while an explicit CLI file takes precedence and does not resurrect siblings. Recovery and
mixed-ending decisions still run through the normal open path. Ctrl+O presents the
most-recently-used order. Confirmed
missing entries are pruned; temporarily inaccessible entries remain recent but are skipped during
automatic restoration. Session state is atomically replaced outside the workspace and never
contains Markdown source, recovery text, or undo history. Missing or corrupt state cannot prevent
startup, and version-one state migrates by restoring only its prior active path.

The first dirty edit schedules a recovery write for 500 ms later; continued typing updates the
pending payload without postponing that deadline. Publications and deletions run through one ordered
background queue. Pending saves coalesce to the newest exact source, while deletion is an ordering
barrier and removes only the journal fingerprint that TermWriter observed. Each JSON entry is mode
0600, written through a same-directory temporary file, flushed, replaced, and followed by a
directory `fsync`. On the next open, TermWriter offers Restore draft / Use disk version / Cancel
opening. A recovered draft whose
saved baseline no longer matches disk is marked as a conflict and cannot be written over the Markdown
path with Ctrl+S; it must be saved under another name or the disk version must be reloaded. Successful
saves and explicit discards remove the journal entry.
Opening a workspace directory also scans trusted entries for Markdown paths that disappeared or can
no longer be read safely; restoring one opens its draft in conflict state so it can only be kept
through Save As.

The journal is recovery state, not a document format or database. Markdown remains the source of
truth. The default recovery location is `~/Library/Application Support/TermWriter/recovery` on
macOS and `$XDG_STATE_HOME/termwriter/recovery` or `~/.local/state/termwriter/recovery` on Linux.

Use **Manage recovery drafts** from the command palette to inspect the current workspace's trusted
entries and any corrupt journals. Trusted drafts can be reopened or retargeted after a Markdown file
is renamed. Retarget never replaces an existing recovery entry. **Archive** removes a stale or
corrupt entry from the active inventory while preserving its exact journal bytes under the recovery
directory's `quarantine/` folder. Quarantined trusted entries can be restored without replacing an
active draft. **Export copy** publishes the archived source as a new no-clobber Markdown file while
keeping the quarantine intact. **Delete expired** considers only valid quarantined entries older than
`recovery.retention_days`, lists and confirms the exact path inventory, and reports every failure;
nothing expires automatically. **Delete forever** requires a separate irreversible confirmation and
also handles corrupt quarantine entries. A draft belonging to any dirty open document cannot be
moved, archived, retargeted onto, or replaced by a quarantine restore.

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

Stable document reads, disk probes, all content hashing, atomic publication, Save As, workspace
indexing, session I/O, recovery reads/mutations, orphan-source validation, and semantic mapping run
in Textual thread workers. A completed probe is
classified on the UI thread
against the latest dirty state, so an edit made during a watcher or transition check cannot be
silently reloaded or left behind. During actual publication the editor is temporarily read-only;
quit, switching, duplicate saves, and Save As dismissal wait until the non-cancellable writer has
finished. Stale worker results are rejected with document identity, path, and baseline tickets.

Save As first rejects exact or normalized spelling variants owned by any open buffer, including a
buffer whose disk file disappeared, then publishes a fully written temporary file with a no-clobber
hard-link step. If the target appears concurrently, TermWriter reports a conflict instead of
replacing it.

If both local and disk content changed, the only choices are:

- save the local version under a new workspace-relative Markdown name;
- explicitly reload the disk version;
- cancel.

Opening or activating another tab preserves the current buffer without prompting. Closing a dirty
tab, reloading it, replacing its source, and quitting still use Save / Discard / Cancel. Quit checks
every open document in order, and Cancel stops the entire quit. Save failures keep the affected
document dirty and stop the requested transition. After Save or Discard, the editor remains
read-only until its ordered recovery cleanup completes; Discard also restores that `Document` to its
saved baseline before a close or quit continuation can run.

If a clean open file disappears or becomes inaccessible, a guarded transition offers Save local as,
Continue without copy, or Cancel. Ctrl+S never recreates the missing original path silently.

The active file is also checked every two seconds. Unchanged file and parent metadata avoids reading
or hashing the source; a size, time, mode, or identity change triggers the full content check. A clean
external edit reloads safely and leaves a visible `Reloaded externally` status. A dirty external
edit, deletion, or inaccessible path keeps the editor source intact, marks a persistent conflict,
and shows one warning; only an explicit save or transition opens the decision dialog. Checks pause
while another modal workflow is active.

The same interval scans visible Markdown paths and folders in the background. External creates,
deletes, and renames refresh the explorer and file-search index within about two seconds, or when
TermWriter regains focus. Renaming an open file is handled conservatively: the original tab is marked
as deleted externally while the renamed path appears in the explorer, rather than guessing that the
two paths represent the same document.

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
filtering, symlinks, file and workspace-text search, bounded content-free workspace sessions and tab
restoration, semantic source mapping, quarantine restore/export/retention/deletion, independent
document tabs, heading navigation
announcements, CLI validation, and Textual Pilot workflows.
Customization tests also exercise remapped keys, runtime TOML reload, user-TCSS precedence, command
discovery, task continuation, termination, and undo grouping. Race-focused worker tests block probes,
loads, workspace indexes, recovery reads, saves, session writes, recovery publication, semantic
parsing, and Save As publication to verify UI
responsiveness, ordered cleanup, stale-result rejection, conflict preservation, and non-cancellable
writer locking.

## Development benchmarks

The installed `termwriter-benchmark` command measures the real semantic mapper, real mounted editor
tabs under Textual's headless application driver, and one real watcher pass over the active plus one
inactive document. It emits JSON so results can be retained and compared without adding a benchmark
framework:

```bash
termwriter-benchmark

termwriter-benchmark \
  --semantic-kib 1024 \
  --tab-kib 64 \
  --tabs 20 \
  --watch-kib 1024 \
  --iterations 10 \
  --warmup 2
```

The second command produced this single-run baseline for commit `3be05a1` on macOS 26.5.2 arm64,
Python 3.12.13, and Textual 8.2.8:

| Path | Workload | Median | p95 / memory result |
|---|---:|---:|---:|
| Semantic map | 1,048,800 bytes | 886.47 ms | 944.31 ms; 1.13 MiB/s |
| Mount tabs | 20 × 65,740 bytes | 33.54 s total | 289,738,826 added traced bytes |
| Watcher pass | 2 files / 2,097,600 bytes | 453.19 ms | 1,251.51 ms |

The tab run measured 523,264,118 traced bytes for all tabs, about 15,249,412 traced bytes per added
tab, and a 1,006,174,208-byte increase between the process peak-RSS high-water marks taken before and
after mounting. `tracemalloc` observes Python allocations; peak RSS is order-dependent process
history, not current resident memory. These figures are comparative, machine-specific diagnostics,
not CI thresholds. For a meaningful comparison, run three fresh processes under the same power
mode, Python environment, dependency versions, and workload, then compare the median run.

## Known limitations

- Each tab's undo/redo history survives runtime tab switching but is intentionally not serialized;
  restored tabs begin with empty histories when first selected. Materialized editors remain mounted,
  so unusually large tab sets eventually consume more memory if every tab is activated. Session
  metadata restores at most 100 views.
- The semantic inspector is line-level diagnostics, not an editing-grade AST. Nested containers are
  represented by their outer block. Valid top-level link-reference definitions use parser
  environment line maps, while malformed or unknown source remains explicit gaps; this metadata is
  tested for diagnostics but not trusted for source splicing. The detail pane truncates very large
  block previews. It provides no inline delimiter offsets, stable block identity, visual-position
  map, incremental parsing, or hybrid editing.
- The experimental semantic reader independently parses paragraph slices, so nonlocal reference
  links and footnotes may not resolve like the full-document preview. Their definitions and every
  unsupported block stay visible as exact source. The mode is a modal snapshot, not a persistent
  reading layout or an editor.
- The cursor-coordinate inspector builds a wrapped document on demand, so invoking it is linear in
  document size and may briefly pause the UI on unusually large files. It exposes narrow grapheme
  splits but does not model IME composition, bidirectional text, or terminal/font width differences.
- Recovery has a nominal 500 ms first-write delay. Cooperative `SIGTERM` and `SIGHUP` requests are
  polled every 50 ms and drain exact dirty-tab journals before exit; a failed shutdown publication
  cancels exit and restores editing. `SIGKILL`, power loss, native crashes, a blocked event loop, or
  a supervisor's too-short TERM-to-KILL grace period can still lose newer in-memory keystrokes. The
  journal is not version history, a backup, or an autosave of the Markdown path. Recovery entries
  contain the draft's plaintext source in a private per-user state directory.
- Recovery mutations use advisory per-journal locks between cooperating TermWriter processes. An
  unrelated program can ignore them, and lock behavior on unusual or network filesystems remains
  filesystem-dependent.
- An orphan draft is revalidated immediately before its prompt, but not continuously while the
  prompt remains open. Another TermWriter instance can publish a newer journal during that decision;
  restoring and then editing the captured draft may supersede the newer recovery journal. The
  Markdown-file conflict guard still applies, but recovery metadata is not a multi-writer history.
- The watcher polls the active file plus one rotating inactive tab and scans visible workspace
  structure every two seconds. It is not an operating-system event watcher. Inactive changes set a
  persistent `!` tab state but never reload a hidden editor or open a dialog. Periodic file probes
  trust unchanged size, mtime, ctime, mode, file identity, and parent identity to skip hashing;
  explicit save, transition, activation, and focus checks remain full-content and authoritative.
  File probing and workspace scanning run in workers, so a completed check can arrive after the disk
  changed again. Very large workspace trees cost more to rescan.
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
- Conflict Save As, recovery retargeting, recovery archiving, and quarantine restoration depend on
  hard-link support in the relevant filesystem and fail cleanly when that publication mechanism is
  unavailable. Conflict Save As is currently available only from conflict recovery.
- A second hash check narrows but cannot eliminate the race between that check and `os.replace` if
  another process writes at exactly that moment. Cooperative file locking would not protect against
  editors that ignore the lock.
- Path validation rejects symlinks and resolved workspace escapes, but it is not hardened against a
  hostile process swapping intermediate path components during the initial open or a new Save As.
  Existing-file saves additionally compare file and parent identities and keep all temporary,
  cleanup, and replacement operations attached to the opened parent directory.
- Session metadata is last-writer-wins between concurrent TermWriter instances. Ctrl+O prunes only
  paths confirmed missing; entries that are temporarily inaccessible remain until they can be
  classified safely.
- Heading navigation emits prioritized status text. TermWriter
  does not currently integrate a native screen-reader announcement API, so it does not claim
  assistive-technology support beyond those terminal-visible cues.
- Preview links are deliberately non-opening. Raw document HTML, JavaScript, and shell text are not
  executed. Inline Markdown links are not native focusable Textual widgets, so TermWriter indexes
  their rendered action metadata within the pinned Textual 8.x compatibility range.
- Alerts use titled blockquotes without GitHub's colors or icons. Preview intentionally omits math,
  underline, subscript, and superscript. A repeated footnote's back arrow returns to the most recently
  followed reference, definition lists use quoted bodies, and images do not render terminal graphics.
- Smart Enter handles common list/task/blockquote prefixes, thematic breaks, fenced code, and the
  distinction between indented code and genuine nested lists. Ambiguous indentation parses the
  source prefix through the cursor, so Enter may have a small delay in extremely large files.
- A malformed `theme.tcss` can prevent startup until the file is corrected. Only a theme present at
  launch is watched; create the templates with `--init-config` before opening the TUI.
- Workspace text search returns one match per source line and caps results at 100. Fuzzy mode scans
  every candidate line before returning its globally strongest matches, with cooperative
  cancellation during long lines. Compound filters do not provide escaping for filenames containing
  commas. Regexes are limited to 500 characters and 50 ms per source line; a timed-out expression
  returns an error instead of results.
- Thread workers cannot stop an in-progress operating-system read or write. TermWriter ignores stale
  read/probe results; an atomic writer is deliberately allowed to finish while the UI stays locked.
- Thread cancellation cannot interrupt an individual operating-system directory or journal read,
  but workspace-index and recovery-read results are revisioned or ticketed and applied only from
  workers when still current.

## Near-term roadmap

1. Turn the experimental modal reader into an opt-in persistent reading pane and measure scroll
   stability while retaining immediate full-source fallback.
2. Design editing-grade block identities and range reconciliation as read-only diagnostics before
   permitting any source splice.
3. Measure whether already materialized inactive editors need a bounded eviction policy without
   weakening dirty source, undo, recovery, or conflict guarantees.

Implementation boundaries and tradeoffs are documented in
[`docs/architecture.md`](docs/architecture.md).
