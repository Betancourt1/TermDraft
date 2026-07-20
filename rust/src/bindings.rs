//! Canonical keyboard actions and resolved user keybindings.

use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;
use std::str::FromStr;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use thiserror::Error;

/// An application action that can be reached through a configured keybinding.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Action {
    Save,
    SaveAs,
    Quit,
    ToggleExplorer,
    FindFile,
    RecentDocuments,
    NextTab,
    PreviousTab,
    CloseTab,
    FindReplace,
    SearchText,
    DocumentOutline,
    TogglePreview,
    PreviewNextHeading,
    PreviewPreviousHeading,
    Undo,
    Redo,
    ShowHelp,
    CommandPalette,
    EnterWriteMode,
    ChangeTheme,
    DuplicateDocument,
    ReloadConfig,
    ManageRecovery,
    MarkdownHelp,
    InspectSemanticBlocks,
    ReadSemanticBlocks,
    InspectCursorCoordinates,
    CursorLeft,
    CursorDown,
    CursorUp,
    CursorRight,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}

/// The interface context in which a binding is active.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BindingScope {
    Global,
    Editor,
    Preview,
    Command,
}

/// The stable configuration ID and default shortcut for one binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindingDefinition {
    pub id: &'static str,
    pub action: Action,
    pub scope: BindingScope,
    pub default: &'static str,
}

macro_rules! binding {
    ($id:literal, $action:ident, $scope:ident, $default:literal) => {
        BindingDefinition {
            id: $id,
            action: Action::$action,
            scope: BindingScope::$scope,
            default: $default,
        }
    };
}

/// The official Python frontend's complete configurable binding contract.
pub const BINDING_DEFINITIONS: &[BindingDefinition] = &[
    binding!("save", Save, Global, "ctrl+s"),
    binding!("save_as", SaveAs, Global, "ctrl+shift+s"),
    binding!("quit", Quit, Global, "ctrl+q"),
    binding!("toggle_explorer", ToggleExplorer, Global, "ctrl+b"),
    binding!("find_file", FindFile, Global, "ctrl+p"),
    binding!("recent_documents", RecentDocuments, Global, "ctrl+o"),
    binding!("next_tab", NextTab, Global, "ctrl+pagedown"),
    binding!("previous_tab", PreviousTab, Global, "ctrl+pageup"),
    binding!("close_tab", CloseTab, Global, "ctrl+f4"),
    binding!("find_replace", FindReplace, Global, "ctrl+f"),
    binding!("search_text", SearchText, Global, "ctrl+shift+f"),
    binding!("document_outline", DocumentOutline, Global, "ctrl+shift+o"),
    binding!("toggle_preview", TogglePreview, Global, "ctrl+e"),
    binding!(
        "preview_next_heading",
        PreviewNextHeading,
        Preview,
        "alt+down"
    ),
    binding!(
        "preview_previous_heading",
        PreviewPreviousHeading,
        Preview,
        "alt+up"
    ),
    binding!("undo", Undo, Editor, "ctrl+z,super+z"),
    binding!("redo", Redo, Editor, "ctrl+y,super+y,ctrl+shift+z"),
    binding!("show_help", ShowHelp, Global, "f1"),
    binding!("command_palette", CommandPalette, Global, "ctrl+backslash"),
    binding!("command_write_mode", EnterWriteMode, Command, "i"),
    binding!("command_change_theme", ChangeTheme, Command, "t"),
    binding!("command_save", Save, Command, "w"),
    binding!("command_save_as", SaveAs, Command, "W"),
    binding!(
        "command_duplicate_document",
        DuplicateDocument,
        Command,
        "D"
    ),
    binding!("command_quit", Quit, Command, "q"),
    binding!("command_toggle_explorer", ToggleExplorer, Command, "e"),
    binding!("command_find_file", FindFile, Command, "f"),
    binding!("command_recent_documents", RecentDocuments, Command, "o"),
    binding!("command_next_tab", NextTab, Command, "]"),
    binding!("command_previous_tab", PreviousTab, Command, "["),
    binding!("command_close_tab", CloseTab, Command, "C"),
    binding!("command_search_text", SearchText, Command, "slash"),
    binding!("command_find_replace", FindReplace, Command, "s"),
    binding!("command_document_outline", DocumentOutline, Command, "S"),
    binding!("command_toggle_preview", TogglePreview, Command, "v"),
    binding!("command_undo", Undo, Command, "u"),
    binding!("command_redo", Redo, Command, "U"),
    binding!("command_reload_config", ReloadConfig, Command, "R"),
    binding!("command_manage_recovery", ManageRecovery, Command, "M"),
    binding!("command_markdown_help", MarkdownHelp, Command, "K"),
    binding!(
        "command_inspect_semantic_blocks",
        InspectSemanticBlocks,
        Command,
        "b"
    ),
    binding!(
        "command_read_semantic_blocks",
        ReadSemanticBlocks,
        Command,
        "B"
    ),
    binding!(
        "command_inspect_cursor_coordinates",
        InspectCursorCoordinates,
        Command,
        "I"
    ),
    binding!("command_open_palette", CommandPalette, Command, "colon"),
    binding!("command_show_help", ShowHelp, Command, "question_mark"),
    binding!("command_cursor_left", CursorLeft, Command, "h"),
    binding!("command_cursor_down", CursorDown, Command, "j"),
    binding!("command_cursor_up", CursorUp, Command, "k"),
    binding!("command_cursor_right", CursorRight, Command, "l"),
    binding!("command_line_start", LineStart, Command, "0"),
    binding!("command_line_end", LineEnd, Command, "dollar_sign"),
    binding!("command_document_start", DocumentStart, Command, "g"),
    binding!("command_document_end", DocumentEnd, Command, "G"),
];

/// A parsed shortcut. Equality follows Crossterm's normalized character semantics.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeySpec(KeyEvent);

impl KeySpec {
    #[must_use]
    pub const fn code(self) -> KeyCode {
        self.0.code
    }

    #[must_use]
    pub const fn modifiers(self) -> KeyModifiers {
        self.0.modifiers
    }

    /// Match the key and every modifier exactly, while ignoring press/repeat metadata.
    #[must_use]
    pub fn matches(self, event: KeyEvent) -> bool {
        self.0 == KeyEvent::new(event.code, event.modifiers)
    }
}

impl FromStr for KeySpec {
    type Err = KeyParseError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        parse_key_spec(spec)
    }
}

/// Why a textual shortcut could not be converted to a terminal key event.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum KeyParseError {
    #[error("key name must not be empty")]
    Empty,
    #[error("key specification contains an empty component")]
    EmptyComponent,
    #[error("modifier {0:?} appears more than once")]
    DuplicateModifier(String),
    #[error("modifier {0:?} must appear before the key name")]
    ModifierAfterKey(String),
    #[error("key specification contains more than one key name")]
    MultipleKeys,
    #[error("key specification contains only modifiers")]
    MissingKey,
    #[error("unsupported key name {0:?}")]
    UnsupportedKey(String),
}

/// A resolved binding with its preserved configuration spelling and parsed keys.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBinding {
    pub definition: &'static BindingDefinition,
    pub text: String,
    pub keys: Vec<KeySpec>,
}

/// Effective, validated shortcuts resolved from defaults and optional overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Keymap {
    effective: BTreeMap<String, String>,
    bindings: Vec<ResolvedBinding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::resolve(&BTreeMap::new()).expect("built-in keybindings must be valid")
    }
}

impl Keymap {
    /// Apply overrides to the official defaults and validate the complete effective map.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown IDs, malformed keys, reserved preview controls, or collisions.
    pub fn resolve(overrides: &BTreeMap<String, String>) -> Result<Self, BindingError> {
        if let Some(unknown) = overrides.keys().find(|id| {
            !BINDING_DEFINITIONS
                .iter()
                .any(|binding| binding.id == id.as_str())
        }) {
            return Err(BindingError::UnknownId(unknown.clone()));
        }

        let mut effective = BINDING_DEFINITIONS
            .iter()
            .map(|binding| (binding.id.to_owned(), binding.default.to_owned()))
            .collect::<BTreeMap<_, _>>();
        for (id, value) in overrides {
            let tokens = split_binding(id, value)?;
            effective.insert(id.clone(), tokens.join(","));
        }

        let mut used = HashMap::<KeySpec, &'static str>::new();
        let mut bindings = Vec::with_capacity(BINDING_DEFINITIONS.len());
        for definition in BINDING_DEFINITIONS {
            let text = effective
                .get(definition.id)
                .cloned()
                .unwrap_or_else(|| definition.default.to_owned());
            let mut keys = Vec::new();
            for token in text.split(',') {
                let key = token
                    .parse::<KeySpec>()
                    .map_err(|source| BindingError::InvalidKey {
                        id: definition.id.to_owned(),
                        key: token.to_owned(),
                        source,
                    })?;
                if is_reserved_preview_key(key) {
                    return Err(BindingError::ReservedPreviewKey(token.to_owned()));
                }
                if let Some(previous) = used.insert(key, definition.id) {
                    return Err(BindingError::Collision {
                        key: token.to_owned(),
                        previous: previous.to_owned(),
                        current: definition.id.to_owned(),
                    });
                }
                keys.push(key);
            }
            bindings.push(ResolvedBinding {
                definition,
                text,
                keys,
            });
        }

        Ok(Self {
            effective,
            bindings,
        })
    }

    #[must_use]
    pub fn binding(&self, id: &str) -> Option<&ResolvedBinding> {
        self.bindings
            .iter()
            .find(|binding| binding.definition.id == id)
    }

    #[must_use]
    pub fn binding_for(&self, scope: BindingScope, action: Action) -> Option<&ResolvedBinding> {
        self.bindings.iter().find(|binding| {
            binding.definition.scope == scope && binding.definition.action == action
        })
    }

    /// Resolve an event only within the supplied context.
    #[must_use]
    pub fn action_for(&self, scope: BindingScope, event: KeyEvent) -> Option<Action> {
        self.bindings
            .iter()
            .filter(|binding| binding.definition.scope == scope)
            .find(|binding| binding.keys.iter().any(|key| key.matches(event)))
            .map(|binding| binding.definition.action)
    }

    #[must_use]
    pub fn matches(&self, id: &str, event: KeyEvent) -> bool {
        self.binding(id)
            .is_some_and(|binding| binding.keys.iter().any(|key| key.matches(event)))
    }

    pub fn bindings(&self) -> impl Iterator<Item = &ResolvedBinding> {
        self.bindings.iter()
    }
}

impl Deref for Keymap {
    type Target = BTreeMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.effective
    }
}

impl<'a> IntoIterator for &'a Keymap {
    type Item = (&'a String, &'a String);
    type IntoIter = std::collections::btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.effective.iter()
    }
}

/// Errors in a complete effective binding map.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum BindingError {
    #[error("unknown keybinding id: {0:?}")]
    UnknownId(String),
    #[error("keybindings.{0} must contain non-empty key names")]
    EmptyBinding(String),
    #[error("invalid key {key:?} for keybindings.{id}: {source}")]
    InvalidKey {
        id: String,
        key: String,
        source: KeyParseError,
    },
    #[error("key {0:?} is reserved for preview link controls")]
    ReservedPreviewKey(String),
    #[error("key {key:?} is assigned to both {previous:?} and {current:?}")]
    Collision {
        key: String,
        previous: String,
        current: String,
    },
}

fn split_binding(id: &str, value: &str) -> Result<Vec<String>, BindingError> {
    let tokens = value
        .split(',')
        .map(str::trim)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if tokens.is_empty() || tokens.iter().any(String::is_empty) {
        return Err(BindingError::EmptyBinding(id.to_owned()));
    }
    Ok(tokens)
}

fn parse_key_spec(spec: &str) -> Result<KeySpec, KeyParseError> {
    if spec.is_empty() {
        return Err(KeyParseError::Empty);
    }
    let parts = spec.split('+').collect::<Vec<_>>();
    if parts.iter().any(|part| part.is_empty()) {
        return Err(KeyParseError::EmptyComponent);
    }

    let mut modifiers = KeyModifiers::NONE;
    let mut code = None;
    for part in parts {
        if let Some(modifier) = parse_modifier(part) {
            if code.is_some() {
                return Err(KeyParseError::ModifierAfterKey(part.to_owned()));
            }
            if modifiers.contains(modifier) {
                return Err(KeyParseError::DuplicateModifier(part.to_owned()));
            }
            modifiers.insert(modifier);
            continue;
        }
        if code.is_some() {
            return Err(KeyParseError::MultipleKeys);
        }
        code = Some(parse_key_code(part)?);
    }
    let code = code.ok_or(KeyParseError::MissingKey)?;
    Ok(KeySpec(KeyEvent::new(code, modifiers)))
}

fn parse_modifier(name: &str) -> Option<KeyModifiers> {
    match name.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(KeyModifiers::CONTROL),
        "alt" | "option" => Some(KeyModifiers::ALT),
        "shift" => Some(KeyModifiers::SHIFT),
        "super" | "cmd" | "command" => Some(KeyModifiers::SUPER),
        _ => None,
    }
}

fn parse_key_code(name: &str) -> Result<KeyCode, KeyParseError> {
    if name.chars().count() == 1 {
        return Ok(KeyCode::Char(name.chars().next().expect("one character")));
    }
    let normalized = name.to_ascii_lowercase();
    let named = match normalized.as_str() {
        "backspace" => KeyCode::Backspace,
        "enter" | "return" => KeyCode::Enter,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "page_up" => KeyCode::PageUp,
        "pagedown" | "page_down" => KeyCode::PageDown,
        "tab" => KeyCode::Tab,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "escape" | "esc" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "slash" | "solidus" => KeyCode::Char('/'),
        "backslash" | "reverse_solidus" => KeyCode::Char('\\'),
        "colon" => KeyCode::Char(':'),
        "semicolon" => KeyCode::Char(';'),
        "question_mark" => KeyCode::Char('?'),
        "dollar_sign" => KeyCode::Char('$'),
        "at" | "commercial_at" => KeyCode::Char('@'),
        "minus" | "hyphen_minus" => KeyCode::Char('-'),
        "plus" | "plus_sign" => KeyCode::Char('+'),
        "underscore" | "low_line" => KeyCode::Char('_'),
        "comma" => KeyCode::Char(','),
        "period" | "full_stop" => KeyCode::Char('.'),
        "left_square_bracket" => KeyCode::Char('['),
        "right_square_bracket" => KeyCode::Char(']'),
        "equals" | "equals_sign" => KeyCode::Char('='),
        "apostrophe" | "single_quote" => KeyCode::Char('\''),
        "quotation_mark" | "double_quote" => KeyCode::Char('"'),
        "grave_accent" | "backtick" => KeyCode::Char('`'),
        _ => {
            if let Some(number) = normalized.strip_prefix('f')
                && let Ok(number) = number.parse::<u8>()
                && (1..=24).contains(&number)
            {
                return Ok(KeyCode::F(number));
            }
            return Err(KeyParseError::UnsupportedKey(name.to_owned()));
        }
    };
    Ok(named)
}

fn is_reserved_preview_key(key: KeySpec) -> bool {
    matches!(
        (key.code(), key.modifiers()),
        (KeyCode::Tab, KeyModifiers::NONE | KeyModifiers::SHIFT)
            | (KeyCode::Enter, KeyModifiers::NONE)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(id, value)| ((*id).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn official_defaults_are_complete_and_resolve() {
        let keymap = Keymap::default();

        assert_eq!(BINDING_DEFINITIONS.len(), 53);
        assert_eq!(keymap.len(), 53);
        assert_eq!(keymap["command_save_as"], "W");
        assert_eq!(keymap["command_next_tab"], "]");
        assert_eq!(keymap["command_previous_tab"], "[");
        assert_eq!(keymap["command_redo"], "U");
        assert_eq!(keymap["command_markdown_help"], "K");
        assert_eq!(keymap["command_change_theme"], "t");
    }

    #[test]
    fn parses_documented_key_names_and_aliases() {
        assert_eq!(
            "ctrl+shift+s".parse::<KeySpec>().unwrap().code(),
            KeyCode::Char('s')
        );
        assert_eq!(
            "ctrl+shift+s".parse::<KeySpec>().unwrap().modifiers(),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        );
        assert_eq!(
            "ctrl+pagedown".parse::<KeySpec>().unwrap().code(),
            KeyCode::PageDown
        );
        assert_eq!("alt+up".parse::<KeySpec>().unwrap().code(), KeyCode::Up);
        assert_eq!("f1".parse::<KeySpec>().unwrap().code(), KeyCode::F(1));
        assert_eq!(
            "slash".parse::<KeySpec>().unwrap().code(),
            KeyCode::Char('/')
        );
        assert_eq!(
            "question_mark".parse::<KeySpec>().unwrap(),
            "?".parse().unwrap()
        );
        assert_eq!(
            "left_square_bracket".parse::<KeySpec>().unwrap(),
            "[".parse().unwrap()
        );
    }

    #[test]
    fn modifiers_match_exactly() {
        let keymap = Keymap::default();
        let control_s = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        let control_shift_s = KeyEvent::new(
            KeyCode::Char('s'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        assert!(keymap.matches("save", control_s));
        assert!(!keymap.matches("save", control_shift_s));
        assert!(keymap.matches("save_as", control_shift_s));
    }

    #[test]
    fn uppercase_command_keys_follow_crossterm_normalization() {
        let keymap = Keymap::default();
        let shifted_w = KeyEvent::new(KeyCode::Char('W'), KeyModifiers::SHIFT);

        assert!(keymap.matches("command_save_as", shifted_w));
        assert!(!keymap.matches("command_save", shifted_w));
        assert_eq!(
            keymap.action_for(BindingScope::Command, shifted_w),
            Some(Action::SaveAs)
        );
    }

    #[test]
    fn overrides_are_trimmed_resolved_and_matchable() {
        let source = overrides(&[("save", "ctrl+alt+s"), ("redo", "ctrl+r, ctrl+shift+r")]);
        let keymap = Keymap::resolve(&source).unwrap();

        assert_eq!(keymap["save"], "ctrl+alt+s");
        assert_eq!(keymap["redo"], "ctrl+r,ctrl+shift+r");
        assert_eq!(keymap["quit"], "ctrl+q");
        assert_eq!(
            keymap.action_for(
                BindingScope::Global,
                KeyEvent::new(
                    KeyCode::Char('s'),
                    KeyModifiers::CONTROL | KeyModifiers::ALT
                )
            ),
            Some(Action::Save)
        );
    }

    #[test]
    fn rejects_unknown_empty_invalid_reserved_and_colliding_overrides() {
        assert!(matches!(
            Keymap::resolve(&overrides(&[("unknown", "ctrl+x")])),
            Err(BindingError::UnknownId(id)) if id == "unknown"
        ));
        assert!(matches!(
            Keymap::resolve(&overrides(&[("save", "ctrl+x, ")])),
            Err(BindingError::EmptyBinding(id)) if id == "save"
        ));
        assert!(matches!(
            Keymap::resolve(&overrides(&[("save", "ctrl+not-a-key")])),
            Err(BindingError::InvalidKey { id, .. }) if id == "save"
        ));
        assert!(matches!(
            Keymap::resolve(&overrides(&[("preview_next_heading", "shift+tab")])),
            Err(BindingError::ReservedPreviewKey(key)) if key == "shift+tab"
        ));
        assert!(matches!(
            Keymap::resolve(&overrides(&[("save", "ctrl+q")])),
            Err(BindingError::Collision { previous, current, .. })
                if previous == "save" && current == "quit"
        ));
    }

    #[test]
    fn punctuation_aliases_collide_like_the_python_frontend() {
        let result = Keymap::resolve(&overrides(&[("save", "?"), ("quit", "question_mark")]));

        assert!(matches!(
            result,
            Err(BindingError::Collision { previous, current, .. })
                if previous == "save" && current == "quit"
        ));
    }

    #[test]
    fn duplicate_tokens_within_one_binding_are_rejected() {
        let result = Keymap::resolve(&overrides(&[("save", "ctrl+x, ctrl+x")]));

        assert!(matches!(
            result,
            Err(BindingError::Collision { previous, current, .. })
                if previous == "save" && current == "save"
        ));
    }
}
