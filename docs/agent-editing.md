# Agent editing preview

TermDraft 0.9 adds an optional local bridge for tools that need the exact active draft instead of a
possibly older file on disk. Sharing is off by default, scoped to one document, and never grants a
tool permission to save.

## Start and stop sharing

Open the command palette and choose **Agent sharing**. TermDraft displays:

- the shared workspace-relative document;
- a private Unix socket path;
- a random session token;
- the read and proposal command shapes.

The tab and status bar show `↔` and `AGENT SHARED` while the active document is shared. Treat the
socket and token together as a temporary local secret. Press `r` in the sharing panel to revoke it.
TermDraft also revokes it when the shared document is closed, retargeted by Save As, renamed or
moved inside TermDraft, or when the application exits.

Only one document can be shared at a time. If another tab becomes active, the session retains its
original identity but read and proposal requests are refused until that document is active again.

## Read the live draft

Use the values shown in the panel:

```bash
termdraft-agent read --socket SOCKET_PATH --token SESSION_TOKEN
```

The JSON response contains `path`, `source`, `revision`, and `dirty`. `source` is the authoritative
editor buffer, including unsaved changes. `revision` combines a monotonic generation with a SHA-256
source digest and must be returned with a proposal.

## Submit a complete-source proposal

Put the proposed complete UTF-8 source in a file, then run:

```bash
termdraft-agent propose \
  --socket SOCKET_PATH \
  --token SESSION_TOKEN \
  --revision REVISION_FROM_READ \
  --origin my-agent \
  proposed.md
```

Use `-` or omit the final path to read the proposed source from standard input. A valid response
contains a `proposal_id` and reports `pending_review`; it does not mean the source was applied.

For a bounded set of replacements, submit a JSON array instead:

```json
[
  { "start": 0, "end": 7, "replacement": "Updated" }
]
```

```bash
termdraft-agent propose-ranges \
  --socket SOCKET_PATH \
  --token SESSION_TOKEN \
  --revision REVISION_FROM_READ \
  edits.json
```

Range offsets are UTF-8 byte offsets into the exact returned `source`. Ranges must be in bounds,
must begin and end on character boundaries, and must not overlap. TermDraft sorts valid ranges
before applying them to the proposed copy.

## Review and safety behavior

TermDraft opens a source diff showing the proposal origin, document, revision generation, and
whether acceptance is still safe. Use:

- `a` to accept the proposal into the buffer;
- `r` to reject it and leave the buffer unchanged;
- `Esc` to defer review without accepting;
- arrow, page, Home, or End keys to scroll the diff.

Acceptance rechecks the active path, edit protection, and complete source revision. If the user
typed, undid, reloaded, changed tabs, or entered a conflict after the tool read the source, a stale or
protected proposal is blocked. Acceptance is also blocked without changing the buffer if the
complete Recovery journal cannot be written, including when its serialized form exceeds 16 MiB.
One proposal can await review at a time.

An accepted proposal:

- is one Undo/Redo group;
- marks the document modified;
- publishes the current crash-recovery journal immediately;
- captures before/after checkpoints when Local History is enabled for the workspace;
- leaves the saved baseline and file on disk unchanged until the user explicitly saves.

The bridge is available on macOS and Linux through Unix domain sockets. It does not open TCP, run a
model, contact a cloud service, expose inactive workspace files, stream edits, or write files
directly.
