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
   │ owns ordered live buffers + one editor per buffer
   ▼
Document tabs ──► active Document/editor ── source text ──► Markdown preview
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

`models/document.py` defines one `Document` per open file. Each instance owns:

- canonical workspace path;
- current source text;
- the exact last loaded or saved source text;
- the last disk `FileSnapshot`;
- UTF-8 versus UTF-8-with-BOM encoding;
- cursor and scroll coordinates;
- conflict and last-save status;
- its mixed-line-ending read-only decision;
- current line-ending classification and normalization target;
- whether a draft is journaled or recovered against a conflicting baseline.

`dirty` is computed as `text != saved_text` or an unresolved recovered-baseline conflict; it is not
an independently mutable flag. Reverting an ordinary edit to the baseline therefore clears dirty
state without a special code path.

Each materialized entry owns a mounted `TextArea` as an editable view, not a second domain model.
Session-restored inactive tabs initially retain only their validated path and tab identity; their
`Document` and editor are created through the normal open pipeline when first selected. Once
materialized, an entry keeps the normalized/exact baseline pair and independent Textual undo stack
for the rest of the process. Widget identity routes `Changed` and selection messages to the matching
`Document`; only the active entry drives preview and status updates. Before save, tab activation, or a
destructive transition,
the coordinator also reads the widget synchronously so a queued message cannot omit the latest
keypress.

## Content-free workspace sessions

`services/session.py` stores one versioned JSON file per canonical workspace under the per-user
state directory. Version two contains ordered open paths, the active path, and nonnegative
cursor/scroll coordinates. It never stores source text, saved baselines, undo history, or recovery
content. Version-one state migrates as one open active path. The workspace hash keeps filenames
opaque; mode-0600 temporary publication, `fsync`, and same-directory `os.replace` preserve the
previous complete session on write failure.

The store rejects files larger than 512 KiB and states with more than 100 document views. The app
loads metadata in a thread worker at the start of mounting. An explicit CLI file suppresses tab
restoration. A directory launch validates the stored paths, creates lightweight tabs in saved order,
and loads only the prior active tab through the normal recovery and mixed-ending decisions. Other
tabs follow that pipeline when selected; missing paths are pruned and access failures are skipped
without dropping their recent view. Writes use one in-flight worker and one
replaceable pending immutable snapshot, so an older thread cannot finish after newer metadata;
clean quit waits for the queue to drain. Ctrl+O opens the bounded MRU order. Each activation caches
the outgoing view, and Save As moves the cached view to the new path. Corrupt session JSON is
preserved and reported. Concurrent TermWriter instances use
last-writer-wins metadata because coordinates are non-authoritative and cannot change Markdown.
Quit traversal does not promote the inspected tabs in MRU order and restores the pre-quit active
buffer before queuing the final snapshot. Once that snapshot is queued, tab activation and document
opening remain sealed until the session and recovery queues drain.

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
5. Only after those decisions does the app register and activate the new `Document`.
6. A new editor is mounted with the selected exact source and an empty history.
7. Its exact/normalized baselines, explorer label, preview revision, focus, and status update.

If loading fails, every existing buffer remains in memory. Opening or activating another path is
non-destructive and therefore does not prompt for dirty work. Closing a tab, replacing its source,
reloading, and quitting remain guarded. Activating a buffer synchronizes the outgoing exact source,
cursor, and scroll; queues its dirty recovery source immediately; changes
`ContentSwitcher.current`; and focuses the incoming editor. It does not call `load_text`, so each
runtime history, selection, and viewport survives tab switches. Reload, discard, or recovery
replacement clears only the affected editor's history because its source baseline changed.
Registration also reuses the stable tab for an already open canonical path. An explicitly restored
recovery draft may replace that clean buffer, but cannot replace an existing dirty buffer.

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

`services/semantic_blocks.py` is a diagnostic boundary, not an editor model. It uses the public
`MarkdownIt.parse` token stream and `Token.map` line ranges with HTML disabled, converts those lines
to exact Python-string offsets for LF/CRLF/CR source, and records every uncovered slice as a
separator or unmapped source. The command-palette inspector parses in a worker, rejects results if
the active document or captured text changed, and discards results while a modal, critical
operation, or exit is active. It can only move the source cursor. Nested containers remain one outer
top-level block. Valid top-level link-reference definitions are added from `parse` environment line
maps after defensive shape and bounds checks; nested definitions are absorbed by their outer
container and malformed reference-like source remains ordinary Markdown. The environment's exact
reference dictionary shape is pinned by the lossless diagnostic corpus but is not treated as an
editing contract. The map is explicitly not sufficient for source splicing.

The opt-in semantic reader reuses that worker and stale-result gate. It mounts a modal snapshot over
the still-mounted full editor, renders only top-level headings and paragraphs with the safe preview
parser and `open_links=False`, and displays every other visible segment through `Static(markup=False)`.
Escape dismisses the snapshot and restores the prior editor focus, cursor, scroll, and undo owner.
The reader has no callback capable of updating `Document`; reference definitions and other nonlocal
syntax remain source fallbacks rather than being reconstructed from render output.

`services/coordinate_diagnostic.py` is a manual, read-only validation aid for the future semantic
editing boundary. It preserves exact source line endings while translating a valid TextArea cursor
into Python-character, UTF-8-byte, logical, and wrapped terminal-cell units through Textual's public
`Document` and `WrappedDocument` models. Extended grapheme checks make unsafe narrow wraps visible;
the service is intentionally one-way and never participates in editing or persistence.

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

Textual renders inline links as action metadata inside `Content`, not as focusable child widgets.
`MarkdownPreview` is therefore one explicit focus stop. While it has focus, Tab/Shift+Tab indexes and
visibly selects those allow-listed `link(...)` spans, Enter dispatches the same `Markdown.LinkClicked`
message as a pointer click, and moving beyond either end returns to the screen focus chain. Footnote
references and backlinks transfer selection to each other; external URLs remain inert because
`open_links=False` and the handler stops every noninternal link. This representation dependency is
contained in the preview widget and covered by Pilot tests under the Textual `<9` version pin.

Heading navigation is independent of link navigation. After a successful render,
`MarkdownPreview` indexes Textual's public `table_of_contents` against its mounted blocks. Alt+Down
and Alt+Up select and scroll headings only while the preview owns focus. A typed `HeadingFocused`
message carries level, label, one-based position, and total to the app. The app prioritizes that text
in the persistent status bar. This is a terminal-visible cue, not a claim of native screen-reader
integration. Boundary presses are inert, so key repeat does not flood the interface. A failed
render clears both link and heading indexes
before returning, so detached blocks cannot remain keyboard-actionable.

## Workspace text search

`services/text_search.py` performs bounded literal, fuzzy, whole-word, or regular-expression searches
over the validated workspace scan. Matching is case-insensitive by default, with an explicit case
option. `services/path_filter.py` parses comma-separated workspace-relative POSIX globs: includes are
ORed, `!` exclusions win, and an exclusion-only filter starts from all paths. File and text search
share this contract. It uses the same safe `load_file` path as document opening, returns zero-based
source coordinates and short line previews, and converts individual read/decode failures into
warnings instead of aborting the search. Invalid filters and invalid or oversized regexes become
result errors before file contents are read. The `regex` engine provides Unicode-aware whole-word
boundaries, full case folding, GIL-releasing immutable-string searches, and a 50 ms per-line timeout
for pathological expressions. Every open `Document.text` is passed as an override: dirty or
conflicted source wins, while a clean document prefers current disk content and still remains
searchable if its path disappeared.

The modal starts search only when Enter is submitted and runs both the recursive scan and file reads
through a Textual thread worker. Cancellation is checked between workspace entries, files, source
lines, and periodically within long fuzzy matches. A submission revision prevents an older callback
from replacing results after the mode, filter, or case option changes. Only the accepted callback
updates widgets on the UI thread. Results are deterministic, same-file aliases are deduplicated, the
list is limited to 100 matching lines, and each line produces at most one result. Fuzzy mode
canonically decomposes Unicode while retaining original source columns, keeps a bounded set while
scanning, then orders matches globally by subsequence tightness, word boundary, source position,
line length, path, and line. File search uses
the same normalized subsequence idea while preserving prefix and substring priority. Selecting a
different file revalidates its path, activates an existing buffer when present, or opens a new tab.
A pending line/column target survives recovery and mixed-line-ending dialogs and is applied only
after the target document is installed.

## Active-document find and replace

`services/document_search.py` finds every non-overlapping escaped-literal span in the active editor
source. The `regex` engine's full Unicode case folding preserves original source offsets even when a
match expands during comparison. The dialog owns only its immutable source snapshot, query,
replacement text, match tuple, and selected index; typed messages ask the app coordinator to select
or replace source.

Selection converts source offsets to TextArea locations without changing Markdown. A single replace
uses the selected range, while Replace all builds one final string and submits one full-document
TextArea edit, preserving a one-step undo. The coordinator then follows the normal Changed path for
dirty state, preview, and recovery. Modal focus prevents document switching during the operation;
the coordinator still rejects a request if the live editor source no longer matches the dialog
snapshot. Read-only documents can search but cannot emit replacement requests.

## Background document I/O

Full Markdown reads, disk probes, content hashes, existing-file publication, Save As publication,
workspace indexing, session load/save, recovery-record reads/mutations, orphan-source validation,
and semantic mapping run through explicit Textual thread workers. Workers receive immutable
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

`config.toml` is parsed with the standard-library `tomllib`. Only three editor booleans, a positive
`recovery.retention_days` integer representable from the current date, and a closed set of binding
IDs are accepted. Values map binding
IDs to key strings, never to Textual action names; unknown sections, options, IDs, empty values, and
duplicate effective keys fail closed. The retention value is consulted only after the user opens
Recovery Manager; it cannot schedule deletion. The initializer uses exclusive creation, does not
replace existing files, and creates new directories and files with mode 0700/0600 where POSIX
permissions apply.

TermWriter starts in the configured COMMAND or WRITE mode, defaulting to COMMAND. In COMMAND mode
the Markdown editor stops consuming printable input and priority application bindings own the
mnemonic keymap. Arrow navigation still passes through the editor; configurable `h`/`j`/`k`/`l`,
line-boundary, and document-boundary actions call TextArea cursor methods without editing source.
Those navigation bindings are enabled only while the active source editor has focus, so explorer,
preview, and modal inputs retain their keys. `i` enters WRITE mode and editor focus; Esc returns to
COMMAND mode. The persistent status bar always leads with the interaction mode and adds FILES or
PREVIEW when focus leaves the source editor.

Configured bindings keep stable IDs and are remapped through Textual's public `App.set_keymap`.
They remain available in both interaction modes. Undo and redo are defined on the Markdown editor
subclass with IDs so remapping removes their original TextArea keys rather than leaving hidden
aliases. F1 and CLI help are generated from the effective keymap, including remapped COMMAND keys.
The command palette exposes fixed application callbacks; configuration cannot add callbacks.

Bundled layout rules live in `default.tcss`. When an existing user `theme.tcss` is present, the App
loads `[default.tcss, theme.tcss]` in that order and enables Textual's CSS file watcher. Inline widget
CSS is avoided so the user file can override normal selectors. Runtime reload reparses TOML,
updates keybindings and editor properties, and applies the new retention age the next time Recovery
Manager opens; an existing theme is independently reloaded by Textual when saved. Creating the theme
after application startup requires one restart.

## Workspace boundary

`Workspace` canonicalizes the root and validates every opened or newly saved file. Its independent
scanner uses `os.scandir`, ignores common generated directories, catches per-directory `OSError`,
and indexes only `.md` and `.markdown` files.

`services/workspace_entries.py` owns the small create, rename, move, and remove surface. New file
publication reuses guarded no-clobber persistence; rename and move reject existing destinations,
workspace escapes, ignored directories, unsupported Markdown suffixes, and moving a folder inside
itself. The app executes mutations in one exclusive worker and reloads the explorer/index only after
success. Clean open documents are retargeted with their session views; dirty documents cannot move,
and any open document blocks removal. Recursive folder removal stays behind an explicit warning that
includes entries hidden by the Markdown-only explorer.

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

For a new Save As destination, an immutable snapshot of open buffer paths first reserves exact and
normalized spelling keys for all live document identities, including paths whose files disappeared.
This conservative rule also prevents case/Unicode spelling variants from becoming duplicate buffers
when the filesystem treats them as one entry. The fully written temporary file is then hard-linked
to the final name. Hard-link creation fails if the name exists, providing no-clobber publication.
The temporary name is then removed.

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
journal. Persistent per-journal lock files serialize mutations by cooperating TermWriter processes;
retarget acquires its source and destination locks in deterministic order. The locks are advisory and
their guarantees still depend on the filesystem. The state root is injectable so tests never use
personal files.

`TermWriterApp` schedules a nominal 500 ms deadline after the first dirty edit. Later edits update
the in-memory payload without moving that deadline, so sustained typing does not reset the timer.
Tab deactivation queues its exact dirty source immediately. One non-exclusive thread-worker pipeline
serializes SAVE and DELETE tickets: pending saves for a path coalesce to the newest source, while a
DELETE removes queued saves and forms an ordering barrier behind any in-flight publication. A save
completion marks `recovery_saved` only when its document identity, path, exact source, encoding, and
baseline still match. Successful Markdown saves, explicit discards, and reloads wait for ordered
cleanup before continuing. The status bar shows `RECOVERY STORED` only after a current publication
succeeds. Destructive transitions keep the affected editor read-only across this cleanup barrier;
Discard restores the model's saved source before its close or quit continuation runs.

The CLI installs temporary `SIGTERM` and, where available, `SIGHUP` handlers around `App.run()` and
restores the process's previous handlers afterward. A handler performs no I/O; it only records the
signal number. A 50 ms Textual interval consumes that request once critical file I/O is idle, reads
every mounted editor into its `Document`, stops the pending debounce timer, freezes all editors, and
queues one exact recovery SAVE for every dirty tab. The existing session/recovery drain calls public
`App.exit()` only when both pipelines are empty. If one of those required recovery publications
fails, shutdown is cancelled and prior editor read-only states are restored. Ctrl+Q remains a
separate interactive Save/Discard/Cancel path. `SIGKILL`, power loss, an unresponsive event loop, and
external TERM-to-KILL deadlines remain outside this cooperative guarantee.

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
original name, while Save As can publish a new workspace-relative copy. Entries belonging to another
workspace are ignored. Immediately before each orphan prompt, a worker rereads the exact
`RecoveryRecord` and source availability so a delayed inventory cannot present an older cooperating
process's draft. This does not lock the record for the duration of the user's dialog decision;
another instance can publish afterward, and recovery metadata remains last-writer-sensitive rather
than a version history.

Every successful publication returns an exact content fingerprint. Cleanup acquires the same
per-journal lock and deletes only that observed version; a newer or newly appeared journal is
preserved and reported instead. This avoids a canceled or delayed worker deleting another instance's
newer draft.

The command-palette recovery manager inventories journals in a worker and returns structured
`RecoveryRecord` values. Valid records are filtered to the current workspace; corrupt records stay
visible because their metadata cannot be trusted enough to associate them. A fingerprint binds every
mutation to the exact listed bytes. Retarget validates workspace containment, publishes the new
journal with a no-clobber hard link, then removes the old record. Archive uses the same no-clobber
rule to preserve exact bytes under `quarantine/` before removing the active entry. Quarantine
inventory retains corrupt entries but only valid trusted entries can be restored. Restore hard-links
the exact archived bytes back into the active inventory without replacing an existing journal;
permanent deletion requires a separate confirmation and fingerprints the selected quarantine version
again. The manager revalidates resolved path containment and protects every dirty open document
immediately before moving, archiving, retargeting, or restoring a journal.

A valid quarantined entry may also be exported to a new workspace-contained Markdown path. Export
re-verifies the listed fingerprint, preserves the stored UTF-8/UTF-8-SIG source exactly, uses the
normal parent-bound `snapshot_file` / `atomic_save` no-clobber path, and leaves the quarantine record
unchanged. Manual retention selects only valid entries strictly older than the configured cutoff.
The confirmation lists every selected path and carries the exact displayed `RecoveryRecord` tuple
into a worker, so a newly appearing old record is not implicitly added after consent. Each deletion
is independently fingerprint-guarded; every partial failure remains in the result and is reported
rather than rolled into a false all-success message. Corrupt entries have no trusted timestamp and
are never age-deleted.

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

Checks run before save, before guarded close/quit transitions, when a tab is reactivated, on
`AppFocus`, and every two seconds through Textual's interval timer. Each periodic pass probes the
active document plus one rotating inactive tab. Periodic probes compare size, mtime, ctime, mode,
file identity, and parent identity first; exact metadata matches reuse the immutable baseline without
opening the file, while any difference performs the stable hashed read. Save, transition,
activation, and focus checks keep using unconditional full hashes. Probing occurs in a worker and
classification occurs against the latest UI-thread document state.
The watcher path never opens a modal: it reloads a clean external edit only when that document is
active. An inactive edit instead gets a persistent `!` tab state; activation performs the normal
authoritative check and can then reload or report a conflict. Polling pauses while a modal, critical
operation, or continuation is active. Save and transition checks remain authoritative and
revalidate immediately before acting.

A separate worker scan on the same interval compares the visible Markdown files and folders with the
last applied workspace scan. Structural differences reload the explorer and update the file-search
index; unchanged scans do neither. The scan also runs when the app regains focus and still ignores
symlinks, unsupported files, and configured ignored directories. External renames are represented as
one removed path and one added path. Open documents are not retargeted by inference, so the existing
deletion/conflict flow continues to protect their in-memory source.

The `TermWriterApp` coordinator owns pending transition and save continuations. Typed modal callbacks
implement these paths:

```text
dirty close/reload/quit ──► Save ──► save succeeds ──► continue
        │             └────► conflict ───────► Save As / Reload / Cancel
        ├────────► Discard ──────────────────► continue
        └────────► Cancel ───────────────────► stay
```

No dialog merely displays a warning and then ignores the answer. A save failure or cancellation
clears the continuation, leaves the source in memory, and stops the transition. External deletion of
a clean file is also guarded before transition because the in-memory source may be the last copy; the
user may save that copy under a new name, explicitly continue without it, or cancel. Multi-document
quit starts with the active buffer, inspects every other buffer without changing MRU order, restores
the original active buffer, and then seals editing while final state writes drain.

## Widget/domain limits

- Widgets render state and emit Textual messages.
- `TermWriterApp` owns ordered live `Document`/editor entries, coordinates one active view, and
  resolves user decisions.
- `Document` and `Workspace` contain state/invariants without Textual imports.
- Persistence and external-change modules perform filesystem work without UI calls.
- File search fuzzy-ranks only the scanner's validated in-process index and has no `ripgrep`
  dependency.
- Text search and document content I/O use thread workers and never invoke workspace commands.
- Configuration contains data only; document/workspace contents never define commands or CSS.

Workspace-index scans, workspace-entry mutations, recovery-record reads, Markdown hashing, stable
reads, reloads, publication, session I/O, recovery mutation, and semantic mapping are worker-backed.
Cooperative cancellation, revisions, document tickets, immutable mutation payloads, and
critical-operation locks preserve callback ordering and conflict checks. An individual
operating-system read cannot be interrupted, so stale-result rejection remains authoritative.

`termwriter-benchmark` is intentionally repository-specific instrumentation rather than runtime
abstraction. It calls the real semantic mapper, mounts the real application and tab editors through
`App.run_test`, and invokes the real bounded active-plus-inactive watcher worker. Its JSON timings,
`tracemalloc` heap values, and process peak-RSS high-water values are comparative observations,
never correctness gates. The benchmark writes only deterministic Markdown and state into its own
temporary directory.

## Textual API baseline

The implementation targets Textual 8.2.8 and relies on documented APIs:

- [`TextArea`](https://textual.textualize.io/widgets/text_area/)
- [`ContentSwitcher`](https://textual.textualize.io/widgets/content_switcher/)
- [`Tabs`](https://textual.textualize.io/widgets/tabs/)
- [`markdown-it-py Token.map`](https://markdown-it-py.readthedocs.io/en/latest/api/markdown_it.token.html)
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
