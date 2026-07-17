# Future semantic block editing in Rust

This document describes editable semantic blocks beyond TermDraft's current read-only diagnostics.
The Rust application contains the legacy Python semantic-block inspector, coordinate diagnostic,
and experimental block reader; none of them can update `Document` or replace the full-source editor.

Today, one full-source `tui-textarea-2` editor remains authoritative. Inactive lines can hide or
style common Markdown markers, but those styles do not change character positions or write rendered
content back to the file. The cursor line always exposes exact source.

## Goal

A future semantic mode could render inactive Markdown blocks while exposing the active block as its
exact source. The file, not the rendered widgets, must remain authoritative:

```text
Markdown source
      │ parse with verified source ranges
      ▼
AST + block index
      │
      ├── inactive block ──► terminal rendering
      └── active block ────► source editor ──► checked range splice
```

Switching blocks must never reconstruct the whole document from rendered output. Unsupported or
ambiguous syntax must fall back to the existing full-source editor without modifying the file.

## Required source model

An editing-grade parser would need every block to retain:

- semantic kind and parent/child relationship;
- exact start and end offsets in the original UTF-8 source;
- logical line range;
- exact source slice, including separators;
- a short-lived identity for mapping nearby edits where practical.

The implementation must keep these units distinct:

1. UTF-8 byte offsets in the complete source;
2. logical textarea `(row, column)` positions;
3. grapheme boundaries for user-visible cursor movement;
4. terminal-cell positions after wrapping and wide-character layout;
5. parser block ranges.

Rust string indexes are UTF-8 byte offsets, while textarea and terminal coordinates are not. A
conversion layer must validate boundaries rather than cast between these units. Tests need LF,
CRLF, CR, Unicode combining marks, CJK, emoji sequences, tabs, missing final newlines, and malformed
Markdown before range splicing can be safe.

## Incremental route

### 1. Read-only block map

Parse the current `Document.text` and display non-overlapping block ranges in a developer-only
overlay. Joining every mapped block and uncovered gap must reproduce the original source exactly.
The first implementation should not edit or save anything.

Status: implemented as the read-only `b` overlay. Rust maps common top-level blocks, preserves every
uncovered source slice, handles LF/CRLF/CR and Unicode offsets, and lets Enter jump to the selected
source line. The map is a diagnostic contract, not an editing model.

### 2. Independent rendered blocks

Render headings and paragraphs from their exact slices while retaining the full-source editor as an
immediate fallback. Keep links inert and leave lists, fences, tables, references, and ambiguous
containers as exact source until each family has verified ranges.

Status: implemented as the read-only `B` overlay. Headings and paragraphs use rendered presentation;
every other visible segment uses exact source fallback. It scrolls independently and cannot edit,
save, or splice blocks. The normal Split view still renders one preview for the whole document.

### 3. One active source block

Allow keyboard selection of a supported block. The selected block receives a small source editor;
all other supported blocks remain rendered. Activation must preserve a semantic anchor plus the
best available viewport position.

### 4. Checked range splice

Commit an edited block only against the exact source revision and range from which it was opened.
After the splice, reparse the document. If the revision changed or ranges cannot be reconciled,
return to the full-source editor with the user's edited text intact and do not save automatically.

### 5. Expand syntax deliberately

Add nested lists, quotes, code fences, tables, definitions, and references one verified family at a
time. Full reparsing is the simplest correct baseline; incremental parsing should be considered only
after measurement shows it is necessary.

## Undo, conflicts, and persistence

Undo must describe source transformations, not widget replacement. The current Rust tabs each own
an independent textarea history. A semantic editor would need one coherent history that can cross
block activation and full-source fallback without losing changes.

The existing persistence boundary remains unchanged: complete source plus `FileSnapshot` is the
unit of conflict detection. External changes must disable semantic splicing and use the existing
conflict/Save As path. Encoding, BOM, and original line endings remain properties of the whole
document, not individual blocks.

## Risks that remain open

- Markdown extensions do not all expose exact, stable source ranges.
- Editing a delimiter can change the kind or extent of neighboring blocks.
- Reference links and definitions create nonlocal rendering dependencies.
- Rendered block height can change sharply after a resize.
- Selection, IME composition, bidirectional text, and terminal width need real interaction tests.
- Large documents may make full parsing or mounting many rendered widgets too expensive.
- Mixed line endings retain exact bytes until explicit consent and the first edit. An editable
  semantic mode would need to decide whether consent normalizes the whole document before any block
  range is opened; the current read-only diagnostics can inspect mixed source safely.

The 20/80 boundary remains the proven full-source editor, presentation-only inline view, and the two
read-only semantic overlays now in the branch. Editable widgets should begin only after the block
map and coordinate diagnostics are treated as tested prerequisites; jumping directly to source
splicing would weaken the source-safety contract for a feature the branch does not yet need.
