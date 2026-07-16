//! Stable reads and conflict-checked atomic publication.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use tempfile::NamedTempFile;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::document::{Document, Encoding, FileSnapshot, LineEnding};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Debug)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
    pub exact_text: String,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub snapshot: FileSnapshot,
}

impl LoadedFile {
    #[must_use]
    pub fn into_document(self) -> Document {
        Document {
            path: self.path,
            text: self.text.clone(),
            saved_text: self.text,
            encoding: self.encoding,
            line_ending: self.line_ending,
            snapshot: self.snapshot,
            conflict: false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("the file changed on disk; refusing to overwrite it")]
    Conflict,
    #[error("the destination already exists")]
    AlreadyExists,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Read a stable regular UTF-8 file without following a final symlink.
///
/// # Errors
///
/// Returns an error when the path is not a stable regular file or its contents are not UTF-8.
pub fn load_file(path: &Path) -> anyhow::Result<LoadedFile> {
    let path = path.to_path_buf();
    let before = fs::symlink_metadata(&path)?;
    anyhow::ensure!(
        !before.file_type().is_symlink(),
        "symbolic links are not editable"
    );
    anyhow::ensure!(before.is_file(), "target is not a regular file");

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(&path)?;
    let opened = file.metadata()?;
    let mut bytes = Vec::with_capacity(opened.len().try_into().unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;

    let opened_snapshot = FileSnapshot::from_bytes_and_metadata(&bytes, &opened);
    let after_snapshot = FileSnapshot::from_bytes_and_metadata(&bytes, &after);
    anyhow::ensure!(
        opened_snapshot.same_origin(&after_snapshot) && opened.len() == after.len(),
        "file changed while it was being read"
    );

    let (encoding, content) = if bytes.starts_with(UTF8_BOM) {
        (Encoding::Utf8Bom, &bytes[UTF8_BOM.len()..])
    } else {
        (Encoding::Utf8, bytes.as_slice())
    };
    let exact_text = std::str::from_utf8(content)
        .map_err(|_| anyhow::anyhow!("only UTF-8 text files are supported"))?
        .to_owned();
    let line_ending = LineEnding::detect(&exact_text);
    let text = normalize_line_endings(&exact_text);
    let snapshot = FileSnapshot::from_bytes_and_metadata(&bytes, &after);

    Ok(LoadedFile {
        path,
        text,
        exact_text,
        encoding,
        line_ending,
        snapshot,
    })
}

/// Publish a document through a same-directory temporary file.
///
/// # Errors
///
/// Returns [`SaveError::Conflict`] for a stale baseline, [`SaveError::AlreadyExists`] for a
/// no-clobber collision, and [`SaveError::Io`] for publication failures.
pub fn save_atomic(
    path: &Path,
    text: &str,
    encoding: Encoding,
    line_ending: LineEnding,
    expected: Option<&FileSnapshot>,
    no_clobber: bool,
) -> Result<FileSnapshot, SaveError> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;

    if no_clobber && path.exists() {
        return Err(SaveError::AlreadyExists);
    }
    if let Some(expected) = expected {
        let current = load_file(path)
            .map_err(|error| io::Error::other(format!("cannot verify destination: {error}")))?;
        if current.snapshot != *expected {
            return Err(SaveError::Conflict);
        }
    }

    let bytes = encode_text(text, encoding, line_ending);
    let mut temporary = NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        let mode = expected.map_or(0o600, |snapshot| snapshot.mode & 0o7777);
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    temporary.write_all(&bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;

    if let Some(expected) = expected {
        let current = load_file(path)
            .map_err(|error| io::Error::other(format!("cannot recheck destination: {error}")))?;
        if current.snapshot != *expected {
            return Err(SaveError::Conflict);
        }
    }

    if no_clobber {
        temporary
            .persist_noclobber(path)
            .map_err(|error| match error.error.kind() {
                io::ErrorKind::AlreadyExists => SaveError::AlreadyExists,
                _ => SaveError::Io(error.error),
            })?;
    } else {
        temporary
            .persist(path)
            .map_err(|error| SaveError::Io(error.error))?;
    }

    sync_directory(parent)?;
    let metadata = fs::metadata(path)?;
    Ok(FileSnapshot::from_bytes_and_metadata(&bytes, &metadata))
}

#[must_use]
pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[must_use]
pub fn encode_text(text: &str, encoding: Encoding, line_ending: LineEnding) -> Vec<u8> {
    let normalized = normalize_line_endings(text);
    let rendered = if line_ending.separator() == "\n" {
        normalized
    } else {
        normalized.replace('\n', line_ending.separator())
    };
    let mut bytes =
        Vec::with_capacity(rendered.len() + usize::from(encoding == Encoding::Utf8Bom) * 3);
    if encoding == Encoding::Utf8Bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(rendered.as_bytes());
    bytes
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_and_save_preserve_bom_and_crlf() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, b"\xef\xbb\xbf# title\r\nbody\r\n").unwrap();

        let loaded = load_file(&path).unwrap();
        assert_eq!(loaded.encoding, Encoding::Utf8Bom);
        assert_eq!(loaded.line_ending, LineEnding::Crlf);
        assert_eq!(loaded.text, "# title\nbody\n");

        save_atomic(
            &path,
            "# title\nchanged\n",
            loaded.encoding,
            loaded.line_ending,
            Some(&loaded.snapshot),
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read(path).unwrap(),
            b"\xef\xbb\xbf# title\r\nchanged\r\n"
        );
    }

    #[test]
    fn stale_snapshot_rejects_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "original").unwrap();
        let loaded = load_file(&path).unwrap();
        fs::write(&path, "external").unwrap();

        let error = save_atomic(
            &path,
            "mine",
            Encoding::Utf8,
            LineEnding::None,
            Some(&loaded.snapshot),
            false,
        )
        .unwrap_err();
        assert!(matches!(error, SaveError::Conflict));
        assert_eq!(fs::read_to_string(path).unwrap(), "external");
    }
}
