//! Safe file and folder operations confined to one workspace.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::workspace::{IGNORED_DIRECTORIES, Workspace, has_editable_suffix};

/// A user-facing workspace file-management failure.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceEntryError {
    #[error("enter a path inside the workspace: {0}")]
    InvalidRelativePath(PathBuf),
    #[error("the workspace root cannot be changed")]
    WorkspaceRoot,
    #[error("path is outside the workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("symbolic links are not supported: {0}")]
    Symlink(PathBuf),
    #[error("path no longer exists: {0}")]
    Missing(PathBuf),
    #[error("not a regular file or directory: {0}")]
    Unsupported(PathBuf),
    #[error("TermDraft cannot manage entries inside an ignored folder: {0}")]
    Ignored(PathBuf),
    #[error("not a folder: {0}")]
    NotDirectory(PathBuf),
    #[error("an entry already exists at {0}")]
    AlreadyExists(PathBuf),
    #[error("a folder cannot be copied or moved inside itself")]
    InsideItself,
    #[error("not an editable text-file path: {0}")]
    UnsupportedDocument(PathBuf),
    #[error("exclusive workspace moves are unavailable on this platform or filesystem")]
    ExclusiveMoveUnavailable,
    #[error("cannot {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot move {path} to Trash: {message}")]
    Trash { path: PathBuf, message: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    File,
    Directory,
}

/// Create one empty file at an explicit location without creating parent folders.
///
/// Unlike opening a document, creation permits any filename. The frontend can warn when a new
/// file does not have an editable suffix.
///
/// # Errors
///
/// Returns [`WorkspaceEntryError`] when the parent or target is unsafe, outside the workspace,
/// inside an ignored directory, missing, or already exists.
pub fn create_file(
    workspace: &Workspace,
    parent: &Path,
    relative: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    let safe_parent = validate_existing(workspace, parent, true)?;
    if safe_parent.kind != EntryKind::Directory {
        return Err(WorkspaceEntryError::NotDirectory(safe_parent.path));
    }
    validate_explicit_relative_path(relative)?;
    let target = validate_new_target(workspace, &safe_parent.path.join(relative))?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| destination_error("create file", &target, error))?;
    Ok(target)
}

/// Create one folder at an explicit location without creating parent folders.
///
/// # Errors
///
/// Returns [`WorkspaceEntryError`] when the parent or target is unsafe, outside the workspace,
/// inside an ignored directory, missing, or already exists.
pub fn create_folder(
    workspace: &Workspace,
    parent: &Path,
    relative: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    let safe_parent = validate_existing(workspace, parent, true)?;
    if safe_parent.kind != EntryKind::Directory {
        return Err(WorkspaceEntryError::NotDirectory(safe_parent.path));
    }
    validate_explicit_relative_path(relative)?;
    let target = validate_new_target(workspace, &safe_parent.path.join(relative))?;
    fs::create_dir(&target).map_err(|error| destination_error("create folder", &target, error))?;
    Ok(target)
}

/// Rename one existing file or folder without replacing another entry.
///
/// # Errors
///
/// Returns [`WorkspaceEntryError`] when `name` is not one basename or when the move is unsafe or
/// cannot be completed without replacement.
pub fn rename_entry(
    workspace: &Workspace,
    source: &Path,
    name: &OsStr,
) -> Result<PathBuf, WorkspaceEntryError> {
    let name_path = Path::new(name);
    if !matches!(
        name_path.components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    ) {
        return Err(WorkspaceEntryError::InvalidRelativePath(
            name_path.to_path_buf(),
        ));
    }
    let safe_source = validate_existing(workspace, source, false)?;
    let target = safe_source.path.with_file_name(name);
    move_validated_entry(workspace, &safe_source, &target)
}

/// Move one existing file or folder to an explicit workspace path without replacement.
///
/// File moves retain the official editable-suffix restriction. Folder moves may use any safe
/// basename.
///
/// # Errors
///
/// Returns [`WorkspaceEntryError`] when either path is unsafe, the target parent is missing, the
/// target exists, a folder would be moved into itself, or the platform cannot move exclusively.
pub fn move_entry(
    workspace: &Workspace,
    source: &Path,
    target: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    let safe_source = validate_existing(workspace, source, false)?;
    move_validated_entry(workspace, &safe_source, target)
}

/// Copy one existing file or folder without moving the source or replacing another entry.
///
/// # Errors
///
/// Returns [`WorkspaceEntryError`] when either path is unsafe, the target parent is missing, the
/// target exists, a folder would be copied into itself, or an I/O operation fails.
pub fn copy_entry(
    workspace: &Workspace,
    source: &Path,
    target: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    let safe_source = validate_existing(workspace, source, false)?;
    let safe_target = validate_new_target(workspace, target)?;
    if safe_source.kind == EntryKind::Directory && safe_target.starts_with(&safe_source.path) {
        return Err(WorkspaceEntryError::InsideItself);
    }

    match safe_source.kind {
        EntryKind::File => copy_file(&safe_source.path, &safe_target)?,
        EntryKind::Directory => copy_directory(&safe_source.path, &safe_target)?,
    }
    Ok(safe_target)
}

/// Move one existing file or folder tree to the operating system Trash.
///
/// # Errors
///
/// Returns [`WorkspaceEntryError`] when the source is unsafe or the operating system rejects the
/// Trash operation. A failed Trash operation leaves cleanup to the platform implementation.
pub fn move_to_trash(workspace: &Workspace, source: &Path) -> Result<PathBuf, WorkspaceEntryError> {
    move_to_trash_with(workspace, source, |path| {
        trash::delete(path).map_err(|error| error.to_string())
    })
}

#[derive(Debug)]
struct ValidatedEntry {
    path: PathBuf,
    kind: EntryKind,
}

fn move_validated_entry(
    workspace: &Workspace,
    source: &ValidatedEntry,
    target: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    let safe_target = validate_new_target(workspace, target)?;
    if source.kind == EntryKind::Directory && safe_target.starts_with(&source.path) {
        return Err(WorkspaceEntryError::InsideItself);
    }
    if source.kind == EntryKind::File && !has_editable_suffix(&safe_target) {
        return Err(WorkspaceEntryError::UnsupportedDocument(safe_target));
    }
    rename_no_replace(&source.path, &safe_target)?;
    Ok(safe_target)
}

fn validate_explicit_relative_path(path: &Path) -> Result<(), WorkspaceEntryError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(WorkspaceEntryError::InvalidRelativePath(path.to_path_buf()));
    }
    Ok(())
}

fn candidate_path(workspace: &Workspace, path: &Path) -> Result<PathBuf, WorkspaceEntryError> {
    let relative = path_relative_to_root(workspace, path)?;
    let mut candidate = workspace.root.clone();
    for component in relative.components() {
        match component {
            Component::Normal(part) => candidate.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if candidate == workspace.root {
                    return Err(WorkspaceEntryError::OutsideWorkspace(path.to_path_buf()));
                }
                candidate.pop();
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(WorkspaceEntryError::OutsideWorkspace(path.to_path_buf()));
            }
        }
    }
    Ok(candidate)
}

fn path_relative_to_root(
    workspace: &Workspace,
    path: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    if !path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if let Ok(relative) = path.strip_prefix(&workspace.root) {
        return Ok(relative.to_path_buf());
    }

    // macOS commonly spells canonical `/private/var/...` paths as `/var/...`. Find the requested
    // spelling of the workspace root without resolving any components inside the workspace.
    for ancestor in path.ancestors() {
        if ancestor
            .canonicalize()
            .is_ok_and(|canonical| canonical == workspace.root)
        {
            return path
                .strip_prefix(ancestor)
                .map(Path::to_path_buf)
                .map_err(|_| WorkspaceEntryError::OutsideWorkspace(path.to_path_buf()));
        }
    }
    Err(WorkspaceEntryError::OutsideWorkspace(path.to_path_buf()))
}

fn validate_existing(
    workspace: &Workspace,
    path: &Path,
    allow_root: bool,
) -> Result<ValidatedEntry, WorkspaceEntryError> {
    let candidate = candidate_path(workspace, path)?;
    if candidate == workspace.root {
        if !allow_root {
            return Err(WorkspaceEntryError::WorkspaceRoot);
        }
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|error| io_error("inspect", &candidate, error))?;
        return Ok(ValidatedEntry {
            path: candidate,
            kind: metadata_kind(&workspace.root, &metadata)?,
        });
    }
    reject_ignored(workspace, &candidate)?;

    let relative = candidate
        .strip_prefix(&workspace.root)
        .expect("candidate paths stay inside the workspace");
    let component_count = relative.components().count();
    let mut current = workspace.root.clone();
    let mut final_metadata = None;
    for (index, component) in relative.components().enumerate() {
        let Component::Normal(part) = component else {
            unreachable!("candidate paths are normalized")
        };
        current.push(part);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                WorkspaceEntryError::Missing(current.clone())
            } else {
                io_error("inspect", &current, error)
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(WorkspaceEntryError::Symlink(current));
        }
        if index + 1 < component_count && !metadata.is_dir() {
            return Err(WorkspaceEntryError::NotDirectory(current));
        }
        final_metadata = Some(metadata);
    }

    let metadata = final_metadata.expect("non-root candidates contain a component");
    Ok(ValidatedEntry {
        path: candidate.clone(),
        kind: metadata_kind(&candidate, &metadata)?,
    })
}

fn validate_new_target(
    workspace: &Workspace,
    target: &Path,
) -> Result<PathBuf, WorkspaceEntryError> {
    let candidate = candidate_path(workspace, target)?;
    if candidate == workspace.root {
        return Err(WorkspaceEntryError::WorkspaceRoot);
    }
    reject_ignored(workspace, &candidate)?;
    let parent = candidate
        .parent()
        .expect("a non-root workspace path always has a parent");
    let safe_parent = validate_existing(workspace, parent, true)?;
    if safe_parent.kind != EntryKind::Directory {
        return Err(WorkspaceEntryError::NotDirectory(safe_parent.path));
    }
    match fs::symlink_metadata(&candidate) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(WorkspaceEntryError::Symlink(candidate))
        }
        Ok(_) => Err(WorkspaceEntryError::AlreadyExists(candidate)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(candidate),
        Err(error) => Err(io_error("inspect", &candidate, error)),
    }
}

fn reject_ignored(workspace: &Workspace, path: &Path) -> Result<(), WorkspaceEntryError> {
    let relative = path
        .strip_prefix(&workspace.root)
        .expect("candidate paths stay inside the workspace");
    if relative.components().any(|component| {
        let Component::Normal(part) = component else {
            return false;
        };
        IGNORED_DIRECTORIES
            .iter()
            .any(|ignored| part == OsStr::new(ignored))
    }) {
        return Err(WorkspaceEntryError::Ignored(path.to_path_buf()));
    }
    Ok(())
}

fn metadata_kind(path: &Path, metadata: &fs::Metadata) -> Result<EntryKind, WorkspaceEntryError> {
    if metadata.is_file() {
        Ok(EntryKind::File)
    } else if metadata.is_dir() {
        Ok(EntryKind::Directory)
    } else {
        Err(WorkspaceEntryError::Unsupported(path.to_path_buf()))
    }
}

fn copy_file(source: &Path, target: &Path) -> Result<(), WorkspaceEntryError> {
    let mut source_file =
        fs::File::open(source).map_err(|error| io_error("copy", source, error))?;
    let mut target_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| destination_error("copy", target, error))?;
    let result = (|| {
        io::copy(&mut source_file, &mut target_file)
            .map_err(|error| io_error("copy", source, error))?;
        let permissions = fs::metadata(source)
            .map_err(|error| io_error("inspect", source, error))?
            .permissions();
        fs::set_permissions(target, permissions)
            .map_err(|error| io_error("set permissions on", target, error))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(target);
    }
    result
}

fn copy_directory(source: &Path, target: &Path) -> Result<(), WorkspaceEntryError> {
    fs::create_dir(target).map_err(|error| destination_error("copy", target, error))?;
    let result = (|| {
        let entries = fs::read_dir(source).map_err(|error| io_error("read", source, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| io_error("read", source, error))?;
            let source_child = entry.path();
            let target_child = target.join(entry.file_name());
            let metadata = fs::symlink_metadata(&source_child)
                .map_err(|error| io_error("inspect", &source_child, error))?;
            if metadata.file_type().is_symlink() {
                return Err(WorkspaceEntryError::Symlink(source_child));
            }
            if metadata.is_dir() {
                copy_directory(&source_child, &target_child)?;
            } else if metadata.is_file() {
                copy_file(&source_child, &target_child)?;
            } else {
                return Err(WorkspaceEntryError::Unsupported(source_child));
            }
        }
        let permissions = fs::metadata(source)
            .map_err(|error| io_error("inspect", source, error))?
            .permissions();
        fs::set_permissions(target, permissions)
            .map_err(|error| io_error("set permissions on", target, error))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(target);
    }
    result
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "android",
    target_os = "linux",
    target_os = "redox"
))]
fn rename_no_replace(source: &Path, target: &Path) -> Result<(), WorkspaceEntryError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, source, CWD, target, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(error) if error == Errno::EXIST || error == Errno::NOTEMPTY => {
            Err(WorkspaceEntryError::AlreadyExists(target.to_path_buf()))
        }
        Err(error)
            if error == Errno::NOSYS
                || error == Errno::INVAL
                || error == Errno::NOTSUP
                || error == Errno::OPNOTSUPP =>
        {
            Err(WorkspaceEntryError::ExclusiveMoveUnavailable)
        }
        Err(error) => Err(io_error("move", source, error.into())),
    }
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "android",
    target_os = "linux",
    target_os = "redox"
)))]
fn rename_no_replace(_source: &Path, _target: &Path) -> Result<(), WorkspaceEntryError> {
    Err(WorkspaceEntryError::ExclusiveMoveUnavailable)
}

fn move_to_trash_with<F>(
    workspace: &Workspace,
    source: &Path,
    delete: F,
) -> Result<PathBuf, WorkspaceEntryError>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let safe_source = validate_existing(workspace, source, false)?.path;
    delete(&safe_source).map_err(|message| WorkspaceEntryError::Trash {
        path: safe_source.clone(),
        message,
    })?;
    Ok(safe_source)
}

fn destination_error(
    operation: &'static str,
    path: &Path,
    error: io::Error,
) -> WorkspaceEntryError {
    if error.kind() == io::ErrorKind::AlreadyExists {
        WorkspaceEntryError::AlreadyExists(path.to_path_buf())
    } else {
        io_error(operation, path, error)
    }
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> WorkspaceEntryError {
    WorkspaceEntryError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(path: &Path) -> Workspace {
        Workspace::from_target(path).unwrap()
    }

    #[test]
    fn creates_explicit_files_and_folders_without_implicit_parents() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(directory.path());

        let folder = create_folder(&workspace, directory.path(), Path::new("notes")).unwrap();
        let file = create_file(&workspace, &folder, Path::new("idea.data")).unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"");
        assert!(matches!(
            create_file(&workspace, directory.path(), Path::new("missing/note.md")),
            Err(WorkspaceEntryError::Missing(_))
        ));
        assert!(!directory.path().join("missing").exists());
        assert!(matches!(
            create_folder(&workspace, directory.path(), Path::new("../outside")),
            Err(WorkspaceEntryError::InvalidRelativePath(_))
        ));
    }

    #[test]
    fn rejects_ignored_paths_and_existing_destinations() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join(".git")).unwrap();
        fs::write(directory.path().join("exists.md"), "existing").unwrap();
        let workspace = workspace(directory.path());

        assert!(matches!(
            create_file(
                &workspace,
                &directory.path().join(".git"),
                Path::new("note.md")
            ),
            Err(WorkspaceEntryError::Ignored(_))
        ));
        assert!(matches!(
            create_file(&workspace, directory.path(), Path::new("exists.md")),
            Err(WorkspaceEntryError::AlreadyExists(_))
        ));
        assert_eq!(
            fs::read_to_string(directory.path().join("exists.md")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn copies_files_and_folders_without_moving_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let source_file = directory.path().join("draft.md");
        let source_folder = directory.path().join("notes");
        fs::write(&source_file, "draft").unwrap();
        fs::create_dir(&source_folder).unwrap();
        fs::write(source_folder.join("visible.md"), "visible").unwrap();
        fs::write(source_folder.join(".hidden.txt"), "hidden").unwrap();
        let workspace = workspace(directory.path());

        let copied_file = copy_entry(&workspace, &source_file, Path::new("draft-copy.md")).unwrap();
        let copied_folder =
            copy_entry(&workspace, &source_folder, Path::new("notes-copy")).unwrap();

        assert_eq!(fs::read_to_string(&source_file).unwrap(), "draft");
        assert_eq!(fs::read_to_string(copied_file).unwrap(), "draft");
        assert_eq!(
            fs::read_to_string(copied_folder.join("visible.md")).unwrap(),
            "visible"
        );
        assert_eq!(
            fs::read_to_string(copied_folder.join(".hidden.txt")).unwrap(),
            "hidden"
        );
    }

    #[test]
    fn copy_rejects_replacement_and_folder_self_copy() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.md");
        let target = directory.path().join("target.md");
        let folder = directory.path().join("folder");
        fs::write(&source, "source").unwrap();
        fs::write(&target, "target").unwrap();
        fs::create_dir(&folder).unwrap();
        let workspace = workspace(directory.path());

        assert!(matches!(
            copy_entry(&workspace, &source, &target),
            Err(WorkspaceEntryError::AlreadyExists(_))
        ));
        assert!(matches!(
            copy_entry(&workspace, &folder, &folder.join("copy")),
            Err(WorkspaceEntryError::InsideItself)
        ));
        assert_eq!(fs::read_to_string(source).unwrap(), "source");
        assert_eq!(fs::read_to_string(target).unwrap(), "target");
    }

    #[test]
    fn rename_and_move_preserve_contents_without_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        fs::create_dir(&archive).unwrap();
        let source = directory.path().join("draft.md");
        fs::write(&source, "# Draft\n").unwrap();
        let workspace = workspace(directory.path());

        let renamed = rename_entry(&workspace, &source, OsStr::new("essay.md")).unwrap();
        let moved = move_entry(&workspace, &renamed, &archive.join("essay.md")).unwrap();

        assert!(!source.exists());
        assert!(!renamed.exists());
        assert_eq!(fs::read_to_string(moved).unwrap(), "# Draft\n");
    }

    #[test]
    fn move_rejects_escape_replacement_self_move_and_non_document_suffix() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let source = root.join("source.md");
        let existing = root.join("existing.md");
        let folder = root.join("folder");
        fs::write(&source, "source").unwrap();
        fs::write(&existing, "existing").unwrap();
        fs::create_dir(&folder).unwrap();
        let workspace = workspace(&root);

        assert!(matches!(
            move_entry(&workspace, &source, &existing),
            Err(WorkspaceEntryError::AlreadyExists(_))
        ));
        assert!(matches!(
            move_entry(&workspace, &source, &outer.path().join("outside.md")),
            Err(WorkspaceEntryError::OutsideWorkspace(_))
        ));
        assert!(matches!(
            move_entry(&workspace, &folder, &folder.join("nested")),
            Err(WorkspaceEntryError::InsideItself)
        ));
        assert!(matches!(
            move_entry(&workspace, &source, Path::new("source.png")),
            Err(WorkspaceEntryError::UnsupportedDocument(_))
        ));
        assert_eq!(fs::read_to_string(source).unwrap(), "source");
        assert_eq!(fs::read_to_string(existing).unwrap(), "existing");
    }

    #[test]
    fn exclusive_move_does_not_replace_a_racing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.md");
        let target = directory.path().join("target.md");
        fs::write(&source, "source").unwrap();
        fs::write(&target, "racer").unwrap();

        assert!(matches!(
            rename_no_replace(&source, &target),
            Err(WorkspaceEntryError::AlreadyExists(_))
        ));
        assert_eq!(fs::read_to_string(source).unwrap(), "source");
        assert_eq!(fs::read_to_string(target).unwrap(), "racer");
    }

    #[test]
    fn exclusive_copy_does_not_remove_a_racing_destination() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.md");
        let target = directory.path().join("target.md");
        fs::write(&source, "source").unwrap();
        fs::write(&target, "racer").unwrap();

        assert!(matches!(
            copy_file(&source, &target),
            Err(WorkspaceEntryError::AlreadyExists(_))
        ));
        assert_eq!(fs::read_to_string(source).unwrap(), "source");
        assert_eq!(fs::read_to_string(target).unwrap(), "racer");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_selected_and_nested_symbolic_links() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.md");
        let linked = directory.path().join("linked.md");
        let folder = directory.path().join("folder");
        fs::write(&source, "source").unwrap();
        symlink(&source, &linked).unwrap();
        fs::create_dir(&folder).unwrap();
        symlink(&source, folder.join("nested.md")).unwrap();
        let workspace = workspace(directory.path());

        assert!(matches!(
            copy_entry(&workspace, &linked, Path::new("copy.md")),
            Err(WorkspaceEntryError::Symlink(_))
        ));
        assert!(matches!(
            copy_entry(&workspace, &folder, Path::new("folder-copy")),
            Err(WorkspaceEntryError::Symlink(_))
        ));
        assert!(!directory.path().join("folder-copy").exists());
    }

    #[test]
    fn trash_preflight_includes_hidden_contents_and_preserves_source_on_failure() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("workspace");
        let fake_trash = outer.path().join("trash");
        let folder = root.join("notes");
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir(&fake_trash).unwrap();
        fs::write(folder.join("visible.md"), "visible").unwrap();
        fs::write(folder.join(".hidden.txt"), "hidden").unwrap();
        let workspace = workspace(&root);

        let failed = move_to_trash_with(&workspace, &folder, |_| Err("unavailable".into()));
        assert!(matches!(failed, Err(WorkspaceEntryError::Trash { .. })));
        assert!(folder.exists());

        let removed = move_to_trash_with(&workspace, &folder, |source| {
            fs::rename(source, fake_trash.join(source.file_name().unwrap()))
                .map_err(|error| error.to_string())
        })
        .unwrap();
        assert_eq!(removed, workspace.root.join("notes"));
        assert!(!folder.exists());
        assert_eq!(
            fs::read_to_string(fake_trash.join("notes/.hidden.txt")).unwrap(),
            "hidden"
        );
    }
}
