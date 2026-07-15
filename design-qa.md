# Command palette design QA

## Reference and render

- Reference: the approved compact 2x2 command-cheatsheet mockup.
- Render: `TermDraftApp.run_test(size=(100, 30))` with the palette open on `README.md`.
- Compared at full-window and palette-focused scales.

## Findings

| Severity | Finding | Resolution |
| --- | --- | --- |
| P2 | The original search field used three rows, placing its icon below the label. | Reduced the field to one row and aligned the icon, cursor, and placeholder. |
| P2 | The original command list repeated full help text and separators, making scanning slow. | Replaced it with four labeled groups, one-line key/action rows, one selection, and one shared description. |
| P2 | A fixed 2x2 layout would clip on narrow terminals. | Added a single-column stacked layout below 58 columns while keeping the results scrollable. |
| P3 | The reference omits Find file and Shortcut help. | Kept both because TermDraft's palette contract requires every application command to remain discoverable. |

## Final check

- No P0, P1, or unresolved P2 differences remain.
- Search, arrow navigation, remapped keys, command execution, selection feedback, and narrow layout are covered by functional tests.
- Final result: passed.
