# Changelog

Notable changes to TermDraft are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use semantic versioning.

TermDraft currently releases on the 0.x line, with no 1.0 roadmap at present. The former 1.x and
2.0 tags were withdrawn on 2026-07-20; their notes remain below as development history rather than
supported releases.

## [Unreleased]

## [0.6.0] - 2026-07-23

### Added

- Added safe Recovery Manager opening for missing and orphan drafts. These drafts enter a protected
  conflict tab that can only publish through Save As, so TermDraft never recreates the unavailable
  original path.
- Added content-free session v3 state with editor and preview scroll restoration. The active
  document opens first and remaining tabs materialize incrementally after the first frame.

### Changed

- Made Files startup non-blocking by showing a shallow workspace snapshot first and completing the
  recursive index in the background, with indexing and scan-warning state kept visible in its title.
- Added rotating inactive-tab checks so external edits, deletion, and unreadability are detected
  without activating every document.
- Made each workspace text search scan the current workspace instead of relying on an older Files
  snapshot, including visible discovery and source-read warnings.

### Fixed

- Made `SIGTERM` and `SIGHUP` drain recovery and session state before restoring terminal mouse,
  screen, cursor-shape, cursor-color, and cursor-visibility state.

## [0.5.1] - 2026-07-22

### Added

- Added the Mist light theme and the mint-on-true-black Void dark theme to the COMMAND theme cycle.

### Changed

- Made the Files sidebar divider easier to resize with a visible handle, a wider mouse grab area,
  and live width feedback while dragging.

### Fixed

- Improved cursor contrast in the Paper and Linen light themes by styling both the COMMAND block
  and its selected character, while preserving the native WRITE cursor and terminal state.

## [0.5.0] - 2026-07-22

### Added

- Added selectable preview links, internal footnote and backlink navigation, and titled GFM alert
  rendering. External destinations remain visible without launching another process.
- Added Unicode-aware outline filtering with explicit source and preview reveal actions.
- Added pointer activation for tabs and preview links plus overlay scrolling, field/control focus,
  list selection, double-click activation, and guarded action prompts.

## [0.4.0] - 2026-07-20

### Changed

- Reset the public version line to 0.4.0 without reusing any previously published tag.
- Made the native Rust/Ratatui application the canonical `termdraft` executable while keeping the
  Python implementation as a compatibility reference and regression oracle.

### Added

- Added Paper and Linen light themes plus Midnight and Carbon dark themes. Press `t` in COMMAND
  mode, or choose **Change theme** from the command palette, to cycle through them.
- Added native mouse cursor placement, preview-position alignment, and click-to-source navigation
  across the Editor, Hybrid, and Preview surfaces.
- Added native Command-key editing shortcuts in Hybrid mode.

### Fixed

- Restored compact folder collapsing in Files and viewport-aware cursor placement in long rendered
  documents.

## 2.0.0 — withdrawn development snapshot (2026-07-16)

### Changed

- Replaced the Python/Textual public application with the native Rust/Ratatui implementation while
  keeping ordinary files, compatible configuration, sessions, and recovery journals authoritative.
- Renamed the experimental `termdraft-rs` package and executable to the canonical `termdraft`
  version 2.0.0.
- Replaced Python wheel releases with native macOS and Linux archives for arm64 and x86_64, plus a
  Rust-built Homebrew formula. Python 1.2.0 remains available as the legacy rollback release.

### Added

- Added the standalone Ratatui/Crossterm workbench with preserved COMMAND/WRITE
  modes, tabs, Files, Inline/Split behavior, outline, and keyboard overlays.
- Ported fuzzy file finding, recent documents, four-mode workspace search, active-document find and
  replace, all workspace create/copy/cut/paste/rename/move/Trash actions, and no-clobber Save As and
  Duplicate flows.
- Ported UTF-8/BOM and uniform/mixed line-ending preservation, explicit normalization consent,
  conflict decisions, per-document dirty exit traversal, compatible sessions, and crash journals.
- Added the Recovery Manager inventory, retarget/archive/restore/export/delete/retention flows,
  exact destructive confirmations, configured retention, and cross-process recovery mutation locks.
- Added all 52 compatible keybinding overrides, live configuration reload, the exact 32-action
  Python palette contract, Markdown help, semantic diagnostics, and cursor-coordinate diagnostics.
- Added main-workbench mouse focus, Files selection/double-click, wheel scrolling, and resizable
  Files and Split dividers.
- Added aligned bordered Markdown tables with horizontal preview scrolling when a table exceeds the
  reading width.
- Updated the documentation with the exhaustive Python/Rust feature, interface, menu,
  popup, command, safety, verification, and historical-performance inventories in
  [RUST_PORT.md](RUST_PORT.md).

## 1.2.0 — withdrawn development snapshot (2026-07-16)

### Added

- Added a default inline preview mode that keeps the cursor line as exact Markdown source and
  presents every other line without common syntax markers. The previous side-by-side source and
  preview layout remains available through `editor.view_mode = "split"`.
- Added command-palette actions for switching modes and managing entries in the focused Files pane.

### Changed

- Replaced persistent editor and preview scrollbars with contextual line and preview progress in the
  status bar, leaving more room for writing.
- Restyled dialogs as compact terminal action panels with framed titles, separated actions, and
  focus-driven emphasis.
- Strengthened the inline heading hierarchy and alignment while keeping the active source line
  legible.
- Kept routine recovery saves silent so the status bar remains focused on user actions.

### Fixed

- Kept the Files pane open after creating a file or folder so keyboard workflows retain context.

## 1.1.1 — withdrawn development snapshot (2026-07-15)

### Fixed

- Wait for complete UI results in asynchronous save, recovery, footnote navigation, workspace
  watcher, and theme reload tests instead of racing their worker and interface callbacks.

## 1.1.0 — withdrawn development snapshot (2026-07-15)

### Added

- Added focused Files keys for creating, copying, cutting, pasting, renaming, and moving entries to
  Trash.
- Added direct COMMAND keys for Save As, duplicate, find and replace, document outline,
  configuration reload, recovery management, Markdown help, and semantic diagnostics.

### Changed

- Open the initial document before indexing large workspaces, show indexing progress in the status
  bar, and open a requested file finder when the scan finishes.
- Changed the editor cursor shape between COMMAND and WRITE modes.
- Reworked the command palette into a searchable two-column keybinding cheatsheet that stacks on
  narrow terminals.
- Centered wide editor and preview reading areas and capped visual source wrapping at 100 columns
  without changing the document.
- Aligned default COMMAND keys with common Vim, Helix, Yazi, and Lazygit conventions. Next tab moved
  from `n` to `]`, previous tab from `p` to `[`, close tab from `c` to `C`, and redo from `r` to `U`.
  Explicit keybinding overrides remain supported.

## 1.0.0 — withdrawn development snapshot (2026-07-14)

### Added

- Resizable file-explorer and editor/preview dividers.
- MIT license metadata and a documented release and Homebrew tap process.

### Changed

- Renamed TermWriter to TermDraft across the product, Python package, commands, configuration, and
  state locations while retaining compatibility discovery for existing local data.
- Unified file and folder creation behind one command.
- Prioritized conflict, modified, recovery, and mixed-ending state in narrow status bars.
- Improved empty-workspace guidance, explorer selection, preview heading contrast, and scrollbars.

### Fixed

- Restored strict type checking across deferred-tab and typed-screen tests.

## [0.3.0]

- Added workspace file management, Save As, duplication, active-document find and replace, and a
  searchable document outline.
- Added workspace change monitoring and deferred inactive-tab restoration.
- Strengthened no-clobber workspace moves, configuration fallback, and tab/session state handling.

## [0.2.0]

- Added explicit COMMAND and WRITE modes with keyboard-first prompts and palette shortcuts.
- Expanded recovery management, orderly shutdown recovery, and inactive-file change monitoring.
- Added semantic diagnostics, an experimental reader, and repeatable development benchmarks.

## [0.1.0]

- Established the local-first terminal writing loop with a file explorer, Markdown source editor,
  rendered preview, protected saves, and crash-recovery journals.

[Unreleased]: https://github.com/Betancourt1/TermDraft/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.6.0
[0.5.1]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.5.1
[0.5.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.5.0
[0.4.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.4.0
