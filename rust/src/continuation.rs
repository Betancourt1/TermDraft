//! Markdown-aware Enter behavior shared by the terminal editor.

use std::sync::OnceLock;

use regex::{Captures, Regex};

#[derive(Debug, Eq, PartialEq)]
pub enum EnterAction {
    Plain,
    Continue(String),
    EndMarker(usize),
}

/// Decide how Enter should behave from the text before the cursor.
#[must_use]
pub fn action_for(line_prefix: &str) -> EnterAction {
    if let Some(captures) = task().captures(line_prefix) {
        let quote = captures.get(1).map_or("", |value| value.as_str());
        let bullet = captures.get(2).map_or("-", |value| value.as_str());
        return from_captures(line_prefix, &captures, 3, format!("{quote}{bullet} [ ] "));
    }
    if let Some(captures) = ordered().captures(line_prefix) {
        let quote = captures.get(1).map_or("", |value| value.as_str());
        let number = captures
            .get(2)
            .and_then(|value| value.as_str().parse::<u64>().ok())
            .unwrap_or(0)
            .saturating_add(1);
        let delimiter = captures.get(3).map_or(".", |value| value.as_str());
        return from_captures(
            line_prefix,
            &captures,
            4,
            format!("{quote}{number}{delimiter} "),
        );
    }
    if let Some(captures) = bullet().captures(line_prefix) {
        let quote = captures.get(1).map_or("", |value| value.as_str());
        let marker = captures.get(2).map_or("-", |value| value.as_str());
        return from_captures(line_prefix, &captures, 3, format!("{quote}{marker} "));
    }
    if let Some(captures) = quote().captures(line_prefix) {
        let prefix = captures.get(1).map_or("> ", |value| value.as_str());
        return from_captures(line_prefix, &captures, 2, prefix.to_owned());
    }
    EnterAction::Plain
}

fn from_captures(
    line: &str,
    captures: &Captures<'_>,
    content_index: usize,
    next_prefix: String,
) -> EnterAction {
    let Some(content) = captures.get(content_index) else {
        return EnterAction::Plain;
    };
    if content.as_str().trim().is_empty() {
        EnterAction::EndMarker(line[..content.start()].chars().count())
    } else {
        EnterAction::Continue(next_prefix)
    }
}

fn task() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(\s*(?:>\s*)*)([-+*])\s+\[[ xX]\]\s+(.*)$")
            .expect("valid task continuation regex")
    })
}

fn ordered() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(\s*(?:>\s*)*)(\d+)([.)])\s+(.*)$")
            .expect("valid ordered-list continuation regex")
    })
}

fn bullet() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(\s*(?:>\s*)*)([-+*])\s+(.*)$").expect("valid bullet continuation regex")
    })
}

fn quote() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(\s*(?:>\s*)+)(.*)$").expect("valid blockquote continuation regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_common_markdown_blocks() {
        assert_eq!(action_for("- item"), EnterAction::Continue("- ".into()));
        assert_eq!(
            action_for("  8) item"),
            EnterAction::Continue("  9) ".into())
        );
        assert_eq!(
            action_for("> - [x] done"),
            EnterAction::Continue("> - [ ] ".into())
        );
        assert_eq!(action_for("> quote"), EnterAction::Continue("> ".into()));
    }

    #[test]
    fn an_empty_marker_ends_the_block() {
        assert_eq!(action_for("  - "), EnterAction::EndMarker(4));
        assert_eq!(action_for("> "), EnterAction::EndMarker(2));
        assert_eq!(action_for("plain"), EnterAction::Plain);
    }
}
