//! Content-free, Python-compatible workspace session storage.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::workspace::has_editable_suffix;

pub const MAX_SESSION_BYTES: u64 = 512 * 1024;
pub const MAX_SESSION_DOCUMENTS: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentViewState {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub scroll_x: f64,
    pub scroll_y: f64,
    pub preview_scroll_x: f64,
    pub preview_scroll_y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionState {
    pub workspace_root: PathBuf,
    pub active_path: Option<PathBuf>,
    pub documents: Vec<DocumentViewState>,
    pub open_paths: Vec<PathBuf>,
}

impl SessionState {
    #[must_use]
    pub fn view_for(&self, path: &Path) -> Option<&DocumentViewState> {
        self.documents.iter().find(|view| view.path == path)
    }
}

#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}

#[derive(Debug)]
pub struct SessionLoad {
    pub state: Option<SessionState>,
    pub warning: Option<String>,
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("cannot resolve the session directory")]
    MissingHome,
    #[error("session has more than {MAX_SESSION_DOCUMENTS} documents")]
    TooManyDocuments,
    #[error("session state exceeds {MAX_SESSION_BYTES} bytes")]
    TooLarge,
    #[error("invalid session state: {0}")]
    Invalid(String),
    #[error("cannot store session state: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionFile {
    version: u8,
    workspace_root: String,
    active_path: Option<String>,
    #[serde(default)]
    open_paths: Vec<String>,
    documents: Vec<ViewFile>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ViewFile {
    path: String,
    line: usize,
    column: usize,
    scroll_x: f64,
    scroll_y: f64,
    #[serde(default)]
    preview_scroll_x: f64,
    #[serde(default)]
    preview_scroll_y: f64,
}

impl SessionStore {
    /// Create a store in the platform's canonical `TermDraft` state directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform has no resolvable home/state directory.
    pub fn platform_default() -> Result<Self, SessionError> {
        Ok(Self::new(default_session_root()?))
    }

    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn path_for(&self, workspace_root: &Path) -> PathBuf {
        let mut digest = Sha256::new();
        #[cfg(unix)]
        digest.update(workspace_root.as_os_str().as_bytes());
        #[cfg(not(unix))]
        digest.update(workspace_root.to_string_lossy().as_bytes());
        self.root.join(format!("{:x}.json", digest.finalize()))
    }

    #[must_use]
    pub fn load(&self, workspace_root: &Path) -> SessionLoad {
        match self.load_inner(workspace_root) {
            Ok(state) => SessionLoad {
                state,
                warning: None,
            },
            Err(SessionError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                SessionLoad {
                    state: None,
                    warning: None,
                }
            }
            Err(error) => SessionLoad {
                state: None,
                warning: Some(format!("Ignoring invalid session state · {error}")),
            },
        }
    }

    /// Atomically store a validated content-free session.
    ///
    /// # Errors
    ///
    /// Returns an error when the state is invalid, too large, or cannot be published.
    pub fn save(&self, state: &SessionState) -> Result<(), SessionError> {
        if state.documents.len() > MAX_SESSION_DOCUMENTS
            || state.open_paths.len() > MAX_SESSION_DOCUMENTS
        {
            return Err(SessionError::TooManyDocuments);
        }
        validate_state(state)?;
        let file = SessionFile {
            version: 3,
            workspace_root: path_string(&state.workspace_root)?,
            active_path: state
                .active_path
                .as_ref()
                .map(|path| relative_string(&state.workspace_root, path))
                .transpose()?,
            open_paths: state
                .open_paths
                .iter()
                .map(|path| relative_string(&state.workspace_root, path))
                .collect::<Result<_, _>>()?,
            documents: state
                .documents
                .iter()
                .map(|view| {
                    Ok(ViewFile {
                        path: relative_string(&state.workspace_root, &view.path)?,
                        line: view.line,
                        column: view.column,
                        scroll_x: view.scroll_x,
                        scroll_y: view.scroll_y,
                        preview_scroll_x: view.preview_scroll_x,
                        preview_scroll_y: view.preview_scroll_y,
                    })
                })
                .collect::<Result<_, SessionError>>()?,
        };
        let mut bytes =
            serde_json::to_vec(&file).map_err(|error| SessionError::Invalid(error.to_string()))?;
        bytes.push(b'\n');
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SESSION_BYTES {
            return Err(SessionError::TooLarge);
        }

        fs::create_dir_all(&self.root)?;
        secure_directory(&self.root)?;
        let mut temporary = NamedTempFile::new_in(&self.root)?;
        #[cfg(unix)]
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.path_for(&state.workspace_root))
            .map_err(|error| SessionError::Io(error.error))?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    fn load_inner(&self, workspace_root: &Path) -> Result<Option<SessionState>, SessionError> {
        let path = self.path_for(workspace_root);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SessionError::Invalid(
                "session path is not a regular file".to_owned(),
            ));
        }
        if metadata.len() > MAX_SESSION_BYTES {
            return Err(SessionError::TooLarge);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW);
        let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
        options
            .open(path)?
            .take(MAX_SESSION_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SESSION_BYTES {
            return Err(SessionError::TooLarge);
        }
        let file: SessionFile = serde_json::from_slice(&bytes)
            .map_err(|error| SessionError::Invalid(error.to_string()))?;
        if !matches!(file.version, 1..=3) {
            return Err(SessionError::Invalid(
                "unsupported session version".to_owned(),
            ));
        }
        if file.documents.len() > MAX_SESSION_DOCUMENTS
            || file.open_paths.len() > MAX_SESSION_DOCUMENTS
        {
            return Err(SessionError::TooManyDocuments);
        }
        let stored_root = PathBuf::from(&file.workspace_root);
        if stored_root != workspace_root {
            return Err(SessionError::Invalid(
                "session belongs to another workspace".to_owned(),
            ));
        }
        let documents = file
            .documents
            .into_iter()
            .map(|view| {
                if !view.scroll_x.is_finite()
                    || view.scroll_x < 0.0
                    || !view.scroll_y.is_finite()
                    || view.scroll_y < 0.0
                    || !view.preview_scroll_x.is_finite()
                    || view.preview_scroll_x < 0.0
                    || !view.preview_scroll_y.is_finite()
                    || view.preview_scroll_y < 0.0
                {
                    return Err(SessionError::Invalid(
                        "view scroll coordinates must be finite and non-negative".to_owned(),
                    ));
                }
                Ok(DocumentViewState {
                    path: resolve_relative(workspace_root, &view.path)?,
                    line: view.line,
                    column: view.column,
                    scroll_x: view.scroll_x,
                    scroll_y: view.scroll_y,
                    preview_scroll_x: view.preview_scroll_x,
                    preview_scroll_y: view.preview_scroll_y,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let active_path = file
            .active_path
            .as_deref()
            .map(|path| resolve_relative(workspace_root, path))
            .transpose()?;
        let open_paths = if file.version == 1 {
            active_path.iter().cloned().collect()
        } else {
            file.open_paths
                .iter()
                .map(|path| resolve_relative(workspace_root, path))
                .collect::<Result<Vec<_>, _>>()?
        };
        let state = SessionState {
            workspace_root: workspace_root.to_path_buf(),
            active_path,
            documents,
            open_paths,
        };
        validate_state(&state)?;
        Ok(Some(state))
    }
}

fn validate_state(state: &SessionState) -> Result<(), SessionError> {
    if state.documents.iter().any(|view| {
        !view.scroll_x.is_finite()
            || view.scroll_x < 0.0
            || !view.scroll_y.is_finite()
            || view.scroll_y < 0.0
            || !view.preview_scroll_x.is_finite()
            || view.preview_scroll_x < 0.0
            || !view.preview_scroll_y.is_finite()
            || view.preview_scroll_y < 0.0
    }) {
        return Err(SessionError::Invalid(
            "view scroll coordinates must be finite and non-negative".to_owned(),
        ));
    }
    let document_paths = state
        .documents
        .iter()
        .map(|view| &view.path)
        .collect::<std::collections::HashSet<_>>();
    if document_paths.len() != state.documents.len() {
        return Err(SessionError::Invalid("duplicate document path".to_owned()));
    }
    let open_paths = state
        .open_paths
        .iter()
        .collect::<std::collections::HashSet<_>>();
    if open_paths.len() != state.open_paths.len() {
        return Err(SessionError::Invalid(
            "duplicate open document path".to_owned(),
        ));
    }
    if state
        .open_paths
        .iter()
        .any(|path| !document_paths.contains(path))
    {
        return Err(SessionError::Invalid(
            "every open document must have a stored view".to_owned(),
        ));
    }
    match &state.active_path {
        Some(active) if !open_paths.contains(active) => Err(SessionError::Invalid(
            "active document must be open".to_owned(),
        )),
        None if !state.open_paths.is_empty() => Err(SessionError::Invalid(
            "open documents require an active document".to_owned(),
        )),
        _ => Ok(()),
    }
}

/// Resolve the preferred session directory, retaining the pre-1.0 fallback.
///
/// # Errors
///
/// Returns an error when no platform state/home directory can be resolved.
pub fn default_session_root() -> Result<PathBuf, SessionError> {
    let (canonical, legacy) = if let Some(root) = env::var_os("XDG_STATE_HOME") {
        let root = PathBuf::from(root);
        (
            root.join("termdraft/sessions"),
            root.join("termwriter/sessions"),
        )
    } else {
        let base = BaseDirs::new().ok_or(SessionError::MissingHome)?;
        #[cfg(target_os = "macos")]
        let root = base.home_dir().join("Library/Application Support");
        #[cfg(not(target_os = "macos"))]
        let root = base.home_dir().join(".local/state");
        #[cfg(target_os = "macos")]
        let names = ("TermDraft/sessions", "TermWriter/sessions");
        #[cfg(not(target_os = "macos"))]
        let names = ("termdraft/sessions", "termwriter/sessions");
        (root.join(names.0), root.join(names.1))
    };
    if canonical.exists() || !legacy.exists() {
        Ok(canonical)
    } else {
        Ok(legacy)
    }
}

fn relative_string(root: &Path, path: &Path) -> Result<String, SessionError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| SessionError::Invalid("document is outside the workspace".to_owned()))?;
    validate_relative(relative)?;
    path_string(relative)
}

fn resolve_relative(root: &Path, value: &str) -> Result<PathBuf, SessionError> {
    let relative = Path::new(value);
    validate_relative(relative)?;
    Ok(root.join(relative))
}

fn validate_relative(path: &Path) -> Result<(), SessionError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !has_editable_suffix(path)
    {
        return Err(SessionError::Invalid(
            "document path must be a supported workspace-relative file".to_owned(),
        ));
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<String, SessionError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| SessionError::Invalid("path is not UTF-8".to_owned()))
}

fn secure_directory(path: &Path) -> Result<(), SessionError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(workspace: &Path) -> SessionState {
        let first = workspace.join("notes/café.md");
        let second = workspace.join("todo.markdown");
        SessionState {
            workspace_root: workspace.to_path_buf(),
            active_path: Some(second.clone()),
            documents: vec![
                DocumentViewState {
                    path: first.clone(),
                    line: 3,
                    column: 8,
                    scroll_x: 1.0,
                    scroll_y: 2.0,
                    preview_scroll_x: 3.0,
                    preview_scroll_y: 4.0,
                },
                DocumentViewState {
                    path: second.clone(),
                    line: 9,
                    column: 2,
                    scroll_x: 5.0,
                    scroll_y: 6.0,
                    preview_scroll_x: 7.0,
                    preview_scroll_y: 8.0,
                },
            ],
            open_paths: vec![first, second],
        }
    }

    #[test]
    fn round_trip_uses_content_free_python_schema() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let store = SessionStore::new(directory.path().join("state"));
        let expected = state(&workspace);

        store.save(&expected).unwrap();
        let loaded = store.load(&workspace).state.unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path_for(&workspace)).unwrap()).unwrap();

        assert_eq!(loaded, expected);
        assert_eq!(payload["version"], 3);
        assert_eq!(payload["open_paths"][0], "notes/café.md");
        assert_eq!(payload["documents"][0]["preview_scroll_y"], 4.0);
        assert!(payload.get("text").is_none());
        assert!(payload.get("content").is_none());
    }

    #[test]
    fn corrupt_state_is_preserved_and_warned() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let store = SessionStore::new(directory.path().join("state"));
        let path = store.path_for(&workspace);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not json \xff").unwrap();

        let result = store.load(&workspace);

        assert!(result.state.is_none());
        assert!(result.warning.is_some());
        assert_eq!(fs::read(path).unwrap(), b"not json \xff");
    }
}
