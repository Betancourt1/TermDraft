//! Compatibility with `TermDraft`'s safe, non-executable user configuration.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use serde::Deserialize;
use thiserror::Error;

use crate::bindings::{BindingError, Keymap};

pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const THEME_FILE_NAME: &str = "theme.tcss";

pub const CONFIG_TEMPLATE: &str = r#"# TermDraft configuration. Unknown options are rejected instead of ignored.

[editor]
auto_continue_lists = true
soft_wrap = true
show_line_numbers = true
# Applied on the next launch: "command" or "write".
startup_mode = "command"
# "inline" previews every line except the cursor line. Use "split" for two panes.
view_mode = "inline"

[recovery]
retention_days = 30

[keybindings]
# Bindings override keys only. They cannot define actions or commands.
# save = "ctrl+s"
"#;

pub const THEME_TEMPLATE: &str = r"/* TermDraft user theme overrides.

   The Rust comparison build preserves the built-in monochrome frontend and does not
   currently evaluate Textual CSS.
*/
";

#[derive(Clone, Debug)]
pub struct Config {
    pub root: PathBuf,
    pub editor: EditorConfig,
    pub recovery: RecoveryConfig,
    /// Entries explicitly loaded from `[keybindings]`.
    pub keybinding_overrides: BTreeMap<String, String>,
    /// The complete validated keymap after applying overrides to official defaults.
    pub keybindings: Keymap,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            editor: EditorConfig::default(),
            recovery: RecoveryConfig::default(),
            keybinding_overrides: BTreeMap::new(),
            keybindings: Keymap::default(),
        }
    }
}

impl Config {
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE_NAME)
    }

    #[must_use]
    pub fn theme_path(&self) -> PathBuf {
        self.root.join(THEME_FILE_NAME)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    pub auto_continue_lists: bool,
    pub soft_wrap: bool,
    pub show_line_numbers: bool,
    pub startup_mode: StartupMode,
    pub view_mode: StartupView,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            auto_continue_lists: true,
            soft_wrap: true,
            show_line_numbers: true,
            startup_mode: StartupMode::Command,
            view_mode: StartupView::Inline,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StartupMode {
    #[default]
    Command,
    Write,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StartupView {
    #[default]
    Inline,
    Split,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryConfig {
    pub retention_days: u32,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self { retention_days: 30 }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigFile {
    editor: EditorConfig,
    recovery: RecoveryConfig,
    keybindings: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot resolve the user configuration directory")]
    MissingHome,
    #[error("{0} must not be empty")]
    EmptyEnvironment(&'static str),
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("recovery.retention_days must be a positive integer")]
    InvalidRetention,
    #[error(transparent)]
    InvalidKeybindings(#[from] BindingError),
    #[error("cannot create configuration at {path}: {source}")]
    Create {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Resolve the same canonical and pre-1.0 configuration locations as Python `TermDraft`.
///
/// # Errors
///
/// Returns an error when no home/current directory exists or an environment override is empty.
pub fn config_root(explicit: Option<&Path>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit {
        return absolutize(path);
    }
    for name in ["TERMDRAFT_CONFIG_HOME", "TERMWRITER_CONFIG_HOME"] {
        if let Some(value) = env::var_os(name) {
            if value.to_string_lossy().trim().is_empty() {
                return Err(ConfigError::EmptyEnvironment(name));
            }
            return absolutize(Path::new(&value));
        }
    }
    let base = BaseDirs::new().ok_or(ConfigError::MissingHome)?;
    let canonical = base.home_dir().join(".termdraft");
    let legacy = base.home_dir().join(".termwriter");
    if canonical.exists() || !legacy.exists() {
        Ok(canonical)
    } else {
        Ok(legacy)
    }
}

/// Load a strict TOML file, returning effective defaults when it is absent.
///
/// # Errors
///
/// Returns an error when the file cannot be read or contains invalid options.
pub fn load(root: PathBuf) -> Result<Config, ConfigError> {
    let path = root.join(CONFIG_FILE_NAME);
    let parsed = match fs::read_to_string(&path) {
        Ok(source) => {
            toml::from_str::<ConfigFile>(&source).map_err(|source| ConfigError::Parse {
                path: path.clone(),
                source,
            })?
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => ConfigFile::default(),
        Err(source) => return Err(ConfigError::Read { path, source }),
    };
    if parsed.recovery.retention_days == 0 {
        return Err(ConfigError::InvalidRetention);
    }
    let keybindings = Keymap::resolve(&parsed.keybindings)?;
    Ok(Config {
        root,
        editor: parsed.editor,
        recovery: parsed.recovery,
        keybinding_overrides: parsed.keybindings,
        keybindings,
    })
}

/// Create missing templates without replacing either existing file.
///
/// # Errors
///
/// Returns an error when the directory or either missing template cannot be created safely.
pub fn initialize(root: PathBuf) -> Result<Config, ConfigError> {
    fs::create_dir_all(&root).map_err(|source| ConfigError::Create {
        path: root.clone(),
        source,
    })?;
    secure_directory(&root)?;
    create_new(&root.join(CONFIG_FILE_NAME), CONFIG_TEMPLATE)?;
    create_new(&root.join(THEME_FILE_NAME), THEME_TEMPLATE)?;
    load(root)
}

fn create_new(path: &Path, content: &str) -> Result<(), ConfigError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(content.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|source| ConfigError::Create {
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(source) => {
            return Err(ConfigError::Create {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    Ok(())
}

fn secure_directory(path: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
            ConfigError::Create {
                path: path.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

fn absolutize(path: &Path) -> Result<PathBuf, ConfigError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|current| current.join(path))
            .map_err(|source| ConfigError::Read {
                path: path.to_path_buf(),
                source,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_editor_compatibility_options() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIG_FILE_NAME),
            r#"[editor]
auto_continue_lists = false
soft_wrap = false
show_line_numbers = false
startup_mode = "write"
view_mode = "split"
[recovery]
retention_days = 45
"#,
        )
        .unwrap();

        let config = load(directory.path().to_path_buf()).unwrap();

        assert!(!config.editor.auto_continue_lists);
        assert!(!config.editor.soft_wrap);
        assert!(!config.editor.show_line_numbers);
        assert_eq!(config.editor.startup_mode, StartupMode::Write);
        assert_eq!(config.editor.view_mode, StartupView::Split);
        assert_eq!(config.recovery.retention_days, 45);
        assert_eq!(config.keybindings["save"], "ctrl+s");
        assert!(config.keybinding_overrides.is_empty());
    }

    #[test]
    fn resolves_keybinding_overrides_over_complete_defaults() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIG_FILE_NAME),
            "[keybindings]\nsave = \"ctrl+alt+s\"\nredo = \"ctrl+r, ctrl+shift+r\"\n",
        )
        .unwrap();

        let config = load(directory.path().to_path_buf()).unwrap();

        assert_eq!(config.keybinding_overrides["save"], "ctrl+alt+s");
        assert_eq!(config.keybinding_overrides["redo"], "ctrl+r, ctrl+shift+r");
        assert_eq!(config.keybindings["save"], "ctrl+alt+s");
        assert_eq!(config.keybindings["redo"], "ctrl+r,ctrl+shift+r");
        assert_eq!(config.keybindings["quit"], "ctrl+q");
        assert_eq!(config.keybindings.len(), 52);
    }

    #[test]
    fn rejects_invalid_effective_keybindings() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join(CONFIG_FILE_NAME);

        fs::write(&config_path, "[keybindings]\nsave = \"ctrl+q\"\n").unwrap();
        assert!(matches!(
            load(directory.path().to_path_buf()),
            Err(ConfigError::InvalidKeybindings(
                BindingError::Collision { .. }
            ))
        ));

        fs::write(
            &config_path,
            "[keybindings]\npreview_next_heading = \"tab\"\n",
        )
        .unwrap();
        assert!(matches!(
            load(directory.path().to_path_buf()),
            Err(ConfigError::InvalidKeybindings(
                BindingError::ReservedPreviewKey(_)
            ))
        ));

        fs::write(&config_path, "[keybindings]\nunknown = \"ctrl+x\"\n").unwrap();
        assert!(matches!(
            load(directory.path().to_path_buf()),
            Err(ConfigError::InvalidKeybindings(BindingError::UnknownId(_)))
        ));
    }

    #[test]
    fn rejects_unknown_options() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIG_FILE_NAME),
            "[editor]\nmagic = true\n",
        )
        .unwrap();

        assert!(matches!(
            load(directory.path().to_path_buf()),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn initialization_does_not_replace_existing_config() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join(CONFIG_FILE_NAME);
        fs::write(&config_path, "[editor]\nsoft_wrap = false\n").unwrap();

        initialize(directory.path().to_path_buf()).unwrap();

        assert_eq!(
            fs::read_to_string(config_path).unwrap(),
            "[editor]\nsoft_wrap = false\n"
        );
        assert!(directory.path().join(THEME_FILE_NAME).is_file());
    }
}
