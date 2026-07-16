# Changelog

Notable changes to TermDraft are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use semantic versioning.

The numbered 1.x releases below describe the Python application on `main`. The Rust comparison is a
branch checkpoint, not a published TermDraft release.

## [Rust port 0.1.0][Rust port comparison] - 2026-07-16

### Added

- Added the standalone `termdraft-rs` Ratatui/Crossterm workbench with preserved COMMAND/WRITE
  modes, tabs, Files, Inline/Split/Source views, search, outline, and keyboard overlays.
- Ported UTF-8/BOM and line-ending preservation, conflict-checked atomic saves, external-change
  handling, no-clobber Create/Save As/Duplicate paths, compatible sessions, and crash journals.
- Added strict compatible editor configuration, Markdown continuation, 39 Rust tests, and the
  Python/Rust comparison in [RUST_PORT.md](RUST_PORT.md).
- Updated all branch-facing documentation to use the Rust binary, implemented shortcuts, Cargo
  gates, Ratatui architecture, and the actual release boundary.

## [Unreleased] — Python reference

### Added

- Render GFM table rows with terminal borders in the default inline editor while keeping the active
  row as exact Markdown source. This change belongs to the Python frontend; the Rust inline view
  currently styles table separators without reconstructing bordered rows.

## [1.2.0] - 2026-07-16

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

## [1.1.1] - 2026-07-15

### Fixed

- Wait for complete UI results in asynchronous save, recovery, footnote navigation, workspace
  watcher, and theme reload tests instead of racing their worker and interface callbacks.

## [1.1.0] - 2026-07-15

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

## [1.0.0] - 2026-07-14

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

[Unreleased]: https://github.com/Betancourt1/TermDraft/compare/v1.2.0...HEAD
[Rust port comparison]: https://github.com/Betancourt1/TermDraft/compare/main...rust-port
[1.2.0]: https://github.com/Betancourt1/TermDraft/compare/v1.1.1...v1.2.0
[1.1.1]: https://github.com/Betancourt1/TermDraft/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/Betancourt1/TermDraft/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v1.0.0
