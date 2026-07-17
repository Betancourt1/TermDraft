# Rust terminal frontend QA

This checklist covers the current Ratatui frontend on the `rust-port` branch. The
goal is to preserve the Python application's recognizable keyboard-first writing hierarchy with the
fewest interface changes practical, not to claim pixel parity with Textual.

## Preserved visual contract

| Surface | Rust acceptance |
| --- | --- |
| Shell | One-line title with `RUST PORT`, optional tabs, Files pane, centered workbench, and compact status line |
| Hierarchy | Monochrome borders/text; brightness and weight identify focus without decorative color |
| Modes | COMMAND uses a block cursor; WRITE uses a bar cursor; both remain visible in status |
| Editor | Inline keeps the active line as exact source; configured Split shows source and preview side by side |
| Preview toggle | `v` switches editor/preview in Inline or narrow layouts and shows/hides preview in wide Split |
| Files | Yazi-style Nerd Font folder/Markdown icons; keyboard navigation and resizing plus click selection/double-click open |
| Tabs | Open order, modified `●`, conflict `!`, active state, and keyboard switching remain legible |
| Menu | Exact 32-action Python group/order contract in a searchable two-column grid with descriptions |
| Overlays | Square bordered panels, concise keyboard footer, visible focus, and cancel-default destructive prompts |
| Recovery | Inventory/detail/target layout, active/quarantine state, protected records, explicit irreversible actions |
| Mouse | Main-pane focus, Files selection/open, wheel scroll, and draggable Files/Split dividers |
| Exit | Alternate screen, raw mode, mouse capture, and cursor shape restore on the normal exit path |

The remaining visible differences are intentional: Python notifications can appear as toasts while
Rust keeps feedback in the status line. Rust overlays, tabs, preview links, and source click
positioning remain keyboard-only.

## Automated coverage

`ui.rs` renders the real application through Ratatui's `TestBackend` and checks the preserved shell,
inline and split workbenches, diagnostic windows, mixed/conflict panels, Recovery Manager states,
destructive confirmations, and narrow/small-terminal bounds.

The app/domain tests separately cover:

- exact 32-action palette group/order/shortcut parity and all 52 effective binding actions;
- COMMAND arrow/Vim navigation, WRITE editing, exact-modifier Markdown continuation, undo/redo, and
  conflict-checked atomic saves;
- mixed-line-ending open/reload/recovery consent, exact no-edit save, and first-edit normalization;
- fuzzy file search, four workspace-search modes, filters, dirty overrides, cancellation, warnings,
  Unicode coordinates, find/replace, and one-step Replace All undo;
- recent order, restored tabs/cursors, content-free sessions, and open-tab reuse;
- create/copy/cut/paste/rename/move/Trash and clean-tab/session retargeting;
- main-pane mouse focus, Files selection/double-click, wheel scrolling, and both divider drags;
- external change conflicts and filename-specific per-document close/quit traversal;
- recovery publish/restore, inventory, locks, retarget, quarantine, export, restore, permanent delete,
  configured retention, alias protection, stale-fingerprint rejection, and per-record errors;
- Markdown help, semantic inspector/reader, and cursor-coordinate diagnostics.

Run the complete gates with:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
```

On the current branch, 159 library tests and 3 binary tests pass.

## Manual PTY check

Launch a disposable UTF-8 fixture at a minimum of 100×24:

```bash
mkdir -p /tmp/termdraft-rs-fixture
printf '# QA\n\nCafé 日本語\n' > /tmp/termdraft-rs-fixture/note.md
XDG_STATE_HOME=/tmp/termdraft-rs-state \
  cargo run --release --locked -- \
  --config-dir /tmp/termdraft-rs-config \
  /tmp/termdraft-rs-fixture/note.md
```

Verify in order:

1. Files and the editor remain readable at 100×24; `Shift+Left`/`Shift+Right` resize Files while it
   is focused, and narrower layouts keep only one workbench pane.
2. While Files is focused, `j`/`k` select, Enter opens, and `a/c/x/p/r/m/d` reach the expected
   no-clobber file/folder flows.
3. `i`, Unicode typing/paste, `Esc`, and `w` preserve the expected bytes.
4. `v` follows Inline/Split behavior without altering source; Alt+Up/Down navigates preview headings.
5. `:` contains the same six groups and 32 ordered actions as Python; `?` shows the compact action
   summary and `--commands` remains the fuller TermDraft-action reference.
6. `f`, `o`, `/`, `s`, and `S` exercise file, recent, workspace, document, and heading navigation.
7. `K`, `b`, `B`, and `I` open the read-only reference/diagnostic windows and return safely.
8. `M` shows active/quarantine/corrupt inventory; Tab changes record/target focus; irreversible
   deletion or retention requires `d`, while Enter/Esc cancels.
9. A mixed-ending fixture requires consent, stays exact after an untouched Save, and normalizes only
   after the first edit.
10. A dirty external edit never overwrites either version and shows only its valid conflict actions.
11. Dirty close/quit prompts each document by name; Enter never discards.
12. Files click/double-click, wheel scroll, and both dividers work; overlay clicks remain inert.
13. A clean `q` restores the normal terminal screen, cursor, raw mode, and mouse reporting.

## Result and accepted gaps

The preserved shell, keyboard modes, full command menu, file workflows, search/replace, mixed and
conflict decisions, Recovery Manager, diagnostics, main mouse regions, source fidelity, dirty
transitions, and normal terminal cleanup pass at this checkpoint.

The remaining UI gaps are the richer Python preview/link/footnote behavior, outline filtering and
preview reveal, collapsible/lazy Files, proactive inactive-tab checks, tab/editor/overlay mouse
interaction, missing/orphan recovery opening, session scroll restoration, TCSS themes, and
cooperative SIGTERM/SIGHUP shutdown. See [RUST_PORT.md](RUST_PORT.md) for the exhaustive inventory
and safety differences.
