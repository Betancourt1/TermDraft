# TermDraft Rust port

**A local-first Markdown editor for the terminal, ported from Python/Textual to Rust.**

This README describes the `rust-port` branch. The Rust binary is `termdraft-rs`; it edits the same
ordinary `.md`, `.markdown`, and `.txt` files as the released Python application and preserves its
keyboard-first workbench without requiring Python or Textual at runtime.

```text
┌ TermDraft · ~/notes                                      RUST PORT ┐
│ journal/2026-07-11.md │ ● projects/termdraft.md                    │
│ Files                    │ Friday                                   │
│  journal/                │                                          │
│   2026-07-11.md          │ Today I learned…                         │
│  projects/               │                                          │
│   termdraft.md           │ # Current line is exact Markdown source  │
├──────────────────────────┴──────────────────────────────────────────┤
│ COMMAND INLINE │ journal/2026-07-11.md ● modified │ 36 words      │
└─────────────────────────────────────────────────────────────────────┘
```

The published `termdraft` command and Homebrew formula still install the Python 1.2 application.
See [RUST_PORT.md](RUST_PORT.md) for the measured comparison and the exact parity boundary.

Rust is now the likely primary direction because the preserved workflow feels more immediate and
the remaining gaps are concentrated in rich preview interaction, unavailable-source recovery,
theme support, and a few deeper platform safeguards. Python remains the public release and product
oracle until that promotion is made explicitly.

## Quick start

Rust 1.88 or newer is required.

```bash
git clone --branch rust-port --single-branch https://github.com/Betancourt1/TermDraft.git
cd TermDraft
cargo run --release --locked -- ~/Documents/notes
```

Open one file directly:

```bash
cargo run --release --locked -- essay.md
```

Or install the branch-local binary in your Cargo bin directory:

```bash
cargo install --path . --locked
termdraft-rs ~/Documents/notes
```

No Rust package or Homebrew formula is published for this comparison branch. The Files pane uses
the same Yazi-style Nerd Font folder and Markdown icons as the Python application, so a Nerd Font is
recommended for the intended interface.

Useful non-interactive commands:

```bash
termdraft-rs --version
termdraft-rs --help
termdraft-rs --commands
termdraft-rs --inspect ~/Documents/notes
```

When running without `cargo install`, replace `termdraft-rs` with
`cargo run --release --locked --`, keeping the extra `--` before application arguments.

## Basic workflow

TermDraft starts in **COMMAND** mode. Press `i` to enter **WRITE** mode and edit the active file;
press `Esc` to return to COMMAND mode.

| Key | Action |
| --- | --- |
| `i` / `Esc` | Enter WRITE / COMMAND mode |
| `w` | Save |
| `W` | Save As to a new workspace-relative path |
| `D` | Duplicate the active document |
| `a` while Files is focused | Create a file or folder |
| `c` / `x` / `p` while Files is focused | Copy / cut / paste an entry |
| `r` / `m` / `d` while Files is focused | Rename / move / Trash an entry |
| `f` | Find a file |
| `/` | Search text across the workspace in literal, fuzzy, word, or regex mode |
| `s` | Find and replace in the active document |
| `S` | Open the document outline |
| `v` | Switch editor/preview, or show/hide preview in a wide Split layout |
| `e` | Show or hide the file explorer |
| `[` / `]` | Switch tabs |
| `:` / `?` | Open the command palette / shortcut help |
| `R` / `M` | Reload configuration / manage recovery drafts |
| `K` / `b` / `B` / `I` | Markdown help / inspect blocks / read blocks / cursor coordinates |
| `q` | Quit safely |

In COMMAND mode, the arrow keys and `h`, `j`, `k`, `l` move through the editor. `Tab` moves focus
between the editor and Files; the same letter keys navigate Files, and `Enter` opens the selected
document. While Files is focused, `Shift+Left` and `Shift+Right` resize its pane; its divider can
also be dragged with the mouse.
Global shortcuts include `Ctrl+S`, `Ctrl+Q`, `Ctrl+P`, `Ctrl+F`, `Ctrl+B`, `Ctrl+E`, and
`Ctrl+PageUp` / `Ctrl+PageDown`.

Run `termdraft-rs --commands` for the effective TermDraft COMMAND, Files, global, editor-action, and
preview-action reference. Press `?` inside the application for a compact scrollable 26-row runtime
summary. [RUST_PORT.md](RUST_PORT.md) also inventories the fixed underlying editor commands.

## What is included

- A standalone Ratatui/Crossterm frontend with the preserved title, tabs, Files pane, centered
  editor, compact status line, command palette, and explicit terminal cursor shapes
- Inline Markdown presentation by default: the cursor line stays exact source while the remaining
  lines are rendered. A configurable resizable split layout adds a semantic read-only preview;
  neither presentation path rewrites the document
- Multiple documents with independent undo histories, restored tabs, active document, and cursor
  positions
- Fuzzy file finding, four-mode workspace search, active-document find and replace, recent documents,
  and heading outline
- File/folder create, copy, cut, paste, rename, move, Trash, Save As, and duplication through
  no-clobber workspace-relative paths
- UTF-8 and UTF-8 BOM support with LF, CRLF, or CR preservation; mixed-ending files require consent,
  remain exact until the first edit, and then normalize to the disclosed target
- Conflict-checked atomic saves, safe external-conflict choices, per-document guarded exits, crash
  journals, and a recovery inventory/retarget/archive/restore/export/delete/retention manager
- All 52 compatible application binding IDs, effective remapping, live `R` reload, and an exact
  32-action command palette using the same grouped grid, order, and descriptions as Python
- Main-pane mouse focus, Files selection/double-click, wheel scrolling, draggable dividers, and
  keyboard Files-pane resizing
- Markdown syntax, semantic-block, experimental reader, and coordinate-diagnostic overlays
- Markdown continuation for bullets, tasks, numbered lists, and quotes

The exact inventories are in [RUST_PORT.md](RUST_PORT.md). The largest remaining gaps are the richer
Python preview and link/footnote interactions, outline filtering/preview reveal, collapsible/lazy
Files and inactive-tab monitoring, full mouse/overlay input, direct opening of missing/orphan
recovery drafts, session scroll restoration, TCSS themes, and public Rust distribution.

## Configuration

Create the compatible configuration templates without replacing existing files:

```bash
termdraft-rs --init-config
termdraft-rs --config-path
```

The default paths are `~/.termdraft/config.toml` and `~/.termdraft/theme.tcss`. The Rust frontend
currently applies these editor settings:

```toml
[editor]
auto_continue_lists = true
soft_wrap = true
show_line_numbers = true
startup_mode = "command" # or "write"
view_mode = "inline"     # or "split"
```

Configuration is strict. The generated template documents all 52 `[keybindings]` IDs; Rust applies
valid overrides to global, editor, preview, and COMMAND actions, rejects collisions and reserved
keys, and reloads `config.toml` with `R`. Invalid reloads leave the active configuration untouched.
`theme.tcss` is still created only for compatibility: Rust never evaluates Textual CSS, always uses
its built-in theme, and requires Python for theme watching. Use `--config-dir PATH` for isolation.

Sessions remain content-free. Crash-recovery journals contain dirty source, and their v2 data formats
are shared with Python. Default state locations align on macOS and Linux/XDG; Windows paths and
recovery locks still differ. For a fully isolated comparison, set
`XDG_STATE_HOME=/tmp/termdraft-rs-state` and pass
`--config-dir /tmp/termdraft-rs-config`.

## Documentation

- [Rust comparison](RUST_PORT.md) — parity, omissions, safety differences, and benchmarks
- [Architecture](docs/architecture.md) — Rust modules, state flow, persistence, and recovery
- [Markdown gallery](docs/markdown-gallery.md) — exercise the current inline and split renderers
- [Semantic editing](docs/semantic-editing.md) — future block-aware editing boundary for Rust
- [Design QA](design-qa.md) — current Ratatui frontend acceptance checks
- [Release guide](docs/releasing.md) — branch verification and the boundary with Python releases
- [Changelog](CHANGELOG.md) — released Python history and this branch's additions

## Development

Run the Rust gates from the repository root:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
```

The Python implementation remains in `src/termdraft` as a reference and regression oracle. Its test
suite can still be run from the prepared development environment with `.venv/bin/pytest -q`; the
current GitHub workflows continue to package and release only that Python app.

TermDraft is released under the [MIT License](LICENSE).
