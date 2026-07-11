# TermWriter architecture

## Product boundary

The MVP is a local Markdown editor, not a document database and not a terminal browser. Markdown
files remain independently useful before, during, and after TermWriter runs. The application never
executes document text, raw HTML, JavaScript, or workspace-defined commands.

The implementation deliberately uses concrete modules instead of service interfaces that have only
one implementation:

```text
CLI target
   │
   ▼
Workspace ───── validated paths / scan index ───── File explorer + file search
   │
   ▼
TermWriterApp coordinator
   │ owns exactly one
   ▼
Document  ───────── source text ─────────► Markdown preview
   ▲  │
   │  └──── cursor / scroll / status ───► Status bar
   │
TextArea source edits
   │
   ├──── load / fingerprint ───────────── Persistence
   └──── save / conflict check ────────── External-change classifier
```

## Document source of truth

`models/document.py` defines the active `Document`. It owns:

- canonical workspace path;
- current source text;
- the exact last loaded or saved source text;
- the last disk `FileSnapshot`;
- UTF-8 versus UTF-8-with-BOM encoding;
- cursor and scroll coordinates;
- conflict and last-save status.

`dirty` is computed as `text != saved_text`; it is not an independently mutable flag. Reverting an
edit to the baseline therefore clears dirty state without a special code path.

The `TextArea` is the editable view, not a second domain model. Its `Changed` message updates
`Document.text`; selection changes update cursor and scroll state. Before save or transition,
the coordinator also reads the widget synchronously so a queued message cannot omit the latest
keypress.

Textual reconstructs multiline text with the first line separator it detects. TermWriter retains a
separate editor baseline so merely loading or focusing a deliberately mixed-ending file does not
dirty or rewrite it. Once the user makes a real edit, Textual's normalized source becomes the local
source. Uniform LF and CRLF are preserved.

## Editor and preview flow

Opening a file is transactional at the application level:

1. `Workspace.validate_document_path` revalidates containment, type, suffix, and symlink policy.
2. `load_file` reads and decodes the complete file into a temporary `LoadedFile` value.
3. Only after loading succeeds does the app replace its active `Document`.
4. `TextArea.load_text` runs while `TextArea.Changed` is suppressed and clears prior undo history.
5. The editor baseline, explorer label, preview revision, focus, and status are updated.

If loading fails, the previously active document remains in memory.

Each source edit increments a preview revision and stops the previous debounce timer. The async
callback verifies its revision before awaiting `Markdown.update`. A stale callback rechecks the
revision, so an old file's render cannot become the final preview for a newer file. Rendering errors
are reported in the UI and never mutate `Document`.

The preview is constructed with `open_links=False`. Markdown links are rendered but do not launch a
browser or another external application.

## Workspace boundary

`Workspace` canonicalizes the root and validates every opened or newly saved file. Its independent
scanner uses `os.scandir`, ignores common generated directories, catches per-directory `OSError`,
and indexes only `.md` and `.markdown` files.

The MVP rejects all explorer symlinks and Markdown file symlinks. This is more restrictive than
following links that happen to resolve inside the root, but keeps both selection and replacement
behavior understandable. Resolved containment and final-file checks are repeated before application
I/O. Path-based checks do not fully defend against a hostile concurrent symlink swap; descriptor-
relative traversal would be a separate hardening project.

## Persistence strategy

`FileSnapshot` contains existence, SHA-256 content digest, byte size, nanosecond modification time,
mode, device, and inode. The digest decides whether content changed; metadata is retained for
diagnostics and permissions. A same-size edit with a restored timestamp is still detected. Touching
identical bytes is not a content conflict.

`load_file` uses `os.open` with `O_NOFOLLOW` where available, verifies a regular file with `fstat`,
reads bytes without universal-newline conversion, and compares pre/post-read identity and metadata.
It retries a moving file once, then fails rather than claiming a stable baseline.

For an existing destination, `atomic_save`:

1. hashes the destination and compares it to the expected snapshot;
2. uses `tempfile.mkstemp` in the same directory;
3. writes encoded bytes, flushes, and calls `fsync`;
4. applies the current destination mode bits to the temporary file;
5. hashes the destination again;
6. calls `os.replace`;
7. attempts a parent-directory `fsync`;
8. hashes the published file and verifies the intended digest.

Temporary paths are cleaned on every failure before publication. A failed `os.replace` leaves the
original name and bytes intact. A failure after publication is different: the name may already point
to the new bytes, so the application reports uncertainty and does not advance its in-memory baseline.

For a new Save As destination, the fully written temporary file is hard-linked to the final name.
Hard-link creation fails if the name exists, providing no-clobber publication. The temporary name is
then removed.

Same-directory `os.replace` provides atomic namespace replacement on normal local POSIX filesystems;
it does not prove identical behavior on every network filesystem. `fsync` improves crash durability
but the filesystem and storage stack retain the final say. Replacement preserves mode bits but not
the old inode, ownership differences, ACLs, extended attributes, or hard-link identity.

There remains a small check-to-replace race: a non-cooperating process can modify the destination
after the second fingerprint and immediately before `os.replace`. Ordinary cross-platform path APIs
do not provide a version-conditional rename. The implementation narrows and documents this window
instead of claiming an absolute guarantee.

## External changes and transitions

`detect_external_change` returns one of:

- `UNCHANGED`: identical bytes or identical missing state;
- `MODIFIED`: disk changed and the local document is clean;
- `DELETED`: disk path disappeared and the local document is clean;
- `CONFLICT`: disk and local document both changed;
- `INACCESSIBLE`: the current disk state cannot be established.

Checks run before save, before guarded file/quit transitions, and on `AppFocus` where the terminal
supports focus events. Save and transition checks remain authoritative because focus events are not
universal.

The `TermWriterApp` coordinator owns pending transition and save continuations. Typed modal callbacks
implement these paths:

```text
dirty open/quit ──► Save ──► save succeeds ──► continue
        │             └────► conflict ───────► Save As / Reload / Cancel
        ├────────► Discard ──────────────────► continue
        └────────► Cancel ───────────────────► stay
```

No dialog merely displays a warning and then ignores the answer. A save failure or cancellation
clears the continuation, leaves the source in memory, and stops the transition. External deletion of
a clean file is also guarded before transition because the in-memory source may be the last copy.

## Widget/domain limits

- Widgets render state and emit Textual messages.
- `TermWriterApp` coordinates one active document and user decisions.
- `Document` and `Workspace` contain state/invariants without Textual imports.
- Persistence and external-change modules perform filesystem work without UI calls.
- File search ranks only the scanner's validated in-process index and has no `ripgrep` dependency.

Synchronous hashing and writes may briefly block the UI for very large Markdown files. Moving I/O to
a worker is a future performance improvement, but it must preserve ordered callbacks and all current
conflict checks.

## Textual API baseline

The implementation targets Textual 8.2.8 and relies on documented APIs:

- [`TextArea`](https://textual.textualize.io/widgets/text_area/)
- [`Markdown`](https://textual.textualize.io/widgets/markdown/)
- [`DirectoryTree`](https://textual.textualize.io/widgets/directory_tree/)
- [screens and typed results](https://textual.textualize.io/guide/screens/)
- [Pilot testing](https://textual.textualize.io/guide/testing/)

