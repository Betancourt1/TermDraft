//! Private, device-local full snapshots for Local History.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use directories::BaseDirs;
#[cfg(unix)]
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::workspace::has_editable_suffix;

pub const MAX_CHECKPOINTS_PER_DOCUMENT: usize = 20;
pub const MAX_CHECKPOINT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TOTAL_CHECKPOINT_BYTES: u64 = 100 * 1024 * 1024;

const WORKSPACE_VERSION: u8 = 1;
const CHECKPOINT_VERSION: u8 = 1;
const MAX_WORKSPACE_FILE_BYTES: u64 = 64 * 1024;
const MAX_CHECKPOINT_FILE_BYTES: u64 = MAX_TOTAL_CHECKPOINT_BYTES;

/// Why a full source snapshot was captured.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointReason {
    Manual,
    PreviousSavedVersion,
    BeforeRestore,
    BeforeExternalReload,
}

/// One trusted Local History full snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    /// Stable identity used to select this exact checkpoint.
    pub id: String,
    /// Current workspace-relative document identity.
    pub document_path: PathBuf,
    /// Workspace-relative path at the time this checkpoint was captured.
    pub captured_path: PathBuf,
    /// Complete plaintext document source.
    pub source: String,
    /// Lowercase hexadecimal `SHA-256` digest of `source`.
    pub digest: String,
    /// UTC `RFC 3339` capture time.
    pub captured_at: String,
    pub reason: CheckpointReason,
}

/// A problem with one stored entry that did not hide other valid history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointWarningKind {
    Corrupt,
    Unreadable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckpointWarning {
    pub path: PathBuf,
    pub kind: CheckpointWarningKind,
    pub message: String,
}

/// Newest-first valid checkpoints plus independently reported storage problems.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckpointList {
    pub checkpoints: Vec<Checkpoint>,
    pub warnings: Vec<CheckpointWarning>,
}

/// The result of a scoped clear or path retarget.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CheckpointMutation {
    pub affected: usize,
    pub warnings: Vec<CheckpointWarning>,
}

/// The result of attempting to append a checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureOutcome {
    Stored {
        checkpoint: Checkpoint,
        pruned: usize,
        warnings: Vec<CheckpointWarning>,
    },
    Disabled,
    Duplicate {
        checkpoint_id: String,
        warnings: Vec<CheckpointWarning>,
    },
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("cannot resolve the Local History directory")]
    MissingHome,
    #[error("checkpoint exceeds {MAX_CHECKPOINT_BYTES} bytes ({bytes} bytes)")]
    TooLarge { bytes: u64 },
    #[error("invalid Local History state: {0}")]
    Invalid(String),
    #[error("Local History changed while it was being updated: {}", .0.display())]
    Stale(PathBuf),
    #[error("cannot access Local History: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct CheckpointStore {
    root: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFile {
    version: u8,
    workspace_root: String,
    enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckpointFile {
    version: u8,
    workspace_root: String,
    checkpoint: Checkpoint,
}

#[derive(Clone, Debug)]
struct StoredCheckpoint {
    checkpoint: Checkpoint,
    path: PathBuf,
    fingerprint: String,
    source_bytes: u64,
    captured_at: OffsetDateTime,
}

#[derive(Clone, Debug)]
struct StoredFile {
    path: PathBuf,
    fingerprint: String,
}

#[derive(Debug, Default)]
struct Inventory {
    checkpoints: Vec<StoredCheckpoint>,
    warnings: Vec<CheckpointWarning>,
}

impl CheckpointStore {
    /// Create a store in the platform's canonical `TermDraft` state directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform has no resolvable home/state directory.
    pub fn platform_default() -> Result<Self, CheckpointError> {
        Ok(Self::new(default_checkpoint_root()?))
    }

    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Return whether Local History was explicitly enabled for this exact workspace.
    ///
    /// Missing state means disabled.
    ///
    /// # Errors
    ///
    /// Returns an error when existing workspace state is unsafe or invalid.
    pub fn is_enabled(&self, workspace_root: &Path) -> Result<bool, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        Ok(self
            .read_workspace_file(workspace_root)?
            .is_some_and(|file| file.enabled))
    }

    /// Persist explicit Local History enablement for this exact workspace.
    ///
    /// Disabling Local History does not clear existing checkpoints.
    ///
    /// # Errors
    ///
    /// Returns an error when existing state is invalid or the update cannot be published.
    pub fn set_enabled(&self, workspace_root: &Path, enabled: bool) -> Result<(), CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let workspace_directory = self.workspace_directory(workspace_root);
        let _lock = StoreLock::acquire(&self.root, &workspace_directory)?;
        let _ = self.read_workspace_file(workspace_root)?;
        let file = WorkspaceFile {
            version: WORKSPACE_VERSION,
            workspace_root: path_string(workspace_root)?,
            enabled,
        };
        let mut bytes = serde_json::to_vec(&file)
            .map_err(|error| CheckpointError::Invalid(error.to_string()))?;
        bytes.push(b'\n');
        write_atomic_replace(&workspace_directory.join("workspace.json"), &bytes)
    }

    /// Append one full snapshot when Local History is enabled.
    ///
    /// Adjacent snapshots with the same source digest are deduplicated. Successful writes prune
    /// the oldest valid entries deterministically to enforce both retention caps.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, oversized snapshots, invalid state, or failed storage.
    pub fn capture(
        &self,
        workspace_root: &Path,
        document_path: &Path,
        source: &str,
        reason: CheckpointReason,
    ) -> Result<CaptureOutcome, CheckpointError> {
        self.capture_at(
            workspace_root,
            document_path,
            source,
            reason,
            OffsetDateTime::now_utc(),
        )
    }

    /// List valid checkpoints for one exact document, newest first.
    ///
    /// Corrupt or unreadable sibling entries are preserved and returned as warnings.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace or document path is invalid, or storage cannot be
    /// scanned safely.
    pub fn list(
        &self,
        workspace_root: &Path,
        document_path: &Path,
    ) -> Result<CheckpointList, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let relative = document_relative(workspace_root, document_path)?;
        let mut inventory = self.inventory(workspace_root)?;
        inventory
            .checkpoints
            .retain(|stored| stored.checkpoint.document_path == relative);
        sort_oldest_first(&mut inventory.checkpoints);
        let checkpoints = inventory
            .checkpoints
            .into_iter()
            .rev()
            .map(|stored| stored.checkpoint)
            .collect();
        Ok(CheckpointList {
            checkpoints,
            warnings: inventory.warnings,
        })
    }

    /// List every valid checkpoint for one exact workspace, newest first.
    ///
    /// Corrupt or unreadable entries are preserved and returned as warnings.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace is invalid or storage cannot be scanned safely.
    pub fn list_all(&self, workspace_root: &Path) -> Result<CheckpointList, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let mut inventory = self.inventory(workspace_root)?;
        sort_oldest_first(&mut inventory.checkpoints);
        let checkpoints = inventory
            .checkpoints
            .into_iter()
            .rev()
            .map(|stored| stored.checkpoint)
            .collect();
        Ok(CheckpointList {
            checkpoints,
            warnings: inventory.warnings,
        })
    }

    /// Clear valid checkpoints for one exact document identity.
    ///
    /// Entries that cannot be attributed safely are preserved and reported as warnings.
    ///
    /// # Errors
    ///
    /// Returns an error when paths are invalid, storage cannot be scanned, or a selected file
    /// changed before deletion.
    pub fn clear_document(
        &self,
        workspace_root: &Path,
        document_path: &Path,
        checkpoint_ids: &[String],
    ) -> Result<CheckpointMutation, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let relative = document_relative(workspace_root, document_path)?;
        let workspace_directory = self.workspace_directory(workspace_root);
        let _lock = StoreLock::acquire(&self.root, &workspace_directory)?;
        let inventory = self.inventory(workspace_root)?;
        let selected = inventory
            .checkpoints
            .iter()
            .filter(|stored| stored.checkpoint.document_path == relative)
            .collect::<Vec<_>>();
        let mut current_ids = selected
            .iter()
            .map(|stored| stored.checkpoint.id.clone())
            .collect::<Vec<_>>();
        current_ids.sort();
        let mut expected_ids = checkpoint_ids.to_vec();
        expected_ids.sort();
        if current_ids != expected_ids {
            return Err(CheckpointError::Stale(
                workspace_directory.join("checkpoints"),
            ));
        }
        let files = selected
            .into_iter()
            .map(|stored| StoredFile {
                path: stored.path.clone(),
                fingerprint: stored.fingerprint.clone(),
            })
            .collect::<Vec<_>>();
        remove_verified_files(&files)?;
        Ok(CheckpointMutation {
            affected: files.len(),
            warnings: inventory.warnings,
        })
    }

    /// Clear the exact confirmed valid checkpoints for one workspace.
    ///
    /// Workspace enablement is left unchanged. Unsafe or unreadable entries are preserved and
    /// reported rather than followed or guessed.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace is invalid, storage cannot be scanned, or a selected
    /// file changed before deletion.
    pub fn clear_all(
        &self,
        workspace_root: &Path,
        checkpoint_ids: &[String],
    ) -> Result<CheckpointMutation, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let workspace_directory = self.workspace_directory(workspace_root);
        let _lock = StoreLock::acquire(&self.root, &workspace_directory)?;
        let inventory = self.inventory(workspace_root)?;
        let mut current_ids = inventory
            .checkpoints
            .iter()
            .map(|stored| stored.checkpoint.id.clone())
            .collect::<Vec<_>>();
        current_ids.sort();
        let mut expected_ids = checkpoint_ids.to_vec();
        expected_ids.sort();
        if current_ids != expected_ids {
            return Err(CheckpointError::Stale(
                workspace_directory.join("checkpoints"),
            ));
        }
        let files = inventory
            .checkpoints
            .iter()
            .map(|stored| StoredFile {
                path: stored.path.clone(),
                fingerprint: stored.fingerprint.clone(),
            })
            .collect::<Vec<_>>();
        remove_verified_files(&files)?;
        Ok(CheckpointMutation {
            affected: files.len(),
            warnings: inventory.warnings,
        })
    }

    /// Retarget current document identities after an in-app file or directory move.
    ///
    /// Historical `captured_path` values and stable checkpoint IDs remain unchanged.
    ///
    /// # Errors
    ///
    /// Returns an error when paths are outside the workspace, a resulting document path is
    /// unsupported, state is invalid, or a selected file changed before replacement.
    pub fn retarget_paths(
        &self,
        workspace_root: &Path,
        source: &Path,
        target: &Path,
    ) -> Result<CheckpointMutation, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let source_relative = scope_relative(workspace_root, source)?;
        let target_relative = scope_relative(workspace_root, target)?;
        let workspace_directory = self.workspace_directory(workspace_root);
        let _lock = StoreLock::acquire(&self.root, &workspace_directory)?;
        let inventory = self.inventory(workspace_root)?;
        let workspace_string = path_string(workspace_root)?;
        let mut prospective = inventory.checkpoints.clone();
        for stored in &mut prospective {
            let Some(retargeted) = retargeted_relative(
                &stored.checkpoint.document_path,
                &source_relative,
                &target_relative,
            ) else {
                continue;
            };
            validate_document_relative(&retargeted)?;
            stored.checkpoint.document_path = retargeted;
        }
        sort_oldest_first(&mut prospective);
        let pruned_ids = pruning_candidates(
            &prospective,
            MAX_CHECKPOINTS_PER_DOCUMENT,
            MAX_TOTAL_CHECKPOINT_BYTES,
        )
        .into_iter()
        .map(|index| prospective[index].checkpoint.id.clone())
        .collect::<HashSet<_>>();
        let mut replacements = Vec::new();

        for stored in &inventory.checkpoints {
            let Some(retargeted) = retargeted_relative(
                &stored.checkpoint.document_path,
                &source_relative,
                &target_relative,
            ) else {
                continue;
            };
            validate_document_relative(&retargeted)?;
            if pruned_ids.contains(&stored.checkpoint.id) {
                continue;
            }
            let mut checkpoint = stored.checkpoint.clone();
            checkpoint.document_path = retargeted;
            let bytes = serialize_checkpoint(&workspace_string, &checkpoint)?;
            replacements.push((stored.clone(), bytes));
        }
        let pruned = prospective
            .iter()
            .filter(|stored| pruned_ids.contains(&stored.checkpoint.id))
            .map(|stored| StoredFile {
                path: stored.path.clone(),
                fingerprint: stored.fingerprint.clone(),
            })
            .collect::<Vec<_>>();

        for (stored, _) in &replacements {
            verify_unchanged(&stored.path, &stored.fingerprint)?;
        }
        for stored in &pruned {
            verify_unchanged(&stored.path, &stored.fingerprint)?;
        }
        for (stored, bytes) in &replacements {
            write_atomic_replace(&stored.path, bytes)?;
        }
        remove_verified_files(&pruned)?;

        Ok(CheckpointMutation {
            affected: replacements.len(),
            warnings: inventory.warnings,
        })
    }

    fn capture_at(
        &self,
        workspace_root: &Path,
        document_path: &Path,
        source: &str,
        reason: CheckpointReason,
        captured_at: OffsetDateTime,
    ) -> Result<CaptureOutcome, CheckpointError> {
        validate_workspace_root(workspace_root)?;
        let relative = document_relative(workspace_root, document_path)?;
        if !self
            .read_workspace_file(workspace_root)?
            .is_some_and(|file| file.enabled)
        {
            return Ok(CaptureOutcome::Disabled);
        }
        let source_bytes = u64::try_from(source.len()).unwrap_or(u64::MAX);
        if source_bytes > MAX_CHECKPOINT_BYTES {
            return Err(CheckpointError::TooLarge {
                bytes: source_bytes,
            });
        }

        let workspace_directory = self.workspace_directory(workspace_root);
        let _lock = StoreLock::acquire(&self.root, &workspace_directory)?;
        if !self
            .read_workspace_file(workspace_root)?
            .is_some_and(|file| file.enabled)
        {
            return Ok(CaptureOutcome::Disabled);
        }

        let mut inventory = self.inventory(workspace_root)?;
        sort_oldest_first(&mut inventory.checkpoints);
        let digest = hex_digest(source.as_bytes());
        if let Some(previous) = inventory
            .checkpoints
            .iter()
            .rev()
            .find(|stored| stored.checkpoint.document_path == relative)
            && previous.checkpoint.digest == digest
        {
            return Ok(CaptureOutcome::Duplicate {
                checkpoint_id: previous.checkpoint.id.clone(),
                warnings: inventory.warnings,
            });
        }

        let captured_at_string = captured_at
            .format(&Rfc3339)
            .map_err(|error| CheckpointError::Invalid(error.to_string()))?;
        let workspace_string = path_string(workspace_root)?;
        let checkpoints_directory = workspace_directory.join("checkpoints");
        ensure_directory(&checkpoints_directory)?;
        let checkpoint = Self::persist_unique_checkpoint(
            &checkpoints_directory,
            &workspace_string,
            &relative,
            source,
            &digest,
            &captured_at_string,
            reason,
        )?;

        let mut current = self.inventory(workspace_root)?;
        let warnings = current.warnings;
        sort_oldest_first(&mut current.checkpoints);
        let candidates = pruning_candidates(
            &current.checkpoints,
            MAX_CHECKPOINTS_PER_DOCUMENT,
            MAX_TOTAL_CHECKPOINT_BYTES,
        );
        let selected = candidates
            .iter()
            .map(|&index| StoredFile {
                path: current.checkpoints[index].path.clone(),
                fingerprint: current.checkpoints[index].fingerprint.clone(),
            })
            .collect::<Vec<_>>();
        remove_verified_files(&selected)?;

        Ok(CaptureOutcome::Stored {
            checkpoint,
            pruned: selected.len(),
            warnings,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_unique_checkpoint(
        checkpoints_directory: &Path,
        workspace_root: &str,
        relative: &Path,
        source: &str,
        digest: &str,
        captured_at: &str,
        reason: CheckpointReason,
    ) -> Result<Checkpoint, CheckpointError> {
        for nonce in 0..=u32::MAX {
            let id = checkpoint_id(workspace_root, relative, digest, captured_at, reason, nonce);
            let checkpoint = Checkpoint {
                id: id.clone(),
                document_path: relative.to_path_buf(),
                captured_path: relative.to_path_buf(),
                source: source.to_owned(),
                digest: digest.to_owned(),
                captured_at: captured_at.to_owned(),
                reason,
            };
            let bytes = serialize_checkpoint(workspace_root, &checkpoint)?;
            let path = checkpoints_directory.join(format!("{id}.json"));
            if persist_new(&path, &bytes)? {
                return Ok(checkpoint);
            }
        }
        Err(CheckpointError::Invalid(
            "could not allocate a unique checkpoint identity".to_owned(),
        ))
    }

    fn read_workspace_file(
        &self,
        workspace_root: &Path,
    ) -> Result<Option<WorkspaceFile>, CheckpointError> {
        let workspace_directory = self.workspace_directory(workspace_root);
        if !validate_optional_store_path(&self.root, &workspace_directory)? {
            return Ok(None);
        }
        let path = workspace_directory.join("workspace.json");
        let Some(bytes) = read_optional_regular_bytes(&path, MAX_WORKSPACE_FILE_BYTES)? else {
            return Ok(None);
        };
        let file: WorkspaceFile = serde_json::from_slice(&bytes)
            .map_err(|error| CheckpointError::Invalid(error.to_string()))?;
        if file.version != WORKSPACE_VERSION {
            return Err(CheckpointError::Invalid(
                "unsupported workspace state version".to_owned(),
            ));
        }
        if Path::new(&file.workspace_root) != workspace_root {
            return Err(CheckpointError::Invalid(
                "workspace state belongs to another workspace".to_owned(),
            ));
        }
        Ok(Some(file))
    }

    fn inventory(&self, workspace_root: &Path) -> Result<Inventory, CheckpointError> {
        let workspace_directory = self.workspace_directory(workspace_root);
        if !validate_optional_store_path(&self.root, &workspace_directory)? {
            return Ok(Inventory::default());
        }
        let directory = workspace_directory.join("checkpoints");
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(CheckpointError::Invalid(
                    "checkpoint storage is not a real directory".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Inventory::default());
            }
            Err(error) => return Err(error.into()),
        }

        let mut paths = Vec::new();
        let mut warnings = Vec::new();
        for entry in fs::read_dir(&directory)? {
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "json")
                    {
                        paths.push(path);
                    }
                }
                Err(error) => warnings.push(CheckpointWarning {
                    path: directory.clone(),
                    kind: CheckpointWarningKind::Unreadable,
                    message: error.to_string(),
                }),
            }
        }
        paths.sort();

        let mut inventory = Inventory {
            warnings,
            ..Inventory::default()
        };
        for path in paths {
            match read_regular_bytes(&path, MAX_CHECKPOINT_FILE_BYTES) {
                Ok(bytes) => {
                    let fingerprint = hex_digest(&bytes);
                    match decode_checkpoint(&path, workspace_root, &bytes, fingerprint) {
                        Ok(stored) => inventory.checkpoints.push(stored),
                        Err(error) => inventory.warnings.push(CheckpointWarning {
                            path,
                            kind: CheckpointWarningKind::Corrupt,
                            message: error.to_string(),
                        }),
                    }
                }
                Err(error @ CheckpointError::Io(_)) => {
                    inventory.warnings.push(CheckpointWarning {
                        path,
                        kind: CheckpointWarningKind::Unreadable,
                        message: error.to_string(),
                    });
                }
                Err(error) => inventory.warnings.push(CheckpointWarning {
                    path,
                    kind: CheckpointWarningKind::Corrupt,
                    message: error.to_string(),
                }),
            }
        }
        Ok(inventory)
    }

    fn workspace_directory(&self, workspace_root: &Path) -> PathBuf {
        self.root.join(path_digest(workspace_root))
    }
}

/// Resolve the preferred Local History directory, retaining the pre-1.0 fallback.
///
/// # Errors
///
/// Returns an error when no platform state/home directory can be resolved.
pub fn default_checkpoint_root() -> Result<PathBuf, CheckpointError> {
    let (canonical, legacy) = if let Some(root) = env::var_os("XDG_STATE_HOME") {
        let root = PathBuf::from(root);
        (
            root.join("termdraft/local-history"),
            root.join("termwriter/local-history"),
        )
    } else {
        let base = BaseDirs::new().ok_or(CheckpointError::MissingHome)?;
        #[cfg(target_os = "macos")]
        let root = base.home_dir().join("Library/Application Support");
        #[cfg(not(target_os = "macos"))]
        let root = base.home_dir().join(".local/state");
        #[cfg(target_os = "macos")]
        let names = ("TermDraft/local-history", "TermWriter/local-history");
        #[cfg(not(target_os = "macos"))]
        let names = ("termdraft/local-history", "termwriter/local-history");
        (root.join(names.0), root.join(names.1))
    };
    if canonical.exists() || !legacy.exists() {
        Ok(canonical)
    } else {
        Ok(legacy)
    }
}

fn serialize_checkpoint(
    workspace_root: &str,
    checkpoint: &Checkpoint,
) -> Result<Vec<u8>, CheckpointError> {
    let file = CheckpointFile {
        version: CHECKPOINT_VERSION,
        workspace_root: workspace_root.to_owned(),
        checkpoint: checkpoint.clone(),
    };
    let mut bytes =
        serde_json::to_vec(&file).map_err(|error| CheckpointError::Invalid(error.to_string()))?;
    bytes.push(b'\n');
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length > MAX_CHECKPOINT_FILE_BYTES {
        return Err(CheckpointError::TooLarge { bytes: length });
    }
    Ok(bytes)
}

fn decode_checkpoint(
    path: &Path,
    workspace_root: &Path,
    bytes: &[u8],
    fingerprint: String,
) -> Result<StoredCheckpoint, CheckpointError> {
    let file: CheckpointFile = serde_json::from_slice(bytes)
        .map_err(|error| CheckpointError::Invalid(error.to_string()))?;
    if file.version != CHECKPOINT_VERSION {
        return Err(CheckpointError::Invalid(
            "unsupported checkpoint version".to_owned(),
        ));
    }
    if Path::new(&file.workspace_root) != workspace_root {
        return Err(CheckpointError::Invalid(
            "checkpoint belongs to another workspace".to_owned(),
        ));
    }
    validate_document_relative(&file.checkpoint.document_path)?;
    validate_document_relative(&file.checkpoint.captured_path)?;
    if !is_lower_hex_digest(&file.checkpoint.id) {
        return Err(CheckpointError::Invalid(
            "checkpoint identity is invalid".to_owned(),
        ));
    }
    let expected_name = format!("{}.json", file.checkpoint.id);
    if path
        .file_name()
        .is_none_or(|name| name != expected_name.as_str())
    {
        return Err(CheckpointError::Invalid(
            "checkpoint filename does not match its identity".to_owned(),
        ));
    }
    let source_bytes = u64::try_from(file.checkpoint.source.len()).unwrap_or(u64::MAX);
    if source_bytes > MAX_CHECKPOINT_BYTES {
        return Err(CheckpointError::TooLarge {
            bytes: source_bytes,
        });
    }
    let expected_digest = hex_digest(file.checkpoint.source.as_bytes());
    if file.checkpoint.digest != expected_digest {
        return Err(CheckpointError::Invalid(
            "checkpoint source digest does not match".to_owned(),
        ));
    }
    let captured_at = OffsetDateTime::parse(&file.checkpoint.captured_at, &Rfc3339)
        .map_err(|error| CheckpointError::Invalid(error.to_string()))?;
    Ok(StoredCheckpoint {
        checkpoint: file.checkpoint,
        path: path.to_path_buf(),
        fingerprint,
        source_bytes,
        captured_at,
    })
}

fn sort_oldest_first(checkpoints: &mut [StoredCheckpoint]) {
    checkpoints.sort_by(|left, right| {
        left.captured_at
            .cmp(&right.captured_at)
            .then_with(|| left.checkpoint.id.cmp(&right.checkpoint.id))
            .then_with(|| left.path.cmp(&right.path))
    });
}

fn pruning_candidates(
    checkpoints: &[StoredCheckpoint],
    maximum_per_document: usize,
    maximum_total_bytes: u64,
) -> Vec<usize> {
    let mut counts = HashMap::<&Path, usize>::new();
    for stored in checkpoints {
        *counts
            .entry(stored.checkpoint.document_path.as_path())
            .or_default() += 1;
    }
    let mut excess = counts
        .into_iter()
        .filter(|(_, count)| *count > maximum_per_document)
        .map(|(path, count)| (path, count - maximum_per_document))
        .collect::<HashMap<_, _>>();
    let mut selected = HashSet::new();
    for (index, stored) in checkpoints.iter().enumerate() {
        if let Some(remaining) = excess.get_mut(stored.checkpoint.document_path.as_path())
            && *remaining > 0
        {
            selected.insert(index);
            *remaining -= 1;
        }
    }

    let mut total = checkpoints
        .iter()
        .enumerate()
        .filter(|(index, _)| !selected.contains(index))
        .map(|(_, stored)| stored.source_bytes)
        .fold(0_u64, u64::saturating_add);
    if total > maximum_total_bytes {
        for (index, stored) in checkpoints.iter().enumerate() {
            if total <= maximum_total_bytes {
                break;
            }
            if selected.insert(index) {
                total = total.saturating_sub(stored.source_bytes);
            }
        }
    }

    let mut selected = selected.into_iter().collect::<Vec<_>>();
    selected.sort_unstable();
    selected
}

fn checkpoint_id(
    workspace_root: &str,
    captured_path: &Path,
    digest: &str,
    captured_at: &str,
    reason: CheckpointReason,
    nonce: u32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"termdraft-local-history-v1\0");
    hasher.update(workspace_root.as_bytes());
    hasher.update(b"\0");
    hasher.update(captured_path.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(digest.as_bytes());
    hasher.update(b"\0");
    hasher.update(captured_at.as_bytes());
    hasher.update(b"\0");
    hasher.update(reason_name(reason).as_bytes());
    hasher.update(nonce.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

const fn reason_name(reason: CheckpointReason) -> &'static str {
    match reason {
        CheckpointReason::Manual => "manual",
        CheckpointReason::PreviousSavedVersion => "previous_saved_version",
        CheckpointReason::BeforeRestore => "before_restore",
        CheckpointReason::BeforeExternalReload => "before_external_reload",
    }
}

fn document_relative(
    workspace_root: &Path,
    document_path: &Path,
) -> Result<PathBuf, CheckpointError> {
    let relative = scope_relative(workspace_root, document_path)?;
    validate_document_relative(&relative)?;
    Ok(relative)
}

fn scope_relative(workspace_root: &Path, path: &Path) -> Result<PathBuf, CheckpointError> {
    let relative = path
        .strip_prefix(workspace_root)
        .map_err(|_| CheckpointError::Invalid("path is outside the workspace".to_owned()))?;
    validate_relative_components(relative)?;
    Ok(relative.to_path_buf())
}

fn validate_document_relative(path: &Path) -> Result<(), CheckpointError> {
    validate_relative_components(path)?;
    if !has_editable_suffix(path) {
        return Err(CheckpointError::Invalid(
            "document path is not a supported editable file".to_owned(),
        ));
    }
    Ok(())
}

fn validate_relative_components(path: &Path) -> Result<(), CheckpointError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CheckpointError::Invalid(
            "path must be workspace-relative without traversal".to_owned(),
        ));
    }
    Ok(())
}

fn validate_workspace_root(path: &Path) -> Result<(), CheckpointError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(CheckpointError::Invalid(
            "workspace root must be an absolute normalized path".to_owned(),
        ));
    }
    let _ = path_string(path)?;
    Ok(())
}

fn retargeted_relative(path: &Path, source: &Path, target: &Path) -> Option<PathBuf> {
    let suffix = path.strip_prefix(source).ok()?;
    Some(target.join(suffix))
}

fn path_string(path: &Path) -> Result<String, CheckpointError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| CheckpointError::Invalid("path is not UTF-8".to_owned()))
}

fn path_digest(path: &Path) -> String {
    let mut hasher = Sha256::new();
    #[cfg(unix)]
    hasher.update(path.as_os_str().as_bytes());
    #[cfg(not(unix))]
    hasher.update(path.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_optional_store_path(
    root: &Path,
    workspace_directory: &Path,
) -> Result<bool, CheckpointError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(CheckpointError::Invalid(
                "Local History root is not a real directory".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    match fs::symlink_metadata(workspace_directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(CheckpointError::Invalid(
                "Local History workspace storage is not a real directory".to_owned(),
            ))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn read_optional_regular_bytes(
    path: &Path,
    maximum: u64,
) -> Result<Option<Vec<u8>>, CheckpointError> {
    match fs::symlink_metadata(path) {
        Ok(_) => read_regular_bytes(path, maximum).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read_regular_bytes(path: &Path, maximum: u64) -> Result<Vec<u8>, CheckpointError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CheckpointError::Invalid(format!(
            "storage path is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > maximum {
        return Err(CheckpointError::TooLarge {
            bytes: metadata.len(),
        });
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
    options
        .open(path)?
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if length > maximum {
        return Err(CheckpointError::TooLarge { bytes: length });
    }
    Ok(bytes)
}

fn ensure_directory(path: &Path) -> Result<(), CheckpointError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CheckpointError::Invalid(format!(
                    "Local History storage is not a real directory: {}",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            if let Some(parent) = path.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), CheckpointError> {
    let parent = path
        .parent()
        .ok_or_else(|| CheckpointError::Invalid("storage path has no parent".to_owned()))?;
    ensure_directory(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| CheckpointError::Io(error.error))?;
    sync_directory(parent)
}

fn persist_new(path: &Path, bytes: &[u8]) -> Result<bool, CheckpointError> {
    let parent = path
        .parent()
        .ok_or_else(|| CheckpointError::Invalid("storage path has no parent".to_owned()))?;
    ensure_directory(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {
            sync_directory(parent)?;
            Ok(true)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(CheckpointError::Io(error.error)),
    }
}

fn verify_unchanged(path: &Path, expected_fingerprint: &str) -> Result<(), CheckpointError> {
    let bytes = read_regular_bytes(path, MAX_CHECKPOINT_FILE_BYTES)?;
    if hex_digest(&bytes) != expected_fingerprint {
        return Err(CheckpointError::Stale(path.to_path_buf()));
    }
    Ok(())
}

fn remove_verified_files(files: &[StoredFile]) -> Result<(), CheckpointError> {
    for file in files {
        verify_unchanged(&file.path, &file.fingerprint)?;
    }
    for file in files {
        fs::remove_file(&file.path)?;
    }
    if let Some(parent) = files.first().and_then(|file| file.path.parent()) {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), CheckpointError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

struct StoreLock {
    _file: File,
}

impl StoreLock {
    fn acquire(root: &Path, workspace_directory: &Path) -> Result<Self, CheckpointError> {
        ensure_directory(root)?;
        ensure_directory(workspace_directory)?;
        let path = workspace_directory.join(".lock");
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path)?;
        if !file.metadata()?.is_file() {
            return Err(CheckpointError::Invalid(
                "Local History lock is not a regular file".to_owned(),
            ));
        }
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        sync_directory(workspace_directory)?;
        #[cfg(unix)]
        flock(&file, FlockOperation::LockExclusive).map_err(std::io::Error::from)?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(directory: &tempfile::TempDir, name: &str) -> PathBuf {
        let path = directory.path().join(name);
        fs::create_dir(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn enable(store: &CheckpointStore, workspace: &Path) {
        store.set_enabled(workspace, true).unwrap();
    }

    fn capture_at(
        store: &CheckpointStore,
        workspace: &Path,
        document: &Path,
        source: &str,
        second: i64,
    ) -> CaptureOutcome {
        store
            .capture_at(
                workspace,
                document,
                source,
                CheckpointReason::Manual,
                OffsetDateTime::from_unix_timestamp(second).unwrap(),
            )
            .unwrap()
    }

    fn stored(outcome: CaptureOutcome) -> Checkpoint {
        match outcome {
            CaptureOutcome::Stored { checkpoint, .. } => checkpoint,
            other => panic!("expected a stored checkpoint, got {other:?}"),
        }
    }

    #[test]
    fn enablement_is_explicit_private_and_persistent() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let root = directory.path().join("history");
        let store = CheckpointStore::new(root.clone());

        assert!(!store.is_enabled(&workspace).unwrap());
        assert!(!root.exists());
        store.set_enabled(&workspace, true).unwrap();
        assert!(
            CheckpointStore::new(root.clone())
                .is_enabled(&workspace)
                .unwrap()
        );
        store.set_enabled(&workspace, false).unwrap();
        assert!(!store.is_enabled(&workspace).unwrap());

        #[cfg(unix)]
        {
            let workspace_directory = store.workspace_directory(&workspace);
            let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(&workspace_directory), 0o700);
            assert_eq!(mode(&workspace_directory.join(".lock")), 0o600);
            assert_eq!(mode(&workspace_directory.join("workspace.json")), 0o600);
        }
    }

    #[test]
    fn disabled_capture_does_not_create_history_state() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let document = workspace.join("note.md");
        let root = directory.path().join("history");
        let store = CheckpointStore::new(root.clone());

        let outcome = store
            .capture(&workspace, &document, "draft", CheckpointReason::Manual)
            .unwrap();

        assert_eq!(outcome, CaptureOutcome::Disabled);
        assert!(!root.exists());
        assert!(
            store
                .list(&workspace, &document)
                .unwrap()
                .checkpoints
                .is_empty()
        );
    }

    #[test]
    fn snapshots_round_trip_newest_first_and_deduplicate_adjacent_source() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let document = workspace.join("notes/note.md");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);

        let first = stored(capture_at(&store, &workspace, &document, "first", 10));
        let duplicate = capture_at(&store, &workspace, &document, "first", 11);
        let second = stored(capture_at(&store, &workspace, &document, "second", 12));
        let listed = store.list(&workspace, &document).unwrap();

        assert!(matches!(
            duplicate,
            CaptureOutcome::Duplicate {
                checkpoint_id,
                ..
            } if checkpoint_id == first.id
        ));
        assert_eq!(listed.checkpoints, vec![second.clone(), first.clone()]);
        assert_eq!(
            store.list_all(&workspace).unwrap().checkpoints,
            vec![second, first.clone()]
        );
        assert!(listed.warnings.is_empty());
        assert_eq!(first.document_path, Path::new("notes/note.md"));
        assert_eq!(first.captured_path, Path::new("notes/note.md"));
        assert_eq!(first.digest, hex_digest(b"first"));
        OffsetDateTime::parse(&first.captured_at, &Rfc3339).unwrap();

        let checkpoints_directory = store.workspace_directory(&workspace).join("checkpoints");
        let checkpoint_path = checkpoints_directory.join(format!("{}.json", first.id));
        let bytes = fs::read(&checkpoint_path).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(payload["version"], CHECKPOINT_VERSION);
        assert_eq!(payload["checkpoint"]["source"], "first");
        assert_eq!(payload["checkpoint"]["reason"], "manual");
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(checkpoints_directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(checkpoint_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn identical_non_adjacent_source_remains_a_distinct_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let document = workspace.join("note.md");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);

        stored(capture_at(&store, &workspace, &document, "same", 1));
        stored(capture_at(&store, &workspace, &document, "different", 2));
        let repeated = stored(capture_at(&store, &workspace, &document, "same", 1));

        let listed = store.list(&workspace, &document).unwrap();
        assert_eq!(listed.checkpoints.len(), 3);
        assert!(
            listed
                .checkpoints
                .iter()
                .filter(|checkpoint| checkpoint.source == "same")
                .any(|checkpoint| checkpoint.id != repeated.id)
        );
    }

    #[test]
    fn oversized_snapshot_is_visible_and_writes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let document = workspace.join("note.md");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);
        let source = "x".repeat(usize::try_from(MAX_CHECKPOINT_BYTES).unwrap() + 1);

        let error = store
            .capture(&workspace, &document, &source, CheckpointReason::Manual)
            .unwrap_err();

        assert!(matches!(error, CheckpointError::TooLarge { .. }));
        assert!(
            store
                .list(&workspace, &document)
                .unwrap()
                .checkpoints
                .is_empty()
        );
    }

    #[test]
    fn per_document_retention_prunes_the_oldest_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let document = workspace.join("note.md");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);
        let mut first_id = String::new();

        for index in 0..=MAX_CHECKPOINTS_PER_DOCUMENT {
            let outcome = capture_at(
                &store,
                &workspace,
                &document,
                &format!("version {index}"),
                i64::try_from(index).unwrap() + 1,
            );
            if index == 0 {
                first_id = stored(outcome).id;
            } else if index == MAX_CHECKPOINTS_PER_DOCUMENT {
                assert!(matches!(outcome, CaptureOutcome::Stored { pruned: 1, .. }));
            }
        }

        let listed = store.list(&workspace, &document).unwrap();
        assert_eq!(listed.checkpoints.len(), MAX_CHECKPOINTS_PER_DOCUMENT);
        assert!(listed.checkpoints.iter().all(|entry| entry.id != first_id));
        assert_eq!(listed.checkpoints[0].source, "version 20");
    }

    #[test]
    fn total_retention_selection_is_deterministic_oldest_first() {
        let checkpoint = |id: &str, path: &str, bytes: u64, second: i64| StoredCheckpoint {
            checkpoint: Checkpoint {
                id: id.to_owned(),
                document_path: PathBuf::from(path),
                captured_path: PathBuf::from(path),
                source: String::new(),
                digest: hex_digest(b""),
                captured_at: OffsetDateTime::from_unix_timestamp(second)
                    .unwrap()
                    .format(&Rfc3339)
                    .unwrap(),
                reason: CheckpointReason::Manual,
            },
            path: PathBuf::from(format!("{id}.json")),
            fingerprint: id.to_owned(),
            source_bytes: bytes,
            captured_at: OffsetDateTime::from_unix_timestamp(second).unwrap(),
        };
        let checkpoints = vec![
            checkpoint("a", "one.md", 4, 1),
            checkpoint("b", "two.md", 4, 2),
            checkpoint("c", "one.md", 4, 3),
        ];

        assert_eq!(pruning_candidates(&checkpoints, 20, 8), vec![0]);
        assert_eq!(pruning_candidates(&checkpoints, 1, 100), vec![0]);
        assert_eq!(pruning_candidates(&checkpoints, 1, 4), vec![0, 1]);
    }

    #[test]
    fn clear_scopes_do_not_cross_documents_or_workspaces() {
        let directory = tempfile::tempdir().unwrap();
        let first_workspace = workspace(&directory, "first");
        let second_workspace = workspace(&directory, "second");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &first_workspace);
        enable(&store, &second_workspace);
        let first_a = first_workspace.join("a.md");
        let first_b = first_workspace.join("b.md");
        let second_a = second_workspace.join("a.md");
        stored(capture_at(&store, &first_workspace, &first_a, "first a", 1));
        stored(capture_at(&store, &first_workspace, &first_b, "first b", 2));
        stored(capture_at(
            &store,
            &second_workspace,
            &second_a,
            "second a",
            3,
        ));

        let first_a_ids = store
            .list(&first_workspace, &first_a)
            .unwrap()
            .checkpoints
            .into_iter()
            .map(|checkpoint| checkpoint.id)
            .collect::<Vec<_>>();
        assert!(matches!(
            store.clear_document(&first_workspace, &first_a, &[]),
            Err(CheckpointError::Stale(_))
        ));
        let cleared = store
            .clear_document(&first_workspace, &first_a, &first_a_ids)
            .unwrap();
        assert_eq!(cleared.affected, 1);
        assert!(
            store
                .list(&first_workspace, &first_a)
                .unwrap()
                .checkpoints
                .is_empty()
        );
        assert_eq!(
            store
                .list(&first_workspace, &first_b)
                .unwrap()
                .checkpoints
                .len(),
            1
        );
        assert_eq!(
            store
                .list(&second_workspace, &second_a)
                .unwrap()
                .checkpoints
                .len(),
            1
        );

        let first_workspace_ids = store
            .list_all(&first_workspace)
            .unwrap()
            .checkpoints
            .into_iter()
            .map(|checkpoint| checkpoint.id)
            .collect::<Vec<_>>();
        let cleared = store
            .clear_all(&first_workspace, &first_workspace_ids)
            .unwrap();
        assert_eq!(cleared.affected, 1);
        assert!(
            store
                .list(&first_workspace, &first_b)
                .unwrap()
                .checkpoints
                .is_empty()
        );
        assert_eq!(
            store
                .list(&second_workspace, &second_a)
                .unwrap()
                .checkpoints
                .len(),
            1
        );
        assert!(store.is_enabled(&first_workspace).unwrap());
    }

    #[test]
    fn retarget_updates_current_paths_but_preserves_capture_provenance_and_ids() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);
        let old = workspace.join("old/note.md");
        let new = workspace.join("new/note.md");
        let original = stored(capture_at(&store, &workspace, &old, "source", 1));

        let changed = store
            .retarget_paths(&workspace, &workspace.join("old"), &workspace.join("new"))
            .unwrap();

        assert_eq!(changed.affected, 1);
        assert!(store.list(&workspace, &old).unwrap().checkpoints.is_empty());
        let retargeted = &store.list(&workspace, &new).unwrap().checkpoints[0];
        assert_eq!(retargeted.id, original.id);
        assert_eq!(retargeted.document_path, Path::new("new/note.md"));
        assert_eq!(retargeted.captured_path, Path::new("old/note.md"));
    }

    #[test]
    fn retarget_prunes_the_oldest_when_path_histories_merge() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);
        let old = workspace.join("old.md");
        let new = workspace.join("new.md");
        for index in 0..15 {
            stored(capture_at(
                &store,
                &workspace,
                &old,
                &format!("old {index}"),
                index + 1,
            ));
            stored(capture_at(
                &store,
                &workspace,
                &new,
                &format!("new {index}"),
                index + 16,
            ));
        }

        store.retarget_paths(&workspace, &old, &new).unwrap();

        assert!(store.list(&workspace, &old).unwrap().checkpoints.is_empty());
        let merged = store.list(&workspace, &new).unwrap().checkpoints;
        assert_eq!(merged.len(), MAX_CHECKPOINTS_PER_DOCUMENT);
        assert!(
            merged
                .iter()
                .any(|checkpoint| checkpoint.source == "old 10")
        );
        assert!(merged.iter().all(|checkpoint| checkpoint.source != "old 9"));
    }

    #[test]
    fn corruption_is_preserved_warned_and_does_not_hide_valid_history() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let document = workspace.join("note.md");
        let store = CheckpointStore::new(directory.path().join("history"));
        enable(&store, &workspace);
        let valid = stored(capture_at(&store, &workspace, &document, "valid", 1));
        let checkpoints_directory = store.workspace_directory(&workspace).join("checkpoints");
        let corrupt = checkpoints_directory.join("broken.json");
        fs::write(&corrupt, b"not json").unwrap();
        let valid_path = checkpoints_directory.join(format!("{}.json", valid.id));
        let mut payload: serde_json::Value =
            serde_json::from_slice(&fs::read(&valid_path).unwrap()).unwrap();
        payload["checkpoint"]["source"] = serde_json::Value::String("tampered".to_owned());
        let tampered_id = "a".repeat(64);
        payload["checkpoint"]["id"] = serde_json::Value::String(tampered_id.clone());
        let tampered = checkpoints_directory.join(format!("{tampered_id}.json"));
        fs::write(&tampered, serde_json::to_vec(&payload).unwrap()).unwrap();

        let listed = store.list(&workspace, &document).unwrap();

        assert_eq!(listed.checkpoints.len(), 1);
        assert_eq!(listed.warnings.len(), 2);
        assert!(
            listed
                .warnings
                .iter()
                .all(|warning| warning.kind == CheckpointWarningKind::Corrupt)
        );
        assert_eq!(fs::read(&corrupt).unwrap(), b"not json");
        let confirmed_ids = listed
            .checkpoints
            .iter()
            .map(|checkpoint| checkpoint.id.clone())
            .collect::<Vec<_>>();
        let cleared = store
            .clear_document(&workspace, &document, &confirmed_ids)
            .unwrap();
        assert_eq!(cleared.affected, 1);
        assert_eq!(fs::read(&corrupt).unwrap(), b"not json");
        assert!(tampered.exists());
        let cleared = store.clear_all(&workspace, &[]).unwrap();
        assert_eq!(cleared.affected, 0);
        assert_eq!(cleared.warnings.len(), 2);
        assert_eq!(fs::read(&corrupt).unwrap(), b"not json");
        assert!(tampered.exists());
    }

    #[test]
    fn stale_fingerprint_blocks_deletion_before_any_file_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.json");
        let second = directory.path().join("second.json");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();
        let files = vec![
            StoredFile {
                path: first.clone(),
                fingerprint: hex_digest(b"first"),
            },
            StoredFile {
                path: second.clone(),
                fingerprint: hex_digest(b"second"),
            },
        ];
        fs::write(&second, b"changed").unwrap();

        let error = remove_verified_files(&files).unwrap_err();

        assert!(matches!(error, CheckpointError::Stale(path) if path == second));
        assert!(first.exists());
        assert!(second.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_substitution_is_rejected_without_following_it() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let workspace = workspace(&directory, "workspace");
        let root = directory.path().join("history");
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &root).unwrap();
        let store = CheckpointStore::new(root);

        let error = store.set_enabled(&workspace, true).unwrap_err();

        assert!(matches!(error, CheckpointError::Invalid(_)));
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
}
