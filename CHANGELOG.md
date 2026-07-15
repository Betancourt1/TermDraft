# Changelog

Notable changes to TermDraft are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use semantic versioning.

## [Unreleased]

## [1.1.1] - 2026-07-15

### Fixed

- Wait for complete UI results in the multi-tab save and footnote navigation tests instead of racing
  disk-write and scroll callbacks.

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

[Unreleased]: https://github.com/Betancourt1/TermDraft/compare/v1.1.1...HEAD
[1.1.1]: https://github.com/Betancourt1/TermDraft/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/Betancourt1/TermDraft/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v1.0.0
