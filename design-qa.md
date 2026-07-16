# Rust terminal frontend QA

This checklist covers the Ratatui frontend on the `rust-port` branch. The previous Textual dialog,
Trash, click, and recovery-manager review belongs to the Python application and is available through
Git history; those surfaces do not exist in this port.

## Preserved visual contract

| Surface | Rust acceptance |
| --- | --- |
| Shell | One-line title with `RUST PORT`, optional tab row, Files pane, centered workbench, and compact status line |
| Hierarchy | Monochrome borders and text; brightness and weight identify focus without decorative color |
| Modes | COMMAND uses a block cursor; WRITE uses a bar cursor; both remain visible in the status line |
| Editor | Inline view keeps the cursor line as exact source; Split and Source remain available with `v` |
| Files | Ordinary Unicode `▸`, `◆`, and `›` symbols; no Nerd Font dependency |
| Overlays | Square bordered panels, concise keyboard footer, and a highlighted selected row |
| Exit | Alternate screen, raw mode, mouse capture, and cursor shape are restored |

The layout intentionally preserves the Python frontend's recognizable writing hierarchy, but it
does not claim pixel parity. The explorer width and split ratio are fixed, overlays are keyboard
only, and mouse input has no application actions.

## Automated checks

`ui.rs::renders_the_preserved_application_shell` renders the real `App` through Ratatui's
`TestBackend` at 100×24 and asserts the visible title, Rust-port marker, Files label, COMMAND mode,
open filename, and rendered cursor.

The app tests separately cover:

- WRITE editing followed by an atomic disk save;
- dirty quit behavior where Enter cannot confirm destruction;
- mixed-line-ending read-only behavior;
- no-clobber Create, Save As, and Duplicate paths;
- clean reload versus dirty external-change conflict;
- workspace refresh, restored tabs/cursors, and content-free sessions;
- recovery restore, stale-baseline Save As enforcement, and journal cleanup.

Run the complete frontend and domain suite with:

```bash
cargo test --locked --all-targets
cargo test --locked --release
```

## Manual PTY check

Launch a disposable UTF-8 Markdown file at a minimum of 100×24:

```bash
mkdir -p /tmp/termdraft-rs-fixture
printf '# QA\n\nCafé 日本語\n' > /tmp/termdraft-rs-fixture/note.md
cargo run --release --locked -- /tmp/termdraft-rs-fixture/note.md
```

Verify the following in order:

1. Files and the editor remain readable at the minimum size.
2. While Files is focused, `j`/`k` select and `Enter` opens; `Tab` toggles Files/editor focus.
3. `i`, Unicode typing, `Esc`, and `w` preserve the exact expected bytes.
4. `v` cycles all three views without altering source.
5. `:`, `?`, `f`, `/`, `s`, and `S` open centered keyboard overlays and Escape closes them.
6. A dirty close or quit cannot discard on Enter; only the labeled key performs the action.
7. A clean `q` restores the normal terminal screen and cursor.

The code checkpoint documented in [RUST_PORT.md](RUST_PORT.md) passed this PTY path against both a
temporary Unicode fixture and a real note under `~/Documents/Other`.

## Result and known gaps

The preserved shell, keyboard modes, source fidelity, dirty decisions, and terminal cleanup pass at
the documented checkpoint. Resizable panes, mouse behavior, file-mutation dialogs, TCSS themes, and
the full recovery manager remain Python-only and should not be represented as Rust QA coverage.
