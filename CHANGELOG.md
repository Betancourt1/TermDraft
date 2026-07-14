# Changelog

Notable changes to TermWriter are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use semantic versioning.

## [Unreleased]

### Added

- Resizable file-explorer and editor/preview dividers.
- MIT license metadata and a documented release and Homebrew tap process.

### Changed

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
