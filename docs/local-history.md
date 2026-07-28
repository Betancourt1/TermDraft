# Local History

Local History is an opt-in, per-workspace safety net for the Rust TermDraft application. It keeps
bounded full-source checkpoints on the same device so an earlier buffer can be compared or restored
without changing the Markdown file immediately.

Open the command palette and choose **Open Local History**. There is intentionally no direct
shortcut. The first screen explains the difference from Undo and Recovery and lets you enable the
feature for the current workspace. **Create checkpoint** is available from the same palette.

## What is captured

Once enabled, TermDraft can retain these checkpoint reasons:

- **Manual** — the current buffer when you choose **Create checkpoint**.
- **Previous saved version** — the old saved baseline, only after a newer Save succeeds.
- **Before restore** — the current buffer before a Local History restore is applied.
- **Before external reload** — the outgoing clean source before TermDraft installs a changed disk
  version automatically.

Failed saves do not create a “previous saved version” checkpoint. Dirty external changes remain
conflicts and are not replaced. Adjacent checkpoints with identical source are deduplicated.

The history list is newest first. Its diff reads **checkpoint → current**, compacts long unchanged
regions, and is bounded for responsive rendering. A truncation notice affects only the displayed
diff; the retained checkpoint remains complete.

## Restore safety

Restore is a revision-checked buffer transaction:

1. TermDraft re-reads the selected checkpoint and verifies that the open source has not changed
   since Local History was opened.
2. It verifies the document is editable and that disk still matches the loaded baseline.
3. It durably captures the current buffer as **Before restore**.
4. It replaces the buffer as one grouped edit.

Disk, the saved baseline, and encoding remain unchanged until an explicit Save. For a
mixed-line-ending document, restore remains blocked until normalization is accepted. After consent,
restore counts as the first edit and activates the already chosen line-ending target, just like
typing or Replace All; Undoing back to the untouched source restores its mixed state. One Undo
returns to the pre-restore buffer, and Redo reapplies the checkpoint.

If the original file is missing, restore keeps the document protected and directs publication
through Save As. External changes, recovery conflicts, missing mixed-line-ending consent, stale
revisions, or a failed pre-restore checkpoint block the operation without partially changing the
buffer.

## Retention and paths

Local History retains at most:

- 20 checkpoints per current document path;
- 16 MiB of source per checkpoint;
- 100 MiB of checkpoint source per workspace.

When a successful capture exceeds a retention limit, the oldest valid checkpoints are pruned first.
An in-app file or folder rename/move retargets the current document path while preserving the path
recorded at capture time. Save As and Duplicate do not retarget the old lineage, so a new path starts
without the active document's prior checkpoints.

Checkpoint identity is currently workspace-path based. Reusing a path that previously had retained
history can therefore reveal that path's older checkpoints; TermDraft does not silently delete them.

## Privacy and clearing

Checkpoints are private plaintext JSON containing complete document source. They are not stored in
the workspace, session file, recovery journal, or Git repository unless the platform state
directory itself is placed there.

- macOS: `~/Library/Application Support/TermDraft/local-history`
- Linux without `XDG_STATE_HOME`: `~/.local/state/termdraft/local-history`
- XDG: `$XDG_STATE_HOME/termdraft/local-history`

The pre-1.0 `TermWriter`/`termwriter` location is reused only when it already exists and the
canonical TermDraft location does not.

Disabling Local History stops captures but keeps existing checkpoints. Clearing document or
workspace history opens a separate confirmation whose default is Cancel. Deletion applies only to
the exact valid checkpoint IDs shown when confirmation opened; a concurrent change cancels the
operation. Corrupt, unreadable, or untrusted entries are preserved and surfaced as warnings rather
than hidden, followed, or swept by a broader clear.

## Undo, Recovery, and Local History

- **Undo** is the in-memory edit sequence for an open document.
- **Recovery** journals a dirty working source so it can survive an interrupted session.
- **Local History** keeps explicitly bounded checkpoints across sessions after workspace opt-in.

Local History does not replace ordinary files, atomic Save, external-conflict handling, Recovery, or
version control. It adds a device-local way to inspect and restore recent source states.
