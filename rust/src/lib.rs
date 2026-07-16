//! Rust implementation of `TermDraft`'s portable core.

pub mod document;
pub mod persistence;
pub mod search;
pub mod workspace;

pub use document::{Document, Encoding, FileSnapshot, LineEnding};
pub use persistence::{LoadedFile, SaveError, load_file, save_atomic};
pub use workspace::{Workspace, WorkspaceEntry, WorkspaceError};
