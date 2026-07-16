//! Minimal crash-recovery journal compatible with `TermDraft`'s v2 JSON shape.

use std::env;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use directories::BaseDirs;
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

use crate::document::{Encoding, FileSnapshot};
use crate::workspace::has_editable_suffix;

const MAX_RECOVERY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct RecoveryEntry {
    pub document_path: PathBuf,
    pub workspace_root: PathBuf,
    pub text: String,
    pub encoding: Encoding,
    base_snapshot: SnapshotFile,
    fingerprint: String,
}

impl RecoveryEntry {
    #[must_use]
    pub fn baseline_matches(&self, current: &FileSnapshot) -> bool {
        self.base_snapshot.digest.as_deref() == Some(&hex_digest(&current.sha256))
            && self.base_snapshot.device == Some(current.device)
            && self.base_snapshot.inode == Some(current.inode)
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

#[derive(Clone, Debug)]
pub struct RecoveryJournal {
    root: PathBuf,
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("cannot resolve the recovery directory")]
    MissingHome,
    #[error("invalid recovery journal: {0}")]
    Invalid(String),
    #[error("recovery journal exceeds {MAX_RECOVERY_BYTES} bytes")]
    TooLarge,
    #[error("cannot access recovery journal: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RecoveryFile {
    version: u8,
    document_path: String,
    workspace_root: String,
    text: String,
    encoding: String,
    base_snapshot: SnapshotFile,
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotFile {
    exists: bool,
    digest: Option<String>,
    size: Option<u64>,
    mtime_ns: Option<u128>,
    ctime_ns: Option<u128>,
    mode: Option<u32>,
    device: Option<u64>,
    inode: Option<u64>,
    parent_device: Option<u64>,
    parent_inode: Option<u64>,
}

impl RecoveryJournal {
    /// Create a journal in the platform's canonical `TermDraft` state directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the platform has no resolvable home/state directory.
    pub fn platform_default() -> Result<Self, RecoveryError> {
        Ok(Self::new(default_recovery_root()?))
    }

    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn path_for(&self, document_path: &Path) -> PathBuf {
        let mut digest = Sha256::new();
        #[cfg(unix)]
        digest.update(document_path.as_os_str().as_bytes());
        #[cfg(not(unix))]
        digest.update(document_path.to_string_lossy().as_bytes());
        self.root.join(format!("{:x}.json", digest.finalize()))
    }

    /// Publish a private atomic journal for the current dirty source.
    ///
    /// # Errors
    ///
    /// Returns an error when paths are invalid or the journal cannot be stored.
    pub fn publish(
        &self,
        document_path: &Path,
        workspace_root: &Path,
        text: &str,
        encoding: Encoding,
        snapshot: &FileSnapshot,
    ) -> Result<RecoveryEntry, RecoveryError> {
        validate_paths(document_path, workspace_root)?;
        let mut entry = RecoveryEntry {
            document_path: document_path.to_path_buf(),
            workspace_root: workspace_root.to_path_buf(),
            text: text.to_owned(),
            encoding,
            base_snapshot: snapshot_file(snapshot),
            fingerprint: String::new(),
        };
        let file = RecoveryFile {
            version: 2,
            document_path: path_string(document_path)?,
            workspace_root: path_string(workspace_root)?,
            text: text.to_owned(),
            encoding: encoding_name(encoding).to_owned(),
            base_snapshot: entry.base_snapshot.clone(),
            updated_at: OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|error| RecoveryError::Invalid(error.to_string()))?,
        };
        let mut bytes =
            serde_json::to_vec(&file).map_err(|error| RecoveryError::Invalid(error.to_string()))?;
        bytes.push(b'\n');
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECOVERY_BYTES {
            return Err(RecoveryError::TooLarge);
        }
        entry.fingerprint = data_fingerprint(&bytes);
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
            .persist(self.path_for(document_path))
            .map_err(|error| RecoveryError::Io(error.error))?;
        File::open(&self.root)?.sync_all()?;
        Ok(entry)
    }

    /// Load and validate one exact document journal.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe, corrupt, oversized, or mismatched journal.
    pub fn load(&self, document_path: &Path) -> Result<Option<RecoveryEntry>, RecoveryError> {
        let path = self.path_for(document_path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(RecoveryError::Invalid(
                "journal path is not a regular file".to_owned(),
            ));
        }
        if metadata.len() > MAX_RECOVERY_BYTES {
            return Err(RecoveryError::TooLarge);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW);
        let mut bytes = Vec::with_capacity(metadata.len().try_into().unwrap_or(0));
        options
            .open(path)?
            .take(MAX_RECOVERY_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECOVERY_BYTES {
            return Err(RecoveryError::TooLarge);
        }
        let file: RecoveryFile = serde_json::from_slice(&bytes)
            .map_err(|error| RecoveryError::Invalid(error.to_string()))?;
        if file.version != 2 {
            return Err(RecoveryError::Invalid(
                "unsupported journal version".to_owned(),
            ));
        }
        let stored_path = PathBuf::from(&file.document_path);
        let workspace_root = PathBuf::from(&file.workspace_root);
        if stored_path != document_path {
            return Err(RecoveryError::Invalid(
                "journal belongs to another document".to_owned(),
            ));
        }
        validate_paths(&stored_path, &workspace_root)?;
        validate_snapshot(&file.base_snapshot)?;
        let encoding = match file.encoding.as_str() {
            "utf-8" => Encoding::Utf8,
            "utf-8-sig" => Encoding::Utf8Bom,
            _ => {
                return Err(RecoveryError::Invalid(
                    "unsupported recovery encoding".to_owned(),
                ));
            }
        };
        Ok(Some(RecoveryEntry {
            document_path: stored_path,
            workspace_root,
            text: file.text,
            encoding,
            base_snapshot: file.base_snapshot,
            fingerprint: data_fingerprint(&bytes),
        }))
    }

    /// Remove one journal after a save or explicit discard.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing journal cannot be removed durably.
    pub fn discard(
        &self,
        document_path: &Path,
        expected_fingerprint: Option<&str>,
    ) -> Result<(), RecoveryError> {
        if let Some(expected) = expected_fingerprint {
            let bytes = match fs::read(self.path_for(document_path)) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            if data_fingerprint(&bytes) != expected {
                return Err(RecoveryError::Invalid(
                    "journal changed before it could be removed".to_owned(),
                ));
            }
        }
        match fs::remove_file(self.path_for(document_path)) {
            Ok(()) => File::open(&self.root)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

/// Resolve the preferred recovery directory, retaining the pre-1.0 fallback.
///
/// # Errors
///
/// Returns an error when no platform state/home directory can be resolved.
pub fn default_recovery_root() -> Result<PathBuf, RecoveryError> {
    let (canonical, legacy) = if let Some(root) = env::var_os("XDG_STATE_HOME") {
        let root = PathBuf::from(root);
        (
            root.join("termdraft/recovery"),
            root.join("termwriter/recovery"),
        )
    } else {
        let base = BaseDirs::new().ok_or(RecoveryError::MissingHome)?;
        #[cfg(target_os = "macos")]
        let root = base.home_dir().join("Library/Application Support");
        #[cfg(not(target_os = "macos"))]
        let root = base.home_dir().join(".local/state");
        #[cfg(target_os = "macos")]
        let names = ("TermDraft/recovery", "TermWriter/recovery");
        #[cfg(not(target_os = "macos"))]
        let names = ("termdraft/recovery", "termwriter/recovery");
        (root.join(names.0), root.join(names.1))
    };
    if canonical.exists() || !legacy.exists() {
        Ok(canonical)
    } else {
        Ok(legacy)
    }
}

fn snapshot_file(snapshot: &FileSnapshot) -> SnapshotFile {
    SnapshotFile {
        exists: true,
        digest: Some(hex_digest(&snapshot.sha256)),
        size: Some(snapshot.size),
        mtime_ns: Some(snapshot.modified_ns),
        ctime_ns: None,
        mode: Some(snapshot.mode),
        device: Some(snapshot.device),
        inode: Some(snapshot.inode),
        parent_device: None,
        parent_inode: None,
    }
}

fn validate_snapshot(snapshot: &SnapshotFile) -> Result<(), RecoveryError> {
    if !snapshot.exists
        || snapshot.digest.as_ref().is_none_or(|digest| {
            digest.len() != 64
                || digest
                    .bytes()
                    .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        })
    {
        return Err(RecoveryError::Invalid(
            "invalid recovery baseline digest".to_owned(),
        ));
    }
    Ok(())
}

fn validate_paths(document_path: &Path, workspace_root: &Path) -> Result<(), RecoveryError> {
    if !document_path.is_absolute()
        || !workspace_root.is_absolute()
        || !document_path.starts_with(workspace_root)
        || !has_editable_suffix(document_path)
    {
        return Err(RecoveryError::Invalid(
            "document is outside its workspace".to_owned(),
        ));
    }
    Ok(())
}

fn encoding_name(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "utf-8",
        Encoding::Utf8Bom => "utf-8-sig",
    }
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in bytes {
        write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    digest
}

fn data_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut fingerprint = String::with_capacity(64);
    for byte in digest {
        write!(fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
    }
    fingerprint
}

fn path_string(path: &Path) -> Result<String, RecoveryError> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| RecoveryError::Invalid("path is not UTF-8".to_owned()))
}

fn secure_directory(path: &Path) -> Result<(), RecoveryError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::load_file;

    #[test]
    fn round_trip_uses_python_v2_shape_and_private_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let document = root.join("note.md");
        fs::write(&document, "saved").unwrap();
        let loaded = load_file(&document).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));

        journal
            .publish(
                &document,
                &root,
                "unsaved",
                loaded.encoding,
                &loaded.snapshot,
            )
            .unwrap();
        let entry = journal.load(&document).unwrap().unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&fs::read(journal.path_for(&document)).unwrap()).unwrap();

        assert_eq!(entry.text, "unsaved");
        assert!(entry.baseline_matches(&loaded.snapshot));
        assert_eq!(payload["version"], 2);
        assert_eq!(
            payload["base_snapshot"]["digest"].as_str().unwrap().len(),
            64
        );
        journal
            .discard(&document, Some(entry.fingerprint()))
            .unwrap();
        assert!(!journal.path_for(&document).exists());
    }

    #[test]
    fn conditional_discard_preserves_a_newer_draft() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let document = root.join("note.md");
        fs::write(&document, "saved").unwrap();
        let loaded = load_file(&document).unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        let first = journal
            .publish(&document, &root, "first", loaded.encoding, &loaded.snapshot)
            .unwrap();
        journal
            .publish(&document, &root, "newer", loaded.encoding, &loaded.snapshot)
            .unwrap();

        assert!(
            journal
                .discard(&document, Some(first.fingerprint()))
                .is_err()
        );
        assert_eq!(journal.load(&document).unwrap().unwrap().text, "newer");
    }
}
