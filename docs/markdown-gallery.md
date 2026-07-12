# TermWriter Markdown gallery

Open this file in TermWriter to compare its source and rendered preview side by side:

```bash
termwriter docs/markdown-gallery.md
```

The Markdown source is always the document. The preview only presents it; it never rewrites this
file.

## Headings

### Third-level heading

#### Fourth-level heading

##### Fifth-level heading

###### Sixth-level heading

## Inline text

Plain text can contain *emphasis*, **strong emphasis**, ~~strikethrough~~, `inline code`, and an
[ordinary link](https://example.com). An image remains a safe terminal placeholder:

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

Only `NOTE`, `TIP`, `IMPORTANT`, `WARNING`, and `CAUTION` receive an alert title. Unknown markers
remain ordinary quoted text:

> [!DANGER]
> This is a normal blockquote, not a sixth alert type.

## Table

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

A statement can refer to a named footnote.[^source]

[^source]: Footnote definitions render at the end with a visible number.

    They may contain additional paragraphs and nested Markdown.

An inline footnote is supported too.^[This definition lives beside its reference in the source.]

## Definition list

TermWriter
: A local-first Markdown editor for the terminal.

Source of truth
: The exact Markdown text on disk and in the active document model.

## Horizontal rule

---

Text after the rule.

## Safe raw HTML fallback

Raw HTML is not interpreted by TermWriter. For example, this remains literal preview text:

<kbd>Ctrl+S</kbd>

Math, underline, subscript, and superscript extensions are not rendered in the current preview.
