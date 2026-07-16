use std::path::PathBuf;

use clap::Parser;

use termdraft::{Workspace, app};

#[derive(Debug, Parser)]
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
}

fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
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

    app::run(workspace)
}
