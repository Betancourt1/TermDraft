# Future semantic block editing

This document describes a possible WYSIWYM mode for TermWriter. It is a design direction, not a
promise that the hard interaction problems are solved. The MVP now includes only Stage 1's read-only
top-level range inspector; rendered-block and hybrid editing remain unimplemented.

## Goal

The Markdown file remains the only authoritative document. An inactive block may be displayed as a
semantic rendering; the block containing the logical cursor exposes its original Markdown source.
Switching modes must never reconstruct the whole file from rendered output.

```text
Markdown source
      │ parse with source ranges
      ▼
AST + block index
      │
      ├── inactive block ──► rendered block
      └── active block ────► source editor ──► source-range splice
```

## Source and AST

A parser must produce an AST whose block nodes retain trustworthy source ranges. Each block record
would need at least:

- semantic kind;
- start and end offsets in the Markdown source;
- start and end logical lines;
- original source slice;
- parent/child relationships for nested structures;
- a short-lived identity that can survive nearby edits where practical.

Source offsets must be defined explicitly. Python string indexes count Unicode code points, UTF-8
uses bytes, Textual cursor columns are logical positions, and terminal cells vary by grapheme width.
Mixing these units is a direct route to corrupt splices or misplaced cursors.

The command-palette-only **Inspect cursor coordinates** diagnostic now reports one immutable cursor
snapshot as an exact Python source offset, UTF-8 byte offset, Textual logical location, wrapped row
and cell, and live terminal-screen offset. It recognizes extended grapheme boundaries and warns
when Textual's current narrow wrapping divides one. Tests cover LF, CRLF, CR, mixed endings, tabs,
combining marks, CJK, emoji, and ZWJ sequences. This validates the coordinate units but deliberately
does not expose a reverse mapping or permit source edits.

The diagnostic prototype uses `markdown-it-py` public block tokens and `Token.map` line ranges. It
converts logical lines to Python-string offsets without normalizing LF, CRLF, or CR and retains every
uncovered source slice. Valid top-level link-reference definitions use defensively validated line
maps from the parser environment and are covered by the same lossless corpus. This is not yet an
editing-grade parser choice: the exact reference metadata shape, inline delimiters, nested
identities, and extension-specific exact ranges are not stable source-splicing contracts.

## Active and inactive blocks

Only the active block would be editable as source. Inactive blocks would be rendered views derived
from their exact source slices. Changing the active block would:

1. commit the old block's current source as a range replacement in the full Markdown string;
2. parse enough of the document to refresh affected ranges;
3. map the logical cursor into the new active source editor;
4. replace the prior active editor with a rendering;
5. preserve viewport position as closely as possible.

The full `Document.text` remains the value saved to disk. Rendered widgets never emit Markdown.

## Position mapping

At least four coordinate systems are involved:

1. source offsets in the full Python string;
2. `(line, column)` positions in the active source block;
3. AST block/range positions;
4. terminal `(row, cell)` positions after wrapping and rendering.

Mappings must account for:

- soft wrapping changing when a pane is resized;
- wide CJK characters, combining marks, emoji sequences, and tabs;
- Markdown markers that occupy source columns but not rendered cells;
- rendered prefixes for lists, quotes, tasks, and headings;
- blocks whose rendered height differs substantially from source height.

A pixel-style “same vertical coordinate” switch is insufficient. The design likely needs semantic
anchors such as block identity plus an offset inside the block, followed by a best-effort visual
scroll correction after layout.

## Incremental implementation strategy

### Stage 1: read-only block diagnostics

Parse a document and display block boundaries/ranges in a developer-only view. Validate that joining
all untouched source slices reproduces the original bytes after encoding. Test Unicode, LF, CRLF,
missing final newlines, nested lists, block quotes, fences, tables, references, and malformed input.

Status: implemented as **Inspect semantic blocks** in the command palette. The worker-backed mapper
lists non-overlapping top-level ranges and explicit separator/unmapped gaps, rejects stale results,
and can move the full-source editor cursor. Nested containers deliberately remain one outer block;
valid top-level link-reference definitions are mapped from parser metadata, while malformed source
remains a normal block or explicit gap. A 26-case corpus independently verifies contiguous offsets,
source and UTF-8 reconstruction, LF/CRLF/CR, Unicode, nested containers, references, extensions, and
malformed input. The inspector never splices, saves, or renders source.

### Stage 2: rendered blocks without editing

Render independent top-level blocks while keeping the existing full-source editor available as a
fallback. Measure scroll stability and parser performance. Do not hide source syntax yet.

Status: a command-palette-only experiment renders top-level headings and paragraphs in a modal
snapshot. Lists, quotes, code, tables, definitions, gaps, and other unsupported constructs remain
exact source fallbacks; links are inert and Escape returns to the untouched full editor. This proves
safe independent mounting, not scroll stability, nonlocal reference resolution, or editing.

### Stage 3: one active source block

Allow clicking or keyboard navigation to select one block. Show that block's exact source while the
rest remain rendered. The first version should support simple paragraphs and headings only; fenced
code, nested containers, and ambiguous ranges stay in full-source mode.

### Stage 4: range-splice editing

Apply edits only to the active block's known source interval, then reparse. If the new parse cannot
reconcile ranges, fall back immediately to the full-source editor with the edited source intact.

### Stage 5: broader constructs and performance

Add lists, quotes, code fences, tables, and extension syntax one verified family at a time. Introduce
incremental parsing only after correctness is established with full reparsing.

## Undo, conflicts, and persistence

Undo must operate on source transformations, not widget replacement. Current full-source tabs each
own a Textual undo history and preserve it across runtime tab switching, but a future hybrid mode
would still need one coherent history for block activation and full-document source transformations.

External-change detection and atomic persistence do not change: the complete Markdown source and its
disk fingerprint remain the conflict boundary. If a conflict appears, semantic editing must stop and
use the same explicit Save As / reload / cancel flow as full-source editing.

## Unresolved risks

- Exact source ranges for every Markdown extension may require parser changes or a custom source map.
- Nested blocks do not always have visually independent boundaries.
- Editing delimiters can change the semantic type and extent of neighboring blocks.
- Reference-style links and definitions create nonlocal rendering dependencies.
- List renumbering must never rewrite source unless the user edits it.
- Mixed line endings need an explicit policy before source splicing can be called lossless.
- IME composition, bidirectional text, grapheme clusters, and terminal-width disagreement need real
  interaction tests.
- The diagnostic can expose grapheme-splitting wraps, but terminal/font width rules can still differ
  from Textual and IME or bidirectional cursor behavior remains unmodeled.
- Widget replacement can disrupt selection, accessibility, scroll position, and undo grouping.
- Large documents may make full reparsing or mounting many rendered widgets too slow.

The safe fallback is always the current full-source `TextArea`. Semantic mode should ship only for
constructs whose source mapping is demonstrably reversible, with unsupported cases falling back
without modifying the file.
