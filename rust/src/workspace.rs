//! Workspace target validation and recursive text-file discovery.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

pub const EDITABLE_SUFFIXES: &[&str] = &["md", "markdown", "txt"];
pub const IGNORED_DIRECTORIES: &[&str] = &[".git", ".venv", "node_modules", "__pycache__"];

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error("target does not exist: {0}")]
    Missing(PathBuf),
    #[error("symbolic links are not supported: {0}")]
    Symlink(PathBuf),
    #[error("unsupported file type: {0}")]
    Unsupported(PathBuf),
    #[error("path is outside the workspace: {0}")]
    Outside(PathBuf),
    #[error("destination must be a workspace-relative file path: {0}")]
    InvalidDestination(PathBuf),
    #[error("destination already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("destination parent is not an existing workspace directory: {0}")]
    MissingParent(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEntry {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceScan {
    pub entries: Vec<WorkspaceEntry>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub initial_file: Option<PathBuf>,
}

impl Workspace {
    /// Resolve a directory workspace or an initial supported file.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the target is missing, unsafe, or unsupported.
    pub fn from_target(target: &Path) -> Result<Self, WorkspaceError> {
        let metadata = std::fs::symlink_metadata(target).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => WorkspaceError::Missing(target.to_path_buf()),
            _ => WorkspaceError::Io(error),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceError::Symlink(target.to_path_buf()));
        }
        let canonical = target.canonicalize()?;
        if metadata.is_file() {
            validate_suffix(&canonical)?;
            let root = canonical.parent().unwrap_or(Path::new("/")).to_path_buf();
            Ok(Self {
                root,
                initial_file: Some(canonical),
            })
        } else if metadata.is_dir() {
            Ok(Self {
                root: canonical,
                initial_file: None,
            })
        } else {
            Err(WorkspaceError::Unsupported(canonical))
        }
    }

    /// Revalidate one document immediately before opening it.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the file escapes the workspace or is unsafe.
    pub fn validate_document_path(&self, path: &Path) -> Result<PathBuf, WorkspaceError> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceError::Symlink(path.to_path_buf()));
        }
        if !metadata.is_file() {
            return Err(WorkspaceError::Unsupported(path.to_path_buf()));
        }
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(&self.root) {
            return Err(WorkspaceError::Outside(canonical));
        }
        validate_suffix(&canonical)?;
        Ok(canonical)
    }

    /// Validate a new document destination without creating it.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the destination is absolute, escapes the workspace, has an
    /// unsupported suffix, has no existing parent, or already exists.
    pub fn new_document_path(&self, relative: &Path) -> Result<PathBuf, WorkspaceError> {
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(WorkspaceError::InvalidDestination(relative.to_path_buf()));
        }
        validate_suffix(relative)?;
        let candidate = self.root.join(relative);
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return Err(WorkspaceError::AlreadyExists(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(WorkspaceError::Io(error)),
        }
        let parent = candidate
            .parent()
            .ok_or_else(|| WorkspaceError::MissingParent(candidate.clone()))?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|_| WorkspaceError::MissingParent(parent.to_path_buf()))?;
        if !canonical_parent.starts_with(&self.root) {
            return Err(WorkspaceError::Outside(candidate));
        }
        if !canonical_parent.is_dir() {
            return Err(WorkspaceError::MissingParent(canonical_parent));
        }
        Ok(canonical_parent.join(
            candidate
                .file_name()
                .ok_or_else(|| WorkspaceError::InvalidDestination(relative.to_path_buf()))?,
        ))
    }

    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        path.canonicalize()
            .is_ok_and(|path| path.starts_with(&self.root))
    }

    #[must_use]
    pub fn scan(&self) -> Vec<WorkspaceEntry> {
        self.scan_report().entries
    }

    #[must_use]
    pub fn scan_top_level(&self) -> WorkspaceScan {
        let ignored: HashSet<&str> = IGNORED_DIRECTORIES.iter().copied().collect();
        let mut report = WorkspaceScan::default();
        let children = match std::fs::read_dir(&self.root) {
            Ok(children) => children,
            Err(error) => {
                report
                    .warnings
                    .push(format!("Cannot scan {}: {error}", self.root.display()));
                return report;
            }
        };
        for child in children {
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    report.warnings.push(format!(
                        "Cannot read an entry in {}: {error}",
                        self.root.display()
                    ));
                    continue;
                }
            };
            let path = child.path();
            let file_type = match child.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    report
                        .warnings
                        .push(format!("Cannot inspect {}: {error}", path.display()));
                    continue;
                }
            };
            if file_type.is_symlink()
                || (file_type.is_dir()
                    && child
                        .file_name()
                        .to_str()
                        .is_some_and(|name| ignored.contains(name)))
                || (!file_type.is_dir() && !has_editable_suffix(&path))
            {
                continue;
            }
            report.entries.push(WorkspaceEntry {
                relative: PathBuf::from(child.file_name()),
                path,
                depth: 0,
                is_dir: file_type.is_dir(),
            });
        }
        sort_entries(&mut report.entries);
        report
    }

    #[must_use]
    pub fn scan_report(&self) -> WorkspaceScan {
        let ignored: HashSet<&str> = IGNORED_DIRECTORIES.iter().copied().collect();
        let mut report = WorkspaceScan::default();
        for result in WalkBuilder::new(&self.root)
            .hidden(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .follow_links(false)
            .filter_entry(move |entry| {
                entry.depth() == 0
                    || entry
                        .file_name()
                        .to_str()
                        .is_none_or(|name| !ignored.contains(name))
            })
            .build()
        {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    report.warnings.push(error.to_string());
                    continue;
                }
            };
            if entry.depth() == 0 || entry.file_type().is_some_and(|kind| kind.is_symlink()) {
                continue;
            }
            let Some(file_type) = entry.file_type() else {
                report
                    .warnings
                    .push(format!("Cannot inspect {}", entry.path().display()));
                continue;
            };
            if !file_type.is_dir() && !has_editable_suffix(entry.path()) {
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(&self.root) else {
                report.warnings.push(format!(
                    "Ignored path outside workspace: {}",
                    entry.path().display()
                ));
                continue;
            };
            report.entries.push(WorkspaceEntry {
                path: entry.path().to_path_buf(),
                depth: relative.components().count().saturating_sub(1),
                relative: relative.to_path_buf(),
                is_dir: file_type.is_dir(),
            });
        }
        sort_entries(&mut report.entries);
        report
    }

    #[must_use]
    pub fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }
}

fn sort_entries(entries: &mut [WorkspaceEntry]) {
    entries.sort_by(|left, right| {
        left.relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .cmp(
                right
                    .relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().to_lowercase()),
            )
    });
}

fn validate_suffix(path: &Path) -> Result<(), WorkspaceError> {
    if has_editable_suffix(path) {
        Ok(())
    } else {
        Err(WorkspaceError::Unsupported(path.to_path_buf()))
    }
}

#[must_use]
pub fn has_editable_suffix(path: &Path) -> bool {
    path.extension()
        .and_then(|suffix| suffix.to_str())
        .is_some_and(|suffix| EDITABLE_SUFFIXES.contains(&suffix.to_ascii_lowercase().as_str()))
}

/// Return whether two existing paths resolve to the same directory entry under different
/// filesystem spellings. Canonical paths keep distinct hardlinks distinct while folding the case
/// and Unicode aliases supported by the host filesystem.
#[must_use]
pub fn paths_are_spelling_aliases(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_only_supported_files_and_skips_generated_directories() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("notes/nested")).unwrap();
        std::fs::create_dir_all(directory.path().join(".git")).unwrap();
        std::fs::write(directory.path().join("notes/a.md"), "a").unwrap();
        std::fs::write(directory.path().join("notes/nested/b.txt"), "b").unwrap();
        std::fs::write(directory.path().join("notes/image.png"), "x").unwrap();
        std::fs::write(directory.path().join(".git/hidden.md"), "x").unwrap();

        let workspace = Workspace::from_target(directory.path()).unwrap();
        let paths = workspace
            .scan()
            .into_iter()
            .map(|entry| entry.relative)
            .collect::<Vec<_>>();
        assert!(paths.contains(&PathBuf::from("notes/a.md")));
        assert!(paths.contains(&PathBuf::from("notes/nested/b.txt")));
        assert!(!paths.contains(&PathBuf::from("notes/image.png")));
        assert!(!paths.iter().any(|path| path.starts_with(".git")));
    }

    #[test]
    fn top_level_scan_defers_nested_files_and_reports_unavailable_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/nested.md"), "nested").unwrap();
        std::fs::write(root.join("top.md"), "top").unwrap();
        let workspace = Workspace::from_target(&root).unwrap();

        let shallow = workspace.scan_top_level();
        assert!(shallow.warnings.is_empty());
        assert!(
            shallow
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("notes"))
        );
        assert!(
            shallow
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("top.md"))
        );
        assert!(
            !shallow
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("notes/nested.md"))
        );
        assert!(
            workspace
                .scan_report()
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("notes/nested.md"))
        );

        std::fs::remove_file(root.join("notes/nested.md")).unwrap();
        std::fs::remove_dir(root.join("notes")).unwrap();
        std::fs::remove_file(root.join("top.md")).unwrap();
        std::fs::remove_dir(&root).unwrap();
        let unavailable = workspace.scan_top_level();
        assert!(unavailable.entries.is_empty());
        assert_eq!(unavailable.warnings.len(), 1);
    }

    #[test]
    fn validates_new_documents_without_escape_or_overwrite() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("notes")).unwrap();
        std::fs::write(directory.path().join("exists.md"), "x").unwrap();
        let workspace = Workspace::from_target(directory.path()).unwrap();

        assert_eq!(
            workspace
                .new_document_path(Path::new("notes/new.md"))
                .unwrap(),
            directory
                .path()
                .canonicalize()
                .unwrap()
                .join("notes/new.md")
        );
        assert!(matches!(
            workspace.new_document_path(Path::new("../escape.md")),
            Err(WorkspaceError::InvalidDestination(_))
        ));
        assert!(matches!(
            workspace.new_document_path(Path::new("exists.md")),
            Err(WorkspaceError::AlreadyExists(_))
        ));
        assert!(matches!(
            workspace.new_document_path(Path::new("image.png")),
            Err(WorkspaceError::Unsupported(_))
        ));
    }
}
