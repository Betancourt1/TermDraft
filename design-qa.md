# Rust terminal frontend QA

This checklist covers the current Ratatui frontend on `main`. The goal is to preserve the legacy
Python application's recognizable keyboard-first writing hierarchy with the fewest interface
changes practical, not to claim pixel parity with Textual.

## Preserved visual contract

| Surface | Rust acceptance |
| --- | --- |
| Shell | One-line title, optional tabs, Files pane, centered workbench, and compact status line |
| Hierarchy | Monochrome borders/text; brightness and weight identify focus without decorative color |
| Modes | COMMAND uses a block cursor; WRITE uses a bar cursor; both remain visible in status |
| Editor | Inline keeps the active line as exact source; configured Split shows source and preview side by side |
| Preview toggle | `v` switches editor/preview in Inline or narrow layouts and shows/hides preview in wide Split |
| Files | Yazi-style Nerd Font folder/Markdown icons; keyboard navigation and resizing plus click selection/double-click open |
| Tabs | Open order, modified `●`, conflict `!`, active state, and keyboard switching remain legible |
| Menu | Python's 32-action group/order contract plus four native actions in a searchable two-column grid |
| Overlays | Square bordered panels, concise keyboard footer, visible focus, and cancel-default destructive prompts |
| Recovery | Inventory/detail/target layout, active/quarantine state, protected records, explicit irreversible actions |
| Local History | Responsive checkpoint/diff panes, explicit opt-in, buffer-only restore, and exact clear confirmation |
| Agent editing | Visible active-draft sharing plus a bounded current/stale proposal diff with explicit accept/reject |
| Mouse | Main-pane focus, Files/overlay selection and actions, wheel scroll, and draggable Files/Split dividers |
| Exit | Alternate screen, raw mode, mouse capture, and cursor shape restore on the normal exit path |

The remaining visible differences are intentional: Python notifications can appear as toasts while
Rust keeps feedback in the status line. Rust adds pointer interaction for overlays, tabs, preview
links, and source positioning without making destructive blank areas clickable.

## Automated coverage

`ui.rs` renders the real application through Ratatui's `TestBackend` and checks the preserved shell,
inline and split workbenches, diagnostic windows, mixed/conflict panels, Recovery Manager states,
destructive confirmations, and narrow/small-terminal bounds.

The app/domain tests separately cover:

- exact Python palette group/order/shortcut parity, four native actions, and all 53 binding IDs;
- COMMAND arrow/Vim navigation, WRITE editing, exact-modifier Markdown continuation, undo/redo, and
  conflict-checked atomic saves;
- mixed-line-ending open/reload/recovery consent, exact no-edit save, and first-edit normalization;
- fuzzy file search, four workspace-search modes, filters, dirty overrides, cancellation, warnings,
  Unicode coordinates, find/replace, and one-step Replace All undo;
- recent order, restored tabs/cursors, content-free sessions, and open-tab reuse;
- monotonic source revisions, unchanged-source stability, debounced preview coalescing, cached
  draws, and stale-render rejection;
- create/copy/cut/paste/rename/move/Trash and clean-tab/session retargeting;
- main-pane mouse focus, Files selection/double-click, wheel scrolling, and both divider drags;
- external change conflicts and filename-specific per-document close/quit traversal;
- recovery publish/restore, inventory, locks, retarget, quarantine, export, restore, permanent delete,
  configured retention, alias protection, stale-fingerprint rejection, and per-record errors;
- Local History opt-in, private storage, capture boundaries, retention, diff bounds, lineage,
  revision-checked one-step restores, exact clear revalidation, and wide/narrow pointer UI;
- private agent-session authentication/revocation, exact unsaved reads, stale/range rejection,
  explicit proposal review, one-step Undo/Redo, Recovery, Local History, and unchanged disk;
- Markdown help, semantic inspector/reader, and cursor-coordinate diagnostics.

Run the complete gates with:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
```

On the 0.9.0 checkpoint, 259 library tests and 7 binary tests pass.

## Manual PTY check

Launch a disposable UTF-8 fixture at a minimum of 100×24:

```bash
mkdir -p /tmp/termdraft-fixture
printf '# QA\n\nCafé 日本語\n' > /tmp/termdraft-fixture/note.md
XDG_STATE_HOME=/tmp/termdraft-test-state \
  cargo run --release --locked -- \
  --config-dir /tmp/termdraft-test-config \
  /tmp/termdraft-fixture/note.md
```

Verify in order:

1. Files and the editor remain readable at 100×24; `Shift+Left`/`Shift+Right` resize Files while it
   is focused, and narrower layouts keep only one workbench pane.
2. While Files is focused, `j`/`k` select, Enter opens, and `a/c/x/p/r/m/d` reach the expected
   no-clobber file/folder flows.
3. `i`, Unicode typing/paste, `Esc`, and `w` preserve the expected bytes.
4. `v` follows Inline/Split behavior without altering source; Alt+Up/Down navigates preview headings.
5. `:` contains the same six groups and 32 ordered Python actions plus Change theme, Create
   checkpoint, Open Local History, and Agent sharing; `?` and `--commands` identify the added
   menu-only actions.
6. `f`, `o`, `/`, `s`, and `S` exercise file, recent, workspace, document, and heading navigation.
7. `K`, `b`, `B`, and `I` open the read-only reference/diagnostic windows and return safely.
8. `M` shows active/quarantine/corrupt inventory; Tab changes record/target focus; irreversible
   deletion or retention requires `d`, while Enter/Esc cancels.
9. A mixed-ending fixture requires consent, stays exact after an untouched Save, and normalizes only
   after the first edit.
10. A dirty external edit never overwrites either version and shows only its valid conflict actions.
11. Dirty close/quit prompts each document by name; Enter never discards.
12. Enable Local History, create a checkpoint, compare it in wide and narrow layouts, restore it,
   verify disk is unchanged, Undo back to the prior buffer, and exercise both cancel-default clear
   confirmations with keyboard and pointer.
13. Open Agent sharing, read the exact unsaved source with `termdraft-agent`, submit a proposal,
    verify the diff, accept it without changing disk, Undo it, then revoke the endpoint.
14. Files and overlay click/double-click, wheel scroll, explicit action labels, and both dividers
   work; blank destructive-confirmation space remains inert.
15. A clean `q` restores the normal terminal screen, cursor, raw mode, and mouse reporting.
16. Complete the long-document and many-tab journey in
    [docs/responsiveness.md](docs/responsiveness.md); the newest edit must reach split preview,
    tab switching must remain immediate, and quitting must follow the ordinary interface.

## Result and accepted gaps

The preserved shell, keyboard modes, full command menu, file workflows, search/replace, mixed and
conflict decisions, Recovery Manager, diagnostics, main mouse regions, source fidelity, dirty
transitions, and normal terminal cleanup pass at this checkpoint.

The remaining UI gaps are Python-equivalent startup recovery cancellation and TCSS theme loading.
The main technical gaps are the remaining synchronous filesystem/state operations, deeper
parent-directory save hardening, and Windows state/lock compatibility. See
[RUST_PORT.md](RUST_PORT.md) for the exhaustive inventory and safety differences.
