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

    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        path.canonicalize()
            .is_ok_and(|path| path.starts_with(&self.root))
    }

    #[must_use]
    pub fn scan(&self) -> Vec<WorkspaceEntry> {
        let ignored: HashSet<&str> = IGNORED_DIRECTORIES.iter().copied().collect();
        let mut entries = WalkBuilder::new(&self.root)
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
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.depth() > 0 && !entry.file_type().is_some_and(|kind| kind.is_symlink())
            })
            .filter_map(|entry| {
                let file_type = entry.file_type()?;
                if !file_type.is_dir() && !has_editable_suffix(entry.path()) {
                    return None;
                }
                let relative = entry.path().strip_prefix(&self.root).ok()?.to_path_buf();
                Some(WorkspaceEntry {
                    path: entry.into_path(),
                    depth: relative.components().count().saturating_sub(1),
                    relative,
                    is_dir: file_type.is_dir(),
                })
            })
            .collect::<Vec<_>>();
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
        entries
    }

    #[must_use]
    pub fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()
    }
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
}
