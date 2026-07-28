# 0.7 responsiveness gates

The 0.7 gates cover the two writing workloads most likely to expose main-loop stalls: a large
semantic split preview and a workspace with many resident editor tabs. They supplement the normal
Rust suite; they do not replace the real terminal journey below.

## Automated workloads

Run both named gates in the optimized profile:

```bash
cargo test --locked --release responsiveness_gate -- --nocapture
```

`responsiveness_gate_long_document_preview_stays_bounded` generates 10,000 Markdown sections:
756,670 source bytes, 39,999 rendered lines, and 10,000 links. The optimized render must complete in
less than 500 ms. The debug allowance is 4 seconds so ordinary development tests remain stable.

`responsiveness_gate_many_tabs_keeps_switching_bounded_without_eviction` opens 24 independent
documents with 256 paragraphs each and switches tabs 240 times. Optimized opening must remain below
1.5 seconds and switching below 250 ms; debug allowances are 6 seconds and 1 second. The fixture
also counts the source strings retained by the document, raw editor, and inline editor. That tracked
residency includes each tab's semantic preview cache and may not exceed six times the fixture's raw
source bytes.

The tracked count is intentionally narrower than process RSS: it excludes allocator, framework,
terminal, and Rust runtime overhead. Its purpose is to catch accidental extra full-source copies or
semantic caches before considering inactive-tab eviction.

## Current local evidence

On 2026-07-25, the debug and optimized fixtures on the development Mac measured:

| Gate | Debug | Optimized |
| --- | ---: | ---: |
| 756,670-byte semantic preview | 98.7 ms | 9.6 ms |
| Open 24 tabs and populate their preview caches | 144.1 ms | 15.1 ms |
| Switch tabs 240 times | 10.4 ms | 0.8 ms |
| Tracked source-and-preview residency | 1,587,720 bytes | 1,587,720 bytes |

At this scale the measured editor-source and semantic-cache residency does not justify inactive-tab
eviction. Revisit that decision only if a larger representative fixture or real RSS evidence fails
a documented gate.

The same development checkpoint also passed a 140×40 real-PTY journey with the optimized
executable. A 35,649-byte, 444-line document remained editable in visible Split mode; unique edits,
undo, and redo reached the semantic preview. Twelve tabs were opened through File Finder from a
24-file disposable workspace, repeated `]`/`[` switching remained immediate, and guarded quit
requested a separate decision for both dirty tabs. Both drafts were discarded through the visible
prompt, terminal state was restored, no repository source edit was saved, and the disposable state
and workspace were removed. A focused post-audit rerun at the same terminal size confirmed that a
completed preview remains visible throughout a new edit, the newest revision replaces it, discard
leaves the fixture unchanged, and the process restores the terminal normally.

## Real-PTY journey

Build the optimized executable and prepare disposable Markdown files outside the repository:

```bash
cargo build --locked --release
fixture_root="$(mktemp -d /tmp/termdraft-0.7.XXXXXX)"
mkdir -p "$fixture_root/config"
printf '[editor]\nview_mode = "split"\n' > "$fixture_root/config/config.toml"
awk 'BEGIN {
  for (i = 1; i <= 10000; i++) {
    printf "## Section %d\n\nParagraph **%d** with [link](https://example.com/%d).\n\n", i, i, i
  }
}' > "$fixture_root/long.md"
for index in $(seq -w 1 24); do
  cp "$fixture_root/long.md" "$fixture_root/note-$index.md"
done
XDG_STATE_HOME="$fixture_root/state" \
  target/release/termdraft --config-dir "$fixture_root/config" "$fixture_root/long.md"
```

Verify through ordinary TermDraft input in a terminal at least 120×30:

1. Use `v` to show the semantic preview and confirm the final section is reachable.
2. Enter WRITE mode with `i`, add a unique line, and confirm split preview shows that newest line
   after the short debounce without freezing cursor movement.
3. Return to COMMAND mode with `Esc`. Use `f`, type each `note-NN.md`, and press Enter to open at
   least 12 additional tabs through the normal file finder.
4. Use `]` and `[` repeatedly. The active tab, source, and preview must agree and tab switching must
   remain immediate.
5. Edit two different tabs, then use `u` and `U`; source, dirty state, and preview must follow the
   newest revision.
6. Quit with `q`, choose the visible save/discard decisions for each dirty document, and confirm the
   process exits normally with terminal state restored.

Record terminal size, commit, optimized gate output, open-tab count, any visible stall, and whether
the final edit appeared in preview. A synthetic benchmark, DOM-like inspection, or unit test alone
does not close this journey.
