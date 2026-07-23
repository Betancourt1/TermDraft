# TermDraft Rust architecture

This document describes the Rust implementation distributed as TermDraft. The Python/Textual
implementation under `src/termdraft` remains a historical compatibility and regression reference.

The Rust port keeps the same product boundary: ordinary files are authoritative, workspace content
cannot define commands or visual rules, the terminal owns only live editing state, and uncertain
overwrites fail closed.

## Runtime shape

```text
CLI target
   │
   ▼
Workspace validation ──► shallow index ────────────────► first Files frame
   │
   └────────────────────► background recursive index ──► Files / file finder
   │
   ▼
App state ─────────────────────────────────────────────► Ratatui render
   │                                                        ▲
   ├── keyboard / paste / mouse event ──────────────────────┤
   ├── background fresh-scan workspace-text search ─────────┤
   ├── 2 s active/inactive-file and workspace poll ─────────┤
   ├── 500 ms recovery flush ────────────────────────────────┤
   ├── SIGTERM / SIGHUP state drain ─────────────────────────┤
   └── content-free session publication ────────────────────┘
```

`rust/src/main.rs` parses the CLI, resolves configuration, validates the target, and either runs a
non-interactive command or starts the full-screen application. `rust/src/app.rs` owns the event loop,
tabs, transitions, overlays, search completion channel, session state, and recovery coordination.

There is no async runtime or general worker pool. Recursive workspace indexing and text search run
on cancellable revisioned threads; file reads/saves, mutations, session I/O, recovery operations,
and diagnostics otherwise run synchronously. Startup only scans the workspace root before the first
frame. This keeps ordering explicit and avoids blocking launch on a recursive walk, although another
slow filesystem operation can still delay a frame. Python performs most I/O through Textual workers.

## Module map

| Module | Responsibility |
| --- | --- |
| `main.rs` | CLI arguments, effective command reference, and `--inspect` |
| `app.rs` | modes, tabs, focus, overlays, events, polling, search worker, transitions, sessions, recovery UI |
| `ui.rs` | responsive Ratatui layout, workbench regions, popup rendering, inline status |
| `theme.rs` | six built-in palettes and final-frame semantic color mapping |
| `bindings.rs` | 53-action contract, parsing, scopes, collision/reserved-key validation |
| `config.rs` | strict compatible TOML, generated templates, paths, editor and recovery settings |
| `editor.rs` | `tui-textarea-2` setup, cursor styling, and inline presentation |
| `document.rs` | live/saved source, exact mixed source, encoding, line endings, conflict state, fingerprint |
| `workspace.rs` | target validation, recursive discovery, new-path containment, path-alias checks |
| `workspace_entries.rs` | create, copy, move, rename, and operating-system Trash safeguards |
| `persistence.rs` | stable reads and conflict-checked atomic/no-clobber publication |
| `path_filter.rs` | shared include/exclude path-filter contract |
| `search.rs` | file ranking, four-mode text search, document replace, and heading outline |
| `continuation.rs` | Markdown list, task, ordered-list, and quote continuation |
| `session.rs` | private content-free Python-compatible v3 tab/cursor/scroll/MRU state |
| `recovery.rs` | private Python-compatible v2 journals, locks, inventory, quarantine, retention, mutations |
| `markdown_help.rs` | Rust-supported syntax and limitation reference |
| `semantic_blocks.rs` | lossless read-only block ranges and reader presentation policy |
| `coordinate_diagnostic.rs` | character, byte, logical, wrapped, grapheme, and screen-coordinate mapping |

The crate forbids unsafe Rust. Domain modules do not depend on terminal rendering, so their path,
source, search, persistence, and recovery invariants can be tested directly.

## Workspace boundary and Files operations

`Workspace::from_target` accepts an existing directory or one supported file. A file target makes
its parent the workspace and opens that file immediately. Editable suffixes are `.md`, `.markdown`,
and `.txt`, case-insensitively.

The scanner uses `ignore::WalkBuilder`, does not follow symlinks, disables repository `.gitignore`
rules, and excludes `.git`, `.venv`, `node_modules`, and `__pycache__`. Startup returns a shallow
sorted root snapshot, then a revisioned background scan installs the full always-expanded directory
and supported-file index while preserving selection. `Files …` stays visible while indexing;
`Files ⚠ N` and a status detail preserve individual walk warnings. The two-second poll requests a
new background scan rather than walking the tree in the event loop.

The Files surface supports explicit file/folder creation, copy, cut, paste, rename, move, and
operating-system Trash. Mutations reject workspace escapes, selected or nested symlinks, ignored
paths, unsupported document destinations, replacement, folder self-copy/move, and unsafe open-tab
changes. Clean open paths and their content-free session/MRU state are retargeted; dirty or
conflicted paths are blocked. Moving a directory to Trash is also blocked if any open tab is inside
it. Copy reads disk source, while Duplicate intentionally publishes the live active-buffer source.

## Document source of truth

Each open `EditorTab` owns:

- one `Document` with current source, saved baseline, conflict/recovery state, and a `FileSnapshot`;
- one `TextArea` with independent cursor, selection, undo, and redo history;
- UTF-8 or UTF-8-with-BOM encoding metadata;
- uniform line-ending metadata or an exact `MixedSource` plus disclosed normalization target.

Dirty state is derived from current source versus the saved source, plus unresolved recovery
conflict. The application synchronizes the active textarea before save, search, recovery, session,
or a transition that needs the latest source.

Uniform LF, CRLF, and CR source is normalized for `TextArea` and re-encoded to its original form.
Mixed source retains both exact and normalized representations. Opening, reloading, or recovering a
mixed document requires Edit and normalize consent. Accepting does not change bytes by itself: an
untouched Save republishes the exact source. The first real edit switches to the disclosed target,
preferring CRLF when any CRLF sequence exists, then LF, then CR. Cancel opening removes the new tab;
cancel after reload/recovery keeps it read-only.

## Editor, views, and mouse regions

The Rust frontend preserves two interaction modes:

- **COMMAND** routes plain arrows plus configured keys to navigation/application actions and uses a
  block cursor.
- **WRITE** sends editing input to `tui-textarea-2` and uses a bar cursor.

It also preserves the two configured view modes:

1. **Inline** keeps the complete source textarea authoritative while semantically compacting
   inactive lines into rendered headings, emphasis, strong text, strikeout, inline code, links,
   nested lists, labeled code fences, and aligned tables. Delimiters are removed rather than
   replaced by equal-width gaps; the cursor line remains exact source.
2. **Split** shows the exact source editor beside a read-only semantic `pulldown-cmark` preview.
   Rust reparses that complete preview synchronously on each draw; Python instead uses a
   revisioned/debounced preview pipeline.

There is no separate Source view. In Inline or a narrow terminal, `v` switches between editor and
preview. In a wide Split layout, it hides or shows the preview. Reading-width limits affect only
rendering and never insert line breaks into the file.

Mouse regions support tab activation, source click positioning/selection, preview-link activation,
Files and overlay row selection/double-click, field/control focus, wheel scrolling, and dragging the
Files and Split dividers. Destructive overlay actions still require a click on their explicit action
label; clicks outside the popup remain inert.

Markdown continuation runs only for an unmodified Enter in editable WRITE mode without a
selection. The pure continuation service either continues a supported marker, ends an empty marker,
or requests an ordinary newline; the grouped editor history keeps that operation undoable as one
action.

## Search and navigation

Search surfaces share validated paths and source coordinates but remain separate:

- file finder fuzzy-ranks indexed relative paths and applies the common include/exclude filter;
- workspace search supports literal, fuzzy, whole-word, or Rust-regex matching, case selection,
  path filters, dirty/open document overrides, deterministic ordering, warnings, and at most 100
  matching lines;
- active-document search supports case selection, previous/next, single replace, Replace All, and
  search-only operation for read-only source;
- outline uses parsed CommonMark headings, including Setext headings, and jumps to the selected
  source line.

Each workspace-search submission performs a fresh recursive scan inside its background worker, so
new filesystem entries do not depend on the current Files snapshot. Every submission increments an
atomic revision; the worker checks cancellation between discovery and source units, and only the
current overlay/query accepts the completion. Discovery plus individual read/decode failures become
visible warnings. Clean open overrides prefer disk, while dirty/conflicted overrides prefer current
source. Rust's `regex` crate provides linear-time matching and deliberately omits
look-around/backreferences; both frontends cap patterns at 500 characters.

Replace All builds one final source edit so undo restores the whole operation in one step. The
outline filters parsed headings with Unicode-aware matching and can reveal a selection in source or
the preview.

## Persistence and external conflicts

`load_file` rejects final symlinks and non-regular files, uses `O_NOFOLLOW` on Unix, reads exact
bytes, validates stable identity/size, decodes UTF-8/BOM, and creates a saved snapshot.

For an existing file, `save_atomic`:

1. verifies the current destination against the loaded snapshot;
2. chooses exact untouched-mixed or encoded normalized source;
3. writes and synchronizes a same-directory temporary file with preserved ordinary mode bits;
4. verifies the destination snapshot again;
5. atomically publishes and synchronizes the directory;
6. advances the document baseline only after success.

New Save As, Duplicate, and export destinations use no-clobber publication. Rust does not retain an
open parent descriptor through publication, retry a moving read, or rehash the final published file
with Python's directory-sync uncertainty reporting. Python therefore remains stronger against a
concurrent parent-directory replacement race, although both reject every external change they
observe before publication.

Every two seconds Rust checks the active document, checks one rotating inactive tab, and requests a
background workspace scan:

- a clean changed active document reloads, including a mixed-ending consent step when required;
- a dirty changed document becomes a persistent conflict without replacing local source;
- deletion or unreadability becomes a conflict instead of recreating or guessing a rename;
- reverting local source to its baseline clears an ordinary conflict when disk matches again.

Inactive probes never activate the document. Clean external edits refresh that tab in place; dirty
changes, deletion, and unreadability install the same persistent conflict state used for the active
document.

Conflict overlays expose only valid decisions: Save local as is always available; Reload external
appears only for readable changed source; Continue without copy appears only for a clean
missing/unavailable close/quit transition; Escape cancels. Quit traverses dirty documents one at a
time, keeps the original MRU order, and checks every tab's disk state before completing.

## Sessions and recovery

Sessions use the content-free version-3 JSON shape shared with Python; recovery remains on the
compatible version-2 journal shape.

Sessions contain workspace-relative open paths, active path, cursor positions, and bounded MRU
views; they never contain source, baselines, or undo history. Rust opens the active restored document
first, then materializes at most one remaining tab per event-loop turn. Cursor position plus editor
and preview horizontal/vertical scroll offsets are restored when each view is laid out. Duplicate
paths, inconsistent active/open/view relationships, invalid relative paths, non-finite scrolls, and
size/count violations are rejected. Version-1 and version-2 files remain readable with missing
fields defaulted safely. Corrupt session state is ignored with a visible warning and can be replaced
by the next valid publication.

Dirty documents are journaled on a nominal 500 ms event-loop interval. Journals contain exact
source, encoding, workspace/path, saved baseline, and timestamp. Private atomic publication and
advisory Unix per-journal `flock` locks protect cooperating mutations. Exact fingerprints make discard,
retarget, quarantine, restore, export, deletion, and retention reject stale inventory rather than
silently acting on newer bytes.

Readable recovery for an existing file offers Restore, Use disk, or Later. A baseline mismatch
restores as a conflict, and mixed recovered source still requires explicit normalization consent.
The Recovery Manager inventories active and quarantined valid/missing/orphan/corrupt records,
protects dirty open journals and case/Unicode spelling aliases, and supports:

- Open, Retarget, and Archive for valid active records;
- Restore, Export copy, and separately confirmed permanent deletion for quarantine;
- configured age-based cleanup of the exact confirmed valid quarantine inventory;
- per-record cleanup failures rather than a false all-success result.

Missing/orphan records can also be opened from Manager. Rust installs their preserved source with
the journal's original baseline in a protected missing/unavailable conflict: ordinary Save cannot
recreate the original path, while Save As publishes to a validated no-clobber workspace path and
retires the recovery journal. Python additionally offers those records automatically during a
directory launch. Rust still installs a readable disk tab before showing ordinary crash recovery,
so Escape means Later rather than Python's pre-install Cancel opening.

## Configuration and command discovery

`config.rs` resolves explicit `--config-dir`, `TERMDRAFT_CONFIG_HOME`, legacy
`TERMWRITER_CONFIG_HOME`, existing canonical/legacy roots, then `~/.termdraft`. TOML rejects unknown
sections/fields, invalid editor values, zero or unrepresentable retention ages, unknown binding IDs,
empty/malformed keys, reserved preview controls, duplicate tokens, and cross-action collisions.

The generated `config.toml` template documents all 53 binding IDs. Rust applies every global,
editor, preview, and COMMAND override; effective mappings drive runtime dispatch, the 33-action
palette, `?`, and `--commands`. `R` validates and applies editor, view, retention, and binding changes
as one replacement; a failed reload leaves the previous configuration untouched. Startup mode
remains startup-only.

`theme.tcss` is created as a no-clobber compatibility template but is never parsed or watched.
Rust instead provides Paper, Linen, and Mist light themes plus Midnight, Void, and Carbon dark
themes. `t` cycles them for the current run; `--safe-mode` remains behaviorally redundant.

The command palette preserves Python's 32 actions and adds the native Change theme action, rendered
as a searchable grouped two-column grid with descriptions and a compact fallback for narrow
terminals. Rust's `?` screen is a scrollable 28-row action summary; `--commands` is the fuller action
reference. Neither substitutes for the underlying editor-key inventory in
[RUST_PORT.md](../RUST_PORT.md).

## Terminal lifecycle

The runtime enters raw mode and the alternate screen, enables mouse capture, and changes cursor
shape with the interaction mode. Ratatui and `TerminalExtrasGuard` restore alternate screen, raw
mode, mouse reporting, and cursor shape on normal return, partial startup failure, and Rust panic.

On Unix, `SIGTERM` and `SIGHUP` set a cooperative shutdown flag. The event loop then publishes the
latest dirty recovery source and content-free session before Ratatui and `TerminalExtrasGuard`
restore mouse reporting, alternate-screen state, cursor shape/color, and cursor visibility. `SIGKILL`
and machine-level termination remain outside any process's cooperative cleanup boundary.

## Verification boundary

The Rust suite covers bindings/config generation and reload, encoding and exact mixed-source
behavior, stable conflict rejection, workspace mutations, search/replace, inline fidelity, main
mouse regions, sessions, recovery core/manager, semantic diagnostics, UI rendering, and dirty
transitions. The standard local gates are:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
```

At this checkpoint, 206 Rust library tests and 4 Rust binary tests pass. The Python reference suite
passes 681 tests with 2 expected platform skips. The exhaustive interface and gap inventory, plus
the explicitly historical benchmark, live in [RUST_PORT.md](../RUST_PORT.md).
