//! Markdown-aware Enter behavior shared by the terminal editor.

use std::sync::OnceLock;

use regex::{Captures, Regex};

#[derive(Debug, Eq, PartialEq)]
pub enum EnterAction {
    Plain,
    Continue(String),
    EndMarker(usize),
}

/// Decide how Enter should behave from a logical source position.
#[must_use]
pub fn action_for(lines: &[String], row: usize, cursor: usize) -> EnterAction {
    let Some(line) = lines.get(row) else {
        return EnterAction::Plain;
    };
    if cursor > line.chars().count()
        || line.starts_with('\t')
        || line.starts_with("    ")
        || thematic_break().is_match(line)
        || fence_line().is_match(line)
        || inside_fence(lines, row)
    {
        return EnterAction::Plain;
    }
    if let Some(captures) = task().captures(line) {
        let quote = captures.get(1).map_or("", |value| value.as_str());
        let bullet = captures.get(2).map_or("-", |value| value.as_str());
        return from_captures(line, cursor, &captures, 3, format!("{quote}{bullet} [ ] "));
    }
    if let Some(captures) = ordered().captures(line) {
        let quote = captures.get(1).map_or("", |value| value.as_str());
        let raw_number = captures.get(2).map_or("0", |value| value.as_str());
        let number = raw_number.parse::<u64>().unwrap_or(0).saturating_add(1);
        let number = if raw_number.starts_with('0') && raw_number.len() > 1 {
            format!("{number:0width$}", width = raw_number.len())
        } else {
            number.to_string()
        };
        let delimiter = captures.get(3).map_or(".", |value| value.as_str());
        return from_captures(
            line,
            cursor,
            &captures,
            4,
            format!("{quote}{number}{delimiter} "),
        );
    }
    if let Some(captures) = bullet().captures(line) {
        let quote = captures.get(1).map_or("", |value| value.as_str());
        let marker = captures.get(2).map_or("-", |value| value.as_str());
        return from_captures(line, cursor, &captures, 3, format!("{quote}{marker} "));
    }
    if let Some(captures) = quote().captures(line) {
        let prefix = captures.get(1).map_or("> ", |value| value.as_str());
        return from_captures(line, cursor, &captures, 2, prefix.to_owned());
    }
    EnterAction::Plain
}

fn from_captures(
    line: &str,
    cursor: usize,
    captures: &Captures<'_>,
    content_index: usize,
    next_prefix: String,
) -> EnterAction {
    let Some(content) = captures.get(content_index) else {
        return EnterAction::Plain;
    };
    let content_start = line[..content.start()].chars().count();
    if cursor < content_start {
        return EnterAction::Plain;
    }
    if content.as_str().trim().is_empty() {
        if cursor == line.chars().count() {
            EnterAction::EndMarker(cursor)
        } else {
            EnterAction::Plain
        }
    } else {
        EnterAction::Continue(next_prefix)
    }
}

fn task() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^(\s*(?:>\s*)*)([-+*])\s+\[[ xX]\](.*)$")
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

fn thematic_break() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^\s*(?:(?:\*\s*){3,}|(?:-\s*){3,}|(?:_\s*){3,})$")
            .expect("valid thematic-break regex")
    })
}

fn fence_line() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\s*(?:>\s*)*(?:`{3,}|~{3,})").expect("valid fence regex"))
}

fn inside_fence(lines: &[String], row: usize) -> bool {
    let mut open: Option<(char, usize)> = None;
    for line in lines.iter().take(row) {
        let candidate = line.trim_start().trim_start_matches(['>', ' ']);
        let Some(marker) = candidate.chars().next() else {
            continue;
        };
        if !matches!(marker, '`' | '~') {
            continue;
        }
        let length = candidate
            .chars()
            .take_while(|character| *character == marker)
            .count();
        if length < 3 {
            continue;
        }
        match open {
            None => open = Some((marker, length)),
            Some((open_marker, open_length))
                if marker == open_marker
                    && length >= open_length
                    && candidate[length..].trim().is_empty() =>
            {
                open = None;
            }
            Some(_) => {}
        }
    }
    open.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_common_markdown_blocks() {
        let action = |line: &str, cursor: usize| action_for(&[line.to_owned()], 0, cursor);
        assert_eq!(action("- item", 6), EnterAction::Continue("- ".into()));
        assert_eq!(
            action("  8) item", 9),
            EnterAction::Continue("  9) ".into())
        );
        assert_eq!(
            action("> - [x] done", 12),
            EnterAction::Continue("> - [ ] ".into())
        );
        assert_eq!(action("> quote", 7), EnterAction::Continue("> ".into()));
        assert_eq!(
            action("009. item", 9),
            EnterAction::Continue("010. ".into())
        );
    }

    #[test]
    fn an_empty_marker_ends_the_block() {
        let action = |line: &str, cursor: usize| action_for(&[line.to_owned()], 0, cursor);
        assert_eq!(action("  - ", 4), EnterAction::EndMarker(4));
        assert_eq!(action("> ", 2), EnterAction::EndMarker(2));
        assert_eq!(action("- [ ]", 5), EnterAction::EndMarker(5));
        assert_eq!(action("plain", 5), EnterAction::Plain);
    }

    #[test]
    fn ambiguous_markdown_and_mid_line_edits_stay_safe() {
        let action = |line: &str, cursor: usize| action_for(&[line.to_owned()], 0, cursor);
        assert_eq!(action("    - code", 10), EnterAction::Plain);
        assert_eq!(action("* * *", 5), EnterAction::Plain);
        assert_eq!(action("- remaining", 2), EnterAction::Continue("- ".into()));
        assert_eq!(
            action_for(&["```".into(), "- code".into()], 1, 6),
            EnterAction::Plain
        );
    }
}
