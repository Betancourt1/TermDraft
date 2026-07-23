//! Crash-recovery journal storage compatible with `TermDraft`'s JSON journals.

use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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

use crate::document::{Encoding, FileSnapshot};
use crate::workspace::has_editable_suffix;

const MAX_RECOVERY_BYTES: u64 = 16 * 1024 * 1024;
const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryEntry {
    pub document_path: PathBuf,
    pub workspace_root: PathBuf,
    pub text: String,
    pub encoding: Encoding,
    pub updated_at: OffsetDateTime,
    base_snapshot: SnapshotFile,
    fingerprint: String,
}

impl RecoveryEntry {
    #[must_use]
    pub fn baseline_matches(&self, current: &FileSnapshot) -> bool {
        self.base_snapshot.exists
            && self.base_snapshot.digest.as_deref() == Some(&hex_digest(&current.sha256))
            && self.base_snapshot.device == Some(current.device)
            && self.base_snapshot.inode == Some(current.inode)
    }

    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    #[must_use]
    pub fn baseline_snapshot(&self) -> FileSnapshot {
        let mut sha256 = [0; 32];
        if let Some(digest) = self.base_snapshot.digest.as_deref() {
            for (index, pair) in digest.as_bytes().chunks_exact(2).enumerate() {
                let high = hex_value(pair[0]).unwrap_or_default();
                let low = hex_value(pair[1]).unwrap_or_default();
                sha256[index] = (high << 4) | low;
            }
        }
        FileSnapshot {
            sha256,
            size: self.base_snapshot.size.unwrap_or_default(),
            modified_ns: self.base_snapshot.mtime_ns.unwrap_or_default(),
            mode: self.base_snapshot.mode.unwrap_or(0o600),
            device: self.base_snapshot.device.unwrap_or_default(),
            inode: self.base_snapshot.inode.unwrap_or_default(),
        }
    }
}

/// The safety state of an inventoried recovery journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryRecordStatus {
    /// The journal and its Markdown source are available.
    Valid,
    /// The journal is valid, but its Markdown source no longer exists.
    Missing,
    /// The journal is valid, but its source is not a safe regular file.
    Orphan,
    /// The journal bytes or stored paths could not be trusted.
    Corrupt,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRecord {
    pub journal_path: PathBuf,
    pub fingerprint: String,
    pub entry: Option<RecoveryEntry>,
    pub error: Option<String>,
    pub quarantined: bool,
    pub has_content_fingerprint: bool,
    pub status: RecoveryRecordStatus,
}

impl RecoveryRecord {
    #[must_use]
    pub const fn is_corrupt(&self) -> bool {
        matches!(self.status, RecoveryRecordStatus::Corrupt)
    }

    #[must_use]
    pub const fn is_orphan(&self) -> bool {
        matches!(
            self.status,
            RecoveryRecordStatus::Missing | RecoveryRecordStatus::Orphan
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRetentionOutcome {
    pub journal_path: PathBuf,
    pub document_path: PathBuf,
    pub updated_at: OffsetDateTime,
    pub deleted: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRetentionResult {
    pub cutoff: OffsetDateTime,
    pub outcomes: Vec<RecoveryRetentionOutcome>,
}

impl RecoveryRetentionResult {
    #[must_use]
    pub fn selected_count(&self) -> usize {
        self.outcomes.len()
    }

    #[must_use]
    pub fn deleted_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.deleted)
            .count()
    }

    #[must_use]
    pub fn failed_count(&self) -> usize {
        self.selected_count() - self.deleted_count()
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_snapshot: Option<SnapshotFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_digest: Option<String>,
    updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
        let updated_at = OffsetDateTime::now_utc();
        let base_snapshot = snapshot_file(snapshot);
        let file = RecoveryFile {
            version: 2,
            document_path: path_string(document_path)?,
            workspace_root: path_string(workspace_root)?,
            text: text.to_owned(),
            encoding: encoding_name(encoding).to_owned(),
            base_snapshot: Some(base_snapshot.clone()),
            base_digest: None,
            updated_at: updated_at
                .format(&Rfc3339)
                .map_err(|error| RecoveryError::Invalid(error.to_string()))?,
        };
        let bytes = serialize_file(&file)?;
        let fingerprint = data_fingerprint(&bytes);
        let destination = self.path_for(document_path);
        let _lock = JournalLock::acquire(&self.root, &destination)?;
        self.publish_bytes(&destination, &bytes)?;
        Ok(RecoveryEntry {
            document_path: document_path.to_path_buf(),
            workspace_root: workspace_root.to_path_buf(),
            text: text.to_owned(),
            encoding,
            updated_at,
            base_snapshot,
            fingerprint,
        })
    }

    /// Load and validate one exact document journal.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsafe, corrupt, oversized, or mismatched journal.
    pub fn load(&self, document_path: &Path) -> Result<Option<RecoveryEntry>, RecoveryError> {
        let path = self.path_for(document_path);
        if is_missing(&path)? {
            return Ok(None);
        }
        let _lock = JournalLock::acquire(&self.root, &path)?;
        let Some(record) = self.inspect_if_present(&path, false)? else {
            return Ok(None);
        };
        let entry = record.entry.ok_or_else(|| {
            RecoveryError::Invalid(
                record
                    .error
                    .unwrap_or_else(|| "journal could not be validated".to_owned()),
            )
        })?;
        if entry.document_path != document_path {
            return Err(RecoveryError::Invalid(
                "journal belongs to another document".to_owned(),
            ));
        }
        Ok(Some(entry))
    }

    /// Return the exact current record for one document.
    ///
    /// # Errors
    ///
    /// Returns an error when an existing journal cannot be trusted.
    pub fn record_for(
        &self,
        document_path: &Path,
    ) -> Result<Option<RecoveryRecord>, RecoveryError> {
        let path = self.path_for(document_path);
        if is_missing(&path)? {
            return Ok(None);
        }
        let _lock = JournalLock::acquire(&self.root, &path)?;
        let Some(record) = self.inspect_if_present(&path, false)? else {
            return Ok(None);
        };
        if record.entry.is_none() {
            return Err(RecoveryError::Invalid(
                record
                    .error
                    .unwrap_or_else(|| "journal could not be validated".to_owned()),
            ));
        }
        Ok(Some(record))
    }

    /// Inventory active journals, retaining corrupt entries when filtering a workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when the recovery directory cannot be safely scanned.
    pub fn list_entries(
        &self,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<RecoveryRecord>, RecoveryError> {
        self.list_directory(&self.root, workspace_root, false)
    }

    /// Inventory quarantined journals, retaining corrupt entries when filtering a workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when quarantine cannot be safely scanned.
    pub fn list_quarantined(
        &self,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<RecoveryRecord>, RecoveryError> {
        let Some(root) = self.quarantine_root(false)? else {
            return Ok(Vec::new());
        };
        self.list_directory(&root, workspace_root, true)
    }

    /// Inventory both active and quarantined journals.
    ///
    /// # Errors
    ///
    /// Returns an error when either storage directory cannot be safely scanned.
    pub fn inventory(
        &self,
        workspace_root: Option<&Path>,
    ) -> Result<Vec<RecoveryRecord>, RecoveryError> {
        let mut records = self.list_entries(workspace_root)?;
        records.extend(self.list_quarantined(workspace_root)?);
        Ok(records)
    }

    /// Move an unchanged active draft to another document identity without replacing a draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the record changed, is quarantined or corrupt, the target is unsafe,
    /// or another recovery entry already owns the destination.
    pub fn retarget(
        &self,
        record: &RecoveryRecord,
        document_path: &Path,
        workspace_root: &Path,
    ) -> Result<RecoveryEntry, RecoveryError> {
        if record.quarantined {
            return Err(RecoveryError::Invalid(
                "cannot retarget a quarantined recovery entry".to_owned(),
            ));
        }
        self.validate_record_location(record)?;
        validate_paths(document_path, workspace_root)?;
        let destination = self.path_for(document_path);
        let _lock = JournalLock::acquire_many(
            &self.root,
            &[record.journal_path.as_path(), destination.as_path()],
        )?;
        let current = self.verify_record(record)?;
        let current_entry = current.entry.ok_or_else(|| {
            RecoveryError::Invalid("cannot retarget a corrupt recovery entry".to_owned())
        })?;

        if destination == record.journal_path {
            if current_entry.document_path == document_path
                && current_entry.workspace_root == workspace_root
            {
                return Ok(current_entry);
            }
            return Err(RecoveryError::Invalid(
                "retarget destination already contains this recovery entry".to_owned(),
            ));
        }

        let mut entry = RecoveryEntry {
            document_path: document_path.to_path_buf(),
            workspace_root: workspace_root.to_path_buf(),
            text: current_entry.text,
            encoding: current_entry.encoding,
            updated_at: current_entry.updated_at,
            base_snapshot: current_entry.base_snapshot,
            fingerprint: String::new(),
        };
        let bytes = serialize_entry(&entry)?;
        entry.fingerprint = data_fingerprint(&bytes);
        persist_new_bytes(&destination, &bytes, "recovery entry already exists")?;
        self.verify_record(record)?;
        remove_and_sync(&record.journal_path)?;
        Ok(entry)
    }

    /// Restore an unchanged trusted quarantine entry without replacing an active draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the record changed, is active or corrupt, or an active journal
    /// already exists for the document.
    pub fn restore_quarantined(
        &self,
        record: &RecoveryRecord,
    ) -> Result<RecoveryEntry, RecoveryError> {
        if !record.quarantined {
            return Err(RecoveryError::Invalid(
                "recovery entry is not quarantined".to_owned(),
            ));
        }
        self.validate_record_location(record)?;
        let _lock = JournalLock::acquire(&self.root, &record.journal_path)?;
        let current = self.verify_record(record)?;
        let current_entry = current.entry.ok_or_else(|| {
            RecoveryError::Invalid("cannot restore a corrupt recovery entry".to_owned())
        })?;
        let destination = self.path_for(&current_entry.document_path);

        fs::hard_link(&record.journal_path, &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RecoveryError::Invalid(format!(
                    "an active recovery entry already exists for {}",
                    destination.display()
                ))
            } else {
                RecoveryError::Io(error)
            }
        })?;
        sync_directory(&self.root)?;
        self.verify_record(record)?;
        remove_and_sync(&record.journal_path)?;
        Ok(current_entry)
    }

    /// Export an unchanged trusted quarantine entry without replacing a document or deleting the
    /// archive.
    ///
    /// # Errors
    ///
    /// Returns an error when the record changed, is active or corrupt, the destination is unsafe,
    /// or any entry already exists at the destination.
    pub fn export_quarantined(
        &self,
        record: &RecoveryRecord,
        destination: &Path,
    ) -> Result<FileSnapshot, RecoveryError> {
        if !record.quarantined {
            return Err(RecoveryError::Invalid(
                "recovery entry is not quarantined".to_owned(),
            ));
        }
        self.validate_record_location(record)?;
        let _lock = JournalLock::acquire(&self.root, &record.journal_path)?;
        let current = self.verify_record(record)?;
        let entry = current.entry.ok_or_else(|| {
            RecoveryError::Invalid("cannot export a corrupt recovery entry".to_owned())
        })?;
        validate_export_destination(destination, &entry.workspace_root)?;
        let bytes = encode_entry_text(&entry);
        self.verify_record(record)?;
        persist_new_bytes(
            destination,
            &bytes,
            "recovery export destination already exists",
        )
    }

    /// Move an unchanged active journal into quarantine without changing its bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the record changed, is misplaced, or would replace an archive.
    pub fn quarantine(&self, record: &RecoveryRecord) -> Result<PathBuf, RecoveryError> {
        if record.quarantined {
            return Err(RecoveryError::Invalid(
                "recovery entry is already quarantined".to_owned(),
            ));
        }
        self.validate_record_location(record)?;
        let _lock = JournalLock::acquire(&self.root, &record.journal_path)?;
        self.verify_record(record)?;
        let quarantine_root = self.quarantine_root(true)?.ok_or_else(|| {
            RecoveryError::Invalid("recovery quarantine was not created".to_owned())
        })?;
        let destination = quarantine_root.join(
            record
                .journal_path
                .file_name()
                .ok_or_else(|| RecoveryError::Invalid("journal has no filename".to_owned()))?,
        );
        fs::hard_link(&record.journal_path, &destination).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RecoveryError::Invalid(format!(
                    "recovery quarantine already contains {}",
                    destination.display()
                ))
            } else {
                RecoveryError::Io(error)
            }
        })?;
        sync_directory(&quarantine_root)?;
        self.verify_record(record)?;
        remove_and_sync(&record.journal_path)?;
        Ok(destination)
    }

    /// Permanently delete the exact quarantined record that was inventoried.
    ///
    /// # Errors
    ///
    /// Returns an error if the record moved, changed, or could not be fingerprinted.
    pub fn delete_quarantined(&self, record: &RecoveryRecord) -> Result<(), RecoveryError> {
        if !record.quarantined {
            return Err(RecoveryError::Invalid(
                "recovery entry is not quarantined".to_owned(),
            ));
        }
        self.validate_record_location(record)?;
        let _lock = JournalLock::acquire(&self.root, &record.journal_path)?;
        let current = self.verify_record(record)?;
        if !record.has_content_fingerprint || !current.has_content_fingerprint {
            return Err(RecoveryError::Invalid(
                "cannot permanently delete recovery bytes that were not fingerprinted".to_owned(),
            ));
        }
        remove_and_sync(&record.journal_path)
    }

    /// Delete valid old quarantine records, reporting each result independently.
    ///
    /// Passing `records` limits deletion to an inventory already confirmed by the caller.
    ///
    /// # Errors
    ///
    /// Returns an error only when the initial inventory cannot be read. Individual deletion
    /// failures are returned as outcomes and do not stop the cleanup.
    pub fn cleanup_quarantined(
        &self,
        before: OffsetDateTime,
        workspace_root: Option<&Path>,
        records: Option<&[RecoveryRecord]>,
    ) -> Result<RecoveryRetentionResult, RecoveryError> {
        let owned;
        let inventory = if let Some(records) = records {
            records
        } else {
            owned = self.list_quarantined(workspace_root)?;
            &owned
        };
        let selected = inventory
            .iter()
            .filter_map(|record| {
                record.entry.as_ref().and_then(|entry| {
                    (record.quarantined
                        && entry.updated_at < before
                        && workspace_root.is_none_or(|root| entry.workspace_root.as_path() == root))
                    .then(|| (record.clone(), entry.clone()))
                })
            })
            .collect::<Vec<_>>();
        let outcomes = selected
            .iter()
            .map(|(record, entry)| match self.delete_quarantined(record) {
                Ok(()) => RecoveryRetentionOutcome {
                    journal_path: record.journal_path.clone(),
                    document_path: entry.document_path.clone(),
                    updated_at: entry.updated_at,
                    deleted: true,
                    error: None,
                },
                Err(error) => RecoveryRetentionOutcome {
                    journal_path: record.journal_path.clone(),
                    document_path: entry.document_path.clone(),
                    updated_at: entry.updated_at,
                    deleted: false,
                    error: Some(error.to_string()),
                },
            })
            .collect();
        Ok(RecoveryRetentionResult {
            cutoff: before,
            outcomes,
        })
    }

    /// Remove one journal only if its exact fingerprint is still current.
    ///
    /// `None` confirms expected absence; it never authorizes deleting a journal that appeared
    /// after the caller checked.
    ///
    /// # Errors
    ///
    /// Returns an error when a journal appeared or changed before deletion.
    pub fn discard(
        &self,
        document_path: &Path,
        expected_fingerprint: Option<&str>,
    ) -> Result<(), RecoveryError> {
        let path = self.path_for(document_path);
        let _lock = JournalLock::acquire(&self.root, &path)?;
        let Some(record) = self.inspect_if_present(&path, false)? else {
            return Ok(());
        };
        let Some(expected) = expected_fingerprint else {
            return Err(RecoveryError::Invalid(
                "journal appeared after cleanup was requested".to_owned(),
            ));
        };
        if record.fingerprint != expected {
            return Err(RecoveryError::Invalid(
                "journal changed before it could be removed".to_owned(),
            ));
        }
        remove_and_sync(&path)
    }

    fn list_directory(
        &self,
        root: &Path,
        workspace_root: Option<&Path>,
        quarantined: bool,
    ) -> Result<Vec<RecoveryRecord>, RecoveryError> {
        if root == self.root && is_missing(root)? {
            return Ok(Vec::new());
        }
        validate_real_directory(root)?;
        let mut paths = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                paths.push(path);
            }
        }
        paths.sort();
        let records = paths
            .iter()
            .map(|path| self.inspect(path, quarantined))
            .filter(|record| {
                record.entry.as_ref().is_none_or(|entry| {
                    workspace_root.is_none_or(|root| entry.workspace_root.as_path() == root)
                })
            })
            .collect();
        Ok(records)
    }

    fn inspect_if_present(
        &self,
        path: &Path,
        quarantined: bool,
    ) -> Result<Option<RecoveryRecord>, RecoveryError> {
        if is_missing(path)? {
            return Ok(None);
        }
        Ok(Some(self.inspect(path, quarantined)))
    }

    fn inspect(&self, path: &Path, quarantined: bool) -> RecoveryRecord {
        let bytes = match read_regular_bytes(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return RecoveryRecord {
                    journal_path: path.to_path_buf(),
                    fingerprint: path_fingerprint(path),
                    entry: None,
                    error: Some(error.to_string()),
                    quarantined,
                    has_content_fingerprint: false,
                    status: RecoveryRecordStatus::Corrupt,
                };
            }
        };
        let fingerprint = data_fingerprint(&bytes);
        match entry_from_bytes(&bytes, &fingerprint) {
            Ok(entry) => {
                let expected = self.path_for(&entry.document_path);
                let filename_matches = if quarantined {
                    expected.file_name() == path.file_name()
                } else {
                    expected == path
                };
                if !filename_matches {
                    return RecoveryRecord {
                        journal_path: path.to_path_buf(),
                        fingerprint,
                        entry: None,
                        error: Some(
                            "recovery journal filename does not match its document".to_owned(),
                        ),
                        quarantined,
                        has_content_fingerprint: true,
                        status: RecoveryRecordStatus::Corrupt,
                    };
                }
                RecoveryRecord {
                    journal_path: path.to_path_buf(),
                    fingerprint,
                    status: source_status(&entry.document_path),
                    entry: Some(entry),
                    error: None,
                    quarantined,
                    has_content_fingerprint: true,
                }
            }
            Err(error) => RecoveryRecord {
                journal_path: path.to_path_buf(),
                fingerprint,
                entry: None,
                error: Some(error.to_string()),
                quarantined,
                has_content_fingerprint: true,
                status: RecoveryRecordStatus::Corrupt,
            },
        }
    }

    fn verify_record(&self, record: &RecoveryRecord) -> Result<RecoveryRecord, RecoveryError> {
        self.validate_record_location(record)?;
        let current = self.inspect(&record.journal_path, record.quarantined);
        if current.fingerprint != record.fingerprint {
            return Err(RecoveryError::Invalid(
                "recovery entry changed after it was listed".to_owned(),
            ));
        }
        Ok(current)
    }

    fn validate_record_location(&self, record: &RecoveryRecord) -> Result<(), RecoveryError> {
        let expected_parent = if record.quarantined {
            self.root.join("quarantine")
        } else {
            self.root.clone()
        };
        if record.journal_path.parent() != Some(expected_parent.as_path()) {
            return Err(RecoveryError::Invalid(
                "recovery entry is outside its storage directory".to_owned(),
            ));
        }
        if record.quarantined {
            self.quarantine_root(false)?;
        }
        Ok(())
    }

    fn quarantine_root(&self, create: bool) -> Result<Option<PathBuf>, RecoveryError> {
        let root = self.root.join("quarantine");
        if create {
            ensure_directory(&root)?;
        } else if is_missing(&root)? {
            return Ok(None);
        }
        validate_real_directory(&root)?;
        Ok(Some(root))
    }

    fn publish_bytes(&self, destination: &Path, bytes: &[u8]) -> Result<(), RecoveryError> {
        ensure_directory(&self.root)?;
        let mut temporary = NamedTempFile::new_in(&self.root)?;
        #[cfg(unix)]
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temporary.write_all(bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(destination)
            .map_err(|error| RecoveryError::Io(error.error))?;
        sync_directory(&self.root)
    }
}

struct JournalLock {
    _files: Vec<File>,
}

impl JournalLock {
    fn acquire(root: &Path, journal_path: &Path) -> Result<Self, RecoveryError> {
        Self::acquire_many(root, &[journal_path])
    }

    fn acquire_many(root: &Path, journal_paths: &[&Path]) -> Result<Self, RecoveryError> {
        ensure_directory(root)?;
        let mut lock_paths = journal_paths
            .iter()
            .map(|journal_path| {
                let filename = journal_path
                    .file_name()
                    .ok_or_else(|| RecoveryError::Invalid("journal has no filename".to_owned()))?;
                Ok(root.join(format!(".{}.lock", filename.to_string_lossy())))
            })
            .collect::<Result<Vec<_>, RecoveryError>>()?;
        lock_paths.sort();
        lock_paths.dedup();

        let mut files = Vec::with_capacity(lock_paths.len());
        for lock_path in lock_paths {
            let mut options = OpenOptions::new();
            options.create(true).read(true).write(true);
            #[cfg(unix)]
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            let file = options.open(&lock_path)?;
            if !file.metadata()?.is_file() {
                return Err(RecoveryError::Invalid(
                    "journal lock is not a regular file".to_owned(),
                ));
            }
            #[cfg(unix)]
            flock(&file, FlockOperation::LockExclusive).map_err(std::io::Error::from)?;
            files.push(file);
        }
        Ok(Self { _files: files })
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

fn serialize_file(file: &RecoveryFile) -> Result<Vec<u8>, RecoveryError> {
    let mut bytes =
        serde_json::to_vec(file).map_err(|error| RecoveryError::Invalid(error.to_string()))?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RECOVERY_BYTES {
        return Err(RecoveryError::TooLarge);
    }
    Ok(bytes)
}

fn serialize_entry(entry: &RecoveryEntry) -> Result<Vec<u8>, RecoveryError> {
    validate_paths(&entry.document_path, &entry.workspace_root)?;
    validate_snapshot(&entry.base_snapshot)?;
    serialize_file(&RecoveryFile {
        version: 2,
        document_path: path_string(&entry.document_path)?,
        workspace_root: path_string(&entry.workspace_root)?,
        text: entry.text.clone(),
        encoding: encoding_name(entry.encoding).to_owned(),
        base_snapshot: Some(entry.base_snapshot.clone()),
        base_digest: None,
        updated_at: entry
            .updated_at
            .format(&Rfc3339)
            .map_err(|error| RecoveryError::Invalid(error.to_string()))?,
    })
}

fn entry_from_bytes(bytes: &[u8], fingerprint: &str) -> Result<RecoveryEntry, RecoveryError> {
    let file: RecoveryFile = serde_json::from_slice(bytes)
        .map_err(|error| RecoveryError::Invalid(format!("invalid JSON: {error}")))?;
    let base_snapshot = match file.version {
        1 => SnapshotFile {
            exists: file.base_digest.is_some(),
            digest: file.base_digest,
            size: None,
            mtime_ns: None,
            ctime_ns: None,
            mode: None,
            device: None,
            inode: None,
            parent_device: None,
            parent_inode: None,
        },
        2 => file.base_snapshot.ok_or_else(|| {
            RecoveryError::Invalid("missing recovery baseline snapshot".to_owned())
        })?,
        _ => {
            return Err(RecoveryError::Invalid(
                "unsupported journal version".to_owned(),
            ));
        }
    };
    validate_snapshot(&base_snapshot)?;
    let document_path = PathBuf::from(file.document_path);
    let workspace_root = PathBuf::from(file.workspace_root);
    validate_paths(&document_path, &workspace_root)?;
    let encoding = match file.encoding.as_str() {
        "utf-8" => Encoding::Utf8,
        "utf-8-sig" => Encoding::Utf8Bom,
        _ => {
            return Err(RecoveryError::Invalid(
                "unsupported recovery encoding".to_owned(),
            ));
        }
    };
    let updated_at = OffsetDateTime::parse(&file.updated_at, &Rfc3339)
        .map_err(|error| RecoveryError::Invalid(format!("invalid update timestamp: {error}")))?;
    Ok(RecoveryEntry {
        document_path,
        workspace_root,
        text: file.text,
        encoding,
        updated_at,
        base_snapshot,
        fingerprint: fingerprint.to_owned(),
    })
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
    if snapshot.exists {
        if snapshot.digest.as_ref().is_none_or(|digest| {
            digest.len() != 64
                || digest
                    .bytes()
                    .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        }) {
            return Err(RecoveryError::Invalid(
                "invalid recovery baseline digest".to_owned(),
            ));
        }
    } else if snapshot.digest.is_some() {
        return Err(RecoveryError::Invalid(
            "missing recovery baseline has a digest".to_owned(),
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
    let resolved_root = resolve_with_missing(workspace_root)?;
    let resolved_document = resolve_with_missing(document_path)?;
    if !resolved_document.starts_with(&resolved_root) {
        return Err(RecoveryError::Invalid(
            "document resolves outside its workspace".to_owned(),
        ));
    }
    Ok(())
}

fn validate_export_destination(
    destination: &Path,
    workspace_root: &Path,
) -> Result<(), RecoveryError> {
    validate_paths(destination, workspace_root)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(RecoveryError::Invalid(format!(
                "recovery export destination already exists: {}",
                destination.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let parent = destination.parent().ok_or_else(|| {
        RecoveryError::Invalid("recovery export destination has no parent".to_owned())
    })?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RecoveryError::Invalid(format!(
            "recovery export parent is not a real directory: {}",
            parent.display()
        )));
    }
    let resolved_root = resolve_with_missing(workspace_root)?;
    let resolved_parent = parent.canonicalize()?;
    if !resolved_parent.starts_with(&resolved_root) {
        return Err(RecoveryError::Invalid(
            "recovery export resolves outside its workspace".to_owned(),
        ));
    }
    Ok(())
}

fn resolve_with_missing(path: &Path) -> Result<PathBuf, RecoveryError> {
    let mut current = path;
    let mut missing = Vec::<OsString>::new();
    loop {
        match current.canonicalize() {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = current.file_name().ok_or(RecoveryError::Io(error))?;
                missing.push(name.to_os_string());
                current = current.parent().ok_or_else(|| {
                    RecoveryError::Invalid("path has no resolvable parent".to_owned())
                })?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn source_status(document_path: &Path) -> RecoveryRecordStatus {
    let metadata = match fs::symlink_metadata(document_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RecoveryRecordStatus::Missing;
        }
        Err(_) => return RecoveryRecordStatus::Orphan,
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return RecoveryRecordStatus::Orphan;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    if options.open(document_path).is_ok() {
        RecoveryRecordStatus::Valid
    } else {
        RecoveryRecordStatus::Orphan
    }
}

fn read_regular_bytes(path: &Path) -> Result<Vec<u8>, RecoveryError> {
    let metadata = fs::symlink_metadata(path)?;
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
    Ok(bytes)
}

fn ensure_directory(path: &Path) -> Result<(), RecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RecoveryError::Invalid(format!(
                    "recovery storage is not a real directory: {}",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    secure_directory(path)
}

fn validate_real_directory(path: &Path) -> Result<(), RecoveryError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RecoveryError::Invalid(format!(
            "recovery storage is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn is_missing(path: &Path) -> Result<bool, RecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn remove_and_sync(path: &Path) -> Result<(), RecoveryError> {
    fs::remove_file(path)?;
    sync_directory(
        path.parent()
            .ok_or_else(|| RecoveryError::Invalid("journal has no parent".to_owned()))?,
    )
}

fn persist_new_bytes(
    destination: &Path,
    bytes: &[u8],
    collision_message: &str,
) -> Result<FileSnapshot, RecoveryError> {
    let parent = destination
        .parent()
        .ok_or_else(|| RecoveryError::Invalid("recovery destination has no parent".to_owned()))?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    let persisted = temporary.persist_noclobber(destination).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            RecoveryError::Invalid(format!("{collision_message}: {}", destination.display()))
        } else {
            RecoveryError::Io(error.error)
        }
    })?;
    sync_directory(parent)?;
    let metadata = persisted.metadata()?;
    Ok(FileSnapshot::from_bytes_and_metadata(bytes, &metadata))
}

fn sync_directory(path: &Path) -> Result<(), RecoveryError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn encoding_name(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "utf-8",
        Encoding::Utf8Bom => "utf-8-sig",
    }
}

fn encode_entry_text(entry: &RecoveryEntry) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(
        entry.text.len() + usize::from(entry.encoding == Encoding::Utf8Bom) * UTF8_BOM.len(),
    );
    if entry.encoding == Encoding::Utf8Bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(entry.text.as_bytes());
    bytes
}

fn hex_digest(bytes: &[u8; 32]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in bytes {
        write!(digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    digest
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn data_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut fingerprint = String::with_capacity(64);
    for byte in digest {
        write!(fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
    }
    fingerprint
}

fn path_fingerprint(path: &Path) -> String {
    let description = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(path).map_or_else(
            |error| format!("unreadable symlink:{error}"),
            |target| format!("symlink:{}", target.display()),
        ),
        Ok(metadata) => format!(
            "special:{}:{}:{:?}",
            metadata.len(),
            metadata.permissions().readonly(),
            metadata.modified().ok()
        ),
        Err(error) => format!("unreadable:{error}"),
    };
    data_fingerprint(description.as_bytes())
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
    use std::time::Duration;

    fn create_document(root: &Path, name: &str, text: &str) -> (PathBuf, FileSnapshot) {
        let path = root.join(name);
        fs::write(&path, text).unwrap();
        let loaded = load_file(&path).unwrap();
        (path, loaded.snapshot)
    }

    fn set_updated_at(path: &Path, updated_at: OffsetDateTime) {
        let bytes = fs::read(path).unwrap();
        let mut file: RecoveryFile = serde_json::from_slice(&bytes).unwrap();
        file.updated_at = updated_at.format(&Rfc3339).unwrap();
        fs::write(path, serialize_file(&file).unwrap()).unwrap();
    }

    fn quarantine_document(
        journal: &RecoveryJournal,
        workspace_root: &Path,
        document_path: &Path,
    ) -> RecoveryRecord {
        let active = journal
            .list_entries(Some(workspace_root))
            .unwrap()
            .into_iter()
            .find(|record| {
                record
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.document_path == document_path)
            })
            .unwrap();
        journal.quarantine(&active).unwrap();
        journal
            .list_quarantined(Some(workspace_root))
            .unwrap()
            .into_iter()
            .find(|record| {
                record
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.document_path == document_path)
            })
            .unwrap()
    }

    #[test]
    fn round_trip_uses_python_v2_shape_and_private_source() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "note.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));

        journal
            .publish(&document, &root, "unsaved", Encoding::Utf8, &snapshot)
            .unwrap();
        let entry = journal.load(&document).unwrap().unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&fs::read(journal.path_for(&document)).unwrap()).unwrap();

        assert_eq!(entry.text, "unsaved");
        assert!(entry.baseline_matches(&snapshot));
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
    fn loads_legacy_digest_only_journal() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let document = root.join("legacy.md");
        let journal = RecoveryJournal::new(root.join("recovery"));
        ensure_directory(&journal.root).unwrap();
        let file = RecoveryFile {
            version: 1,
            document_path: document.to_string_lossy().into_owned(),
            workspace_root: root.to_string_lossy().into_owned(),
            text: "legacy draft".to_owned(),
            encoding: "utf-8".to_owned(),
            base_snapshot: None,
            base_digest: Some("a".repeat(64)),
            updated_at: "2026-07-11T12:00:00+00:00".to_owned(),
        };
        fs::write(journal.path_for(&document), serialize_file(&file).unwrap()).unwrap();

        let entry = journal.load(&document).unwrap().unwrap();

        assert_eq!(entry.text, "legacy draft");
        assert_eq!(entry.updated_at.year(), 2026);
        assert!(!entry.baseline_matches(&FileSnapshot {
            sha256: [0xaa; 32],
            size: 0,
            modified_ns: 0,
            mode: 0,
            device: 0,
            inode: 0,
        }));
    }

    #[test]
    fn inventory_classifies_valid_missing_orphan_and_corrupt_records() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (valid, valid_snapshot) = create_document(&root, "valid.md", "saved");
        let (missing, missing_snapshot) = create_document(&root, "missing.md", "saved");
        let (orphan, orphan_snapshot) = create_document(&root, "orphan.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        for (path, snapshot) in [
            (&valid, &valid_snapshot),
            (&missing, &missing_snapshot),
            (&orphan, &orphan_snapshot),
        ] {
            journal
                .publish(path, &root, "draft", Encoding::Utf8, snapshot)
                .unwrap();
        }
        fs::remove_file(&missing).unwrap();
        fs::remove_file(&orphan).unwrap();
        fs::create_dir(&orphan).unwrap();
        let corrupt_path = journal.root.join(format!("{}.json", "f".repeat(64)));
        fs::write(&corrupt_path, b"not JSON\0\xff").unwrap();

        let records = journal.list_entries(Some(&root)).unwrap();

        assert_eq!(records.len(), 4);
        assert_eq!(
            records
                .iter()
                .find(|record| record
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.document_path == valid))
                .unwrap()
                .status,
            RecoveryRecordStatus::Valid
        );
        assert_eq!(
            records
                .iter()
                .find(|record| record
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.document_path == missing))
                .unwrap()
                .status,
            RecoveryRecordStatus::Missing
        );
        assert_eq!(
            records
                .iter()
                .find(|record| record
                    .entry
                    .as_ref()
                    .is_some_and(|entry| entry.document_path == orphan))
                .unwrap()
                .status,
            RecoveryRecordStatus::Orphan
        );
        let corrupt = records
            .iter()
            .find(|record| record.journal_path == corrupt_path)
            .unwrap();
        assert!(corrupt.is_corrupt());
        assert!(corrupt.has_content_fingerprint);
    }

    #[test]
    fn corrupt_entry_can_be_quarantined_without_changing_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        ensure_directory(&journal.root).unwrap();
        let source = journal.root.join(format!("{}.json", "e".repeat(64)));
        let bytes = b"{definitely not JSON}\r\n";
        fs::write(&source, bytes).unwrap();
        let record = journal.list_entries(None).unwrap().pop().unwrap();

        let destination = journal.quarantine(&record).unwrap();

        assert!(!source.exists());
        assert_eq!(fs::read(destination).unwrap(), bytes);
        assert!(journal.list_entries(None).unwrap().is_empty());
    }

    #[test]
    fn quarantine_refuses_a_record_that_changed_after_inventory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let journal = RecoveryJournal::new(root.join("recovery"));
        ensure_directory(&journal.root).unwrap();
        let source = journal.root.join(format!("{}.json", "e".repeat(64)));
        fs::write(&source, b"old corrupt bytes").unwrap();
        let record = journal.list_entries(None).unwrap().pop().unwrap();
        fs::write(&source, b"new corrupt bytes").unwrap();

        let error = journal.quarantine(&record).unwrap_err();

        assert!(error.to_string().contains("changed after it was listed"));
        assert_eq!(fs::read(source).unwrap(), b"new corrupt bytes");
    }

    #[test]
    fn conditional_discard_preserves_a_newer_draft() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "note.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        let first = journal
            .publish(&document, &root, "first", Encoding::Utf8, &snapshot)
            .unwrap();
        journal
            .publish(&document, &root, "newer", Encoding::Utf8, &snapshot)
            .unwrap();

        assert!(
            journal
                .discard(&document, Some(first.fingerprint()))
                .is_err()
        );
        assert_eq!(journal.load(&document).unwrap().unwrap().text, "newer");
    }

    #[test]
    fn expected_absence_never_deletes_a_journal_that_appeared() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "note.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        assert!(journal.record_for(&document).unwrap().is_none());
        journal
            .publish(
                &document,
                &root,
                "other instance",
                Encoding::Utf8,
                &snapshot,
            )
            .unwrap();

        let error = journal.discard(&document, None).unwrap_err();

        assert!(error.to_string().contains("appeared after cleanup"));
        assert!(journal.load(&document).unwrap().is_some());
    }

    #[test]
    fn retarget_preserves_the_draft_baseline_and_timestamp() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (source, snapshot) = create_document(&root, "old.md", "saved");
        let target = root.join("renamed.markdown");
        let journal = RecoveryJournal::new(root.join("recovery"));
        let original = journal
            .publish(
                &source,
                &root,
                "# Café\r\nno final newline",
                Encoding::Utf8Bom,
                &snapshot,
            )
            .unwrap();
        let record = journal.record_for(&source).unwrap().unwrap();

        let moved = journal.retarget(&record, &target, &root).unwrap();

        assert_eq!(moved.document_path, target);
        assert_eq!(moved.workspace_root, root);
        assert_eq!(moved.text, original.text);
        assert_eq!(moved.encoding, original.encoding);
        assert_eq!(moved.updated_at, original.updated_at);
        assert!(moved.baseline_matches(&snapshot));
        assert!(!journal.path_for(&source).exists());
        assert_eq!(journal.load(&target).unwrap().unwrap(), moved);
    }

    #[test]
    fn retarget_never_replaces_an_existing_recovery_draft() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (source, source_snapshot) = create_document(&root, "old.md", "saved old");
        let (target, target_snapshot) = create_document(&root, "occupied.md", "saved destination");
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(
                &source,
                &root,
                "source draft",
                Encoding::Utf8,
                &source_snapshot,
            )
            .unwrap();
        journal
            .publish(
                &target,
                &root,
                "destination draft",
                Encoding::Utf8,
                &target_snapshot,
            )
            .unwrap();
        let record = journal.record_for(&source).unwrap().unwrap();
        let source_bytes = fs::read(journal.path_for(&source)).unwrap();
        let target_bytes = fs::read(journal.path_for(&target)).unwrap();

        let error = journal.retarget(&record, &target, &root).unwrap_err();

        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(journal.path_for(&source)).unwrap(), source_bytes);
        assert_eq!(fs::read(journal.path_for(&target)).unwrap(), target_bytes);
    }

    #[test]
    fn retarget_refuses_a_record_that_changed_after_inventory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (source, snapshot) = create_document(&root, "old.md", "saved");
        let target = root.join("renamed.md");
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(&source, &root, "listed", Encoding::Utf8, &snapshot)
            .unwrap();
        let record = journal.record_for(&source).unwrap().unwrap();
        journal
            .publish(&source, &root, "newer", Encoding::Utf8, &snapshot)
            .unwrap();

        let error = journal.retarget(&record, &target, &root).unwrap_err();

        assert!(error.to_string().contains("changed after it was listed"));
        assert!(!journal.path_for(&target).exists());
        assert_eq!(journal.load(&source).unwrap().unwrap().text, "newer");
    }

    #[test]
    fn restore_quarantined_preserves_exact_journal_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "café 東京.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        let original = journal
            .publish(
                &document,
                &root,
                "# Café\r\n\r\n東京\nno final newline",
                Encoding::Utf8Bom,
                &snapshot,
            )
            .unwrap();
        let active_path = journal.path_for(&document);
        let original_bytes = fs::read(&active_path).unwrap();
        let quarantined = quarantine_document(&journal, &root, &document);

        let restored = journal.restore_quarantined(&quarantined).unwrap();

        assert_eq!(restored, original);
        assert_eq!(fs::read(&active_path).unwrap(), original_bytes);
        assert!(journal.list_quarantined(Some(&root)).unwrap().is_empty());
    }

    #[test]
    fn restore_quarantined_never_replaces_an_active_draft() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "note.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(
                &document,
                &root,
                "archived draft",
                Encoding::Utf8,
                &snapshot,
            )
            .unwrap();
        let quarantined = quarantine_document(&journal, &root, &document);
        let archived_bytes = fs::read(&quarantined.journal_path).unwrap();
        journal
            .publish(
                &document,
                &root,
                "new active draft",
                Encoding::Utf8,
                &snapshot,
            )
            .unwrap();
        let active_path = journal.path_for(&document);
        let active_bytes = fs::read(&active_path).unwrap();

        let error = journal.restore_quarantined(&quarantined).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("active recovery entry already exists")
        );
        assert_eq!(fs::read(active_path).unwrap(), active_bytes);
        assert_eq!(fs::read(&quarantined.journal_path).unwrap(), archived_bytes);
    }

    #[test]
    fn export_quarantined_preserves_text_bytes_and_the_archive() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "original.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        let text = "# Café\r\n\r\n東京\nno final newline";
        journal
            .publish(&document, &root, text, Encoding::Utf8Bom, &snapshot)
            .unwrap();
        let quarantined = quarantine_document(&journal, &root, &document);
        let archived_bytes = fs::read(&quarantined.journal_path).unwrap();
        let export_root = root.join("exports");
        fs::create_dir(&export_root).unwrap();
        let destination = export_root.join("recovered.markdown");

        let exported = journal
            .export_quarantined(&quarantined, &destination)
            .unwrap();

        let mut expected = UTF8_BOM.to_vec();
        expected.extend_from_slice(text.as_bytes());
        assert_eq!(fs::read(&destination).unwrap(), expected);
        assert_eq!(exported, load_file(&destination).unwrap().snapshot);
        assert_eq!(fs::read(&quarantined.journal_path).unwrap(), archived_bytes);
        assert_eq!(journal.list_quarantined(Some(&root)).unwrap().len(), 1);
    }

    #[test]
    fn export_quarantined_never_overwrites_or_exports_a_stale_record() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (document, snapshot) = create_document(&root, "original.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        journal
            .publish(&document, &root, "listed draft", Encoding::Utf8, &snapshot)
            .unwrap();
        let quarantined = quarantine_document(&journal, &root, &document);
        let occupied = root.join("occupied.md");
        fs::write(&occupied, "existing document").unwrap();

        let collision = journal
            .export_quarantined(&quarantined, &occupied)
            .unwrap_err();

        assert!(collision.to_string().contains("destination already exists"));
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "existing document");

        let replacement = fs::read(&quarantined.journal_path)
            .unwrap()
            .windows(b"listed draft".len())
            .position(|window| window == b"listed draft")
            .unwrap();
        let mut changed_bytes = fs::read(&quarantined.journal_path).unwrap();
        changed_bytes[replacement..replacement + b"listed draft".len()]
            .copy_from_slice(b"newest draft");
        fs::write(&quarantined.journal_path, &changed_bytes).unwrap();
        let destination = root.join("recovered.md");

        let stale = journal
            .export_quarantined(&quarantined, &destination)
            .unwrap_err();

        assert!(stale.to_string().contains("changed after it was listed"));
        assert!(!destination.exists());
        assert_eq!(fs::read(&quarantined.journal_path).unwrap(), changed_bytes);
    }

    #[test]
    fn export_quarantined_rejects_a_symlink_escape() {
        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let workspace = directory.path().join("workspace");
            let outside = directory.path().join("outside");
            fs::create_dir(&workspace).unwrap();
            fs::create_dir(&outside).unwrap();
            let root = workspace.canonicalize().unwrap();
            let (document, snapshot) = create_document(&root, "original.md", "saved");
            let journal = RecoveryJournal::new(root.join("recovery"));
            journal
                .publish(&document, &root, "archived", Encoding::Utf8, &snapshot)
                .unwrap();
            let quarantined = quarantine_document(&journal, &root, &document);
            std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();

            let error = journal
                .export_quarantined(&quarantined, &root.join("linked/escaped.md"))
                .unwrap_err();

            assert!(error.to_string().contains("resolves outside its workspace"));
            assert!(!outside.join("escaped.md").exists());
            assert!(quarantined.journal_path.exists());
        }
    }

    #[test]
    fn retention_deletes_only_confirmed_old_valid_quarantine() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (old, old_snapshot) = create_document(&root, "old.md", "saved");
        let (recent, recent_snapshot) = create_document(&root, "recent.md", "saved");
        let journal = RecoveryJournal::new(root.join("recovery"));
        let now = OffsetDateTime::now_utc();
        for (path, snapshot, age) in [
            (&old, &old_snapshot, Duration::from_secs(90 * 24 * 60 * 60)),
            (
                &recent,
                &recent_snapshot,
                Duration::from_secs(2 * 24 * 60 * 60),
            ),
        ] {
            journal
                .publish(path, &root, "draft", Encoding::Utf8, snapshot)
                .unwrap();
            let active_path = journal.path_for(path);
            set_updated_at(&active_path, now - age);
            let record = journal
                .list_entries(Some(&root))
                .unwrap()
                .into_iter()
                .find(|record| record.journal_path == active_path)
                .unwrap();
            journal.quarantine(&record).unwrap();
        }
        let quarantine = journal.list_quarantined(Some(&root)).unwrap();
        let old_record = quarantine
            .iter()
            .find(|record| record.entry.as_ref().unwrap().document_path == old)
            .unwrap()
            .clone();

        let result = journal
            .cleanup_quarantined(
                now - Duration::from_secs(30 * 24 * 60 * 60),
                Some(&root),
                Some(std::slice::from_ref(&old_record)),
            )
            .unwrap();

        assert_eq!(result.selected_count(), 1);
        assert_eq!(result.deleted_count(), 1);
        let remaining = journal.list_quarantined(Some(&root)).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].entry.as_ref().unwrap().document_path, recent);
    }

    #[test]
    fn symlinked_quarantine_is_rejected() {
        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let journal = RecoveryJournal::new(root.join("recovery"));
            ensure_directory(&journal.root).unwrap();
            let outside = root.join("outside");
            fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, journal.root.join("quarantine")).unwrap();

            let error = journal.list_quarantined(None).unwrap_err();

            assert!(error.to_string().contains("not a real directory"));
        }
    }
}
