# TermDraft implementation inventory

This document compares the legacy Python/Textual implementation with the pre-1.0 Rust/Ratatui
application. Both edit the same ordinary `.md`, `.markdown`, and `.txt` files and use compatible
configuration, session, and recovery formats. Their default state locations align on macOS and
Linux/XDG; Windows differences are inventoried below.

Rust is now the primary source implementation: it preserves nearly all of the keyboard-first
workflow, feels materially more immediate in use, and the historical measurements below show much
lower process-start and first-frame latency. TermDraft 0.4 makes this implementation the public
`termdraft` command and Homebrew package. Python remains a historical compatibility reference.

## Run either implementation

Rust 1.88 or newer is required.

```bash
git clone https://github.com/Betancourt1/TermDraft.git
cd TermDraft
cargo run --release --locked -- ~/Documents/notes
cargo run --release --locked -- essay.md
```

To install the Rust executable:

```bash
cargo install --path . --locked
termdraft ~/Documents/notes
```

The Python reference remains available from a prepared checkout as `.venv/bin/termdraft`.

On macOS and Linux/XDG, both implementations normally share compatible configuration, session, and
recovery locations. Sessions contain no Markdown; recovery journals contain private dirty source.
For an isolated comparison, set `XDG_STATE_HOME=/tmp/termdraft-test-state` and pass
`--config-dir /tmp/termdraft-test-config`.

## Inventory snapshot

This inventory was recounted from the Python source and the current Rust implementation:

- Python contains **32 palette actions** and **52 binding IDs**; Rust preserves those contracts and
  adds **Change theme** plus `command_change_theme`, for **33 actions** and **53 binding IDs**;
- Python defines 19 modal dialog classes plus its command palette, or **20 concrete modal types**;
- Rust defines 23 `Overlay` variants; `SearchResults` and `Message` have no construction site, so
  **21 overlay types are currently user-reachable**;
- shared dialog types serve several workflows, so the operation-by-operation popup matrix below is
  the authoritative interface comparison;
- the Rust suite passes **186 library tests plus 4 binary tests**; the Python suite passes
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
| Command palette | Python's 32 actions plus the native Change theme action, preserving six groups and effective shortcuts |
| Configurable commands | The 52 compatible binding IDs plus `command_change_theme`; generated template entries, strict parsing, collision checks, effective overrides, and `R` reload |
| File finding | Fuzzy relative-path search, shared include/exclude filters, normalized matching, bounded ranking |
| Workspace text search | Literal, fuzzy, whole-word, and regex modes; case toggle; path filter; dirty/open overrides; warnings; deterministic 100-result bound; cancellable revisioned background search |
| Active-document search | Case-aware find, previous/next, single replace, Replace All, read-only search, and one-step undo for Replace All |
| Recent documents | Content-free MRU order independent of open-tab order, missing-path pruning, and reopening closed documents |
| Workspace mutations | Create file/folder, copy, cut, paste, rename, move, and operating-system Trash with no-clobber, containment, and ignored-path protection |
| Save As and Duplicate | Workspace-relative no-clobber publication; Save As retargets; Duplicate leaves the original active and preserves its existing dirty/clean state |
| Encoding and ordinary line endings | UTF-8/UTF-8 BOM plus uniform LF, CRLF, or CR round trips |
| Mixed line endings | Explicit Edit and normalize consent; cancel opening or keep read-only; accepted but untouched source remains byte-identical; the first edit uses the same CRLF-first normalization target |
| External conflicts | Local source remains in memory; Save local as, Reload external, Continue without copy when safe, and Cancel are state-gated |
| Dirty close and quit | Each dirty document gets its own filename-specific Save, Discard, or Cancel decision; Enter does not confirm destruction |
| Markdown continuation | Common bullets, ordered markers, tasks, quotes, empty-marker termination, and ordinary Enter fallback |
| Recovery storage | Python-compatible v2 journals, exact fingerprints, advisory Unix cross-process mutation locks, inventory, retarget, quarantine, restore, export, permanent deletion, and configured retention cleanup |
| Recovery Manager for readable sources | `M`/palette access, active/quarantined/corrupt inventory, protected dirty drafts, target input, guarded actions, destructive confirmations, and per-record cleanup errors |
| Diagnostic references | Markdown syntax, semantic source-block inspector, experimental block reader, and cursor-coordinate inspector |
| Normal terminal lifecycle | Alternate screen, raw mode, mouse reporting, and cursor shape restore after normal exit, error unwinding, or Rust panic |

## Intentionally or differently implemented

| Area | Python/Textual | Rust/Ratatui | User-visible effect |
| --- | --- | --- | --- |
| Executable and runtime | Legacy `termdraft`; Python/Textual and Nerd Font icons | Published `termdraft`; one native executable with the same Files icons | Rust starts with less runtime overhead; both interfaces expect a Nerd Font for the intended icons |
| Shell chrome | Textual styling and Nerd Font glyphs | Four built-in Ratatui themes and Yazi-style Nerd Font glyphs | Same hierarchy and Files icon language, not pixel parity |
| Palette layout | Responsive grouped two-column cheatsheet that stacks when narrow; descriptions below | Searchable grouped two-column grid with descriptions and a compact narrow fallback | Actions, order, shortcuts, and explanatory copy match |
| Shortcut help | Generated TermDraft-action reference | Scrollable 28-row action summary | Rust `--commands` is the fuller TermDraft-action reference; `?` is intentionally more compact |
| Preview engine | `markdown-it-py`/Textual with tables, tasks, alerts, footnotes, definitions, link selection, and internal footnote navigation | Active-line source plus rendered inactive lines by default; semantic `pulldown-cmark` split preview with headings, inline styles, links, code, lists/tasks, quotes, horizontally scrollable tables, footnotes, and definitions | Source remains authoritative in both; interactive link/footnote navigation and alerts remain Python-only |
| Explorer model | Lazy, collapsible Textual `DirectoryTree` plus asynchronous indexing | Always-expanded sorted snapshot from `ignore::WalkBuilder` | Rust is simpler and predictable; large trees can occupy more space and delay first frame |
| Search regex engine | Python `regex`, full case folding, and a per-line timeout | Rust `regex`, a linear-time syntax subset, and the same 500-character input limit | Common regexes work; look-around/backreferences accepted by Python are not Rust syntax |
| Workspace-search discovery | Fresh cancellable workspace scan for each submission, including scan warnings | Searches the indexed snapshot captured before the popup opened; source reads are cancellable and warn individually | New filesystem entries can be absent in Rust until the popup closes and the next poll refreshes Files |
| Markdown continuation details | Preserves marker spacing, validates nested indentation with CommonMark, and intercepts only unmodified Enter | Normalizes supported marker spacing and conservatively rejects tab/four-space-leading lines; only unmodified Enter is intercepted | Common continuations match; unusual spacing and nested indentation can differ |
| I/O scheduling | Most scans, reads, probes, saves, mutations, sessions, recovery, and semantic work run in Textual workers | Workspace text search has a background thread; most other I/O runs in the event loop | Rust has less coordination code, but a slow filesystem operation can pause drawing |
| Preview updates | Revisioned/debounced render pipeline | Re-parses the complete split preview synchronously on each draw | Rust stays simple, but large documents can add per-frame work |
| Session restoration | Deferred inactive tabs plus cursor and scroll restoration | Eager tab loading, cursor restoration, and zeroed scroll fields in the compatible v2 file | Tab/order/MRU parity; viewport restoration differs |
| Session validation | Rejects duplicate paths and inconsistent active/open/view relationships | Validates each relative path and bounds but accepts some cross-field inconsistencies | Malformed hand-edited Rust state can be tolerated where Python rejects it |
| External monitoring | Active tab plus rotating inactive-tab probes and focus checks | Active document plus a two-second workspace rescan | An inactive Rust tab is checked after activation, not proactively |
| Save hardening | Parent-descriptor-bound publication, stable-read retry, post-publication digest verification, detailed directory-sync uncertainty | Same-directory atomic save with two destination snapshot checks and directory sync, but path-based publication | Both reject observed conflicts; Python remains stronger against parent-directory replacement races |
| Recovery startup | Decisions occur before a document is installed; directory launches also offer missing/orphan drafts | Existing readable documents open before Restore/Use disk/Later; missing/orphan records stay visible in Manager | Normal crash recovery works; startup and unavailable-source flows differ |
| Feedback | Persistent status plus Textual notifications/toasts | Persistent status messages | Rust is quieter and keeps transient feedback in one line |
| Mouse | Textual tabs, tree, editor, preview links, inputs, rows, and buttons participate | Main-pane focus, Files selection/double-click, hybrid-editor click/drag selection, wheel scrolling, and divider dragging | Rust overlays remain keyboard-only and tab clicking is incomplete |
| Inline presentation | Transforms headings, quotes, lists/tasks, rules, fences, images/links, and GFM table borders | Keeps the active line exact and transforms the same common block and inline syntax elsewhere | Editing bytes remain exact while both interfaces provide a rendered writing surface |
| Directory-copy symlinks | `shutil.copytree` follows nested symlinks by default | Rejects a nested symlink and removes the partial destination | Rust is intentionally stricter and will refuse some Python copies |
| Windows state and locks | `%LOCALAPPDATA%/TermDraft` plus `msvcrt` recovery locks | Unix-style non-macOS state resolution and no advisory recovery lock outside Unix | Shared default locations and cooperating locks are not Windows-compatible yet |

## Still missing from the Rust frontend

These are current gaps, not items that were merely absent in the early port:

1. **Rich preview extensions and interaction.** Rust renders GFM tables, tasks, definitions,
   footnotes, and image labels, but does not provide alerts, link selection, internal
   footnote/backlink navigation, or outline-to-preview reveal. Preview links and footnotes are
   deliberately inert.
2. **Outline query and destination controls.** Rust lists parsed headings and jumps to source, but
   has no filter input and no separate Show in preview action.
3. **Collapsible/lazy Files and proactive inactive-tab monitoring.** Rust uses an always-expanded
   synchronous snapshot, silently drops individual walk errors, and checks only the active document.
4. **Complete mouse interaction.** Tabs, preview links, and all overlays are keyboard-only. The
   implemented main-pane mouse actions are listed below.
5. **Missing/orphan recovery opening.** Manager inventories and can retarget/archive unavailable
   records, but Rust cannot install their source safely without a trustworthy `FileSnapshot`.
   Python can restore them into an unavailable-path conflict and then Save As.
6. **Python-equivalent startup recovery cancellation.** Rust `Esc` means Later after the readable
   disk document is already open; Python can Cancel opening before installing it.
7. **Session scroll offsets, lazy tab materialization, and full cross-field validation.** Rust stores
   compatible zero scroll fields, loads restored tabs immediately, and does not reject every
   duplicate/inconsistent relationship that Python rejects.
8. **`theme.tcss`.** Rust creates the compatibility template but never parses or watches Textual
   CSS. It provides four built-in runtime themes instead; `--safe-mode` has no additional effect.
9. **Full background-I/O and cooperative signal shutdown.** Rust lacks Python's critical-operation
   worker/ticket pipeline and its SIGTERM/SIGHUP recovery-and-session drain.
10. **The deepest Python persistence guarantees.** Rust does not bind publication to an open parent
    descriptor, retry a moving read, or verify the final published digest with Python's uncertainty
    reporting.
11. **Windows state-path and lock compatibility.** Rust does not yet use Python's LocalAppData state
    root or its Windows recovery locking implementation.

## Main interface comparison

| Surface | Python/Textual | Rust/Ratatui | Status |
| --- | --- | --- | --- |
| Title | App/workspace title | Same title | Parity |
| Tabs | Clickable Textual tabs with modified/conflict state | Keyboard tabs with `●` modified and `!` conflict indicators | Keyboard parity; mouse gap |
| Files | Collapsible tree, icons, click and keyboard | Always-expanded snapshot, matching Nerd Font icons, click/double-click, keyboard, and keyboard/mouse resizing | Different tree model |
| Workbench | Inline editor or resizable split source/preview | Active-line source plus rendered inactive lines, or resizable split source/semantic preview | Presentation and layout parity for common Markdown |
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

The Rust palette preserves the Python actions and adds one native theme command. The table lists
every menu action in display order; the key shown is its default COMMAND/context shortcut and
follows effective remaps where the action has a configurable ID.

| Group | Actions in order | Default keys |
| --- | --- | --- |
| DOCUMENT | Save; Save as; Duplicate; Find file; Recent documents; Close tab | `w`; `W`; `D`; `f`; `o`; `C` |
| NAVIGATE | Next tab; Previous tab; Search workspace; Find and replace; Outline; Explorer | `]`; `[`; `/`; `s`; `S`; `e` |
| FILES | Create; Copy; Cut; Paste; Rename; Move; Trash | `a`; `c`; `x`; `p`; `r`; `m`; `d` |
| MODE | Write mode; Command mode | `i`; `Esc` |
| EDIT | Undo; Redo; Reload config; Inspect blocks; Read blocks | `u`; `U`; `R`; `b`; `B` |
| VIEW | Preview; Change theme; Recovery drafts; Shortcut help; Markdown help; Cursor coordinates; Quit | `v`; `t`; `M`; `?`; `K`; `I`; `q` |

Python presents its actions as six visible groups in a responsive grid. Rust keeps the same groups
and fuzzy-filters its complete 33-action set.

## Complete popup and window inventory

| User-facing surface | Python/Textual | Rust/Ratatui | Status or changed behavior |
| --- | --- | --- | --- |
| Command palette | Search, six responsive groups, descriptions, mouse/keyboard | Search, same grouped grid/actions/order/descriptions, keyboard | Keyboard and layout parity; mouse remains a gap |
| Shortcut help | Full TermDraft-action binding reference | Scrollable 28-row action summary | Rust is intentionally shorter; every row remains reachable at 80x24 and `--commands` is fuller |
| Markdown syntax help | `HelpDialog` with supported syntax and examples | Dedicated scrollable Markdown Help overlay | Parity, with Rust limitations stated truthfully |
| Find file | Query, include/exclude filter, ranked results | Same fields, filter contract, ranking, and open action | Parity |
| Workspace search | Query, literal/fuzzy/word/regex, case, filter, fresh-scan warnings/results | Same controls and cancellable source search over the pre-popup index snapshot | Control/result parity; discovery and scan-warning timing differ |
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

The IDs and defaults below are shared by both frontends. They cover TermDraft application actions,
not every built-in editor command; the fixed editor keymaps are inventoried separately below. Rust
applies overrides at runtime, rejects unknown/empty/malformed/reserved/colliding keys, refreshes help
and menu shortcuts from the effective map, and reloads a valid replacement map with `R` without
partially applying invalid configuration.

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
| Change theme | — | `command_change_theme` = `t` |
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

## Complete fixed editor and preview control inventory

These are the default effective controls with an editable document focused in WRITE mode. The
configurable TermDraft bindings above run first; remapping one can expose or consume an underlying
editor-engine shortcut. The two engines can also disagree on exact punctuation word boundaries and
wrapped-page positioning even when the command intent matches.

### Shared editor controls

| Behavior | Effective keys in both |
| --- | --- |
| Insert text or replace selection | Any printable character |
| Indent to the next four-column stop | `Tab` |
| Newline / Markdown continuation | Unmodified `Enter` |
| Move one visual position or row | `←`, `→`, `↑`, `↓` |
| Move by word | `Ctrl+←`, `Ctrl+→` |
| Move to logical line start / end | `Home`, `Ctrl+A` / `End` |
| Move by page | `PageUp`, `PageDown` |
| Extend selection by position or row | `Shift+←`, `Shift+→`, `Shift+↑`, `Shift+↓` |
| Extend selection by word | `Ctrl+Shift+←`, `Ctrl+Shift+→` |
| Extend selection to a line edge | `Shift+Home`, `Shift+End` |
| Delete previous character or selection | `Backspace` |
| Delete next character or selection | `Delete`, `Ctrl+D` |
| Delete previous word or selection | `Ctrl+W`, `Alt+Backspace` |
| Delete next word or selection | `Alt+Delete` |
| Delete to line end; at EOL join the next line, with no selection | `Ctrl+K` |
| TermDraft grouped undo | `Ctrl+Z`, `Super+Z` |
| TermDraft grouped redo | `Ctrl+Y`, `Super+Y`, `Ctrl+Shift+Z` |

### Same editor key, changed behavior

| Key | Python/Textual | Rust/Ratatui |
| --- | --- | --- |
| Modified `Enter` | Only exact Enter has an editor action | Shift/Super/Ctrl/Alt+Enter inserts a plain newline without continuation |
| `Ctrl+U` | Delete to line start; at column zero join the previous line | Extra raw-engine undo alias |
| `Ctrl+V` | Paste Textual's application clipboard | Scroll down one editor page |
| `Ctrl+C` | Copy to the application clipboard and retain selection | Copy to `tui-textarea`'s private yank buffer and clear selection |
| `Ctrl+X` | Cut to the application clipboard; without selection, cut the line | Cut to the private yank buffer; without selection, do nothing |
| `Ctrl+K` with a selection | Delete from the active cursor to line end | Delete the selected range |

Bracketed terminal paste still inserts source in Rust, but it is an `Event::Paste`, not a Ctrl+V
editor command. Rust's private yank buffer has no reachable default paste key because TermDraft's
configured redo owns Ctrl+Y before `tui-textarea` can treat it as yank.

On macOS, `Super+C` copies the editor selection without clearing it, `Super+X` cuts it to the
system clipboard, `Super+V` uses Ghostty's bracketed-paste event (with a direct clipboard fallback),
and `Super+Z` reaches TermDraft's grouped undo through the Kitty keyboard protocol.

### Python-only fixed editor controls

| Behavior | Python keys |
| --- | --- |
| Delete previous word | `Ctrl+Backspace` |
| Delete to line start | `Super+Backspace` |
| Delete complete line(s) intersecting selection | `Ctrl+Shift+K` |
| Select current line | `F6` |
| Select all | `F7` |

### Rust-only fixed editor controls

| Behavior | Rust keys |
| --- | --- |
| Newline alias | `Ctrl+M` |
| Delete previous character | `Ctrl+H` |
| Delete to line start; at column zero join previous line | `Ctrl+J` |
| Delete previous / next word | `Alt+H` / `Alt+D` |
| Extra raw-engine undo / redo | `Ctrl+U` / `Ctrl+R` |
| Move down one row | `Ctrl+N` |
| Move forward / back by word | `Alt+F` / `Alt+B` |
| Move down one paragraph | `Alt+]`, `Alt+N`, `Ctrl+↓` |
| Move up one paragraph | `Alt+[`, `Alt+P`, `Ctrl+↑` |
| Move to line start | `Ctrl+Alt+B`, `Ctrl+Alt+←`; `Home` also accepts otherwise unused Ctrl/Alt modifiers |
| Move to line end | `Ctrl+Alt+F`, `Ctrl+Alt+→`; `End` also accepts otherwise unused Ctrl/Alt modifiers |
| Move to document start | `Alt+<`, `Ctrl+Alt+P`, `Ctrl+Alt+↑` |
| Move to document end | `Alt+>`, `Ctrl+Alt+N`, `Ctrl+Alt+↓` |
| Scroll down / up one page | `Ctrl+V` / `Alt+V` |
| Extend a Rust movement/page selection | Add `Shift` to any reachable movement above |

`PageUp` and `PageDown` also accept otherwise unused Ctrl/Alt modifiers. Exact Ctrl+PageUp and
Ctrl+PageDown remain TermDraft tab commands. The raw Ctrl+U/Ctrl+R aliases bypass TermDraft's grouped
undo/redo entry points.

### Default priority overrides

| TermDraft shortcut | Underlying editor behavior it shadows |
| --- | --- |
| `Ctrl+E` · toggle preview | Python and Rust line-end alias |
| `Ctrl+P` · find file | Rust cursor-up alias |
| `Ctrl+B` · toggle Files | Rust cursor-left alias |
| `Ctrl+F` · find/replace | Rust cursor-right alias |
| `Ctrl+Shift+F` · workspace search | Rust shifted cursor-right alias |
| `Ctrl+PageUp` / `Ctrl+PageDown` · tabs | Base page/viewport commands |
| `Ctrl+Y` · redo | Rust private-yank paste |
| Configured undo/redo keys | Both libraries' corresponding defaults |

In COMMAND mode, TermDraft owns input. With editor focus, arrows move in both versions and Rust's
configurable `h`/`j`/`k`/`l`, `0`/`$`, and `g`/`G` commands provide its Yazi/Vim-style aliases.

### Fixed preview controls

| Behavior | Python/Textual | Rust/Ratatui |
| --- | --- | --- |
| Next / previous rendered heading | Configured `Alt+↓` / `Alt+↑` | Same |
| Link traversal | `Tab` / `Shift+Tab`; leaves preview at either end | Inert |
| Activate selected link or footnote | `Enter` | Inert |
| Scroll one row | No fixed keyboard command | `↑`/`k`, `↓`/`j` |
| Scroll one page | No fixed keyboard command | `PageUp`, `PageDown` |
| Start / end of preview | No fixed keyboard command | `Home`/`g`, `End`/`G` |
| Explicitly inert directional controls | Unbound | `←`, `→`, `h`, `l`, `0`, `$` are consumed without action |

## Other fixed and contextual controls

| Context | Keys | Behavior and interface difference |
| --- | --- | --- |
| Mode/overlay | `Esc` | Enter COMMAND; close/cancel the current overlay; destructive continuations are cleared |
| Main workbench | `Tab` in COMMAND | Toggle Files/editor focus when Files is visible |
| Files navigation | `↑`/`k`, `↓`/`j`, `Enter`/`→`/`l`, `←`/`h` | Select, open, or return to editor |
| Files actions | `a`, `c`, `x`, `p`, `r`, `m`, `d` | Create, copy, cut, paste, rename, move, Trash |
| Preview scroll | Python: mouse/scrollbar; Rust: `↑`/`↓`, `k`/`j`, PageUp/PageDown, Home/End, `g`/`G` | Configured Alt+Up/Down selects headings in both; fixed keyboard scrolling is Rust-only |
| Preview links | Python: Tab/Shift+Tab/Enter; Rust: inert | Python selects internal links/footnotes; Rust consumes these keys without activation |
| Popup lists | `↑`/`↓`, Home/End where shown, Enter, Esc | Keyboard selection/submission/cancel |
| Popup fields | Tab/Shift+Tab, arrows, Home/End, Ctrl+A/Ctrl+E, paste | Rust implements Unicode-aware keyboard editing; Python additionally supports mouse focus/buttons |
| Rust mouse | Click Files/editor/preview focus; Files select/double-click; hybrid editor position/selection; wheel; drag dividers | Tabs, links, and overlays remain unhandled |

## Complete CLI comparison

| CLI surface | Legacy Python `termdraft` | Pre-1.0 Rust `termdraft` |
| --- | --- | --- |
| Positional `TARGET` | Directory or editable file; defaults to `.` | Same |
| `--config-dir PATH` | Select configuration root | Same |
| `--safe-mode` | Ignore `theme.tcss` for this launch | Built-in themes are always used; the flag has no additional effect |
| `--init-config` | No-clobber `config.toml` and `theme.tcss` templates | Same compatible templates |
| `--config-path` | Print resolved config/theme paths | Same |
| `--commands` | Effective TermDraft command, Files, global, editor-action, and preview-action reference | Same section structure and effective mappings; accurately states preview links are inert |
| `--version` / `--help` | Python package identity/help | Rust package identity/help |
| `--inspect TARGET` | Not available | Rust-only validation/index count without opening the TUI |
| `termdraft-benchmark` | Installed Python-only CLI: `--semantic-kib`, `--tab-kib`, `--tabs`, `--watch-kib`, `--iterations`, `--warmup`, `--help` | Not available | Developer measurement surface, not an editor command |

The three utility flags `--init-config`, `--config-path`, and `--commands` are mutually exclusive in
both implementations. Rust additionally exposes `--inspect`; it is a diagnostic, not an editor
feature.

## Verification

Current code state: the primary Rust implementation on `main`.

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
.venv/bin/pytest -q
```

Results at this checkpoint:

- 170 Rust library tests and 4 Rust binary tests pass;
- strict Clippy and rustfmt pass;
- the Python suite passes 681 tests with 2 expected platform skips;
- source-format and documentation diff checks pass.

## Historical performance snapshot

The following numbers are preserved from the earlier `987cafc` checkpoint, before the recent
search, Files, mouse, keymap, diagnostics, mixed-ending, conflict, and Recovery Manager parity work.
They were **not rerun for the current implementation** and must not be presented as current benchmark results. They do
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

## Port history

The port remains split into reviewable commits that now live directly on `main`; the Git history is
the durable record without freezing another soon-stale commit list here.
