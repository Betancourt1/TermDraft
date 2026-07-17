//! Rust implementation of `TermDraft`'s portable core.

pub mod app;
pub mod bindings;
pub mod config;
pub mod continuation;
pub mod coordinate_diagnostic;
pub mod document;
pub mod editor;
pub mod markdown;
pub mod markdown_help;
pub mod path_filter;
pub mod persistence;
pub mod recovery;
pub mod search;
pub mod semantic_blocks;
pub mod session;
pub mod ui;
pub mod workspace;
pub mod workspace_entries;

pub use document::{Document, Encoding, FileSnapshot, LineEnding};
pub use persistence::{LoadedFile, SaveError, load_file, save_atomic};
pub use workspace::{Workspace, WorkspaceEntry, WorkspaceError};
