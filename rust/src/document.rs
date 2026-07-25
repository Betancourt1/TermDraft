//! Exact document state shared by the editor and persistence layer.

use std::fs::Metadata;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

/// Encoding forms supported by `TermDraft`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
}

/// Line-ending forms that matter to byte-preserving editing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    None,
    Lf,
    Crlf,
    Cr,
    Mixed,
}

impl LineEnding {
    #[must_use]
    pub fn detect(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut lf = 0;
        let mut crlf = 0;
        let mut cr = 0;
        let mut index = 0;

        while index < bytes.len() {
            match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                    crlf += 1;
                    index += 2;
                }
                b'\r' => {
                    cr += 1;
                    index += 1;
                }
                b'\n' => {
                    lf += 1;
                    index += 1;
                }
                _ => index += 1,
            }
        }

        match (lf > 0, crlf > 0, cr > 0) {
            (false, false, false) => Self::None,
            (true, false, false) => Self::Lf,
            (false, true, false) => Self::Crlf,
            (false, false, true) => Self::Cr,
            _ => Self::Mixed,
        }
    }

    #[must_use]
    pub const fn separator(self) -> &'static str {
        match self {
            Self::Crlf => "\r\n",
            Self::Cr => "\r",
            Self::None | Self::Lf | Self::Mixed => "\n",
        }
    }

    /// Choose the stable separator used after the user accepts normalization.
    #[must_use]
    pub fn mixed_target(text: &str) -> Option<Self> {
        if Self::detect(text) != Self::Mixed {
            return None;
        }
        if text.contains("\r\n") {
            Some(Self::Crlf)
        } else if text.contains('\n') {
            Some(Self::Lf)
        } else {
            Some(Self::Cr)
        }
    }
}

/// Content and origin identity used to reject stale saves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSnapshot {
    pub sha256: [u8; 32],
    pub size: u64,
    pub modified_ns: u128,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
}

impl FileSnapshot {
    #[must_use]
    pub fn from_bytes_and_metadata(bytes: &[u8], metadata: &Metadata) -> Self {
        let digest = Sha256::digest(bytes);
        let mut sha256 = [0; 32];
        sha256.copy_from_slice(&digest);

        #[cfg(unix)]
        let (mode, device, inode) = (metadata.mode(), metadata.dev(), metadata.ino());
        #[cfg(not(unix))]
        let (mode, device, inode) = (0, 0, 0);

        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());

        Self {
            sha256,
            size: metadata.len(),
            modified_ns,
            mode,
            device,
            inode,
        }
    }

    #[must_use]
    pub fn same_origin(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }
}

/// Monotonic in-memory identity for the current source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceRevision {
    pub generation: u64,
    pub sha256: [u8; 32],
}

impl SourceRevision {
    #[must_use]
    pub fn initial(source: &str) -> Self {
        Self {
            generation: 0,
            sha256: source_digest(source),
        }
    }
}

/// Returned when a caller tries to replace source based on an older revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevisionMismatch {
    pub expected: SourceRevision,
    pub actual: SourceRevision,
}

#[derive(Clone, Debug)]
pub struct MixedSource {
    exact: String,
    normalized: String,
    target: LineEnding,
    consented: bool,
}

impl MixedSource {
    #[must_use]
    pub fn new(exact: String, normalized: String, target: LineEnding) -> Self {
        Self {
            exact,
            normalized,
            target,
            consented: false,
        }
    }

    #[must_use]
    pub const fn target(&self) -> LineEnding {
        self.target
    }
}

/// One open document. The editor owns normalized LF text; persistence owns bytes.
#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
    pub saved_text: String,
    pub encoding: Encoding,
    pub line_ending: LineEnding,
    pub mixed_source: Option<MixedSource>,
    pub snapshot: FileSnapshot,
    pub conflict: bool,
    pub recovery_conflict: bool,
    pub source_revision: SourceRevision,
}

impl Document {
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.text != self.saved_text || self.recovery_conflict
    }

    #[must_use]
    pub fn is_editable(&self) -> bool {
        self.line_ending != LineEnding::Mixed
            || self
                .mixed_source
                .as_ref()
                .is_some_and(|source| source.consented)
    }

    /// Accept the previously detected mixed-ending normalization target.
    pub fn accept_mixed_line_endings(&mut self) -> bool {
        if self.line_ending != LineEnding::Mixed {
            return false;
        }
        let Some(source) = self.mixed_source.as_mut() else {
            return false;
        };
        source.consented = true;
        true
    }

    #[must_use]
    pub fn mixed_line_ending_target(&self) -> Option<LineEnding> {
        self.mixed_source.as_ref().map(MixedSource::target)
    }

    /// Synchronize normalized editor text while retaining untouched mixed source bytes.
    pub fn update_from_editor(&mut self, editor_text: String) -> bool {
        let updated = if let Some(source) = self.mixed_source.as_ref() {
            if editor_text == source.normalized {
                self.line_ending = LineEnding::Mixed;
                source.exact.clone()
            } else if source.consented {
                self.line_ending = source.target;
                editor_text
            } else {
                return false;
            }
        } else {
            editor_text
        };
        self.replace_source(updated)
    }

    /// Replace source only if the caller still targets the current revision.
    ///
    /// # Errors
    ///
    /// Returns the expected and actual revisions when the caller targets stale source.
    pub fn update_from_editor_if_revision(
        &mut self,
        expected: SourceRevision,
        editor_text: String,
    ) -> Result<bool, RevisionMismatch> {
        if expected != self.source_revision {
            return Err(RevisionMismatch {
                expected,
                actual: self.source_revision,
            });
        }
        Ok(self.update_from_editor(editor_text))
    }

    /// Install source and advance its identity when the bytes actually changed.
    pub fn replace_source(&mut self, source: String) -> bool {
        if source == self.text {
            return false;
        }
        self.text = source;
        self.source_revision = SourceRevision {
            generation: self.source_revision.generation + 1,
            sha256: source_digest(&self.text),
        };
        true
    }

    pub(crate) fn continue_source_revision_from(&mut self, previous: &Self) {
        if self.text == previous.text {
            self.source_revision = previous.source_revision;
        } else {
            self.source_revision.generation = previous.source_revision.generation + 1;
        }
    }

    #[must_use]
    pub fn word_count(&self) -> usize {
        self.text
            .unicode_words()
            .filter(|word| word.chars().any(char::is_alphanumeric))
            .count()
    }

    pub fn mark_saved(&mut self, snapshot: FileSnapshot) {
        self.saved_text.clone_from(&self.text);
        self.snapshot = snapshot;
        self.conflict = false;
        self.recovery_conflict = false;
        if self.line_ending != LineEnding::Mixed {
            self.mixed_source = None;
        }
    }
}

fn source_digest(source: &str) -> [u8; 32] {
    Sha256::digest(source.as_bytes()).into()
}

trait UnicodeWords {
    fn unicode_words(&self) -> unicode_segmentation::UnicodeWords<'_>;
}

impl UnicodeWords for str {
    fn unicode_words(&self) -> unicode_segmentation::UnicodeWords<'_> {
        unicode_segmentation::UnicodeSegmentation::unicode_words(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_line_endings() {
        assert_eq!(LineEnding::detect("plain"), LineEnding::None);
        assert_eq!(LineEnding::detect("a\nb\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("a\r\nb\r\n"), LineEnding::Crlf);
        assert_eq!(LineEnding::detect("a\rb\r"), LineEnding::Cr);
        assert_eq!(LineEnding::detect("a\r\nb\n"), LineEnding::Mixed);
        assert_eq!(
            LineEnding::mixed_target("a\nb\r\nc\r"),
            Some(LineEnding::Crlf)
        );
        assert_eq!(LineEnding::mixed_target("a\nb\r"), Some(LineEnding::Lf));
        assert_eq!(
            LineEnding::mixed_target("a\rb\r\nc"),
            Some(LineEnding::Crlf)
        );
        assert_eq!(LineEnding::mixed_target("a\nb"), None);
    }

    #[test]
    fn source_revision_advances_only_when_source_changes() {
        let mut document = test_document("first");
        let initial = document.source_revision;

        assert!(!document.update_from_editor("first".to_owned()));
        assert_eq!(document.source_revision, initial);

        assert!(document.update_from_editor("second".to_owned()));
        assert_eq!(document.source_revision.generation, initial.generation + 1);
        assert_ne!(document.source_revision.sha256, initial.sha256);
    }

    #[test]
    fn expected_revision_rejects_stale_source_atomically() {
        let mut document = test_document("first");
        let expected = document.source_revision;
        assert!(document.update_from_editor("second".to_owned()));
        let current = document.source_revision;

        let result = document.update_from_editor_if_revision(expected, "stale".to_owned());

        assert_eq!(
            result,
            Err(RevisionMismatch {
                expected,
                actual: current,
            })
        );
        assert_eq!(document.text, "second");
        assert_eq!(document.source_revision, current);
    }

    fn test_document(source: &str) -> Document {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        std::fs::write(&path, source).unwrap();
        crate::persistence::load_file(&path)
            .unwrap()
            .into_document()
    }
}
