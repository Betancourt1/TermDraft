<div align="center">

# TermDraft

**Write Markdown without leaving the terminal.**

A local-first, keyboard-first writing workbench for ordinary Markdown and text files.

[![Latest release](https://img.shields.io/github/v/release/Betancourt1/TermDraft?style=flat-square&color=6e6e6e)](https://github.com/Betancourt1/TermDraft/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/Betancourt1/TermDraft/ci.yml?branch=main&style=flat-square&label=build)](https://github.com/Betancourt1/TermDraft/actions/workflows/ci.yml)
[![Homebrew](https://img.shields.io/badge/Homebrew-available-6e6e6e?style=flat-square)](#install)
[![MIT license](https://img.shields.io/github/license/Betancourt1/TermDraft?style=flat-square&color=6e6e6e)](LICENSE)

[Quick start](#quick-start) · [Workflow](#workflow) · [Install](#install) ·
[Configuration](#configuration) · [Documentation](#documentation)

</div>

## Quick start

```bash
brew install Betancourt1/tap/termdraft
termdraft ~/Documents/notes
```

TermDraft edits existing `.md`, `.markdown`, and `.txt` files in place. There is no proprietary
document format, cloud account, or background service.

## Why TermDraft

- **Your files stay ordinary.** Work directly with the folders and text files you already have.
- **The keyboard stays in charge.** Modal writing, workspace navigation, search, and file management
  share one compact interface.
- **Source and structure stay together.** Hybrid mode renders inactive Markdown while keeping the
  current line exact; split mode places source and semantic preview side by side. Neither view
  rewrites the document.
- **Local work is treated carefully.** Atomic saves, conflict checks, crash journals, recovery tools,
  and session restoration protect unfinished writing.

## Workflow

TermDraft opens in **COMMAND** mode. Press `i` to enter **WRITE** mode and edit; press `Esc` to
return to COMMAND mode.

| Key | Action |
| --- | --- |
| `i` / `Esc` | Enter WRITE / COMMAND mode |
| `w` | Save |
| `f` | Find a file |
| `/` | Search the workspace in literal, fuzzy, word, or regex mode |
| `s` | Find and replace in the active document |
| `S` | Open the document outline |
| `v` | Switch editor/preview, or show/hide preview in a wide split layout |
| `e` | Show or hide Files |
| `[` / `]` | Switch tabs |
| `:` / `?` | Open the command palette / shortcut help |
| `q` | Quit safely |

In COMMAND mode, the arrow keys and `h`, `j`, `k`, `l` move through the editor. `Tab` moves focus
between the editor and Files; `Enter` opens the selected document. Press `?` in the application for
the compact runtime reference, or run this outside it for the complete effective keymap:

```bash
termdraft --commands
```

<details>
<summary><strong>More workspace and navigation shortcuts</strong></summary>

| Key | Action |
| --- | --- |
| `W` | Save As to a new workspace-relative path |
| `D` | Duplicate the active document |
| `a` in Files | Create a file or folder |
| `c` / `x` / `p` in Files | Copy / cut / paste an entry |
| `r` / `m` / `d` in Files | Rename / move / Trash an entry |
| `R` / `M` | Reload configuration / manage recovery drafts |
| `K` / `b` / `B` / `I` | Markdown help / inspect blocks / read blocks / cursor coordinates |

While Files is focused, `Shift+Left` and `Shift+Right` resize its pane; its divider can also be
dragged with the mouse. Global shortcuts include `Ctrl+S`, `Ctrl+Q`, `Ctrl+P`, `Ctrl+F`, `Ctrl+B`,
`Ctrl+E`, and `Ctrl+PageUp` / `Ctrl+PageDown`.

In the focused preview, `Left`/`Right` or `h`/`l` scroll wide tables horizontally; `0` and `$` jump
to their edges.

</details>

## Install

### Homebrew

```bash
brew install Betancourt1/tap/termdraft
termdraft ~/Documents/notes
```

### Build from source

Rust 1.88 or newer is required.

```bash
git clone https://github.com/Betancourt1/TermDraft.git
cd TermDraft
cargo run --release --locked -- ~/Documents/notes
```

Open one file directly:

```bash
cargo run --release --locked -- essay.md
```

Or install the binary in your Cargo bin directory:

```bash
cargo install --path . --locked
termdraft ~/Documents/notes
```

Useful non-interactive commands:

```bash
termdraft --version
termdraft --help
termdraft --commands
termdraft --inspect ~/Documents/notes
```

When running without `cargo install`, replace `termdraft` with
`cargo run --release --locked --`, keeping the extra `--` before application arguments.

## Features

- Multiple documents with independent undo histories, restored tabs, active document, and cursor
  positions
- Fuzzy file finding, four-mode workspace search, active-document find and replace, recent
  documents, and heading outline
- File and folder creation, copy, cut, paste, rename, move, Trash, Save As, and duplication through
  no-clobber workspace-relative paths
- UTF-8 and UTF-8 BOM support with LF, CRLF, or CR preservation
- Mouse focus, Files selection and double-click, hybrid-editor text selection, wheel scrolling,
  draggable dividers, and keyboard Files-pane resizing
- Markdown continuation for bullets, tasks, numbered lists, and quotes

<details>
<summary><strong>Complete implementation details</strong></summary>

- A standalone Ratatui/Crossterm frontend with the title, tabs, Files pane, centered editor, compact
  status line, command palette, and explicit terminal cursor shapes
- Hybrid Markdown presentation by default, plus a configurable resizable split layout with a
  semantic read-only preview; neither presentation path rewrites the document
- Mixed-ending files remain exact until the first edit and normalize only to a disclosed target
  after consent
- Conflict-checked atomic saves, safe external-conflict choices, per-document guarded exits, crash
  journals, and a recovery inventory/retarget/archive/restore/export/delete/retention manager
- All 52 compatible application binding IDs, effective remapping, live `R` reload, and an exact
  32-action command palette
- Markdown syntax, semantic-block, experimental reader, and coordinate-diagnostic overlays
- Sessions remain content-free; crash-recovery journals contain dirty source and share their v2
  data format with the legacy Python implementation
- Default state locations align on macOS and Linux/XDG; Windows paths and recovery locks still
  differ

The exact inventory, measured comparison, safety differences, and remaining parity gaps are in
[RUST_PORT.md](RUST_PORT.md).

</details>

## Configuration

Create compatible configuration templates without replacing existing files:

```bash
termdraft --init-config
termdraft --config-path
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

Configuration is strict. The generated template documents all 52 `[keybindings]` IDs; valid
overrides apply to global, editor, preview, and COMMAND actions. Collisions, reserved keys, and
unknown settings are rejected. Invalid live reloads leave the active configuration untouched.

`theme.tcss` is created for compatibility but is not evaluated by the Rust frontend, which uses its
built-in monochrome theme. Use `--config-dir PATH` for an isolated configuration.

For a fully isolated comparison, set `XDG_STATE_HOME=/tmp/termdraft-test-state` and pass
`--config-dir /tmp/termdraft-test-config`.

## Terminal typography

A Nerd Font is recommended for the intended Files icons. On macOS, the interface can be tried in
[Server Mono](https://github.com/internet-development/www-server-mono) without changing the global
Ghostty configuration. Ghostty's bundled Symbols Nerd Font supplies the icons:

```bash
brew install --cask font-server-mono
open -na Ghostty.app --args \
  --font-family="Server Mono" \
  --font-family="Symbols Nerd Font Mono" \
  --working-directory="$PWD" \
  -e cargo run --release --locked -- ~/Documents/notes
```

## Project status

TermDraft 2.0 is the Rust/Ratatui application on `main`. It replaces the Python/Textual runtime and
does not require Python at runtime. Python 1.2 remains in `src/termdraft` as a legacy release,
compatibility reference, and regression oracle.

The largest remaining differences are richer Python preview and link/footnote interactions,
outline filtering and preview reveal, collapsible/lazy Files and inactive-tab monitoring, complete
mouse/overlay input, direct opening of missing recovery drafts, session scroll restoration, and
TCSS themes. See [RUST_PORT.md](RUST_PORT.md) for the accepted parity boundary.

## Documentation

- [Rust comparison](RUST_PORT.md) — parity, omissions, safety differences, and benchmarks
- [Architecture](docs/architecture.md) — Rust modules, state flow, persistence, and recovery
- [Markdown gallery](docs/markdown-gallery.md) — exercise the inline and split renderers
- [Semantic editing](docs/semantic-editing.md) — future block-aware editing boundary
- [Design QA](design-qa.md) — Ratatui frontend acceptance checks
- [Release guide](docs/releasing.md) — native artifact, GitHub, and Homebrew checklist
- [Changelog](CHANGELOG.md) — Rust 2.x and legacy Python 1.x history

## Development

Run the Rust gates from the repository root:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo test --locked --release
```

The Python reference suite can still be run from the prepared development environment with
`.venv/bin/pytest -q`. Public 2.x releases package only the Rust application.

## License

TermDraft is released under the [MIT License](LICENSE).
