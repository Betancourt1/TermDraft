//! Deterministic file, workspace-text, document, and heading search primitives.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use pulldown_cmark::{Event, HeadingLevel, Options as MarkdownOptions, Parser, Tag, TagEnd};
use regex::{Regex, RegexBuilder};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::{UnicodeNormalization, char::canonical_combining_class};

use crate::path_filter::{PathFilter, parse_path_filter};
use crate::persistence::load_file;
use crate::workspace::WorkspaceEntry;

pub const DEFAULT_FILE_RESULT_LIMIT: usize = 50;
pub const DEFAULT_TEXT_RESULT_LIMIT: usize = 100;
pub const MAX_PREVIEW_LENGTH: usize = 160;
pub const MAX_REGEX_LENGTH: usize = 500;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextSearchMode {
    #[default]
    Literal,
    WholeWord,
    Regex,
    Fuzzy,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextSearchOptions {
    pub mode: TextSearchMode,
    pub file_filter: Option<String>,
    pub case_sensitive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextSearchOverride {
    pub path: PathBuf,
    pub text: String,
    pub prefer_disk: bool,
}

impl TextSearchOverride {
    #[must_use]
    pub fn new(path: PathBuf, text: String) -> Self {
        Self {
            path,
            text,
            prefer_disk: false,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextSearchResult {
    pub matches: Vec<TextMatch>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
}

pub struct TextSearchRequest<'a> {
    pub files: &'a [PathBuf],
    pub query: &'a str,
    pub limit: usize,
    pub active_override: Option<&'a TextSearchOverride>,
    pub overrides: &'a [TextSearchOverride],
    pub options: TextSearchOptions,
    pub root: Option<&'a Path>,
    pub should_cancel: Option<&'a dyn Fn() -> bool>,
}

impl<'a> TextSearchRequest<'a> {
    #[must_use]
    pub fn new(files: &'a [PathBuf], query: &'a str) -> Self {
        Self {
            files,
            query,
            limit: DEFAULT_TEXT_RESULT_LIMIT,
            active_override: None,
            overrides: &[],
            options: TextSearchOptions::default(),
            root: None,
            should_cancel: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DocumentSearchMatch {
    /// Unicode scalar offset, matching editor row/column coordinates.
    pub start: usize,
    /// Exclusive Unicode scalar offset.
    pub end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MatchDirection {
    Next,
    Previous,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReplaceError {
    #[error("match {index} has an invalid source range")]
    InvalidRange { index: usize },
    #[error("match {index} overlaps or precedes the previous match")]
    Unordered { index: usize },
}

#[must_use]
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let query = normalize_case(query).chars().collect::<Vec<_>>();
    if query.is_empty() {
        return Some(0);
    }
    let candidate = normalize_case(candidate).chars().collect::<Vec<_>>();
    let mut score = 0_i64;
    let mut cursor = 0;
    let mut previous_match = None;

    for wanted in query {
        let offset = candidate[cursor..]
            .iter()
            .position(|character| *character == wanted)?;
        let matched = cursor + offset;
        score += 100 - i64::try_from(offset).unwrap_or(100).min(100);
        if previous_match.is_some_and(|previous| previous + 1 == matched) {
            score += 35;
        }
        if matched == 0 || matches!(candidate[matched - 1], '/' | '_' | '-' | ' ') {
            score += 25;
        }
        previous_match = Some(matched);
        cursor = matched + 1;
    }
    Some(score - i64::try_from(candidate.len()).unwrap_or(i64::MAX))
}

/// Search file entries using the official 50-result file-finder contract.
#[must_use]
pub fn search_files<'a>(query: &str, entries: &'a [WorkspaceEntry]) -> Vec<&'a WorkspaceEntry> {
    search_files_with_filter(query, entries, DEFAULT_FILE_RESULT_LIMIT, None)
}

#[must_use]
pub fn search_files_with_filter<'a>(
    query: &str,
    entries: &'a [WorkspaceEntry],
    limit: usize,
    path_filter: Option<&PathFilter>,
) -> Vec<&'a WorkspaceEntry> {
    if limit == 0 {
        return Vec::new();
    }
    let query = normalize_case(query.trim());
    let candidates = entries.iter().filter(|entry| {
        !entry.is_dir && path_filter.is_none_or(|filter| filter.matches_relative(&entry.relative))
    });
    if query.is_empty() {
        return candidates.take(limit).collect();
    }

    let mut ranked = candidates
        .filter_map(|entry| file_rank(entry, &query).map(|rank| (rank, entry)))
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_rank, _), (right_rank, _)| left_rank.cmp(right_rank));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect()
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FileRank {
    scope: u8,
    priority: i64,
    span: usize,
    start: usize,
    folded_path: String,
    path: String,
}

fn file_rank(entry: &WorkspaceEntry, needle: &str) -> Option<FileRank> {
    let relative = path_text(&entry.relative);
    let relative_folded = normalize_case(&relative);
    let name = entry
        .relative
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let name_folded = normalize_case(&name);

    if relative_folded.contains(needle) {
        let mut score = 0_i64;
        if name_folded.starts_with(needle) {
            score += 100;
        } else if name_folded.contains(needle) {
            score += 70;
        }
        if relative_folded
            .split('/')
            .any(|part| part.starts_with(needle))
        {
            score += 30;
        }
        score -= i64::try_from(relative.chars().count()).unwrap_or(i64::MAX);
        return Some(FileRank {
            scope: 0,
            priority: -score,
            span: 0,
            start: 0,
            folded_path: relative_folded,
            path: relative,
        });
    }

    let (scope, fuzzy) = find_fuzzy_match(&name_folded, needle)
        .map(|result| (1, result))
        .or_else(|| find_fuzzy_match(&relative_folded, needle).map(|result| (2, result)))?;
    Some(FileRank {
        scope,
        priority: i64::try_from(fuzzy.gap_count).unwrap_or(i64::MAX),
        span: fuzzy.span,
        start: fuzzy.start,
        folded_path: relative_folded,
        path: relative,
    })
}

#[derive(Clone, Copy, Debug)]
struct FuzzyMatch {
    start: usize,
    gap_count: usize,
    span: usize,
}

fn find_fuzzy_match(haystack: &str, needle: &str) -> Option<FuzzyMatch> {
    let haystack = haystack.chars().collect::<Vec<_>>();
    let needle = needle.chars().collect::<Vec<_>>();
    let first_character = *needle.first()?;
    let mut best: Option<FuzzyMatch> = None;
    for first in haystack
        .iter()
        .enumerate()
        .filter_map(|(index, character)| (*character == first_character).then_some(index))
    {
        let mut cursor = first + 1;
        let mut last = first;
        let mut complete = true;
        for wanted in &needle[1..] {
            let Some(offset) = haystack[cursor..]
                .iter()
                .position(|character| character == wanted)
            else {
                complete = false;
                break;
            };
            last = cursor + offset;
            cursor = last + 1;
        }
        if complete {
            let span = last - first + 1;
            let candidate = FuzzyMatch {
                start: first,
                gap_count: span - needle.len(),
                span,
            };
            if best.is_none_or(|current| {
                (candidate.gap_count, candidate.span, candidate.start)
                    < (current.gap_count, current.span, current.start)
            }) {
                best = Some(candidate);
            }
        }
    }
    best
}

/// Backward-compatible literal search over one already-loaded source.
#[must_use]
pub fn search_text(path: &Path, text: &str, query: &str, limit: usize) -> Vec<TextMatch> {
    search_text_with_options(path, text, query, limit, &TextSearchOptions::default()).matches
}

#[must_use]
pub fn search_text_with_options(
    path: &Path,
    text: &str,
    query: &str,
    limit: usize,
    options: &TextSearchOptions,
) -> TextSearchResult {
    if query.is_empty() || limit == 0 {
        return TextSearchResult::default();
    }
    let matcher = match LineMatcher::new(query, options) {
        Ok(matcher) => matcher,
        Err(error) => return TextSearchResult::from_error(error),
    };
    let mut results = source_matches(path, text, &matcher, limit, None);
    if options.mode == TextSearchMode::Fuzzy {
        results.sort_by(|left, right| left.0.cmp(&right.0));
    }
    TextSearchResult {
        matches: results
            .into_iter()
            .take(limit)
            .map(|(_, result)| result)
            .collect(),
        warnings: Vec::new(),
        error: None,
    }
}

/// Search validated workspace files and open-document overrides without shelling out.
#[must_use]
pub fn search_workspace_text(request: &TextSearchRequest<'_>) -> TextSearchResult {
    if request.query.is_empty() || request.limit == 0 {
        return TextSearchResult::default();
    }
    let matcher = match LineMatcher::new(request.query, &request.options) {
        Ok(matcher) => matcher,
        Err(error) => return TextSearchResult::from_error(error),
    };
    let path_filter = match parse_path_filter(request.options.file_filter.as_deref()) {
        Ok(filter) => filter,
        Err(error) => {
            return TextSearchResult::from_error(format!("Invalid file filter: {error}"));
        }
    };
    if path_filter.is_some() && request.root.is_none() {
        return TextSearchResult::from_error(
            "A workspace root is required when using a file filter.".to_owned(),
        );
    }

    let mut override_by_path = HashMap::new();
    for source_override in request.overrides.iter().chain(request.active_override) {
        let path = canonical_override_path(request.files, &source_override.path);
        override_by_path.insert(path, source_override);
    }
    let mut candidates = request.files.iter().cloned().collect::<HashSet<_>>();
    candidates.extend(override_by_path.keys().cloned());
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by_key(|path| path_sort_key(path));

    let fuzzy_mode = request.options.mode == TextSearchMode::Fuzzy;
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    for path in candidates {
        if cancelled(request.should_cancel) {
            break;
        }
        if !fuzzy_mode && results.len() >= request.limit {
            break;
        }
        if path_filter
            .as_ref()
            .is_some_and(|filter| request.root.is_none_or(|root| !filter.matches(&path, root)))
        {
            continue;
        }

        let source_override = override_by_path.get(&path).copied();
        let text = if source_override.is_some_and(|source| !source.prefer_disk) {
            source_override.map_or_else(String::new, |source| source.text.clone())
        } else {
            match load_file(&path) {
                Ok(file) => file.text,
                Err(error) => {
                    if let Some(source) = source_override {
                        warnings.push(format!("Using open source for {}: {error}", path.display()));
                        source.text.clone()
                    } else {
                        warnings.push(format!("Cannot search {}: {error}", path.display()));
                        continue;
                    }
                }
            }
        };
        if cancelled(request.should_cancel) {
            break;
        }

        let remaining = if fuzzy_mode {
            request.limit.saturating_mul(2).max(request.limit)
        } else {
            request.limit.saturating_sub(results.len())
        };
        results.extend(source_matches(
            &path,
            &text,
            &matcher,
            remaining,
            request.should_cancel,
        ));
        if fuzzy_mode && results.len() >= request.limit.saturating_mul(2) {
            results.sort_by(|left, right| left.0.cmp(&right.0));
            results.truncate(request.limit);
        }
    }
    if fuzzy_mode {
        results.sort_by(|left, right| left.0.cmp(&right.0));
    }
    TextSearchResult {
        matches: results
            .into_iter()
            .take(request.limit)
            .map(|(_, result)| result)
            .collect(),
        warnings,
        error: None,
    }
}

impl TextSearchResult {
    fn from_error(error: String) -> Self {
        Self {
            matches: Vec::new(),
            warnings: Vec::new(),
            error: Some(error),
        }
    }
}

enum LineMatcher {
    Literal {
        needle: String,
        case_sensitive: bool,
    },
    WholeWord {
        needle: String,
        case_sensitive: bool,
        prefix_boundary: bool,
        suffix_boundary: bool,
    },
    Regex(Regex),
    Fuzzy {
        needle: String,
        case_sensitive: bool,
    },
}

impl LineMatcher {
    fn new(query: &str, options: &TextSearchOptions) -> Result<Self, String> {
        match options.mode {
            TextSearchMode::Literal => Ok(Self::Literal {
                needle: if options.case_sensitive {
                    query.to_owned()
                } else {
                    case_fold(query)
                },
                case_sensitive: options.case_sensitive,
            }),
            TextSearchMode::WholeWord => Ok(Self::WholeWord {
                needle: if options.case_sensitive {
                    query.to_owned()
                } else {
                    case_fold(query)
                },
                case_sensitive: options.case_sensitive,
                prefix_boundary: query.chars().next().is_some_and(is_word_character),
                suffix_boundary: query.chars().next_back().is_some_and(is_word_character),
            }),
            TextSearchMode::Regex => {
                if query.chars().count() > MAX_REGEX_LENGTH {
                    return Err(format!(
                        "Regular expression is limited to {MAX_REGEX_LENGTH} characters."
                    ));
                }
                RegexBuilder::new(query)
                    .case_insensitive(!options.case_sensitive)
                    .unicode(true)
                    .build()
                    .map(Self::Regex)
                    .map_err(|error| format!("Invalid regular expression: {error}"))
            }
            TextSearchMode::Fuzzy => Ok(Self::Fuzzy {
                needle: fuzzy_form(query, options.case_sensitive),
                case_sensitive: options.case_sensitive,
            }),
        }
    }

    fn find(
        &self,
        line: &str,
        should_cancel: Option<&dyn Fn() -> bool>,
    ) -> Option<LineSearchMatch> {
        match self {
            Self::Literal {
                needle,
                case_sensitive,
            } => literal_column(line, needle, *case_sensitive).map(LineSearchMatch::literal),
            Self::WholeWord {
                needle,
                case_sensitive,
                prefix_boundary,
                suffix_boundary,
            } => whole_word_column(
                line,
                needle,
                *case_sensitive,
                *prefix_boundary,
                *suffix_boundary,
            )
            .map(LineSearchMatch::literal),
            Self::Regex(pattern) => pattern
                .find(line)
                .map(|found| LineSearchMatch::literal(line[..found.start()].chars().count())),
            Self::Fuzzy {
                needle,
                case_sensitive,
            } => fuzzy_line_match(line, needle, *case_sensitive, should_cancel),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct LineSearchMatch {
    column: usize,
    fuzzy_rank: (usize, usize, usize, usize),
}

impl LineSearchMatch {
    fn literal(column: usize) -> Self {
        Self {
            column,
            fuzzy_rank: (0, 0, 0, 0),
        }
    }
}

type SearchRank = (usize, usize, usize, usize, String, String, usize, usize);

fn source_matches(
    path: &Path,
    text: &str,
    matcher: &LineMatcher,
    limit: usize,
    should_cancel: Option<&dyn Fn() -> bool>,
) -> Vec<(SearchRank, TextMatch)> {
    let fuzzy_mode = matches!(matcher, LineMatcher::Fuzzy { .. });
    let mut results = Vec::new();
    let path_key = path_sort_key(path);
    for (line_number, line) in logical_lines(text).into_iter().enumerate() {
        if cancelled(should_cancel) {
            break;
        }
        let Some(found) = matcher.find(line, should_cancel) else {
            continue;
        };
        let rank = (
            found.fuzzy_rank.0,
            found.fuzzy_rank.1,
            found.fuzzy_rank.2,
            found.fuzzy_rank.3,
            path_key.0.clone(),
            path_key.1.clone(),
            line_number,
            found.column,
        );
        results.push((
            rank,
            TextMatch {
                path: path.to_path_buf(),
                line: line_number,
                column: found.column,
                preview: line_preview(line, found.column),
            },
        ));
        if fuzzy_mode && results.len() >= limit.saturating_mul(2) {
            results.sort_by(|left, right| left.0.cmp(&right.0));
            results.truncate(limit);
        } else if !fuzzy_mode && results.len() >= limit {
            break;
        }
    }
    results
}

fn literal_column(line: &str, needle: &str, case_sensitive: bool) -> Option<usize> {
    if case_sensitive {
        return line.find(needle).map(|byte| line[..byte].chars().count());
    }
    let (folded, columns) = case_fold_with_columns(line);
    let byte = folded.find(needle)?;
    let index = folded[..byte].chars().count();
    columns.get(index).copied()
}

fn whole_word_column(
    line: &str,
    needle: &str,
    case_sensitive: bool,
    prefix_boundary: bool,
    suffix_boundary: bool,
) -> Option<usize> {
    let (haystack, columns) = if case_sensitive {
        (line.to_owned(), (0..line.chars().count()).collect())
    } else {
        case_fold_with_columns(line)
    };
    let line_characters = line.chars().collect::<Vec<_>>();
    for (byte, _) in haystack.match_indices(needle) {
        let folded_start = haystack[..byte].chars().count();
        let folded_end = folded_start + needle.chars().count();
        let source_start = *columns.get(folded_start)?;
        let source_end = columns.get(folded_end.saturating_sub(1))?.saturating_add(1);
        let valid_prefix = !prefix_boundary
            || source_start == 0
            || !is_word_character(line_characters[source_start - 1]);
        let valid_suffix = !suffix_boundary
            || source_end == line_characters.len()
            || !is_word_character(line_characters[source_end]);
        if valid_prefix && valid_suffix {
            return Some(source_start);
        }
    }
    None
}

fn fuzzy_line_match(
    line: &str,
    needle: &str,
    case_sensitive: bool,
    should_cancel: Option<&dyn Fn() -> bool>,
) -> Option<LineSearchMatch> {
    let (haystack, source_columns) = fuzzy_form_with_columns(line, case_sensitive, should_cancel)?;
    let haystack = haystack.chars().collect::<Vec<_>>();
    let needle = needle.chars().collect::<Vec<_>>();
    let first_character = *needle.first()?;
    let mut best: Option<LineSearchMatch> = None;
    for (candidate_count, first) in haystack
        .iter()
        .enumerate()
        .filter_map(|(index, character)| (*character == first_character).then_some(index))
        .enumerate()
    {
        if candidate_count % 256 == 0 && cancelled(should_cancel) {
            return None;
        }
        let mut cursor = first + 1;
        let mut last = first;
        let mut complete = true;
        for (needle_index, wanted) in needle[1..].iter().enumerate() {
            if needle_index % 256 == 0 && cancelled(should_cancel) {
                return None;
            }
            let Some(offset) = haystack[cursor..]
                .iter()
                .position(|character| character == wanted)
            else {
                complete = false;
                break;
            };
            last = cursor + offset;
            cursor = last + 1;
        }
        if complete {
            let span = last - first + 1;
            let boundary_penalty =
                usize::from(first > 0 && is_word_character(haystack[first.saturating_sub(1)]));
            let candidate = LineSearchMatch {
                column: source_columns[first],
                fuzzy_rank: (span - needle.len(), boundary_penalty, first, haystack.len()),
            };
            if best.is_none_or(|current| candidate.fuzzy_rank < current.fuzzy_rank) {
                best = Some(candidate);
            }
        }
    }
    best
}

fn logical_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        if matches!(bytes[index], b'\r' | b'\n') {
            lines.push(&text[start..index]);
            if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            start = index + 1;
        }
        index += 1;
    }
    lines.push(&text[start..]);
    lines
}

fn line_preview(line: &str, column: usize) -> String {
    let characters = line.chars().collect::<Vec<_>>();
    if characters.len() <= MAX_PREVIEW_LENGTH {
        return line.to_owned();
    }

    let start = column
        .min(characters.len())
        .saturating_sub(MAX_PREVIEW_LENGTH / 3);
    let prefix = usize::from(start > 0);
    let mut content_width = MAX_PREVIEW_LENGTH - prefix - 1;
    let mut end = (start + content_width).min(characters.len());
    let suffix = usize::from(end < characters.len());
    if suffix == 0 {
        content_width += 1;
        end = (start + content_width).min(characters.len());
    }

    let mut preview = String::new();
    if prefix == 1 {
        preview.push('…');
    }
    preview.extend(&characters[start..end]);
    if suffix == 1 {
        preview.push('…');
    }
    preview
}

/// Return non-overlapping literal matches using Unicode scalar offsets.
#[must_use]
pub fn find_document_matches(
    source: &str,
    query: &str,
    case_sensitive: bool,
) -> Vec<DocumentSearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }
    if case_sensitive {
        return source
            .match_indices(query)
            .map(|(byte, matched)| DocumentSearchMatch {
                start: source[..byte].chars().count(),
                end: source[..byte + matched.len()].chars().count(),
            })
            .collect();
    }

    let needle = case_fold(query);
    let (folded, columns) = case_fold_with_columns(source);
    let mut matches = Vec::new();
    let mut byte_cursor = 0;
    while let Some(relative_byte) = folded[byte_cursor..].find(&needle) {
        let start_byte = byte_cursor + relative_byte;
        let end_byte = start_byte + needle.len();
        let folded_start = folded[..start_byte].chars().count();
        let folded_end = folded[..end_byte].chars().count();
        let Some(&start) = columns.get(folded_start) else {
            break;
        };
        let Some(end) = columns
            .get(folded_end.saturating_sub(1))
            .map(|column| column + 1)
        else {
            break;
        };
        let candidate = DocumentSearchMatch { start, end };
        if matches.last() != Some(&candidate) {
            matches.push(candidate);
        }
        byte_cursor = end_byte;
    }
    matches
}

/// Replace an ordered set of captured non-overlapping Unicode source spans.
///
/// # Errors
///
/// Rejects out-of-bounds, reversed, overlapping, or unordered match ranges.
pub fn replace_document_matches(
    source: &str,
    matches: &[DocumentSearchMatch],
    replacement: &str,
) -> Result<String, ReplaceError> {
    if matches.is_empty() {
        return Ok(source.to_owned());
    }
    let byte_offsets = source
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(source.len()))
        .collect::<Vec<_>>();
    let source_length = byte_offsets.len() - 1;
    let mut previous_end = 0;
    let mut replaced = String::with_capacity(source.len());
    for (index, source_match) in matches.iter().enumerate() {
        if source_match.start > source_match.end || source_match.end > source_length {
            return Err(ReplaceError::InvalidRange { index });
        }
        if index > 0 && source_match.start < previous_end {
            return Err(ReplaceError::Unordered { index });
        }
        replaced.push_str(&source[byte_offsets[previous_end]..byte_offsets[source_match.start]]);
        replaced.push_str(replacement);
        previous_end = source_match.end;
    }
    replaced.push_str(&source[byte_offsets[previous_end]..]);
    Ok(replaced)
}

#[must_use]
pub fn initial_document_match_index(
    matches: &[DocumentSearchMatch],
    anchor_offset: usize,
) -> Option<usize> {
    matches
        .iter()
        .position(|source_match| source_match.start >= anchor_offset)
        .or_else(|| (!matches.is_empty()).then_some(0))
}

#[must_use]
pub fn cycle_document_match_index(
    match_count: usize,
    selected: Option<usize>,
    direction: MatchDirection,
) -> Option<usize> {
    if match_count == 0 {
        return None;
    }
    match direction {
        MatchDirection::Next => Some(selected.map_or(0, |index| (index + 1) % match_count)),
        MatchDirection::Previous => Some(selected.map_or(match_count - 1, |index| {
            (index + match_count - 1) % match_count
        })),
    }
}

#[must_use]
pub fn location_to_offset(source: &str, location: (usize, usize)) -> usize {
    let (starts, ends) = line_boundaries(source);
    let row = location.0.min(starts.len() - 1);
    let column = location.1.min(ends[row] - starts[row]);
    starts[row] + column
}

#[must_use]
pub fn offset_to_location(source: &str, offset: usize) -> (usize, usize) {
    let (starts, ends) = line_boundaries(source);
    let bounded = offset.min(source.chars().count());
    let row = starts.partition_point(|start| *start <= bounded) - 1;
    (row, bounded.min(ends[row]) - starts[row])
}

fn line_boundaries(source: &str) -> (Vec<usize>, Vec<usize>) {
    let characters = source.chars().collect::<Vec<_>>();
    let mut starts = vec![0];
    let mut ends = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        if matches!(characters[index], '\r' | '\n') {
            ends.push(index);
            if characters[index] == '\r' && characters.get(index + 1) == Some(&'\n') {
                index += 1;
            }
            starts.push(index + 1);
        }
        index += 1;
    }
    ends.push(characters.len());
    (starts, ends)
}

/// Build the heading outline from the same `CommonMark` parser used by the preview.
#[must_use]
pub fn heading_outline(text: &str) -> Vec<(usize, usize, String)> {
    let mut options = MarkdownOptions::empty();
    options.insert(MarkdownOptions::ENABLE_STRIKETHROUGH);
    options.insert(MarkdownOptions::ENABLE_TASKLISTS);
    options.insert(MarkdownOptions::ENABLE_HEADING_ATTRIBUTES);
    options.insert(MarkdownOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    options.insert(MarkdownOptions::ENABLE_SUPERSCRIPT);
    options.insert(MarkdownOptions::ENABLE_SUBSCRIPT);

    let mut headings = Vec::new();
    let mut active: Option<(usize, usize, String)> = None;
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                active = Some((
                    byte_to_line(text, range.start),
                    heading_level(level),
                    String::new(),
                ));
            }
            Event::Text(content) | Event::Code(content) => {
                if let Some((_, _, label)) = active.as_mut() {
                    label.push_str(&content);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some((_, _, label)) = active.as_mut() {
                    label.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((line, level, label)) = active.take() {
                    headings.push((line, level, label.trim().to_owned()));
                }
            }
            _ => {}
        }
    }
    headings
}

fn heading_level(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn byte_to_line(source: &str, byte: usize) -> usize {
    let bytes = &source.as_bytes()[..byte.min(source.len())];
    let mut lines = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            lines += 1;
            if bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
        } else if bytes[index] == b'\n' {
            lines += 1;
        }
        index += 1;
    }
    lines
}

fn fuzzy_form(value: &str, case_sensitive: bool) -> String {
    let folded = if case_sensitive {
        value.to_owned()
    } else {
        case_fold(value)
    };
    folded.nfd().collect()
}

fn fuzzy_form_with_columns(
    value: &str,
    case_sensitive: bool,
    should_cancel: Option<&dyn Fn() -> bool>,
) -> Option<(String, Vec<usize>)> {
    let mut characters = Vec::new();
    let mut source_columns = Vec::new();
    for (column, character) in value.chars().enumerate() {
        if column % 256 == 0 && cancelled(should_cancel) {
            return None;
        }
        let transformed = if case_sensitive {
            character.to_string()
        } else {
            character.case_fold().collect()
        };
        for folded in transformed.chars() {
            for normalized in folded.to_string().nfd() {
                characters.push(normalized);
                source_columns.push(column);
            }
        }
    }

    let intermediate = characters.iter().collect::<String>();
    let normalized = intermediate.nfd().collect::<String>();
    if normalized == intermediate {
        return Some((normalized, source_columns));
    }

    let mut columns_by_character: HashMap<char, VecDeque<usize>> = HashMap::new();
    for (character, column) in characters.into_iter().zip(source_columns) {
        columns_by_character
            .entry(character)
            .or_default()
            .push_back(column);
    }
    let mut normalized_columns = Vec::new();
    for (index, character) in normalized.chars().enumerate() {
        if index % 256 == 0 && cancelled(should_cancel) {
            return None;
        }
        normalized_columns.push(columns_by_character.get_mut(&character)?.pop_front()?);
    }
    Some((normalized, normalized_columns))
}

fn case_fold_with_columns(value: &str) -> (String, Vec<usize>) {
    let mut folded = String::new();
    let mut columns = Vec::new();
    for (column, character) in value.chars().enumerate() {
        for folded_character in character.case_fold() {
            folded.push(folded_character);
            columns.push(column);
        }
    }
    (folded, columns)
}

fn case_fold(value: &str) -> String {
    value.case_fold().collect()
}

fn normalize_case(value: &str) -> String {
    value.nfc().case_fold().collect()
}

fn is_word_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric() || canonical_combining_class(character) != 0
}

fn path_text(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn path_sort_key(path: &Path) -> (String, String) {
    let path = path_text(path);
    (normalize_case(&path), path)
}

fn canonical_override_path(files: &[PathBuf], path: &Path) -> PathBuf {
    if files.iter().any(|candidate| candidate == path) {
        return path.to_path_buf();
    }
    let key = path_sort_key(path).0;
    files
        .iter()
        .find(|candidate| {
            path_sort_key(candidate).0 == key && paths_are_spelling_aliases(candidate, path)
        })
        .cloned()
        .unwrap_or_else(|| path.to_path_buf())
}

fn paths_are_spelling_aliases(left: &Path, right: &Path) -> bool {
    let (Ok(left_metadata), Ok(right_metadata)) = (fs::metadata(left), fs::metadata(right)) else {
        return false;
    };
    #[cfg(unix)]
    {
        left_metadata.dev() == right_metadata.dev() && left_metadata.ino() == right_metadata.ino()
    }
    #[cfg(not(unix))]
    {
        left.canonicalize().ok() == right.canonicalize().ok()
    }
}

fn cancelled(callback: Option<&dyn Fn() -> bool>) -> bool {
    callback.is_some_and(|callback| callback())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;

    use super::*;
    use crate::path_filter::parse_path_filter;

    fn entry(relative: &str) -> WorkspaceEntry {
        WorkspaceEntry {
            path: Path::new("/workspace").join(relative),
            relative: PathBuf::from(relative),
            depth: 0,
            is_dir: false,
        }
    }

    #[test]
    fn fuzzy_matches_path_segments() {
        assert!(fuzzy_score("rdm", "docs/README.md").is_some());
        assert!(fuzzy_score("xyz", "docs/README.md").is_none());
    }

    #[test]
    fn file_search_matches_python_ranking_and_limit() {
        let entries = vec![
            entry("research/daily/map.md"),
            entry("random.md"),
            entry("readme.md"),
            entry("rdm-notes.md"),
        ];
        let found = search_files("rdm", &entries)
            .into_iter()
            .map(|entry| entry.relative.clone())
            .collect::<Vec<_>>();

        assert_eq!(
            found,
            [
                "rdm-notes.md",
                "readme.md",
                "random.md",
                "research/daily/map.md"
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn file_search_normalizes_unicode_and_applies_filter() {
        let entries = vec![
            entry("Cafe\u{301} Notes.markdown"),
            entry("docs/private.md"),
            entry("docs/public.md"),
        ];
        assert_eq!(
            search_files("CAFÉN", &entries)[0].relative,
            entries[0].relative
        );

        let filter = parse_path_filter(Some("docs/**/*.md, !**/private.md"))
            .unwrap()
            .unwrap();
        assert_eq!(
            search_files_with_filter("", &entries, 50, Some(&filter))[0].relative,
            Path::new("docs/public.md")
        );
    }

    #[test]
    fn text_search_reports_unicode_columns_and_one_match_per_line() {
        let matches = search_text(Path::new("note.md"), "ßx CAFÉ\nAlpha alpha", "ALPHA", 10);
        assert_eq!((matches[0].line, matches[0].column), (1, 0));

        let expanded = search_text(Path::new("note.md"), "ßx", "x", 10);
        assert_eq!(expanded[0].column, 1);
    }

    #[test]
    fn whole_word_search_uses_full_case_folding_and_unicode_boundaries() {
        let options = TextSearchOptions {
            mode: TextSearchMode::WholeWord,
            ..TextSearchOptions::default()
        };
        let result = search_text_with_options(
            Path::new("note.md"),
            "Straße\nSTRASSE\nStrassen\nVorstraße\n",
            "strasse",
            10,
            &options,
        );
        assert_eq!(
            result
                .matches
                .iter()
                .map(|found| (found.line, found.column))
                .collect::<Vec<_>>(),
            vec![(0, 0), (1, 0)]
        );
    }

    #[test]
    fn whole_word_treats_combining_marks_as_word_characters() {
        let options = TextSearchOptions {
            mode: TextSearchMode::WholeWord,
            ..TextSearchOptions::default()
        };
        let standalone = "cafe\u{301}";
        let result = search_text_with_options(
            Path::new("note.md"),
            &format!("{standalone}ine\n{standalone}\n"),
            standalone,
            10,
            &options,
        );
        assert_eq!(result.matches[0].line, 1);
        assert_eq!(result.matches.len(), 1);
    }

    #[test]
    fn regex_errors_are_returned_before_source_search() {
        let options = TextSearchOptions {
            mode: TextSearchMode::Regex,
            ..TextSearchOptions::default()
        };
        let invalid = search_text_with_options(Path::new("missing.md"), "", "[", 10, &options);
        let oversized = search_text_with_options(
            Path::new("missing.md"),
            "",
            &"x".repeat(MAX_REGEX_LENGTH + 1),
            10,
            &options,
        );

        assert!(
            invalid
                .error
                .unwrap()
                .starts_with("Invalid regular expression:")
        );
        assert_eq!(
            oversized.error.as_deref(),
            Some("Regular expression is limited to 500 characters.")
        );
    }

    #[test]
    fn regex_includes_logical_empty_and_trailing_lines() {
        let options = TextSearchOptions {
            mode: TextSearchMode::Regex,
            case_sensitive: true,
            ..TextSearchOptions::default()
        };
        let found = search_text_with_options(Path::new("note.md"), "alpha\n\n", "^$", 10, &options);
        assert_eq!(
            found
                .matches
                .iter()
                .map(|item| item.line)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn workspace_search_sorts_deduplicates_filters_and_uses_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let docs = directory.path().join("docs");
        fs::create_dir(&docs).unwrap();
        let a = docs.join("a.md");
        let b = docs.join("b.md");
        let excluded = directory.path().join("excluded.markdown");
        fs::write(&a, "disk a\n").unwrap();
        fs::write(&b, "needle b\n").unwrap();
        let overrides = vec![TextSearchOverride::new(a.clone(), "needle a\n".to_owned())];
        let files = vec![b.clone(), excluded, a.clone(), a.clone()];
        let mut request = TextSearchRequest::new(&files, "needle");
        request.overrides = &overrides;
        request.root = Some(directory.path());
        request.options.file_filter = Some("docs/**/*.md".to_owned());

        let result = search_workspace_text(&request);

        assert_eq!(
            result
                .matches
                .iter()
                .map(|found| found.path.clone())
                .collect::<Vec<_>>(),
            vec![a, b]
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn workspace_search_reports_load_warnings_and_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.md");
        let source_override = TextSearchOverride {
            path: missing.clone(),
            text: "local draft".to_owned(),
            prefer_disk: true,
        };
        let mut request = TextSearchRequest::new(&[], "draft");
        request.active_override = Some(&source_override);

        let result = search_workspace_text(&request);

        assert_eq!(result.matches[0].path, missing);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("Using open source"));
    }

    #[test]
    fn fuzzy_text_search_ranks_globally_and_preserves_unicode_columns() {
        let options = TextSearchOptions {
            mode: TextSearchMode::Fuzzy,
            ..TextSearchOptions::default()
        };
        let result = search_text_with_options(
            Path::new("note.md"),
            "alpha beta gamma\nprefix abg\nxxabg\nabg\nX Straße\n",
            "abg",
            10,
            &options,
        );
        assert_eq!(
            result
                .matches
                .iter()
                .take(4)
                .map(|found| (found.line, found.column))
                .collect::<Vec<_>>(),
            vec![(3, 0), (1, 7), (2, 2), (0, 4)]
        );

        let unicode =
            search_text_with_options(Path::new("note.md"), "X Straße\n", "ss", 10, &options);
        assert_eq!(unicode.matches[0].column, 6);
    }

    #[test]
    fn fuzzy_text_search_applies_limit_after_ranking_and_respects_case() {
        let fuzzy = TextSearchOptions {
            mode: TextSearchMode::Fuzzy,
            ..TextSearchOptions::default()
        };
        let limited = search_text_with_options(
            Path::new("note.md"),
            "alpha beta gamma\nabg\n",
            "abg",
            1,
            &fuzzy,
        );
        assert_eq!(limited.matches[0].line, 1);

        let case_sensitive = TextSearchOptions {
            mode: TextSearchMode::Fuzzy,
            case_sensitive: true,
            ..TextSearchOptions::default()
        };
        let sensitive = search_text_with_options(
            Path::new("note.md"),
            "Alpha Beta\n",
            "ab",
            10,
            &case_sensitive,
        );
        assert!(sensitive.matches.is_empty());
    }

    #[test]
    fn cancellation_stops_between_lines() {
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() > 5
        };
        let files = Vec::new();
        let source_override =
            TextSearchOverride::new(PathBuf::from("note.md"), "no match\n".repeat(100));
        let mut request = TextSearchRequest::new(&files, "needle");
        request.active_override = Some(&source_override);
        request.should_cancel = Some(&cancelled);

        assert!(search_workspace_text(&request).matches.is_empty());
        assert_eq!(checks.get(), 6);
    }

    #[test]
    fn long_preview_is_bounded_and_keeps_match_visible() {
        let source = format!("{} needle {}", "a".repeat(300), "z".repeat(300));
        let result = search_text(Path::new("note.md"), &source, "needle", 10);
        let preview = &result[0].preview;

        assert!(preview.chars().count() <= MAX_PREVIEW_LENGTH);
        assert!(preview.contains("needle"));
        assert!(preview.starts_with('…'));
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn document_matches_and_replacement_preserve_unicode_spans() {
        let source = "Straße and STRASSE";
        let matches = find_document_matches(source, "strasse", false);

        assert_eq!(
            matches,
            vec![
                DocumentSearchMatch { start: 0, end: 6 },
                DocumentSearchMatch { start: 11, end: 18 }
            ]
        );
        assert_eq!(
            replace_document_matches(source, &matches, "street").unwrap(),
            "street and street"
        );
    }

    #[test]
    fn replacement_rejects_invalid_ranges() {
        assert_eq!(
            replace_document_matches("abc", &[DocumentSearchMatch { start: 1, end: 4 }], "x"),
            Err(ReplaceError::InvalidRange { index: 0 })
        );
        assert_eq!(
            replace_document_matches(
                "abc",
                &[
                    DocumentSearchMatch { start: 1, end: 2 },
                    DocumentSearchMatch { start: 0, end: 1 }
                ],
                "x"
            ),
            Err(ReplaceError::Unordered { index: 1 })
        );
    }

    #[test]
    fn locations_support_mixed_line_endings() {
        let source = "one\r\ntwo\nthree\rfour";

        assert_eq!(location_to_offset(source, (0, 3)), 3);
        assert_eq!(location_to_offset(source, (1, 2)), 7);
        assert_eq!(location_to_offset(source, (3, 4)), source.chars().count());
        assert_eq!(offset_to_location(source, 7), (1, 2));
        assert_eq!(offset_to_location(source, source.chars().count()), (3, 4));
    }

    #[test]
    fn document_match_navigation_wraps() {
        let matches = vec![
            DocumentSearchMatch { start: 2, end: 3 },
            DocumentSearchMatch { start: 5, end: 6 },
        ];
        assert_eq!(initial_document_match_index(&matches, 4), Some(1));
        assert_eq!(initial_document_match_index(&matches, 9), Some(0));
        assert_eq!(
            cycle_document_match_index(2, Some(1), MatchDirection::Next),
            Some(0)
        );
        assert_eq!(
            cycle_document_match_index(2, Some(0), MatchDirection::Previous),
            Some(1)
        );
    }

    #[test]
    fn outline_uses_parser_headings_and_source_lines() {
        assert_eq!(
            heading_outline(
                "# One\ntext\n\nSetext *two*\n---\n\n```md\n# not a heading\n```\n### Three\n"
            ),
            vec![
                (0, 1, "One".to_owned()),
                (3, 2, "Setext two".to_owned()),
                (9, 3, "Three".to_owned())
            ]
        );
    }
}
