# TermDraft Python/Rust parity inventory

This branch compares the released Python/Textual application with a standalone Rust/Ratatui
frontend. Both edit the same ordinary `.md`, `.markdown`, and `.txt` files and use compatible
configuration, session, and recovery formats.

Rust is now the likely primary direction: it preserves nearly all of the keyboard-first workflow,
the current branch feels materially more immediate in use, and the historical measurements below
show much lower process-start and first-frame latency. That is a direction, not a release switch.
The published `termdraft` command and Homebrew formula still install Python 1.2; Rust remains the
branch-local `termdraft-rs` binary until its remaining gaps and distribution work are accepted.

## Run either implementation

Rust 1.88 or newer is required.

```bash
git clone --branch rust-port --single-branch https://github.com/Betancourt1/TermDraft.git
cd TermDraft
cargo run --release --locked -- ~/Documents/notes
cargo run --release --locked -- essay.md
```

To install only the branch-local executable:

```bash
cargo install --path . --locked
termdraft-rs ~/Documents/notes
```

The Python reference remains available from a prepared checkout as `termdraft` or
`.venv/bin/termdraft`.

Both implementations normally share compatible configuration, session, and recovery locations.
Sessions contain no Markdown; recovery journals contain private dirty source. For an isolated Rust
comparison, set `XDG_STATE_HOME=/tmp/termdraft-rs-state` and pass
`--config-dir /tmp/termdraft-rs-config`.

## Inventory snapshot

This inventory was recounted from the Python source and Rust checkpoint `8a95a4e`:

- both command palettes contain exactly **32 actions** in the same six groups and order;
- both configuration contracts contain exactly **52 binding IDs**, all of which reach implemented
  Rust behavior;
- Python defines 19 modal dialog classes plus its command palette, or **20 concrete modal types**;
- Rust defines 23 `Overlay` variants; `SearchResults` and `Message` have no construction site, so
  **21 overlay types are currently user-reachable**;
- shared dialog types serve several workflows, so the operation-by-operation popup matrix below is
  the authoritative interface comparison;
- the Rust suite passes **151 library tests plus 3 binary tests**; the Python suite passes
  **681 tests with 2 platform skips**.

Neither frontend has a native menu bar. In both, the searchable command palette is the application
menu; the focused Files key layer is a second contextual menu.

## Now parity-complete

| Feature | Current shared behavior |
| --- | --- |
| Local file model | Direct editing of `.md`, `.markdown`, and `.txt`; no database or workspace command execution |
| Main workbench | Title, optional tabs, Files pane, editor/preview workbench, persistent mode/status line |
| Interaction modes | Explicit COMMAND and WRITE modes, block/bar cursors, `Esc` protection, Yazi-style movement |
| Runtime tabs | Independent buffers and undo histories; open-file reuse; next, previous, and guarded close |
| Command palette | The same 32 actions, six groups, group order, labels, and effective shortcuts |
| Configurable commands | The same 52 stable binding IDs; generated template entries, strict parsing, collision checks, effective overrides, and `R` reload |
| File finding | Fuzzy relative-path search, shared include/exclude filters, normalized matching, bounded ranking |
| Workspace text search | Literal, fuzzy, whole-word, and regex modes; case toggle; path filter; dirty/open overrides; warnings; deterministic 100-result bound; cancellable revisioned background search |
| Active-document search | Case-aware find, previous/next, single replace, Replace All, read-only search, and one-step undo for Replace All |
| Recent documents | Content-free MRU order independent of open-tab order, missing-path pruning, and reopening closed documents |
| Workspace mutations | Create file/folder, copy, cut, paste, rename, move, and operating-system Trash with no-clobber, containment, ignored-path, and symlink protection |
| Save As and Duplicate | Workspace-relative no-clobber publication; Save As retargets; Duplicate leaves the original active and dirty |
| Encoding and ordinary line endings | UTF-8/UTF-8 BOM plus uniform LF, CRLF, or CR round trips |
| Mixed line endings | Explicit Edit and normalize consent; cancel opening or keep read-only; accepted but untouched source remains byte-identical; the first edit uses the same CRLF-first normalization target |
| External conflicts | Local source remains in memory; Save local as, Reload external, Continue without copy when safe, and Cancel are state-gated |
| Dirty close and quit | Each dirty document gets its own filename-specific Save, Discard, or Cancel decision; Enter does not confirm destruction |
| Markdown continuation | Bullets, ordered markers, tasks, quotes, empty-marker termination, and ordinary Enter fallback |
| Recovery storage | Python-compatible v2 journals, exact fingerprints, advisory Unix cross-process mutation locks, inventory, retarget, quarantine, restore, export, permanent deletion, and configured retention cleanup |
| Recovery Manager for readable sources | `M`/palette access, active/quarantined/corrupt inventory, protected dirty drafts, target input, guarded actions, destructive confirmations, and per-record cleanup errors |
| Diagnostic references | Markdown syntax, semantic source-block inspector, experimental block reader, and cursor-coordinate inspector |
| Normal terminal lifecycle | Alternate screen, raw mode, mouse reporting, and cursor shape restore after normal exit, error unwinding, or Rust panic |

## Intentionally or differently implemented

| Area | Python/Textual | Rust/Ratatui | User-visible effect |
| --- | --- | --- | --- |
| Executable and runtime | Published `termdraft`; Python/Textual and Nerd Font icons | Branch-local `termdraft-rs`; one native executable and ordinary Unicode symbols | Rust starts with less runtime overhead and needs no Nerd Font; Python remains the supported package |
| Shell chrome | Textual styling and Nerd Font glyphs | Monochrome Ratatui chrome, `RUST PORT` badge, `▸`, `◆`, and `›` | Same hierarchy, not pixel parity |
| Palette layout | Responsive grouped two-column cheatsheet that stacks when narrow; descriptions below | One searchable list with a group column and shortcut on every row | Actions and order match; Rust shows less explanatory copy at once |
| Shortcut help | Generated exhaustive effective reference | 25-row effective summary | Rust `--commands` is the exhaustive reference; `?` is intentionally more compact |
| Preview engine | `markdown-it-py`/Textual with tables, tasks, alerts, footnotes, definitions, link selection, and internal footnote navigation | Presentation-only inline styling plus a best-effort `tui-markdown` split preview | Source remains authoritative in both; richer preview behavior is still Python-only |
| Explorer model | Lazy, collapsible Textual `DirectoryTree` plus asynchronous indexing | Always-expanded sorted snapshot from `ignore::WalkBuilder` | Rust is simpler and predictable; large trees can occupy more space and delay first frame |
| Search regex engine | Python `regex`, full case folding, and a per-line timeout | Rust `regex`, a linear-time syntax subset, and the same 500-character input limit | Common regexes work; look-around/backreferences accepted by Python are not Rust syntax |
| I/O scheduling | Most scans, reads, probes, saves, mutations, sessions, recovery, and semantic work run in Textual workers | Workspace text search has a background thread; most other I/O runs in the event loop | Rust has less coordination code, but a slow filesystem operation can pause drawing |
| Session restoration | Deferred inactive tabs plus cursor and scroll restoration | Eager tab loading, cursor restoration, and zeroed scroll fields in the compatible v2 file | Tab/order/MRU parity; viewport restoration differs |
| External monitoring | Active tab plus rotating inactive-tab probes and focus checks | Active document plus a two-second workspace rescan | An inactive Rust tab is checked after activation, not proactively |
| Save hardening | Parent-descriptor-bound publication, stable-read retry, post-publication digest verification, detailed directory-sync uncertainty | Same-directory atomic save with two destination snapshot checks and directory sync, but path-based publication | Both reject observed conflicts; Python remains stronger against parent-directory replacement races |
| Recovery startup | Decisions occur before a document is installed; directory launches also offer missing/orphan drafts | Existing readable documents open before Restore/Use disk/Later; missing/orphan records stay visible in Manager | Normal crash recovery works; startup and unavailable-source flows differ |
| Feedback | Persistent status plus Textual notifications/toasts | Persistent status messages | Rust is quieter and keeps transient feedback in one line |
| Mouse | Textual tabs, tree, editor, preview links, inputs, rows, and buttons participate | Main-pane focus, Files selection/double-click, wheel scrolling, and divider dragging | Rust overlays remain keyboard-only and editor/tab clicking is incomplete |

## Still missing from the Rust frontend

These are current gaps, not items that were merely absent in the early port:

1. **Rich preview extensions and interaction.** Rust does not reconstruct bordered GFM tables,
   task/alert/definition/footnote presentation, image placeholders, link selection, internal
   footnote/backlink navigation, or outline-to-preview reveal. Preview links and footnotes are
   deliberately inert.
2. **Outline query and destination controls.** Rust lists parsed headings and jumps to source, but
   has no filter input and no separate Show in preview action.
3. **Collapsible/lazy Files and proactive inactive-tab monitoring.** Rust uses an always-expanded
   synchronous snapshot, silently drops individual walk errors, and checks only the active document.
4. **Complete mouse interaction.** Tabs, editor click positioning/selection, preview links, and all
   overlays are keyboard-only. The implemented main-pane mouse actions are listed below.
5. **Missing/orphan recovery opening.** Manager inventories and can retarget/archive unavailable
   records, but Rust cannot install their source safely without a trustworthy `FileSnapshot`.
   Python can restore them into an unavailable-path conflict and then Save As.
6. **Python-equivalent startup recovery cancellation.** Rust `Esc` means Later after the readable
   disk document is already open; Python can Cancel opening before installing it.
7. **Session scroll offsets and lazy tab materialization.** Rust stores compatible zero scroll
   fields and loads restored tabs immediately.
8. **`theme.tcss`.** Rust creates the compatibility template but never parses or watches Textual
   CSS; `--safe-mode` therefore has no additional visual effect.
9. **Full background-I/O and cooperative signal shutdown.** Rust lacks Python's critical-operation
   worker/ticket pipeline and its SIGTERM/SIGHUP recovery-and-session drain.
10. **The deepest Python persistence guarantees.** Rust does not bind publication to an open parent
    descriptor, retry a moving read, or verify the final published digest with Python's uncertainty
    reporting.
11. **Public distribution.** There is no Rust release workflow, downloadable artifact, crates.io
    package, Homebrew formula, or stable Rust tag namespace yet.

## Main interface comparison

| Surface | Python/Textual | Rust/Ratatui | Status |
| --- | --- | --- | --- |
| Title | App/workspace title | Same plus `RUST PORT` | Deliberate badge |
| Tabs | Clickable Textual tabs with modified/conflict state | Keyboard tabs with `●` modified and `!` conflict indicators | Keyboard parity; mouse gap |
| Files | Collapsible tree, icons, click and keyboard | Always-expanded snapshot, Unicode symbols, click/double-click and keyboard | Different tree model |
| Workbench | Inline editor or resizable split source/preview | Inline editor or resizable split source/preview | Layout parity |
| `v` in Inline/narrow | Switch editor and preview | Switch editor and preview | Parity |
| `v` in wide Split | Show/hide preview | Show/hide preview | Parity |
| Modes | COMMAND/WRITE in status; block/bar cursor | COMMAND/WRITE in status; block/bar cursor | Parity |
| Status | Mode, focus, path/state, word/position/progress, notifications | Mode, focus, path/state, word/position/progress, one-line messages | Same hierarchy, quieter feedback |
| Reading width | Presentation-only cap; source bytes unchanged | Presentation-only cap; source bytes unchanged | Parity |
| Dividers | Mouse-resizable Files and split panes | Mouse-resizable Files and split panes | Parity |
| Narrow layout | One active editor/preview pane | One active editor/preview pane | Parity |
| Popup chrome | Framed, focus-driven Textual action panels | Square bordered keyboard panels | Recognizably aligned, not pixel parity |

There is no third Source view in either current interface. Inline is always a source editor with
presentation-only styling; Split shows that source editor beside the read-only preview.

## Complete menu inventory

The final Rust palette mirrors the Python palette exactly. The table lists every menu action in
display order; the key shown is its default COMMAND/context shortcut and follows effective remaps
where the action has a configurable ID.

| Group | Actions in order | Default keys |
| --- | --- | --- |
| DOCUMENT | Save; Save as; Duplicate; Find file; Recent documents; Close tab | `w`; `W`; `D`; `f`; `o`; `C` |
| NAVIGATE | Next tab; Previous tab; Search workspace; Find and replace; Outline; Explorer | `]`; `[`; `/`; `s`; `S`; `e` |
| FILES | Create; Copy; Cut; Paste; Rename; Move; Trash | `a`; `c`; `x`; `p`; `r`; `m`; `d` |
| MODE | Write mode; Command mode | `i`; `Esc` |
| EDIT | Undo; Redo; Reload config; Inspect blocks; Read blocks | `u`; `U`; `R`; `b`; `B` |
| VIEW | Preview; Recovery drafts; Shortcut help; Markdown help; Cursor coordinates; Quit | `v`; `M`; `?`; `K`; `I`; `q` |

Python presents these as six visible groups in a responsive grid. Rust keeps one visible list and
prints the group on every row. Both fuzzy-filter the complete 32-action set.

## Complete popup and window inventory

| User-facing surface | Python/Textual | Rust/Ratatui | Status or changed behavior |
| --- | --- | --- | --- |
| Command palette | Search, six responsive groups, descriptions, mouse/keyboard | Search, same 32 actions/groups/order, one list, keyboard | Functional parity; layout/mouse differ |
| Shortcut help | Full effective binding reference | 25-row effective summary | Rust is intentionally shorter; `--commands` is exhaustive |
| Markdown syntax help | `HelpDialog` with supported syntax and examples | Dedicated scrollable Markdown Help overlay | Parity, with Rust limitations stated truthfully |
| Find file | Query, include/exclude filter, ranked results | Same fields, filter contract, ranking, and open action | Parity |
| Workspace search | Query, literal/fuzzy/word/regex, case, filter, warnings/results | Same controls and asynchronous revision/cancellation behavior | Parity |
| Recent documents | MRU list and open | MRU list and open | Parity |
| Find and replace | Query, replacement, case, previous/next, replace/all | Same, including F3/Shift+F3 and one-step Replace All undo | Parity |
| Document outline | Filter, source jump, preview reveal | Heading list and source jump | Rust lacks filter and preview destination |
| Save As | Suggested relative path, validation, Save/Cancel buttons | Blank relative-path input, validation, Enter/Esc | Same publication; suggestion and mouse controls differ |
| Conflict copy | Save local version under a new path | Same path input after `s` | Parity |
| Duplicate | Suggested relative path and buttons | Blank relative-path input and keyboard | Same publication; suggestion and mouse controls differ |
| Create file/folder | Relative input; trailing slash selects folder | Relative input; trailing slash selects folder | Parity |
| Rename | Source-aware relative input and buttons | Source-aware input and keyboard | Parity |
| Move | Workspace-relative destination and buttons | Workspace-relative destination and keyboard | Parity |
| Trash confirmation | File/folder detail, hidden-content warning, confirm/cancel | Same safety detail; `y`/Esc | Functional parity; mouse differs |
| Unsaved close | Save, Discard, Cancel for the active file | `y`, `n`, Esc for the active file | Parity; Enter inert |
| Unsaved quit | Per-document filename-specific traversal | Per-document filename-specific traversal | Parity; original active/MRU order restored |
| External conflict | Save local as; Reload when possible; Continue without copy when safe; Cancel | `s`, conditional `r`, conditional `n`, Esc | Parity |
| Mixed line endings | Edit and normalize; Cancel opening/Keep read-only | Enter/`e`; Esc with the same contextual meanings | Parity; untouched accepted bytes remain exact |
| Startup crash recovery | Restore; Use disk; Cancel opening | `r` Restore; `d` Use disk; Esc Later | Partial: Rust has already opened the readable disk tab |
| Recovery Manager | Records, details, target, Open/Retarget/Archive/Restore/Export/Delete/retention | Same operations via records/target focus and `o`/`r`/`a`/`x` | Readable-source parity; keyboard-only |
| Missing/orphan recovery | Automatic offer and Manager Open into unavailable-path conflict | Visible in Manager; Retarget or Archive only | Missing in Rust |
| Permanent recovery deletion | Separate cancel-default confirmation | Separate confirmation; only `d` deletes, Enter/Esc cancel | Parity |
| Retention cleanup | Exact listed inventory, configured age, cancel-default confirmation | Exact listed inventory, configured age, only `d` deletes, per-record errors | Parity |
| Semantic inspector | Read-only segments and source jump | Read-only segments and source jump | Parity |
| Experimental semantic reader | Render headings/paragraphs; exact source fallback | Same block policy in a scrollable overlay | Parity |
| Coordinate inspector | Source, character, UTF-8, wrapped, grapheme, screen coordinates | Same diagnostic families | Parity |
| Notifications | Textual toasts plus status | Status line only | Intentional Rust simplification |

All Rust overlays are keyboard-only. Their text inputs support Unicode insertion, character-aware
left/right/delete/backspace, Home/End, Ctrl+A/Ctrl+E, and terminal paste.

## Complete configurable binding inventory

The IDs and defaults below are shared by both frontends. Rust applies overrides at runtime, rejects
unknown/empty/malformed/reserved/colliding keys, refreshes help and menu shortcuts from the effective
map, and reloads a valid replacement map with `R` without partially applying invalid configuration.

| Action | Direct/global/editor/preview ID and default | COMMAND ID and default |
| --- | --- | --- |
| Save | `save` = `Ctrl+S` | `command_save` = `w` |
| Save As | `save_as` = `Ctrl+Shift+S` | `command_save_as` = `W` |
| Quit | `quit` = `Ctrl+Q` | `command_quit` = `q` |
| Toggle explorer | `toggle_explorer` = `Ctrl+B` | `command_toggle_explorer` = `e` |
| Find file | `find_file` = `Ctrl+P` | `command_find_file` = `f` |
| Recent documents | `recent_documents` = `Ctrl+O` | `command_recent_documents` = `o` |
| Next tab | `next_tab` = `Ctrl+PageDown` | `command_next_tab` = `]` |
| Previous tab | `previous_tab` = `Ctrl+PageUp` | `command_previous_tab` = `[` |
| Close tab | `close_tab` = `Ctrl+F4` | `command_close_tab` = `C` |
| Find/replace | `find_replace` = `Ctrl+F` | `command_find_replace` = `s` |
| Workspace text search | `search_text` = `Ctrl+Shift+F` | `command_search_text` = `/` |
| Document outline | `document_outline` = `Ctrl+Shift+O` | `command_document_outline` = `S` |
| Toggle preview | `toggle_preview` = `Ctrl+E` | `command_toggle_preview` = `v` |
| Next preview heading | `preview_next_heading` = `Alt+Down` | — |
| Previous preview heading | `preview_previous_heading` = `Alt+Up` | — |
| Undo | `undo` = `Ctrl+Z, Super+Z` | `command_undo` = `u` |
| Redo | `redo` = `Ctrl+Y, Super+Y, Ctrl+Shift+Z` | `command_redo` = `U` |
| Shortcut help | `show_help` = `F1` | `command_show_help` = `?` |
| Command palette | `command_palette` = `Ctrl+\` | `command_open_palette` = `:` |
| Enter WRITE | — | `command_write_mode` = `i` |
| Duplicate document | — | `command_duplicate_document` = `D` |
| Reload config | — | `command_reload_config` = `R` |
| Manage recovery | — | `command_manage_recovery` = `M` |
| Markdown help | — | `command_markdown_help` = `K` |
| Inspect semantic blocks | — | `command_inspect_semantic_blocks` = `b` |
| Read semantic blocks | — | `command_read_semantic_blocks` = `B` |
| Inspect cursor coordinates | — | `command_inspect_cursor_coordinates` = `I` |
| Cursor left | — | `command_cursor_left` = `h` |
| Cursor down | — | `command_cursor_down` = `j` |
| Cursor up | — | `command_cursor_up` = `k` |
| Cursor right | — | `command_cursor_right` = `l` |
| Source-line start | — | `command_line_start` = `0` |
| Source-line end | — | `command_line_end` = `$` |
| Document start | — | `command_document_start` = `g` |
| Document end | — | `command_document_end` = `G` |

### Fixed and contextual controls

| Context | Keys | Behavior and interface difference |
| --- | --- | --- |
| Mode/overlay | `Esc` | Enter COMMAND; close/cancel the current overlay; destructive continuations are cleared |
| Main workbench | `Tab` in COMMAND | Toggle Files/editor focus when Files is visible |
| Files navigation | `↑`/`k`, `↓`/`j`, `Enter`/`→`/`l`, `←`/`h` | Select, open, or return to editor |
| Files actions | `a`, `c`, `x`, `p`, `r`, `m`, `d` | Create, copy, cut, paste, rename, move, Trash |
| Preview scroll | `↑`/`k`, `↓`/`j`, PageUp/PageDown, Home/End, `g`/`G` | Same scrolling in both; configured Alt+Up/Down select headings |
| Preview links | Python: Tab/Shift+Tab/Enter; Rust: inert | Python selects internal links/footnotes; Rust consumes these keys without activation |
| Popup lists | `↑`/`↓`, Home/End where shown, Enter, Esc | Keyboard selection/submission/cancel |
| Popup fields | Tab/Shift+Tab, arrows, Home/End, Ctrl+A/Ctrl+E, paste | Rust implements Unicode-aware keyboard editing; Python additionally supports mouse focus/buttons |
| Rust mouse | Click Files/editor/preview focus; Files select/double-click; wheel; drag dividers | Tabs, editor positioning/selection, links, and overlays remain unhandled |

## Complete CLI comparison

| CLI surface | Python `termdraft` | Rust `termdraft-rs` |
| --- | --- | --- |
| Positional `TARGET` | Directory or editable file; defaults to `.` | Same |
| `--config-dir PATH` | Select configuration root | Same |
| `--safe-mode` | Ignore `theme.tcss` for this launch | Built-in theme is already always used |
| `--init-config` | No-clobber `config.toml` and `theme.tcss` templates | Same compatible templates |
| `--config-path` | Print resolved config/theme paths | Same |
| `--commands` | Full effective command, Files, global, editor, and preview reference | Same section structure and effective mappings; accurately states preview links are inert |
| `--version` / `--help` | Python package identity/help | Rust package identity/help |
| `--inspect TARGET` | Not available | Rust-only validation/index count without opening the TUI |

The three utility flags `--init-config`, `--config-path`, and `--commands` are mutually exclusive in
both implementations. Rust additionally exposes `--inspect`; it is a diagnostic, not an editor
feature.

## Verification

Current code checkpoint: `8a95a4e`.

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
.venv/bin/pytest -q
```

Results at this checkpoint:

- 151 Rust library tests and 3 Rust binary tests pass;
- strict Clippy and rustfmt pass;
- the Python suite passes 681 tests with 2 expected platform skips;
- source-format and documentation diff checks pass.

## Historical performance snapshot

The following numbers are preserved from the earlier `987cafc` checkpoint, before the recent
search, Files, mouse, keymap, diagnostics, mixed-ending, conflict, and Recovery Manager parity work.
They were **not rerun for `8a95a4e`** and must not be presented as current benchmark results. They do
explain why the Rust build can feel more fluid, especially at process start and first frame.

Environment at that historical checkpoint: macOS 26.5.2 on arm64, Python 3.12.13 / TermDraft 1.2.0,
Rust 1.97.0 / `termdraft-rs` 0.1.0. Timings were medians from interleaved warm runs.

| Historical measurement at `987cafc` | Python | Rust | Historical result |
| --- | ---: | ---: | ---: |
| `--version` subprocess startup | 181.270 ms | 2.751 ms | Rust 65.89× faster |
| End-to-end scan of 5,001 documents | 84.630 ms | 13.590 ms | Rust 6.23× faster |
| In-process workspace scan | 62.847 ms | 10.652 ms | Rust 5.90× faster |
| First visible COMMAND frame in a 100×30 PTY | 246.538 ms | 35.821 ms | Rust 6.88× faster |
| Safe load of one 1 MiB file | 0.388 ms | 5.771 ms | Rust 14.87× slower |

The historical optimized arm64 binary was 4,002,784 bytes. The 1 MiB load result is the useful
counterexample to “Rust is faster everywhere”: that early Rust read path hashed and materialized the
source several times. The old first-frame comparison also favored different indexing strategies:
Python drew while indexing asynchronously; Rust completed its synchronous scan before drawing.

The one-off harness was not committed, so this table is evidence from that checkpoint rather than a
reproducible current benchmark suite.

## Branch history

The port remains split into reviewable checkpoints. The current history is available in GitHub's
[main-to-rust-port comparison](https://github.com/Betancourt1/TermDraft/compare/main...rust-port)
without freezing another soon-stale commit list here.
