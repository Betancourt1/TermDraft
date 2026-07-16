//! Small deterministic search primitives used by the TUI.

use std::path::{Path, PathBuf};

use crate::workspace::WorkspaceEntry;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

#[must_use]
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    let mut score = 0_i64;
    let mut cursor = 0;
    let mut previous_match = None;

    for wanted in query.chars() {
        let tail = candidate.get(cursor..)?;
        let offset = tail.find(wanted)?;
        let matched = cursor + offset;
        score += 100 - i64::try_from(offset).unwrap_or(100).min(100);
        if previous_match.is_some_and(|previous| previous + wanted.len_utf8() == matched) {
            score += 35;
        }
        if matched == 0
            || candidate[..matched]
                .chars()
                .next_back()
                .is_some_and(|character| matches!(character, '/' | '_' | '-' | ' '))
        {
            score += 25;
        }
        previous_match = Some(matched);
        cursor = matched + wanted.len_utf8();
    }
    Some(score - i64::try_from(candidate.len()).unwrap_or(i64::MAX))
}

#[must_use]
pub fn search_files<'a>(query: &str, entries: &'a [WorkspaceEntry]) -> Vec<&'a WorkspaceEntry> {
    let mut scored = entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .filter_map(|entry| {
            let candidate = entry.relative.to_string_lossy();
            fuzzy_score(query, &candidate).map(|score| (score, entry))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.relative.cmp(&right.relative))
    });
    scored.into_iter().map(|(_, entry)| entry).collect()
}

#[must_use]
pub fn search_text(path: &Path, text: &str, query: &str, limit: usize) -> Vec<TextMatch> {
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        for byte in line
            .char_indices()
            .map(|(byte, _)| byte)
            .chain(std::iter::once(line.len()))
        {
            if line[byte..].to_lowercase().starts_with(&query_lower) {
                let column = line[..byte].chars().count();
                matches.push(TextMatch {
                    path: path.to_path_buf(),
                    line: line_index,
                    column,
                    preview: line.trim().to_owned(),
                });
                if matches.len() == limit {
                    return matches;
                }
            }
        }
    }
    matches
}

#[must_use]
pub fn heading_outline(text: &str) -> Vec<(usize, usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(line, source)| {
            let hashes = source
                .chars()
                .take_while(|character| *character == '#')
                .count();
            if (1..=6).contains(&hashes) && source.as_bytes().get(hashes) == Some(&b' ') {
                Some((line, hashes, source[hashes + 1..].trim().to_owned()))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matches_path_segments() {
        assert!(fuzzy_score("rdm", "docs/README.md").is_some());
        assert!(fuzzy_score("xyz", "docs/README.md").is_none());
    }

    #[test]
    fn outlines_atx_headings_only() {
        assert_eq!(
            heading_outline("# One\ntext\n### Three\n####### no"),
            vec![(0, 1, "One".to_owned()), (2, 3, "Three".to_owned())]
        );
    }

    #[test]
    fn text_search_reports_character_columns() {
        let matches = search_text(Path::new("note.md"), "Café needle", "needle", 10);
        assert_eq!(matches[0].column, 5);
    }
}
