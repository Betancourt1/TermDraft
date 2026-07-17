use std::path::PathBuf;

use clap::Parser;

use termdraft::config::{self, Config};
use termdraft::{Workspace, app};

const COMMAND_ROWS: &[(&str, &str)] = &[
    ("command_write_mode", "Enter WRITE mode"),
    ("command_save", "Save the current document"),
    ("command_save_as", "Save the current document as"),
    (
        "command_duplicate_document",
        "Duplicate the current document",
    ),
    ("command_quit", "Quit safely"),
    ("command_toggle_explorer", "Show or hide files"),
    ("command_find_file", "Find a text file"),
    ("command_recent_documents", "Open a recent document"),
    ("command_next_tab", "Activate the next open document tab"),
    (
        "command_previous_tab",
        "Activate the previous open document tab",
    ),
    ("command_close_tab", "Close the active tab safely"),
    ("command_search_text", "Search workspace text"),
    ("command_find_replace", "Find and replace in the document"),
    ("command_document_outline", "Open document outline"),
    ("command_toggle_preview", "Show, hide, or focus the preview"),
    ("command_undo", "Undo"),
    ("command_redo", "Redo"),
    ("command_reload_config", "Reload configuration"),
    ("command_manage_recovery", "Manage recovery drafts"),
    ("command_markdown_help", "Show Markdown syntax help"),
    ("command_inspect_semantic_blocks", "Inspect semantic blocks"),
    (
        "command_read_semantic_blocks",
        "Read semantic blocks experimentally",
    ),
    (
        "command_inspect_cursor_coordinates",
        "Inspect cursor coordinates",
    ),
    ("command_open_palette", "Open the command palette"),
    ("command_show_help", "Show shortcut help"),
    ("command_cursor_left", "Move left"),
    ("command_cursor_down", "Move down"),
    ("command_cursor_up", "Move up"),
    ("command_cursor_right", "Move right"),
    ("command_line_start", "Move to source-line start"),
    ("command_line_end", "Move to source-line end"),
    ("command_document_start", "Move to document start"),
    ("command_document_end", "Move to document end"),
];

const FILE_ROWS: &[(&str, &str)] = &[
    ("a", "Create a file or folder"),
    ("c", "Copy the selected file or folder"),
    ("x", "Cut the selected file or folder"),
    (
        "p",
        "Paste into the selected folder or beside the selected file",
    ),
    ("r", "Rename the selected file or folder"),
    ("m", "Move the selected file or folder"),
    ("d", "Move the selected file or folder to Trash"),
];

const CONFIGURED_ROWS: &[(&str, &str)] = &[
    ("save", "Save the current document"),
    ("save_as", "Save the active document under a new path"),
    ("quit", "Quit safely"),
    ("toggle_explorer", "Show or hide files"),
    ("find_file", "Find a text file"),
    ("recent_documents", "Open a recent document"),
    ("next_tab", "Activate the next open document tab"),
    ("previous_tab", "Activate the previous open document tab"),
    ("close_tab", "Close the active tab safely"),
    ("find_replace", "Find and replace in the active document"),
    (
        "search_text",
        "Search workspace text (literal / fuzzy / word / regex)",
    ),
    ("document_outline", "Search headings in the active document"),
    (
        "toggle_preview",
        "Show or hide preview; switch pane when narrow",
    ),
    (
        "preview_next_heading",
        "Select the next heading in the focused preview",
    ),
    (
        "preview_previous_heading",
        "Select the previous heading in the focused preview",
    ),
    ("undo", "Undo"),
    ("redo", "Redo"),
    ("command_palette", "Open the command palette"),
    ("show_help", "Show shortcut help"),
];

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(
    name = "termdraft",
    version,
    about = "A local-first Markdown editor for the terminal"
)]
struct Arguments {
    #[arg(default_value = ".")]
    target: PathBuf,

    /// Validate the target and print the indexed document count without opening the TUI.
    #[arg(long)]
    inspect: bool,

    /// Configuration directory (default: ~/.termdraft).
    #[arg(long)]
    config_dir: Option<PathBuf>,

    /// Keep the built-in Rust theme for this launch.
    #[arg(long)]
    safe_mode: bool,

    /// Create missing no-clobber configuration templates, then exit.
    #[arg(long, conflicts_with_all = ["config_path", "commands"])]
    init_config: bool,

    /// Print the resolved configuration paths, then exit.
    #[arg(long, conflicts_with_all = ["init_config", "commands"])]
    config_path: bool,

    /// Show the effective frontend settings and shortcuts, then exit.
    #[arg(long, conflicts_with_all = ["init_config", "config_path"])]
    commands: bool,
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let root = config::config_root(arguments.config_dir.as_deref())?;
    if arguments.init_config {
        let config = config::initialize(root)?;
        println!("Configuration: {}", config.config_path().display());
        println!("Theme:         {}", config.theme_path().display());
        return Ok(());
    }
    if arguments.config_path {
        println!("{}", root.join(config::CONFIG_FILE_NAME).display());
        println!("{}", root.join(config::THEME_FILE_NAME).display());
        return Ok(());
    }
    let config = config::load(root)?;
    if arguments.commands {
        print_commands(&config);
        return Ok(());
    }
    let workspace = Workspace::from_target(&arguments.target)?;
    if arguments.inspect {
        let files = workspace
            .scan()
            .into_iter()
            .filter(|entry| !entry.is_dir)
            .count();
        println!("{}\t{files} documents", workspace.root.display());
        return Ok(());
    }

    app::run_with_config(workspace, config)
}

fn print_commands(config: &Config) {
    print!("{}", format_command_help(config));
}

fn format_command_help(config: &Config) -> String {
    let mut command_rows = vec![("Esc".to_owned(), "Enter COMMAND mode")];
    command_rows.extend(
        COMMAND_ROWS.iter().map(|(id, description)| {
            (display_command_keys(binding_text(config, id)), *description)
        }),
    );
    let file_rows = FILE_ROWS
        .iter()
        .map(|(keys, description)| ((*keys).to_owned(), *description))
        .collect::<Vec<_>>();
    let configured_rows = CONFIGURED_ROWS
        .iter()
        .map(|(id, description)| (display_keys(binding_text(config, id)), *description))
        .collect::<Vec<_>>();
    let mut control_rows = vec![
        (
            "Up / Down / j / k in preview".to_owned(),
            "Scroll the focused preview",
        ),
        (
            "PageUp / PageDown in preview".to_owned(),
            "Scroll the focused preview by one page",
        ),
        (
            "Home / End / g / G in preview".to_owned(),
            "Move to the start or end of the preview",
        ),
        (
            "Left / Right / h / l in preview".to_owned(),
            "Scroll a wide preview table horizontally",
        ),
        (
            "0 / $ in preview".to_owned(),
            "Move to the left or right edge of a wide table",
        ),
        (
            "Tab / Shift+Tab in preview".to_owned(),
            "Preview links remain visible but cannot be selected",
        ),
        (
            "Enter in preview".to_owned(),
            "Links and footnotes remain inert",
        ),
    ];
    if config.editor.auto_continue_lists {
        control_rows.push((
            "Enter in a list".to_owned(),
            "Continue it; an empty marker ends it",
        ));
    }
    let width = command_rows
        .iter()
        .chain(&file_rows)
        .chain(&configured_rows)
        .chain(&control_rows)
        .map(|(keys, _)| keys.chars().count())
        .max()
        .unwrap_or_default()
        + 3;
    let command_palette = display_command_keys(binding_text(config, "command_open_palette"));
    let global_palette = display_keys(binding_text(config, "command_palette"));

    format!(
        "TermDraft commands\n\nModes and COMMAND keys\n{}\n\nFocused Files keys\n{}\n\nConfigured shortcuts (available in both modes)\n{}\n\nEditor and preview controls\n{}\n\nPress {command_palette} in COMMAND mode or {global_palette} in either mode to search all commands, including:\n  Save, find file, open recent,\n  next/previous/close tab, search workspace text,\n  toggle files, toggle preview,\n  undo, redo,\n  reload configuration, manage recovery drafts, inspect semantic blocks,\n  read semantic blocks experimentally, inspect cursor coordinates,\n  shortcut help,\n  Markdown syntax help, and safe quit.\n\nFocus Files to create, copy, cut, paste, rename, move, or trash workspace entries.\n",
        format_rows(&command_rows, width),
        format_rows(&file_rows, width),
        format_rows(&configured_rows, width),
        format_rows(&control_rows, width),
    )
}

fn binding_text<'a>(config: &'a Config, id: &str) -> &'a str {
    config
        .keybindings
        .binding(id)
        .expect("command help IDs must exist in the complete keymap")
        .text
        .as_str()
}

fn format_rows(rows: &[(String, &str)], width: usize) -> String {
    rows.iter()
        .map(|(keys, description)| format!("{keys:<width$}{description}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn display_keys(keys: &str) -> String {
    keys.split(',')
        .map(str::trim)
        .map(display_key)
        .collect::<Vec<_>>()
        .join(" / ")
}

fn display_command_keys(keys: &str) -> String {
    keys.split(',')
        .map(str::trim)
        .map(|key| {
            if key.chars().count() == 1 {
                key.to_owned()
            } else {
                display_key(key)
            }
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn display_key(key: &str) -> String {
    key.split('+')
        .map(|part| {
            let normalized = part.to_ascii_lowercase();
            match normalized.as_str() {
                "alt" | "option" => "Alt".to_owned(),
                "backslash" => "\\".to_owned(),
                "colon" => ":".to_owned(),
                "ctrl" | "control" => "Ctrl".to_owned(),
                "dollar_sign" => "$".to_owned(),
                "escape" | "esc" => "Esc".to_owned(),
                "question_mark" => "?".to_owned(),
                "shift" => "Shift".to_owned(),
                "slash" => "/".to_owned(),
                "super" | "cmd" | "command" => "Super".to_owned(),
                _ if part.chars().count() == 1
                    || normalized
                        .strip_prefix('f')
                        .is_some_and(|number| number.parse::<u8>().is_ok()) =>
                {
                    part.to_uppercase()
                }
                _ => normalized
                    .split('_')
                    .map(|word| {
                        let mut characters = word.chars();
                        characters.next().map_or_else(String::new, |first| {
                            format!("{}{}", first.to_ascii_uppercase(), characters.as_str())
                        })
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use clap::CommandFactory;
    use termdraft::bindings::Keymap;

    use super::*;

    #[test]
    fn cli_uses_the_canonical_product_name_and_package_version() {
        let command = Arguments::command();

        assert_eq!(command.get_name(), "termdraft");
        assert_eq!(command.get_version(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn command_help_matches_the_user_facing_python_sections() {
        let help = format_command_help(&Config::default());

        for section in [
            "TermDraft commands",
            "Modes and COMMAND keys",
            "Focused Files keys",
            "Configured shortcuts (available in both modes)",
            "Editor and preview controls",
        ] {
            assert!(help.contains(section));
        }
        for guidance in [
            "i                                 Enter WRITE mode",
            "a                                 Create a file or folder",
            "Ctrl+S                            Save the current document",
            "Preview links remain visible but cannot be selected",
            "Links and footnotes remain inert",
            "Left / Right / h / l in preview",
            "Move to the left or right edge of a wide table",
            "Enter in a list",
            "Press : in COMMAND mode or Ctrl+\\ in either mode",
        ] {
            assert!(
                help.contains(guidance),
                "missing command guidance {guidance}"
            );
        }
        assert!(!help.contains("command_write_mode"));
        assert!(!help.contains("Activate the selected preview link"));
    }

    #[test]
    fn command_help_uses_effective_remapped_keys_and_editor_settings() {
        let overrides = BTreeMap::from([
            ("command_write_mode".to_owned(), "t".to_owned()),
            ("command_open_palette".to_owned(), "@".to_owned()),
            ("command_palette".to_owned(), "ctrl+alt+p".to_owned()),
            ("save".to_owned(), "ctrl+alt+s".to_owned()),
        ]);
        let config = Config {
            editor: termdraft::config::EditorConfig {
                auto_continue_lists: false,
                ..termdraft::config::EditorConfig::default()
            },
            keybindings: Keymap::resolve(&overrides).unwrap(),
            ..Config::default()
        };

        let help = format_command_help(&config);

        assert!(help.contains("t                                 Enter WRITE mode"));
        assert!(help.contains("Ctrl+Alt+S"));
        assert!(help.contains("Press @ in COMMAND mode or Ctrl+Alt+P in either mode"));
        assert!(!help.contains("Enter in a list"));
    }

    #[test]
    fn key_labels_preserve_command_case_and_format_configured_modifiers() {
        assert_eq!(display_command_keys("i,W,question_mark"), "i / W / ?");
        assert_eq!(
            display_keys("ctrl+y,super+y,ctrl+shift+z"),
            "Ctrl+Y / Super+Y / Ctrl+Shift+Z"
        );
        assert_eq!(display_keys("ctrl+pagedown"), "Ctrl+Pagedown");
    }
}
