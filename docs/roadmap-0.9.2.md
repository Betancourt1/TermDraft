# TermDraft 0.9.2 roadmap

## Outcome and status

TermDraft 0.9.2 makes explicit workspace Agent context safe enough to prepare as a local release
candidate. The repository contains the implementation and local verification, but 0.9.2 is not a
published release until its exact commit passes hosted CI, a new tag produces verified native
artifacts, the GitHub release is published, and Homebrew is updated and tested. None of those
publication steps are part of this roadmap-preparation change.

The release preserves the local-first editing contract:

- Agent sharing remains off until the user enables it for the current workspace.
- Workspace context is read-only and limited to supported text documents inside that workspace.
- A proposal targets the exact active document path and source revision returned by `read`.
- Every valid proposal still requires visible human review and cannot save a file directly.
- Accept remains one Undo group, publishes Recovery first, participates in opt-in Local History,
  marks the buffer dirty, and leaves disk unchanged until Save.

## Deliberate change from the 0.9 gate

The original 0.9 acceptance gate authorized one active document and explicitly excluded inactive or
workspace-wide access. Commit `29addae` later expanded that consent boundary: enabling Agent sharing
now exposes read-only context for the supported documents in the current workspace. Version 0.9.2
adopts that expansion deliberately instead of rewriting the earlier gate.

The expansion is one-way. Agents can read workspace context, but proposals remain limited to the
active document and require both its workspace-relative path and its exact revision. Inactive
document proposals, direct writes, automatic saves, cloud transport, and background sharing remain
out of scope.

## Research and decisions

| Area | Evidence | 0.9.2 decision | State |
| --- | --- | --- | --- |
| Session discovery | `d290921` removed normal socket/token copy-paste and refuses ambiguous live sessions. | Keep the 0.9.1 discovery foundation unchanged. | Adopted baseline |
| Sandbox diagnostics | `a6ecbd0` distinguishes a denied private socket from an absent session. | Include the accurate recovery instruction; do not weaken socket permissions. | Complete |
| Workspace context | `29addae` added sorted read-only workspace responses with live open-buffer precedence. | Adopt the expanded scope explicitly and document it in the release module. | Complete |
| Proposal identity | Revision-only requests could be retargeted after a tab switch when two documents had identical source. | Require the workspace-relative path and revision; reject before review if either target changed. | Complete in `6e23b53` |
| Snapshot responsiveness | Recursive discovery and unopened-file reads ran in the editor event loop. | Snapshot bounded open buffers at request time, then collect unopened files on one cancellable worker. | Complete in `619d263` |
| Snapshot memory | The 64 MiB bridge cap was applied only after the complete response had been accumulated and encoded. | Preflight 16 MiB per document and 64 MiB aggregate limits, count encoded JSON while collecting, and fail the whole request without partial context. | Complete in `619d263` |
| Missing and replaced paths | A missing open buffer was absent from the disk scan; an open path replaced by a symlink must not re-enter through the live-buffer override. | Include genuinely missing open buffers; exclude symlink, non-file, unreadable-metadata, ignored, unsupported, outside, and outside-resolving paths. | Complete in `619d263` |
| Dependency tree | Locked direct dependencies are coherent; reported duplicates are transitive. `ignore` is locked at 0.4.29 while its current patch documentation is 0.4.31. | Avoid unrelated dependency churn in this safety release; schedule a separate patch review. | Deferred |
| Plane history | The earlier 0.9 module describes active-document-only scope. | Keep the dedicated 0.9.2 module and reconcile it with the final release commit and publication evidence. | Final reconciliation pending |
| Local configuration | The existing `~/.termdraft/config.toml` intentionally enables `appearance.transparent_background`. | Restore the complete transparent-surface implementation from `6ebfade` without rewriting the user configuration. | Complete in the final candidate |
| Terminal lifecycle | A closed terminal could surface I/O errors and stale refused Agent sessions remained discoverable. | Exit cleanly on disconnect and prune refused stale session directories. | Complete in `d539ee1` |
| Editor viewport | Rebuilding Inline presentation after edits reset the derived viewport, while multiline paste moved the caret to the bottom boundary. | Preserve the caret's prior screen row for every edit and use the nearest boundary only when no visible row is available. | Complete in `e8db662` plus the final cursor-row correction |

The implementation follows the existing standard-library worker/channel model. Rust's
[`recv_timeout`](https://doc.rust-lang.org/stable/std/sync/mpsc/struct.Receiver.html) supports a
short interruptible wait without adding an async runtime. Serde JSON's
[`to_writer`](https://docs.rs/serde_json/latest/serde_json/fn.to_writer.html) supports exact byte
counting and direct socket serialization without retaining a second encoded response. The existing
[`ignore::WalkBuilder`](https://docs.rs/ignore/latest/ignore/struct.WalkBuilder.html) traversal keeps
the workspace exclusions authoritative while cancellation is checked between entries.

## Delivery sequence

### 1. Safety and performance implementation — complete locally

- Preserve credential-free same-user discovery and accurate permission-denied diagnostics.
- Bind proposals to both the document path and revision.
- Move recursive workspace collection off the editor loop.
- Bound open snapshots, disk reads, encoded aggregate size, and revocation latency.
- Cover identical-document tab switches, missing versus symlink-replaced open paths, per-document
  and aggregate caps, bounded file reads, and pending-request revocation.

### 2. Final release candidate — current stage

- Set Cargo package metadata to 0.9.2 and date the changelog section.
- Run formatting and strict Clippy.
- Run complete Rust debug and optimized suites plus the Python compatibility suite.
- Build both optimized binaries and verify version/help/commands/inspection surfaces.
- Launch the optimized editor in a real PTY against a disposable Markdown workspace and exit
  cleanly without changing the fixture.
- Reconcile the commit-backed module, issues, and this roadmap in Plane.

The final candidate source tree passed the following checks on 2026-08-23. Its resulting commit SHA
must be recorded in the hosted CI, release, tap, and Plane evidence before publication:

- strict formatting and all-target/all-feature Clippy pass at package version 0.9.2;
- debug and optimized Rust suites each pass 281 library plus 7 binary tests;
- the Python compatibility oracle passes 681 tests with the expected 2 platform skips;
- optimized `termdraft` and `termdraft-agent` both report 0.9.2, help/commands complete, and
  repository inspection reports 23 supported documents;
- an isolated-state real-PTY launch used the normal user configuration, rendered a disposable
  Markdown fixture, exited cleanly, and preserved its SHA-256
  `b2c20430742d4b49fc7e4804c297a79aae62b06a2c8b0aa0c05c312587467c69`;
- all disposable PTY, configuration, and state directories were removed after verification.

These checks establish local readiness for the transparency, terminal-lifecycle, and
viewport-preservation lineage. They do not establish hosted CI, artifact, or publication state.

### 3. Publication — authorized, pending final audit and verification

- Push the exact audited candidate and wait for hosted macOS and Linux CI on that commit.
- Create a new `v0.9.2` tag only after hosted checks pass.
- Verify all four native archives, `SHA256SUMS`, both binaries, and a disposable real-terminal
  journey from the downloaded artifact.
- Publish the verified GitHub draft, update the Homebrew formula to its exact archive digest, run
  the tap audit/build/test sequence, and verify a fresh install.

Local success, a Plane Done state, or a prepared changelog does not imply that any publication step
has happened.

## Deferred technical debt

These items are deliberately not folded into 0.9.2:

- streaming, pagination, or revision-delta workspace responses;
- proposals for inactive documents or more than one pending proposal;
- a general worker pool or conversion of ordinary reads, saves, Recovery, Local History, and file
  mutations to background I/O;
- parent-descriptor-bound save publication, stable-read retries, and post-publication digest
  verification;
- Windows state paths, recovery locks, and a Windows Agent transport;
- patch dependency updates without a separate compatibility review;
- automatic rewriting of unknown or locally added configuration sections;
- network, cloud, model-hosting, collaborative/CRDT, or automatic-write features.

Each deferred item changes a separate product or compatibility boundary and should receive its own
acceptance contract, tests, independent audit, commit, and Plane issue before implementation.

## Release acceptance

The 0.9.2 candidate is ready for publication review only when:

1. the repository is clean at the audited candidate commit;
2. version output from `termdraft` and `termdraft-agent` is exactly 0.9.2;
3. formatting, strict Clippy, complete debug/release Rust tests, and Python compatibility tests pass;
4. the real-PTY smoke journey exits cleanly with the disposable fixture unchanged;
5. Plane contains the 0.9.2 module, every recent commit-backed change, deferred publication work,
   and a comprehensive roadmap in the module description whose state matches the repository;
6. no push, tag, GitHub release, Homebrew update, or installed-app claim is made without observed
   evidence from that separate workflow.
