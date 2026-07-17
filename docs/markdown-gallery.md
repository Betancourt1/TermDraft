# TermDraft Markdown gallery

Open this file in the Rust port. With the default Inline configuration, `v` switches between the
source editor and rendered preview. With `editor.view_mode = "split"` in a wide terminal, `v`
shows or hides the preview beside the source editor:

```bash
cargo run --release --locked -- docs/markdown-gallery.md
```

If `termdraft-rs` is installed, use `termdraft-rs docs/markdown-gallery.md` instead. The Markdown
source is always the document. Inline styles and the split preview only present it; neither rewrites
this file.

The current Rust inline view styles headings, emphasis, strong text, strikeout, inline code, links,
tasks, bullets, and table separators. The split view uses `tui-markdown` for a broader best-effort
preview. Links, images, footnotes, and raw HTML are not interactive.

| Construct | Inline view | Split preview |
| --- | --- | --- |
| Headings and common inline markers | Presentation-only styling | Rendered terminal text |
| Lists, tasks, quotes, and code | Exact source with selected marker styling | Best-effort rendering |
| Tables | Pipes are styled; source remains exact | Table extension is not enabled |
| GFM alerts, footnotes, definitions | Exact Markdown source | No dedicated interactive behavior |
| Raw HTML | Exact Markdown source | Omitted rather than executed |

## Headings

Press `S` in COMMAND mode to open the current heading outline. Choose a heading with the arrow keys
and `Enter`; the source cursor moves to that line.

### Third-level heading

#### Fourth-level heading

##### Fifth-level heading

###### Sixth-level heading

## Inline text

Plain text can contain *emphasis*, **strong emphasis**, ~~strikethrough~~, `inline code`, and an
[ordinary link](https://example.com). The Rust frontend does not open links or load images; Inline
and the source editor retain image syntax, while the preview can present only terminal text:

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

The Rust port does not add special alert controls or actions. These remain Markdown blockquote
source, and an unknown marker is treated no differently:

> [!DANGER]
> This is a normal blockquote, not a sixth alert type.

## Table

This remains a useful source-fidelity fixture. The Rust inline view styles the pipe separators, but
the split renderer is not configured with a table extension and does not reconstruct bordered rows.

| Syntax | Preview behavior |
| --- | --- |
| `**text**` | Strong emphasis |
| `- [x] task` | Checked task symbol |
| `` `code` `` | Inline code |

## Code

```python
def greeting(name: str) -> str:
    return f"Hello, {name}!"
```

    Four leading spaces also make a code block.
    Markdown markers such as **this** stay literal here.

## Footnotes

A statement can refer to a named footnote.[^source] This syntax remains a parsing fixture; footnote
navigation and backlinks are not ported.

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
