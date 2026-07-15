# Terminal dialog design QA

## Reference and render

- Reference: the selected Yazi-like mockup with a border title, footer divider, plain actions,
  and inverse focus.
- Render: `TermDraftApp.run_test(size=(153, 50))` with the real Trash confirmation open.
- Evidence: `termdraft-dialog-option1.png` in the task visualization directory.

## Findings

| Severity | Finding | Resolution |
| --- | --- | --- |
| P1 | Textual variants rendered actions as conventional filled buttons. | Flattened every dialog action at rest and reserved the inverse block for focus or press. |
| P1 | Dialog titles occupied a separate content row instead of reading as part of the frame. | Added a shared `TerminalDialog` that places centered titles directly in the border. |
| P2 | Actions lacked the reference's clear separation from message content. | Added one thin footer divider to every action group. |
| P2 | Primary, warning, and error variants competed with keyboard focus. | Neutralized variant chrome while preserving labels, handlers, and disabled states. |
| P2 | Compact multi-row grids could collapse button content at very narrow widths. | Removed grid-only inner padding and retained the existing responsive scroll behavior. |
| P3 | The reference uses generic `yes` and `no` labels. | Kept precise actions such as `Move to Trash` and `Reload external` to preserve decision clarity. |

## Final check

- The rendered Trash dialog matches the reference's square frame, centered border title,
  monochrome footer divider, flat resting actions, and compact inverse focus.
- Functional tests cover click behavior, busy and disabled states, action focus, and the
  24-column recovery-manager layout.
- No P0, P1, or unresolved P2 differences remain.
- Final result: passed.
