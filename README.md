# TermDraft

**A local-first Markdown editor for the terminal.**

TermDraft gives you a file explorer, source editor, and rendered preview in one keyboard-first
interface. It works directly with ordinary `.md`, `.markdown`, and `.txt` files—no database,
proprietary project format, or import step.

```text
┌ TermDraft · ~/notes ─────────────────────────────────────────────────────────┐
│ journal/2026-07-11.md │ ● projects/termdraft.md                             │
│ Files                    │ Markdown source            │ Rendered preview     │
│  journal/                │ # Friday                   │ Friday               │
│   2026-07-11.md          │                            │                      │
│  projects/               │ Today I learned…           │ Today I learned…     │
│   termdraft.md           │                            │                      │
├──────────────────────────┴────────────────────────────┴──────────────────────┤
│ COMMAND | journal/2026-07-11.md ● modified | 36 words                      │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Why TermDraft?

- **Your files stay yours.** Edit the Markdown already on your disk with any other tool at any
  time.
- **Writing stays in the terminal.** Browse, edit, preview, search, and manage files without
  switching applications.
- **The keyboard comes first.** COMMAND and WRITE modes keep navigation fast while protecting text
  from accidental edits.
- **Markdown is visible as you write.** The side-by-side preview supports common extensions,
  including tables, task lists, footnotes, and alerts.
- **Unsaved work is treated carefully.** Atomic saves, external-change detection, guarded exits,
  and crash-recovery drafts reduce the chance of losing work.

## Quick start

Install with Homebrew on macOS or Linux:

```bash
brew install Betancourt1/tap/termdraft
```

Open a folder of notes:

```bash
termdraft ~/Documents/notes
```

Or open one file directly:

```bash
termdraft essay.md
```

TermDraft requires Python 3.12 or newer and a terminal supported by
[Textual](https://textual.textualize.io/). Its file icons require a Nerd Font or Symbols Nerd Font
fallback.

### Install from source

```bash
git clone https://github.com/Betancourt1/TermDraft.git
cd TermDraft
python3.12 -m venv .venv
source .venv/bin/activate
python -m pip install .
termdraft .
```

## The basic workflow

TermDraft starts in **COMMAND** mode. Press `i` to enter **WRITE** mode and edit the active file;
press `Esc` to return to COMMAND mode.

| Key | Action |
| --- | --- |
| `i` | Start writing |
| `Esc` | Return to COMMAND mode |
| `w` | Save |
| `f` | Find a file |
| `/` | Search across the workspace |
| `v` | Show, hide, or focus the preview |
| `:` | Open the command palette |
| `?` | Show the current shortcuts |
| `q` | Quit safely |

The file explorer uses familiar Yazi-style keys: `a` creates, `r` renames, `c` copies, `x` cuts,
`p` pastes, and `d` moves an entry to Trash. Standard shortcuts such as `Ctrl+S`, `Ctrl+P`,
`Ctrl+F`, and `Ctrl+Q` also work in both modes.

Run this at any time for the complete, always-current key reference:

```bash
termdraft --commands
```

## What is included

- Multiple open documents with independent undo histories and restored sessions
- File search, workspace text search, find and replace, and document outline
- Resizable explorer, editor, and preview panes with a focused layout for narrow terminals
- File and folder creation, rename, copy, move, and Trash operations
- Configurable editor behavior, keybindings, and Textual CSS theme
- Safe Markdown rendering with raw HTML shown as text instead of executed
- Recovery-draft management for crashes, conflicts, and missing files

TermDraft deliberately remains a source editor with a read-only preview. It does not rewrite your
Markdown or hide the underlying syntax. Experimental semantic-block reading exists, but hybrid
WYSIWYM editing is not part of the current product.

## Configuration

Generate editable configuration and theme files:

```bash
termdraft --init-config
termdraft --config-path
```

The defaults live in `~/.termdraft/config.toml` and `~/.termdraft/theme.tcss`. Open `?` inside the
app or run `termdraft --commands` after remapping keys; both show the effective configuration rather
than a hard-coded shortcut list.

## Learn more

- [Markdown syntax gallery](docs/markdown-gallery.md) — preview the supported syntax inside
  TermDraft
- [Architecture](docs/architecture.md) — understand the application and data-safety design
- [Semantic editing notes](docs/semantic-editing.md) — read the direction for future structured
  editing
- [Changelog](CHANGELOG.md) — see what changed between releases
- [Release guide](docs/releasing.md) — publish and verify a new version

## Development

```bash
python -m pip install -e ".[dev]"
ruff format --check .
ruff check .
mypy
pytest -q
```

[GitHub Releases](https://github.com/Betancourt1/TermDraft/releases) provides tagged source
artifacts and checksums. TermDraft is released under the [MIT License](LICENSE).
