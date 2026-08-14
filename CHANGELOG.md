# Changelog

Notable changes to TermDraft are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and releases use semantic versioning.

TermDraft currently releases on the 0.x line, with no 1.0 roadmap at present. The former 1.x and
2.0 tags were withdrawn on 2026-07-20; their notes remain below as development history rather than
supported releases.

## [Unreleased]

## [0.9.2] - 2026-08-13

### Added

- Added `termdraft-agent workspace` to read every supported document in the explicitly shared
  workspace, using exact unsaved buffers for open tabs and stable disk reads for unopened files.

### Changed

- Expanded Agent sharing from one fixed document to the current workspace while keeping proposals
  revision-bound to the active document and preserving explicit review, Recovery, Undo, and no-save
  behavior.
- Build Agent workspace snapshots on a cancellable worker, cap each document at 16 MiB, and enforce
  the 64 MiB encoded-response limit while collecting the snapshot rather than after accumulating
  the complete workspace in memory.

### Fixed

- Report when a sandbox denies access to an existing Agent sharing socket instead of incorrectly
  claiming that no sharing session is active.
- Bind every Agent proposal to the workspace-relative document path returned by `read`, preventing
  an active-tab switch from retargeting a proposal when two documents have the same source revision.
- Keep an open document's live buffer in Agent workspace results when its disk file is missing,
  while continuing to exclude ignored directories, unsupported files, and outside paths.

## [0.9.1] - 2026-08-10

### Changed

- Let `termdraft-agent` discover the one explicitly shared local session, so normal read and
  proposal commands no longer require users to copy a socket path or token.
- Replaced connection secrets in the Agent sharing panel with credential-free command examples.

### Security

- Store automatic connection details only inside the existing mode-0700 temporary session
  directory in a mode-0600 descriptor, and remove them when sharing is revoked.
- Refuse automatic discovery when more than one live Agent sharing session exists instead of
  guessing which draft the agent should access.

## [0.9.0] - 2026-08-09

### Added

- Added explicit per-document Agent sharing through a private same-user Unix socket and revocable
  session token. The endpoint exposes only the active shared document's exact live source and
  current revision.
- Added the `termdraft-agent` thin client for reading a shared draft and submitting complete-source
  or UTF-8 byte-range proposals.
- Added an in-editor proposal diff with explicit Accept and Reject actions. Accepted proposals are
  one Undo group, publish crash recovery immediately, participate in opt-in Local History, mark the
  buffer modified, and leave disk unchanged until Save.

### Security

- Reject stale, overlapping, out-of-bounds, oversized, inactive-document, and protected-document
  proposals before the editor changes.
- Revoke the private endpoint and pending proposal on explicit revocation, document close, Save As,
  in-app rename or move, and application shutdown.

## [0.8.0] - 2026-07-30

### Added

- Added `--debug` sessions with an in-app input/command strip and a flushed temporary event log;
  typed and pasted document content is not copied into the trace.
- Added opt-in, per-workspace Local History with private full-source checkpoints for manual
  captures, previous saved versions, pre-restore buffers, and outgoing clean reloads.
- Added newest-first checkpoint-to-current diffs plus safe buffer-only restores that remain one
  Undo step and leave disk unchanged until Save.

### Fixed

- Mapped Linux `Ctrl+C`, `Ctrl+X`, and `Ctrl+V` to the system clipboard through Wayland or X11
  without changing the existing macOS `Super+C`, `Super+X`, and `Super+V` path.
- Stopped mouse cursor mapping from looping between both sides of a wide character when a selection
  passes through its middle terminal cell.
- Defaulted new files without an extension to `.md` so they remain visible and indexed.

## [0.7.0] - 2026-07-27

### Added

- Added a monotonic generation and SHA-256 identity for every in-memory source revision, including
  atomic expected-revision checks for asynchronous consumers.
- Added reproducible long-document and many-tab responsiveness gates with a real-PTY acceptance
  journey.

### Changed

- Removed repeated split-preview parsing from the draw path. Rapid edits now debounce into one
  background render, unchanged revisions reuse cached output, and stale completions cannot replace
  newer source.
- Persisted the selected built-in theme in the configuration directory and restore it on the next
  launch.

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

[Unreleased]: https://github.com/Betancourt1/TermDraft/compare/v0.9.2...HEAD
[0.9.2]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.9.2
[0.9.1]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.9.1
[0.9.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.9.0
[0.8.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.8.0
[0.7.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.7.0
[0.6.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.6.0
[0.5.1]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.5.1
[0.5.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.5.0
[0.4.0]: https://github.com/Betancourt1/TermDraft/releases/tag/v0.4.0
