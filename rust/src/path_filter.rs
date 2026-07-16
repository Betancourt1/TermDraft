//! Workspace-relative include and exclude glob filters.

use std::collections::HashMap;
use std::path::Path;

use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathFilter {
    includes: Vec<String>,
    excludes: Vec<String>,
}

impl PathFilter {
    #[must_use]
    pub fn includes(&self) -> &[String] {
        &self.includes
    }

    #[must_use]
    pub fn excludes(&self) -> &[String] {
        &self.excludes
    }

    /// Return whether `path` is beneath `root` and passes this filter.
    #[must_use]
    pub fn matches(&self, path: &Path, root: &Path) -> bool {
        path.strip_prefix(root)
            .ok()
            .is_some_and(|relative| self.matches_relative(relative))
    }

    /// Match an already workspace-relative path.
    #[must_use]
    pub fn matches_relative(&self, relative: &Path) -> bool {
        let parts = path_parts(relative);
        if parts.is_empty() {
            return false;
        }
        let included = self.includes.is_empty()
            || self
                .includes
                .iter()
                .any(|pattern| matches_glob(&parts, pattern));
        included
            && !self
                .excludes
                .iter()
                .any(|pattern| matches_glob(&parts, pattern))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct PathFilterError(String);

/// Parse comma-separated globs, with `!` marking exclusions.
///
/// A single-component glob matches the basename at any depth. `**` matches
/// zero or more complete path components. Matching is case-insensitive and
/// canonically Unicode-normalized.
///
/// # Errors
///
/// Returns [`PathFilterError`] for empty terms, absolute paths, parent
/// components, and exclusions without a pattern.
pub fn parse_path_filter(expression: Option<&str>) -> Result<Option<PathFilter>, PathFilterError> {
    let Some(expression) = expression.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };

    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    for raw_term in expression.split(',') {
        let term = raw_term.trim();
        if term.is_empty() {
            return Err(PathFilterError("empty patterns are not allowed".to_owned()));
        }

        let (is_exclusion, pattern) = if let Some(pattern) = term.strip_prefix('!') {
            (true, pattern.trim())
        } else {
            (false, term)
        };
        if pattern.is_empty() {
            return Err(PathFilterError(
                "an exclusion requires a glob after '!'".to_owned(),
            ));
        }
        if pattern.starts_with('/') {
            return Err(PathFilterError(format!(
                "patterns must be workspace relative: {pattern}"
            )));
        }

        let parts = pattern
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.contains(&"..") {
            return Err(PathFilterError(format!(
                "parent path components are not allowed: {pattern}"
            )));
        }
        if parts.is_empty() {
            return Err(PathFilterError(
                "patterns must name a workspace path".to_owned(),
            ));
        }

        let normalized = parts.join("/");
        if is_exclusion {
            excludes.push(normalized);
        } else {
            includes.push(normalized);
        }
    }

    Ok(Some(PathFilter { includes, excludes }))
}

fn path_parts(path: &Path) -> Vec<String> {
    path.components()
        .map(|part| normalize(&part.as_os_str().to_string_lossy()))
        .collect()
}

fn matches_glob(path_parts: &[String], pattern: &str) -> bool {
    let pattern_parts = pattern.split('/').map(normalize).collect::<Vec<_>>();
    if pattern_parts.len() == 1 {
        return component_matches(
            path_parts.last().map_or("", String::as_str),
            &pattern_parts[0],
        );
    }
    match_path_parts(path_parts, &pattern_parts, 0, 0, &mut HashMap::new())
}

fn match_path_parts(
    path_parts: &[String],
    pattern_parts: &[String],
    path_index: usize,
    pattern_index: usize,
    memo: &mut HashMap<(usize, usize), bool>,
) -> bool {
    if let Some(result) = memo.get(&(path_index, pattern_index)) {
        return *result;
    }
    let result = if pattern_index == pattern_parts.len() {
        path_index == path_parts.len()
    } else if pattern_parts[pattern_index] == "**" {
        match_path_parts(
            path_parts,
            pattern_parts,
            path_index,
            pattern_index + 1,
            memo,
        ) || (path_index < path_parts.len()
            && match_path_parts(
                path_parts,
                pattern_parts,
                path_index + 1,
                pattern_index,
                memo,
            ))
    } else {
        path_index < path_parts.len()
            && component_matches(&path_parts[path_index], &pattern_parts[pattern_index])
            && match_path_parts(
                path_parts,
                pattern_parts,
                path_index + 1,
                pattern_index + 1,
                memo,
            )
    };
    memo.insert((path_index, pattern_index), result);
    result
}

fn component_matches(value: &str, pattern: &str) -> bool {
    let value = value.chars().collect::<Vec<_>>();
    let pattern = pattern.chars().collect::<Vec<_>>();
    match_component_chars(&value, &pattern, 0, 0, &mut HashMap::new())
}

fn match_component_chars(
    value: &[char],
    pattern: &[char],
    value_index: usize,
    pattern_index: usize,
    memo: &mut HashMap<(usize, usize), bool>,
) -> bool {
    if let Some(result) = memo.get(&(value_index, pattern_index)) {
        return *result;
    }
    let result = match pattern.get(pattern_index) {
        None => value_index == value.len(),
        Some('*') => {
            match_component_chars(value, pattern, value_index, pattern_index + 1, memo)
                || (value_index < value.len()
                    && match_component_chars(value, pattern, value_index + 1, pattern_index, memo))
        }
        Some('?') => {
            value_index < value.len()
                && match_component_chars(value, pattern, value_index + 1, pattern_index + 1, memo)
        }
        Some('[') => {
            parse_character_class(pattern, pattern_index).is_some_and(|(class, next_pattern)| {
                value
                    .get(value_index)
                    .is_some_and(|character| class.matches(*character))
                    && match_component_chars(value, pattern, value_index + 1, next_pattern, memo)
            })
        }
        Some(character) => {
            value.get(value_index) == Some(character)
                && match_component_chars(value, pattern, value_index + 1, pattern_index + 1, memo)
        }
    };
    memo.insert((value_index, pattern_index), result);
    result
}

#[derive(Debug)]
struct CharacterClass {
    negated: bool,
    ranges: Vec<(char, char)>,
}

impl CharacterClass {
    fn matches(&self, character: char) -> bool {
        let contained = self
            .ranges
            .iter()
            .any(|(start, end)| *start <= character && character <= *end);
        contained != self.negated
    }
}

fn parse_character_class(pattern: &[char], open: usize) -> Option<(CharacterClass, usize)> {
    let mut index = open + 1;
    let negated = pattern
        .get(index)
        .is_some_and(|character| matches!(character, '!' | '^'));
    index += usize::from(negated);
    let mut ranges = Vec::new();
    while let Some(&character) = pattern.get(index) {
        if character == ']' && !ranges.is_empty() {
            return Some((CharacterClass { negated, ranges }, index + 1));
        }
        if pattern.get(index + 1) == Some(&'-') {
            let end = *pattern.get(index + 2)?;
            if end == ']' {
                return None;
            }
            ranges.push((character, end));
            index += 3;
        } else {
            ranges.push((character, character));
            index += 1;
        }
    }
    None
}

fn normalize(value: &str) -> String {
    value.nfc().case_fold().collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn empty_filter_is_disabled() {
        assert_eq!(parse_path_filter(None).unwrap(), None);
        assert_eq!(parse_path_filter(Some("  ")).unwrap(), None);
    }

    #[test]
    fn includes_are_ored_and_exclusions_win() {
        let filter = parse_path_filter(Some(
            " docs/**/*.md, *.markdown, !docs/drafts/**, !private.markdown ",
        ))
        .unwrap()
        .unwrap();
        let root = Path::new("/workspace");

        assert!(filter.matches(Path::new("/workspace/docs/guide.md"), root));
        assert!(filter.matches(Path::new("/workspace/docs/deep/guide.md"), root));
        assert!(filter.matches(Path::new("/workspace/notes.markdown"), root));
        assert!(!filter.matches(Path::new("/workspace/docs/drafts/idea.md"), root));
        assert!(!filter.matches(Path::new("/workspace/private.markdown"), root));
        assert!(!filter.matches(Path::new("/workspace/notes.txt"), root));
    }

    #[test]
    fn exclusion_only_starts_with_every_path() {
        let filter = parse_path_filter(Some("!archive/**, !private.md"))
            .unwrap()
            .unwrap();

        assert!(filter.matches_relative(Path::new("notes/public.md")));
        assert!(!filter.matches_relative(Path::new("archive/old.md")));
        assert!(!filter.matches_relative(Path::new("notes/private.md")));
    }

    #[test]
    fn matching_normalizes_unicode_and_case() {
        let filter = parse_path_filter(Some("CAFÉ/**/*.MD")).unwrap().unwrap();
        let decomposed = PathBuf::from("cafe\u{301}/Résumé.md");

        assert!(filter.matches_relative(&decomposed));
        assert!(!filter.matches(Path::new("/outside/CAFÉ/note.md"), Path::new("/workspace")));
    }

    #[test]
    fn component_globs_support_question_marks_and_classes() {
        let filter = parse_path_filter(Some("docs/file-?.[mt][dx]"))
            .unwrap()
            .unwrap();

        assert!(filter.matches_relative(Path::new("docs/file-a.md")));
        assert!(filter.matches_relative(Path::new("docs/file-1.tx")));
        assert!(!filter.matches_relative(Path::new("docs/file-long.md")));
    }

    #[test]
    fn invalid_terms_are_rejected() {
        for expression in [
            "*.md,,!archive/**",
            "*.md, ",
            "!",
            "/absolute/*.md",
            "../*.md",
            "docs/../*.md",
        ] {
            assert!(parse_path_filter(Some(expression)).is_err(), "{expression}");
        }
    }
}
