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

No Rust package or Homebrew formula is published for this comparison branch. The interface uses
ordinary Unicode symbols and does not require a Nerd Font.

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
| `a` while Files is focused | Create a Markdown or text file |
| `f` | Find a file |
| `/` | Search text across the workspace |
| `s` | Find the next match in the active document |
| `S` | Open the document outline |
| `v` | Cycle Inline, Split, and Source views |
| `e` | Show or hide the file explorer |
| `[` / `]` | Switch tabs |
| `:` / `?` | Open the command palette / shortcut help |
| `q` | Quit safely |

In COMMAND mode, `h`, `j`, `k`, and `l` move through the editor. `Tab` moves focus between the
editor and Files; the same movement keys navigate Files, and `Enter` opens the selected document.
Global shortcuts include `Ctrl+S`, `Ctrl+Q`, `Ctrl+P`, `Ctrl+F`, `Ctrl+B`, `Ctrl+E`, and
`Ctrl+PageUp` / `Ctrl+PageDown`.

Run `termdraft-rs --commands` for the effective editor settings and palette actions. Press `?`
inside the application for the fuller runtime shortcut screen.

## What is included

- A standalone Ratatui/Crossterm frontend with the preserved title, tabs, Files pane, centered
  editor, compact status line, command palette, and explicit terminal cursor shapes
- Inline Markdown presentation by default, plus split preview and exact source views; presentation
  changes appearance only and never reconstructs the document
- Multiple documents with independent undo histories, restored tabs, active document, and cursor
  positions
- Fuzzy file finding, literal workspace search, active-document find, and heading outline
- File creation, Save As, and duplication through no-clobber workspace-relative paths
- UTF-8 and UTF-8 BOM support with LF, CRLF, or CR preservation; mixed-ending files open read-only
- Conflict-checked atomic saves, external-change polling, guarded dirty exits, and crash journals
- Markdown continuation for bullets, tasks, numbered lists, and quotes

The intentionally unported features are listed in [RUST_PORT.md](RUST_PORT.md). The largest gaps are
file-tree mutations beyond creation/duplication, replace, mouse actions, keybinding remapping, TCSS
themes, and the full Python recovery manager.

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

Configuration is strict and is read at startup. Existing `[keybindings]` entries are displayed by
`--commands` but are not remapped in the Rust frontend. `theme.tcss` is created for compatibility
but is not evaluated, the built-in Rust theme is always used, and live reload is not implemented.
Use `--config-dir PATH` for an isolated configuration directory.

Sessions remain content-free. Crash-recovery journals contain dirty source and use the existing
TermDraft state directories so the Python and Rust implementations can understand their v2 data.
For a fully isolated comparison, set `XDG_STATE_HOME=/tmp/termdraft-rs-state` and pass
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

The unchanged Python implementation remains in `src/termdraft` as a reference and regression
oracle. Its existing test suite can still be run from a prepared development environment with
`pytest -q`; the current GitHub workflows continue to package and release only that Python app.

TermDraft is released under the [MIT License](LICENSE).
