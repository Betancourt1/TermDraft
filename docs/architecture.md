# TermDraft Rust architecture

This document describes the implementation on the `rust-port` branch. The released
[Python/Textual architecture](https://github.com/Betancourt1/TermDraft/blob/main/docs/architecture.md)
remains available on `main`.

The port keeps a deliberately small boundary: ordinary files are authoritative, the terminal UI
owns only live editing state, and persistence refuses uncertain overwrites.

## Runtime shape

```text
CLI target
   │
   ▼
Workspace validation ──► recursive file index
   │
   ▼
App state ──────────────► Ratatui render
   │                         ▲
   ├── keyboard event ───────┤
   ├── 2 s disk poll ─────────┤
   ├── 500 ms recovery flush ─┤
   └── session publication ───┘
```

`rust/src/main.rs` parses the CLI, resolves configuration, validates the target, and either runs a
non-interactive command or starts the terminal application. `rust/src/app.rs` owns the single event
loop and all transitions. There is no async runtime or background worker pool in this comparison
build.

The initial workspace scan is synchronous. This makes startup behavior simple and deterministic,
but a very large workspace can delay the first frame. The Python app indexes asynchronously, which
is an important difference when interpreting startup benchmarks.

## Module map

| Module | Responsibility |
| --- | --- |
| `main.rs` | CLI arguments and non-interactive commands |
| `app.rs` | modes, tabs, focus, overlays, events, polling, and transitions |
| `ui.rs` | Ratatui layout and rendering |
| `editor.rs` | `tui-textarea-2` setup, cursor styling, and inline presentation |
| `workspace.rs` | target validation, recursive discovery, and new-path containment |
| `document.rs` | live source, saved baseline, encoding, line endings, and fingerprint |
| `persistence.rs` | stable reads and conflict-checked atomic publication |
| `search.rs` | fuzzy file matching, literal text search, and heading outline |
| `continuation.rs` | Markdown list, task, ordered-list, and quote continuation |
| `config.rs` | strict compatible TOML configuration |
| `session.rs` | content-free v2 tab and cursor state |
| `recovery.rs` | private v2 dirty-source journals |

The crate forbids unsafe Rust. Domain modules do not depend on the terminal event loop, which keeps
file and state invariants directly testable without rendering a real terminal.

## Workspace boundary

`Workspace::from_target` accepts an existing directory or one supported file. A file target makes
its parent the workspace and opens that file immediately. Editable suffixes are `.md`, `.markdown`,
and `.txt`, case-insensitively.

The workspace scanner uses `ignore::WalkBuilder`, does not follow symbolic links, and excludes
`.git`, `.venv`, `node_modules`, and `__pycache__`. It deliberately does not apply repository
`.gitignore` rules because the editor is browsing user documents rather than build inputs.

New files, Save As destinations, and duplicates must be relative paths whose parent already exists
inside the workspace. Absolute paths, `..`, symbolic-link escapes, unsupported suffixes, and
existing destinations are rejected. The Rust port does not create directories or mutate existing
workspace entries beyond saving an open document.

## Document source of truth

Each open `EditorTab` owns:

- one `Document` with normalized LF source and its saved baseline;
- one `TextArea` with independent cursor, selection, and undo history;
- encoding and original line-ending metadata;
- one `FileSnapshot` for conflict detection.

`FileSnapshot` combines SHA-256, size, modification time, permission mode, device, and inode. Dirty
state is a source comparison against `saved_text`, plus any explicit disk conflict. The application
synchronizes the active textarea into `Document.text` before save, search, recovery, or transitions
that need current source.

Only UTF-8 and UTF-8 with BOM are editable. LF, CRLF, and CR are normalized for the textarea and
encoded back to their original form on save. Mixed line endings open read-only because choosing one
ending would silently rewrite source bytes.

## Editor and rendering

The Rust frontend preserves two explicit modes:

- **COMMAND** routes keys to navigation and application actions and uses a block cursor.
- **WRITE** sends editing keys to `tui-textarea-2` and uses a bar cursor.

The current view cycles through:

1. **Inline** — the full-source textarea remains mounted. Inactive lines receive presentation-only
   styles for headings, emphasis, strong text, strikeout, inline code, links, tasks, bullets, and
   table separators. The cursor line remains exact source.
2. **Split** — the exact source editor and a `tui-markdown` preview share the workbench.
3. **Source** — only the exact source editor is drawn.

Inline presentation changes styles, not characters. It is cleared after every frame and does not
participate in save, undo, search, or recovery. The centered editor is capped visually at 108
terminal columns; this never inserts line breaks into the file.

Markdown continuation is evaluated only for an unmodified Enter key in WRITE mode with no active
selection. The continuation service returns a small action: continue a marker, end an empty marker,
or insert a normal newline.

## Search and navigation

The Files pane is an always-expanded snapshot of the workspace. A two-second poll rescans it and
tries to retain the selected path. The explorer is not a watched, collapsible tree.

Search surfaces are intentionally separate:

- file finder performs fuzzy matching over indexed relative paths;
- workspace search performs a literal search and returns at most 100 matches;
- active-document find delegates to the textarea search API;
- outline extracts ATX headings and jumps to the selected line.

Workspace search loads unopened files when needed. Search results identify the file, one-based line
in the UI, and a compact preview. Replace is not implemented.

## Persistence and conflicts

`load_file` rejects final symbolic links and non-regular files, opens with `O_NOFOLLOW` on Unix,
reads the bytes once, and checks that the opened file identity and size stayed stable. It then
detects the BOM and line-ending form and constructs the saved fingerprint.

`save_atomic` follows this publication sequence:

1. verify the current file against the opening snapshot;
2. encode source with the original BOM and line ending;
3. write a same-directory temporary file with preserved permission bits;
4. flush and synchronize the temporary file;
5. verify the destination snapshot a second time;
6. atomically publish, refusing an existing destination for no-clobber operations;
7. synchronize the parent directory;
8. replace the document baseline only after success.

If an external change invalidates the snapshot, the save returns a conflict instead of overwriting
the other version. Save As remains available because it publishes to a new no-clobber path.

This is strong enough for the comparison port but not identical to the Python persistence service.
The Rust path does not keep every ancestor bound to open directory descriptors, so the Python app
remains stronger against a concurrent parent-directory replacement race. Cross-process save and
recovery locks are also not ported.

## External changes

Every two seconds, the event loop checks the active document and refreshes the workspace index:

- a clean active document changed on disk is reloaded and keeps its cursor as closely as possible;
- a dirty active document changed on disk is marked as a conflict and retains the local source;
- deletion marks an open document as conflicted instead of guessing whether a rename occurred;
- newly discovered files appear in Files without retargeting existing tabs.

Inactive tabs are not checked in a background rotation; after a tab switch, the newly active tab is
checked on the next two-second pass. The poll is deliberately simple: there is no filesystem watcher
and no attempt to infer file identity across rename or move operations.

## Sessions and crash recovery

Sessions and recovery use JSON version 2 shapes compatible with the Python application.

Sessions contain workspace-relative open paths, the active tab, and cursor positions. They never
contain Markdown source, accept at most 100 documents, are capped at 512 KiB, and are published
atomically with private permissions where supported. An invalid session is ignored for restoration
with a visible warning and may be replaced by the next valid session publication.

Dirty documents are journaled on a nominal 500 ms event-loop interval. A recovery journal contains
the source, encoding, workspace, document path, saved baseline, and timestamp. On open, the user can
Restore, Use disk, or decide Later. A baseline mismatch restores as a conflict, which requires Save
As rather than an unsafe overwrite. Journals are removed only after an exact save or discard check.

The Rust implementation does not include the Python recovery inventory, quarantine, retention
cleanup, missing/orphan draft workflows, or cross-process locks. The compatible
`recovery.retention_days` setting is parsed but no Rust cleanup job currently consumes it.

## Configuration

`config.rs` resolves an explicit `--config-dir`, `TERMDRAFT_CONFIG_HOME`, the legacy
`TERMWRITER_CONFIG_HOME`, existing canonical/legacy home directories, and finally
`~/.termdraft`. TOML parsing rejects unknown sections and fields.

The Rust frontend applies startup mode, startup view, soft wrapping, line numbers, and Markdown list
continuation. It reports compatible keybinding overrides but does not apply them. It creates
`theme.tcss` as a no-clobber compatibility template but does not evaluate Textual CSS. Configuration
is loaded once at startup.

## Terminal lifecycle

The runtime enters raw mode and the alternate screen, enables mouse capture for terminal parity,
and changes cursor shape with the editing mode. `TerminalExtrasGuard` restores mouse reporting,
cursor shape, alternate screen, and raw mode on normal exit, partial startup failure, or panic.

Mouse events are not mapped to application actions in this port.

## Verification boundary

Rust tests cover document encoding, line endings, stable conflict rejection, workspace containment,
no-clobber paths, Unicode search, inline source fidelity, external changes, dirty prompts, sessions,
and recovery. The standard local gates are:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
```

The unchanged Python suite remains a product oracle for the mature behavior that has not been
ported. [RUST_PORT.md](../RUST_PORT.md) records the tested checkpoint, PTY acceptance, omissions,
and performance comparison.
