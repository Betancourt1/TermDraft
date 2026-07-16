use std::path::PathBuf;

use clap::Parser;

use termdraft::config::{self, Config};
use termdraft::{Workspace, app};

#[derive(Debug, Parser)]
#[allow(clippy::struct_excessive_bools)]
#[command(
    name = "termdraft-rs",
    version,
    about = "Experimental Rust port of TermDraft"
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
    println!(
        "Editor: startup={:?} view={:?} wrap={} line-numbers={} list-continuation={}",
        config.editor.startup_mode,
        config.editor.view_mode,
        config.editor.soft_wrap,
        config.editor.show_line_numbers,
        config.editor.auto_continue_lists
    );
    println!();
    println!("COMMAND shortcuts");
    for command in termdraft::app::COMMANDS {
        println!(
            "  {:<10} {:<8} {}",
            command.group, command.shortcut, command.label
        );
    }
    if !config.keybindings.is_empty() {
        println!();
        println!("Configured Python-frontend overrides (reported, not remapped in this port):");
        for (action, binding) in &config.keybindings {
            println!("  {action:<34} {binding}");
        }
    }
}
