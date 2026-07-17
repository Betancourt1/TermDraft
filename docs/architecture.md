# TermDraft Rust architecture

This document describes the implementation on the `rust-port` branch. The released
[Python/Textual architecture](https://github.com/Betancourt1/TermDraft/blob/main/docs/architecture.md)
remains the reference for the public `termdraft` package.

The Rust port keeps the same product boundary: ordinary files are authoritative, workspace content
cannot define commands or visual rules, the terminal owns only live editing state, and uncertain
overwrites fail closed.

## Runtime shape

```text
CLI target
   │
   ▼
Workspace validation ──► synchronous recursive index ──► Files / file finder
   │
   ▼
App state ─────────────────────────────────────────────► Ratatui render
   │                                                        ▲
   ├── keyboard / paste / mouse event ──────────────────────┤
   ├── background workspace-text search ────────────────────┤
   ├── 2 s active-file and workspace poll ──────────────────┤
   ├── 500 ms recovery flush ────────────────────────────────┤
   └── content-free session publication ────────────────────┘
```

`rust/src/main.rs` parses the CLI, resolves configuration, validates the target, and either runs a
non-interactive command or starts the full-screen application. `rust/src/app.rs` owns the event loop,
tabs, transitions, overlays, search completion channel, session state, and recovery coordination.

There is no async runtime or general worker pool. Workspace text search runs on a cancellable
revisioned thread; initial indexing, file reads/saves, mutations, session I/O, recovery operations,
and diagnostics otherwise run synchronously. This keeps ordering explicit, but a large initial scan
or slow filesystem operation can delay a frame. Python prioritizes the first document and performs
most of that work through Textual workers.

## Module map

| Module | Responsibility |
| --- | --- |
| `main.rs` | CLI arguments, effective command reference, and `--inspect` |
| `app.rs` | modes, tabs, focus, overlays, events, polling, search worker, transitions, sessions, recovery UI |
| `ui.rs` | responsive Ratatui layout, workbench regions, popup rendering, inline status |
| `bindings.rs` | canonical 52-action contract, parsing, scopes, collision/reserved-key validation |
| `config.rs` | strict compatible TOML, generated templates, paths, editor and recovery settings |
| `editor.rs` | `tui-textarea-2` setup, cursor styling, and inline presentation |
| `document.rs` | live/saved source, exact mixed source, encoding, line endings, conflict state, fingerprint |
| `workspace.rs` | target validation, recursive discovery, new-path containment, path-alias checks |
| `workspace_entries.rs` | create, copy, move, rename, and operating-system Trash safeguards |
| `persistence.rs` | stable reads and conflict-checked atomic/no-clobber publication |
| `path_filter.rs` | shared include/exclude path-filter contract |
| `search.rs` | file ranking, four-mode text search, document replace, and heading outline |
| `continuation.rs` | Markdown list, task, ordered-list, and quote continuation |
| `session.rs` | private content-free Python-compatible v2 tab/cursor/MRU state |
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
rules, and excludes `.git`, `.venv`, `node_modules`, and `__pycache__`. It returns one sorted,
always-expanded snapshot containing directories and supported files. A two-second rescan updates
Files while trying to preserve the selected path. Individual walk errors are skipped rather than
shown as Python-style scan warnings.

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

Mouse regions support workbench focus, Files row selection/double-click, Files/editor/preview wheel
scrolling, and dragging the Files and Split dividers. Tabs, source click positioning/selection,
preview links, and every overlay remain keyboard-only.

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

Workspace search is the one background operation. It searches the existing Files snapshot captured
when the request is submitted rather than performing Python's fresh workspace scan. Every submission
increments an atomic revision; the worker checks cancellation between source units, and only the
current overlay/query accepts the completion. Clean open overrides prefer disk, dirty/conflicted
overrides prefer current source, and individual read/decode failures become warnings. Rust's `regex`
crate provides linear-time matching and deliberately omits look-around/backreferences; both
frontends cap patterns at 500 characters.

Replace All builds one final source edit so undo restores the whole operation in one step. The Rust
outline does not yet expose Python's query field or Show in preview destination.

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

Every two seconds Rust checks the active document and rescans the workspace:

- a clean changed active document reloads, including a mixed-ending consent step when required;
- a dirty changed document becomes a persistent conflict without replacing local source;
- deletion or unreadability becomes a conflict instead of recreating or guessing a rename;
- reverting local source to its baseline clears an ordinary conflict when disk matches again.

Conflict overlays expose only valid decisions: Save local as is always available; Reload external
appears only for readable changed source; Continue without copy appears only for a clean
missing/unavailable close/quit transition; Escape cancels. Quit traverses dirty documents one at a
time, keeps the original MRU order, and checks every tab's disk state before completing.

Rust does not rotate inactive tabs through background probes. A previously inactive file is checked
after activation on the next poll.

## Sessions and recovery

Sessions and recovery use version-2 JSON shapes compatible with Python.

Sessions contain workspace-relative open paths, active path, cursor positions, and bounded MRU
views; they never contain source, baselines, or undo history. Rust eagerly loads restored tabs and
restores cursors. It reads/writes compatible scroll fields as zero, so viewport restoration remains
a Python-only behavior. Rust validates individual relative paths and size/count bounds, but lacks
Python's duplicate-path and active/open/view cross-field checks. Corrupt session state is ignored
with a visible warning and can be replaced by the next valid publication.

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

Missing/orphan records remain visible and can be retargeted or archived, but Rust cannot open their
source into an unavailable-path conflict because it cannot construct a trustworthy `FileSnapshot`.
Python offers those records automatically during a directory launch and can restore them for Save
As. Rust also installs a readable disk tab before showing crash recovery, so Escape means Later
rather than Python's pre-install Cancel opening.

## Configuration and command discovery

`config.rs` resolves explicit `--config-dir`, `TERMDRAFT_CONFIG_HOME`, legacy
`TERMWRITER_CONFIG_HOME`, existing canonical/legacy roots, then `~/.termdraft`. TOML rejects unknown
sections/fields, invalid editor values, zero or unrepresentable retention ages, unknown binding IDs,
empty/malformed keys, reserved preview controls, duplicate tokens, and cross-action collisions.

The generated `config.toml` template documents all 52 binding IDs. Rust applies every global,
editor, preview, and COMMAND override; effective mappings drive runtime dispatch, the 32-action
palette, `?`, and `--commands`. `R` validates and applies editor, view, retention, and binding changes
as one replacement; a failed reload leaves the previous configuration untouched. Startup mode
remains startup-only.

`theme.tcss` is created as a no-clobber compatibility template but is never parsed or watched.
Rust always uses its built-in theme, so `--safe-mode` is behaviorally redundant.

The command palette contains the same 32 actions in the same six groups/order as Python, rendered as
a searchable grouped two-column grid with descriptions and a compact fallback for narrow terminals.
Rust's `?` screen is a scrollable 26-row action summary; `--commands` is the fuller TermDraft-action
reference. Neither substitutes for the underlying editor-key inventory in
[RUST_PORT.md](../RUST_PORT.md).

## Terminal lifecycle

The runtime enters raw mode and the alternate screen, enables mouse capture, and changes cursor
shape with the interaction mode. Ratatui and `TerminalExtrasGuard` restore alternate screen, raw
mode, mouse reporting, and cursor shape on normal return, partial startup failure, and Rust panic.

Unlike Python's CLI, Rust does not install cooperative SIGTERM/SIGHUP handlers that drain dirty
recovery and session queues before exiting. Forced process termination can therefore leave terminal
state and the newest sub-500 ms edit outside the normal cleanup path.

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

On the current branch, 159 Rust library tests and 3 Rust binary tests pass. The Python suite remains
the product oracle and passes 681 tests with 2 expected platform skips. The exhaustive interface and
gap inventory, plus the explicitly historical benchmark, live in [RUST_PORT.md](../RUST_PORT.md).
