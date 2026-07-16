//! Rust implementation of `TermDraft`'s portable core.

pub mod app;
pub mod document;
pub mod editor;
pub mod persistence;
pub mod search;
pub mod ui;
pub mod workspace;

pub use document::{Document, Encoding, FileSnapshot, LineEnding};
pub use persistence::{LoadedFile, SaveError, load_file, save_atomic};
pub use workspace::{Workspace, WorkspaceEntry, WorkspaceError};
