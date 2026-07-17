//! User-facing Markdown reference shared by the Rust frontend.

/// Supported Markdown syntax and the current preview interaction contract.
pub const MARKDOWN_SYNTAX_HELP: &str = r#"Headings           # H1 through ###### H6
Emphasis           *italic*, **bold**, ~~strikethrough~~
Bullets            - item   (also * or +)
Numbered lists     1. item
Nested lists       Indent the nested marker by at least three spaces
Tasks              - [ ] pending   - [x] done
Quotes             > quoted text
Alerts             > [!NOTE] then > body (NOTE/TIP/IMPORTANT/WARNING/CAUTION)
Links              [label](https://example.com)
Images             ![alt](path) (alt text only; image data is omitted)
Code               `inline` or fenced ``` blocks
Tables             | A | B | with a | --- | --- | separator row
Footnote ref       Text[^note]
Footnote body      [^note]: source (on its own later line)
Definitions        Term followed on the next line by : Definition
Subscript          H~2~O
Superscript        x^2^
Rules              ---

Enter continues bullets, numbered lists, tasks, and blockquotes. Press Enter on
an empty marker to end the list. Press Esc for COMMAND mode and i to return to WRITE mode. In the
focused preview, links and footnotes remain visible but inert; Tab, Shift+Tab, and Enter do not
activate them.
Alt+Down and Alt+Up move between rendered headings and show the current heading position.
External URLs remain inert. Raw HTML is ignored and omitted from the preview.
Subscript and superscript render as dim italic text.

Not rendered yet: image data, raw HTML, math, and underline.
Markdown has no portable __underline__ syntax; double underscores mean bold.
A nested ordered item is "   1. item", not "1.1.".
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_supported_syntax_family() {
        for section in [
            "Headings",
            "Emphasis",
            "Bullets",
            "Numbered lists",
            "Nested lists",
            "Tasks",
            "Quotes",
            "Alerts",
            "Links",
            "Images",
            "Code",
            "Tables",
            "Footnote ref",
            "Footnote body",
            "Definitions",
            "Subscript",
            "Superscript",
            "Rules",
        ] {
            assert!(
                MARKDOWN_SYNTAX_HELP
                    .lines()
                    .any(|line| line.starts_with(section)),
                "missing Markdown help section {section}"
            );
        }
    }

    #[test]
    fn preserves_interaction_and_limitation_guidance() {
        for guidance in [
            "Enter continues bullets",
            "Press Esc for COMMAND mode",
            "links and footnotes remain visible but inert",
            "Tab, Shift+Tab, and Enter do not\nactivate them",
            "Alt+Down and Alt+Up move between rendered headings",
            "External URLs remain inert",
            "Images             ![alt](path) (alt text only; image data is omitted)",
            "Raw HTML is ignored and omitted from the preview.",
            "Subscript and superscript render as dim italic text.",
            "Not rendered yet: image data, raw HTML, math, and underline.",
            "double underscores mean bold",
            "not \"1.1.\"",
        ] {
            assert!(
                MARKDOWN_SYNTAX_HELP.contains(guidance),
                "missing Markdown guidance {guidance}"
            );
        }
        assert!(!MARKDOWN_SYNTAX_HELP.contains("terminal placeholder"));
        assert!(!MARKDOWN_SYNTAX_HELP.contains("Raw HTML is displayed as text"));
        assert!(!MARKDOWN_SYNTAX_HELP.contains('\r'));
        assert!(MARKDOWN_SYNTAX_HELP.ends_with('\n'));
    }
}
