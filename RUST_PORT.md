# TermDraft Rust comparison port

This branch is a standalone Rust port built to answer a concrete question: how does TermDraft
change when its portable core and terminal frontend run without Python or Textual?

It is intentionally an adjacent implementation, not a replacement release. The Python app remains
untouched and its tests remain the product oracle. The Rust binary is named `termdraft-rs`, uses the
same ordinary Markdown files, and shares compatible configuration, session, and recovery paths.

## Run it

Rust 1.88 or newer is required.

```bash
cargo run --release -- ~/Documents/notes
cargo run --release -- essay.md
```

Useful non-interactive checks:

```bash
cargo run --release -- --inspect ~/Documents/notes
cargo run --release -- --commands
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```

The original Python app still runs normally with `termdraft` or `.venv/bin/termdraft`.

## What moved to Rust

| Concern | Python implementation | Rust implementation |
| --- | --- | --- |
| Terminal shell | Textual app, screens, widgets, TCSS | Ratatui, Crossterm, `tui-textarea-2` |
| Document state | Python models | Owned Rust `Document` and `FileSnapshot` types |
| Reads and saves | descriptor-oriented persistence service | stable UTF-8 reads and conflict-checked same-directory atomic saves |
| Workspace index | Python recursive model | `ignore::WalkBuilder` with the same editable suffixes |
| Search and outline | Python services and screens | Rust fuzzy file search, literal workspace search, and heading outline |
| Configuration | strict `tomllib` data | strict Serde/TOML data using the same editor keys and locations |
| Sessions | content-free JSON | Python-compatible v2 JSON with tabs and cursor positions |
| Crash recovery | full recovery manager | Python-compatible v2 journals plus Restore / Use disk / Later |

The Rust source is 5,251 lines across `rust/src`, compared with 13,711 Python lines under `src`.
Those numbers are directional, not a language productivity claim: the Rust port deliberately omits
several mature workflows listed below.

## Preserved user experience

- The title, tab row, file sidebar, centered editor, and compact status line keep the existing
  monochrome terminal hierarchy.
- COMMAND and WRITE remain explicit modes with block/bar terminal cursors and Yazi-style movement.
- Inline preview remains the default. Only inactive lines hide common Markdown markers; the cursor
  line and saved source remain exact. Split preview and plain source views remain available.
- Tabs keep independent editor and undo histories. `[` and `]` switch tabs.
- File finder, workspace text search, in-document find, document outline, grouped command palette,
  and help overlays remain keyboard-first.
- `a`, `W`, and `D` create, Save As, and duplicate through no-clobber workspace-relative paths.
- UTF-8 BOM, LF, CRLF, and CR are preserved. Mixed-ending files stay read-only.
- Clean external edits reload; dirty external edits or deletion become conflicts without replacing
  the in-memory source.
- Dirty exits use Save / Discard / Cancel. Enter is intentionally inert in destructive prompts.
- Sessions contain no Markdown. Dirty recovery journals contain source, are private and atomic, and
  are removed only by exact-version save/discard cleanup.

## Intentional differences and remaining gaps

The Rust build currently favors a small, inspectable comparison over feature-count parity:

- File/folder rename, move, copy/cut/paste, and operating-system Trash are not ported.
- Find works; replace, recent-documents UI, semantic-block inspectors, coordinate diagnostics, and
  the Markdown reference screen are not ported.
- The explorer is an always-expanded index rather than a collapsible watched tree.
- Existing editor configuration is applied, but keybinding overrides are report-only, TCSS is not
  evaluated, and live configuration reload is absent.
- Recovery covers existing files and the common crash flow. The recovery manager, quarantine,
  retention cleanup, missing/orphan drafts, and cross-process file locks remain Python-only.
- Mouse capture is restored safely, but the Rust workbench has no mouse actions.
- Rust persistence performs two snapshot checks and atomic same-directory publication, but it does
  not yet bind every ancestor to directory file descriptors. The Python service remains stronger
  against a parent-directory replacement race and reports directory-sync uncertainty more fully.

## Safety and verification

The Rust implementation has dedicated tests for BOM/CRLF round trips, stale-save rejection,
workspace containment, no-clobber creation, Unicode search coordinates, inline-preview source
fidelity, mixed-ending read-only enforcement, external-change conflicts, guarded exits, sessions,
and recovery baselines.

At the final code checkpoint (`987cafc`):

- 39 Rust debug and release tests pass.
- strict Clippy and rustfmt checks pass.
- the unchanged Python suite passes 679 tests with 2 platform skips.
- the optimized arm64 macOS binary is 4,002,784 bytes.
- a real PTY launch against both this repository and a Unicode-named document under
  `~/Documents/Other` rendered the preserved workbench and restored the alternate screen, mouse
  reporting, and cursor shape cleanly on exit.

The benchmark table below uses one machine and fixed local fixtures; it is useful for relative
shape, not a universal performance guarantee.

Environment: macOS 26.5.2 on arm64, Python 3.12.13 / TermDraft 1.2.0, and Rust
1.97.0 / `termdraft-rs` 0.1.0. Timings are medians from interleaved warm runs.

| Measurement | Python | Rust | Result |
| --- | ---: | ---: | ---: |
| `--version` subprocess startup | 181.270 ms | 2.751 ms | Rust 65.89× faster |
| End-to-end scan of 5,001 documents | 84.630 ms | 13.590 ms | Rust 6.23× faster |
| In-process workspace scan | 62.847 ms | 10.652 ms | Rust 5.90× faster |
| First visible COMMAND frame in a PTY | 246.538 ms | 35.821 ms | Rust 6.88× faster |
| Safe load of one 1 MiB file | 0.388 ms | 5.771 ms | Rust 14.87× slower |

The load result is the useful counterexample to “Rust is faster everywhere.” The current Rust read
path calculates SHA-256 three times while checking read stability and creating the final snapshot,
then materializes both exact and normalized strings. Reducing that redundant hashing is the clearest
optimization opportunity; it was left visible rather than expanding this port into an optimization
project.

The first-frame fixture used a 100×30 PTY and isolated configuration/state roots. Python begins its
workspace index asynchronously while Rust completes its scan before drawing, so this row compares
time to a usable shell, not time to Python's completed index. The standalone Rust binary size also
is not directly comparable with a Python wheel plus an external interpreter.

## Branch history

The port is split into reviewable checkpoints rather than one rewrite commit:

```text
4385d51  Add Rust core foundation
e2f1590  Build standalone Rust terminal frontend
058cc27  Port configuration and writing behavior to Rust
723efb5  Add Rust file creation and Save As flows
e62946c  Handle external changes in the Rust frontend
8b24e73  Harden Rust editing and persistence safety
7dabf1a  Restore workspace sessions in the Rust port
987cafc  Add crash recovery to the Rust port
```
