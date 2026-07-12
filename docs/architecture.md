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
   │                                  ▲
   │                     config.toml / theme.tcss
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
   ├──── Enter prefix ───────────────── Markdown continuation rules
   ├──── load / fingerprint ───────────── Persistence
   ├──── scheduled dirty source ───────── Recovery journal
   └──── save / periodic poll ─────────── External-change classifier
```

## Document source of truth

`models/document.py` defines the active `Document`. It owns:

- canonical workspace path;
- current source text;
- the exact last loaded or saved source text;
- the last disk `FileSnapshot`;
- UTF-8 versus UTF-8-with-BOM encoding;
- cursor and scroll coordinates;
- conflict and last-save status;
- current line-ending classification and normalization target;
- whether a draft is journaled or recovered against a conflicting baseline.

`dirty` is computed as `text != saved_text` or an unresolved recovered-baseline conflict; it is not
an independently mutable flag. Reverting an ordinary edit to the baseline therefore clears dirty
state without a special code path.

The `TextArea` is the editable view, not a second domain model. Its `Changed` message updates
`Document.text`; selection changes update cursor and scroll state. Before save or transition,
the coordinator also reads the widget synchronously so a queued message cannot omit the latest
keypress.

Textual reconstructs multiline text by preferring CRLF when present, then LF, then CR. TermWriter retains
both Textual's normalized editor baseline and the exact source represented by that baseline. Merely
loading, recovering, focusing, or saving a deliberately mixed-ending source therefore does not dirty
or rewrite it. Before the editor becomes active, a modal identifies the normalization target and
requires consent. Once the user makes a real edit, Textual's normalized source becomes the local
source. Uniform LF and CRLF are preserved.

## Editor and preview flow

Opening a file is transactional at the application level:

1. `Workspace.validate_document_path` revalidates containment, type, suffix, and symlink policy.
2. `load_file` reads and decodes the complete file into a temporary `LoadedFile` value.
3. A valid recovery journal is offered without mutating the loaded disk baseline.
4. If the selected current source has mixed separators, normalization consent is required.
5. Only after those decisions does the app replace its active `Document`.
6. `TextArea.load_text` runs while `TextArea.Changed` is suppressed and clears prior undo history.
7. Exact and normalized editor baselines, explorer label, preview revision, focus, and status update.

If loading fails, the previously active document remains in memory.

`MarkdownEditor` intercepts Enter only for an empty selection and delegates the source/cursor
calculation to the pure `services/markdown_continuation.py` function. It continues bullets, ordered
markers, tasks, and blockquotes, terminates empty markers, and leaves fenced or indented code to
TextArea. Prefixes containing a tab or four-space run are ambiguous, so that narrow case parses the
source prefix through the cursor with CommonMark and continues only if the current line begins a real
`list_item_open` token. The replacement is one TextArea edit, so undo removes the inserted marker and
newline together. Disabling `editor.auto_continue_lists` restores TextArea's ordinary Enter behavior.

Each source edit increments a preview revision and stops the previous debounce timer. The async
callback verifies its revision before awaiting `Markdown.update`. A stale callback rechecks the
revision, so an old file's render cannot become the final preview for a newer file. Rendering errors
are reported in the UI and never mutate `Document`.

The preview is constructed with `open_links=False` and a dedicated `markdown-it-py` parser factory.
The `gfm-like2` preset provides tables, task metadata, five GFM alert kinds, and
single/double-tilde strikethrough; `mdit-py-plugins` parses footnotes and definition lists. HTML
parsing remains disabled. A final core rule turns task metadata into visible `☐` / `☑` text and
normalizes unsupported alert, footnote, and definition tokens into Textual-supported blockquotes,
inline links, paragraphs, and bullet lists. Footnote labels use one custom `MarkdownBlock` solely as
an internal scroll target; the preview intercepts those generated fragments and records the last
followed reference for `↩` navigation. Definition bodies become supported blockquotes under bold
terms. Unknown alert kinds remain ordinary blockquotes. Other Markdown links render but do not
launch a browser or another external application. None of these preview transforms change
`Document.text`.

## Workspace text search

`services/text_search.py` performs bounded literal, whole-word, or regular-expression searches over
the validated workspace scan. Matching is case-insensitive by default, with an explicit case option
and one case-insensitive workspace-relative POSIX glob. It uses the same safe `load_file` path as
document opening, returns zero-based source coordinates and short line previews, and converts
individual read/decode failures into warnings instead of aborting the search. Invalid and oversized
regexes become result errors before file contents are read. The `regex` engine provides Unicode-aware
whole-word boundaries, full case folding, GIL-releasing immutable-string searches, and a 50 ms
per-line timeout for pathological expressions. The active `Document.text` is passed as a fallback:
dirty source always wins, while a clean document prefers current disk content and still remains
searchable if its path disappeared.

The modal starts search only when Enter is submitted and runs both the recursive scan and file reads
through a Textual thread worker. Cancellation is checked between workspace entries, files, and source
lines; only the result callback updates widgets on the UI thread. Results are deterministic,
same-file aliases are deduplicated, the list is limited to 100 matching lines, and each line produces
at most one result. Selecting a different file revalidates its path and enters the same guarded
transition used by the explorer and file search; a dirty current document therefore still requires
Save / Discard / Cancel. A pending line/column target survives recovery and mixed-line-ending dialogs
and is applied only after the new document is installed.

## Background document I/O

Full Markdown reads, disk probes, content hashes, existing-file publication, Save As publication,
and orphan-source validation run through explicit Textual thread workers. Workers receive immutable
paths, source strings, encodings, snapshots, and document tickets; they never mutate `Document` or a
widget. Expected and unexpected failures are converted into result values, and
`App.call_from_thread` is the only route back to coordinator callbacks.

A document ticket contains the active `Document` identity, canonical path, saved snapshot, and a
monotonic generation. The generation advances when a document is installed, retargeted, reloaded,
saved, or accepts a changed metadata snapshot. Every callback verifies the whole ticket before it
can apply a result. Watcher probes classify their snapshot against the document's *current* dirty
state on the UI thread. The callback first synchronizes visible editor source because Textual's
`Changed` message may still be queued; therefore typing while a slow probe runs turns an external
edit into a conflict instead of an automatic reload. A transition probe that observes a new local
edit re-enters the normal Save / Discard / Cancel guard.

Actual publication and reload/open installation are critical operations. The editor becomes
temporarily read-only, duplicate save/switch/quit actions are rejected, and Save As disables its
input and dismissal controls. This is intentional because cancelling a Textual thread worker does
not stop an operating-system write already in progress. Read/probe workers may finish after
cancellation, but a cancelled or stale worker cannot pass its ticket check. `atomic_save` remains one
indivisible worker operation so its initial check, temporary write, second check, replacement, and
verification retain their existing ordering.

## User configuration

The CLI resolves the configuration root in this order: explicit `--config-dir`,
`TERMWRITER_CONFIG_HOME`, then `~/.termwriter`. Configuration is intentionally user-level rather
than workspace-level, so merely opening a repository cannot install key behavior or visual rules.

`config.toml` is parsed with the standard-library `tomllib`. Only three editor booleans and a closed
set of binding IDs are accepted. Values map binding IDs to key strings, never to Textual action
names; unknown sections, options, IDs, empty values, and duplicate effective keys fail closed. The
initializer uses exclusive creation, does not replace existing files, and creates new directories
and files with mode 0700/0600 where POSIX permissions apply.

Bindings keep stable IDs and are remapped through Textual's public `App.set_keymap`. Undo and redo
are defined on the Markdown editor subclass with IDs so remapping removes their original TextArea
keys rather than leaving hidden aliases. F1 and CLI help are generated from the effective keymap.
The command palette exposes fixed application callbacks; configuration cannot add callbacks.

Bundled layout rules live in `default.tcss`. When an existing user `theme.tcss` is present, the App
loads `[default.tcss, theme.tcss]` in that order and enables Textual's CSS file watcher. Inline widget
CSS is avoided so the user file can override normal selectors. Runtime reload reparses TOML and
updates keybindings and editor properties; an existing theme is independently reloaded by Textual
when saved. Creating the theme after application startup requires one restart.

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

## Crash-recovery journal

`services/recovery.py` stores optional recovery state outside the workspace. The filename is an
opaque SHA-256 of the absolute document path. A versioned JSON entry contains the document path,
workspace root, exact current source, UTF-8 encoding, the saved baseline fingerprint and file/parent
identity, and a timezone-aware update time. It never replaces or changes a Markdown file.

Each journal update creates a mode-0600 temporary file in the recovery directory, writes exact UTF-8
JSON, flushes and `fsync`s it, publishes it with same-directory `os.replace`, and attempts to `fsync`
the directory. Failed pre-publication writes clean the temporary entry and retain the prior complete
journal. The state root is injectable so tests never use personal files.

`TermWriterApp` schedules a nominal 500 ms deadline after the first dirty edit. Later edits update
the in-memory payload without moving that deadline, so sustained typing does not reset the timer.
Synchronous work that blocks Textual's event loop can still delay callback execution.
Successful Markdown saves, explicit discards, and reloads delete the entry. The status bar shows
`RECOVERY STORED` only after journal publication succeeds.

On open, Restore draft keeps the freshly loaded Markdown source as `saved_text` and installs the
journal source as current `text`, so dirty state remains derived rather than forced. If the journal's
baseline content or file/parent identity differs from current disk, `recovery_conflict` prevents
Ctrl+S publication even though the disk was freshly loaded; the user must choose Save As, Reload, or
Cancel. The original baseline is retained when a recovered draft is edited and re-journaled, so a
second crash cannot erase the conflict. Legacy digest-only entries have unknown origin and therefore
fail closed as conflicts. Use disk version deletes the entry. Cancel opening changes neither version.

When a workspace directory starts without an explicit initial file, the journal scanner validates
hashed filenames, entry schemas, workspace containment, and Markdown suffixes. Entries whose source
path is missing, no longer a regular file, inaccessible, or invalid UTF-8 are offered as orphan
recovery. Restoring one installs an unavailable-path conflict: Ctrl+S cannot recreate or replace the
original name, while Save As can publish a new workspace-relative copy. Corrupt entries are skipped
with a warning; entries belonging to another workspace are ignored.

The journal narrows crash loss but is not autosave or history. The 500 ms window, state-directory
write failures, forced termination during the timer, and storage failure can still lose unsaved text.
Journal contents are plaintext source protected by ordinary per-user directory and mode-0600 file
permissions.

## External changes and transitions

`detect_external_change` returns one of:

- `UNCHANGED`: identical bytes at the same file/parent identity, or identical missing state;
- `MODIFIED`: disk changed and the local document is clean;
- `DELETED`: disk path disappeared and the local document is clean;
- `CONFLICT`: disk and local document both changed;
- `INACCESSIBLE`: the current disk state cannot be established.

Checks run before save, before guarded file/quit transitions, on `AppFocus`, and every two seconds
through Textual's interval timer. Probing occurs in a worker and classification occurs against the
latest UI-thread document state. The watcher path never opens a modal: it reloads a clean external
edit, or marks dirty/deleted/inaccessible state as a persistent conflict with one warning. Polling
pauses while a modal, critical operation, or continuation is active. Save and transition checks
remain authoritative and revalidate immediately before acting.

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
- Text search and document content I/O use thread workers and never invoke workspace commands.
- Configuration contains data only; document/workspace contents never define commands or CSS.

Recovery-journal writes and workspace-index scans remain synchronous and may briefly block the UI for
an unusually large dirty document or workspace. Markdown document hashing, stable reads, reloads, and
publication are worker-backed; generation tickets and critical-operation locks preserve callback
ordering and conflict checks.

## Textual API baseline

The implementation targets Textual 8.2.8 and relies on documented APIs:

- [`TextArea`](https://textual.textualize.io/widgets/text_area/)
- [`Markdown`](https://textual.textualize.io/widgets/markdown/)
- [`mdit-py-plugins`](https://mdit-py-plugins.readthedocs.io/en/latest/)
- [`regex`](https://pypi.org/project/regex/)
- [`App.set_keymap`](https://textual.textualize.io/api/app/#textual.app.App.set_keymap)
- [command palette](https://textual.textualize.io/guide/command_palette/)
- [workers](https://textual.textualize.io/guide/workers/)
- [Textual CSS](https://textual.textualize.io/guide/CSS/)
- [`DirectoryTree`](https://textual.textualize.io/widgets/directory_tree/)
- [screens and typed results](https://textual.textualize.io/guide/screens/)
- [`MessagePump.set_interval`](https://textual.textualize.io/api/message_pump/#textual.message_pump.MessagePump.set_interval)
- [Pilot testing](https://textual.textualize.io/guide/testing/)
