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
I/O. Existing-file saves also compare the loaded file and parent-directory identities, then perform
temporary creation, cleanup, and publication relative to an open parent descriptor. Initial loads
and new Save As validation are not a complete adversarial sandbox; fully closing their validation/use
window would require descriptor-relative traversal from the workspace root.

## Persistence strategy

`FileSnapshot` contains existence, SHA-256 content digest, byte size, nanosecond modification time,
mode, file device/inode, and parent-directory device/inode. The digest is the primary content check;
replacing the file or parent identity is also treated as an external change even when bytes match.
An existing save requires both content and origin to match its loaded baseline. A same-size edit with
a restored timestamp is still detected. Touching identical bytes in place is not a content conflict.

`load_file` uses `os.open` with `O_NOFOLLOW` where available, verifies a regular file with `fstat`,
reads bytes without universal-newline conversion, and compares pre/post-read identity and metadata.
It retries a moving file once, then fails rather than claiming a stable baseline.

For an existing destination, `atomic_save`:

1. opens the parent directory and verifies its identity against the loaded snapshot;
2. hashes the destination through that descriptor and compares content and origin;
3. creates a random mode-0600 temporary entry relative to the open directory;
4. writes encoded bytes, flushes, and calls `fsync`;
5. hashes the destination again and verifies its mode and the lexical parent's identity;
6. attempts to apply the verified destination mode bits to the temporary file;
7. calls descriptor-relative `os.replace`;
8. attempts a parent-directory `fsync`;
9. hashes the published file and verifies the intended digest and parent identity.

Temporary entries are unlinked through the retained parent descriptor on every safe failure before
publication, even if that directory is renamed. A failed `os.replace` leaves the original name and
bytes intact. A failure after publication is different: the name may already point to the new bytes,
so the application reports uncertainty and does not advance its in-memory baseline.

For a new Save As destination, the fully written temporary file is hard-linked to the final name.
Hard-link creation fails if the name exists, providing no-clobber publication. The temporary name is
then removed.

Same-directory `os.replace` provides atomic namespace replacement on normal local POSIX filesystems;
it does not prove identical behavior on every network filesystem. `fsync` improves crash durability
but the filesystem and storage stack retain the final say. Replacement preserves ordinary mode bits
where the filesystem permits, but special setuid/setgid bits are not guaranteed. It does not preserve
the old inode, ownership differences, ACLs, extended attributes, or hard-link identity.

There remains a small check-to-replace race: a non-cooperating process can modify the destination
after the second fingerprint and immediately before `os.replace`. Ordinary cross-platform path APIs
do not provide a version-conditional rename. The implementation narrows and documents this window
instead of claiming an absolute guarantee.

## External changes and transitions

`detect_external_change` returns one of:

- `UNCHANGED`: identical bytes at the same file/parent identity, or identical missing state;
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
a clean file is also guarded before transition because the in-memory source may be the last copy; the
user may save that copy under a new name, explicitly continue without it, or cancel.

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
