# TermDraft Markdown gallery

Open this file in TermDraft. With the default Inline configuration, `v` switches between the
source editor and rendered preview. With `editor.view_mode = "split"` in a wide terminal, `v`
shows or hides the preview beside the source editor:

```bash
cargo run --release --locked -- docs/markdown-gallery.md
```

If `termdraft` is installed, use `termdraft docs/markdown-gallery.md` instead. The Markdown
source is always the document. Inline styles and the split preview only present it; neither rewrites
this file.

The current Rust inline view renders headings, emphasis, strong text, strikeout, inline code, links,
indented lists, quotes, labeled code fences, rules, and aligned table borders outside the active
source line. The split view uses a semantic Markdown parser. Footnotes navigate inside the preview;
ordinary URLs remain selectable but inert, image data is omitted, and raw HTML never executes.

| Construct | Inline view | Split preview |
| --- | --- | --- |
| Headings and common inline markers | Compact rendered terminal text | Rendered terminal text |
| Lists, tasks, and quotes | Rendered with nested indentation | Rendered with nested indentation |
| Fenced code | `BASH`, `CODE`, or `CODE · LANGUAGE` rail | Labeled rail with a dark surface |
| Tables | Aligned borders outside the active source line | Aligned bordered table |
| GFM alerts, footnotes, definitions | Source-faithful fallback where unsupported | Titled alerts and internal footnote navigation |
| Raw HTML | Exact Markdown source | Omitted rather than executed |

## Headings

Press `S` in COMMAND mode to open the current heading outline. Type to filter by heading text, use
the arrow keys to choose a match, then press `Enter` to jump to its source or `Ctrl+Enter` to reveal
it in the preview.

### Third-level heading

#### Fourth-level heading

##### Fifth-level heading

###### Sixth-level heading

## Inline text

Plain text can contain *emphasis*, **strong emphasis**, ~~strikethrough~~, `inline code`, and an
[ordinary link](https://example.com). In the focused preview, `Tab` and `Shift+Tab` select links and
`Enter` follows footnotes without opening external URLs. Inline and the source editor retain image
syntax, while the preview can present only terminal text:

![Mountain silhouette](mountain.png)

Backslash escapes keep \*asterisks\* literal. A trailing backslash creates a hard\
line break.

## Lists and tasks

- First bullet
- Second bullet
  - Nested bullet
  - [ ] Pending task
  - [x] Completed task

1. First ordered item
2. Second ordered item
   1. Nested ordered item
   2. Another nested item

## Quotes and GFM alerts

> A normal blockquote can contain **formatted text**.

> [!NOTE]
> Useful context that deserves attention.

> [!TIP]
> A practical suggestion.

> [!IMPORTANT]
> Information required to complete the task.

> [!WARNING]
> A condition that could cause a problem.

> [!CAUTION]
> A possible negative outcome.

The split preview turns the five supported alert markers into titled callouts. An unknown marker
remains an ordinary blockquote:

> [!DANGER]
> This is a normal blockquote, not a sixth alert type.

## Table

This remains a useful source-fidelity fixture. The Rust inline view renders borders outside the
active source line, and the split renderer reconstructs a bordered table without changing source.

| Syntax | Preview behavior |
| --- | --- |
| `**text**` | Strong emphasis |
| `- [x] task` | Checked task symbol |
| `` `code` `` | Inline code |

## Code

Shell fences use a `BASH` label. Other language fences use `CODE · LANGUAGE`, while an unlabeled
fence or four-space block uses `CODE`. The label, rail, and dark surface distinguish code from
prose without adding prompts to the source.

```python
def greeting(name: str) -> str:
    return f"Hello, {name}!"
```

    Four leading spaces also make a code block.
    Markdown markers such as **this** stay literal here.

## Footnotes

A statement can refer to a named footnote.[^source] Select the rendered reference with `Tab` and
press `Enter` to reveal its definition; activate the definition label to return to the reference.

[^source]: A named footnote definition remains part of the exact source.

    They may contain additional paragraphs and nested Markdown.

An inline-footnote-shaped fixture remains source.^[This definition lives beside its reference.]

## Definition list

Definition-list rendering is not ported. These lines verify that unsupported syntax remains intact:

TermDraft Rust port
: A local-first Markdown editor for the terminal using Ratatui.

: This definition remains ordinary source in the current Rust editor.

Source of truth
: The exact Markdown text on disk and in the active document model.

## Horizontal rule

---

Text after the rule.

## Safe raw HTML fallback

Raw HTML is never executed by the Rust frontend. Inline/source editing retains the literal text,
while the split renderer omits the tag rather than executing it:

<kbd>Ctrl+S</kbd>

Math and underline extensions are not enabled. The split renderer supports its own subscript and
superscript presentation, while the source editor keeps their exact Markdown syntax.
