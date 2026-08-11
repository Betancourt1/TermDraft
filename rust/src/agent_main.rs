use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};
use termdraft::agent_bridge::{
    AgentAction, AgentConnection, MAX_AGENT_SOURCE_BYTES, ProposalChange, RangeEdit,
    discover_connection, request,
};

#[derive(Debug, Parser)]
#[command(
    name = "termdraft-agent",
    version,
    about = "Read one explicitly shared TermDraft buffer and submit reviewable proposals"
)]
struct Arguments {
    #[command(subcommand)]
    command: AgentCommand,
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Read the exact live unsaved source and its revision.
    Read(Connection),
    /// Propose a complete replacement from a UTF-8 file or standard input.
    Propose {
        #[command(flatten)]
        connection: Connection,
        /// Revision returned by `read`.
        #[arg(long)]
        revision: String,
        /// Short identifier shown in `TermDraft`'s review overlay.
        #[arg(long, default_value = "termdraft-agent")]
        origin: String,
        /// UTF-8 replacement file; use `-` or omit it for standard input.
        #[arg(default_value = "-")]
        source: PathBuf,
    },
    /// Propose non-overlapping UTF-8 byte ranges from a JSON file or standard input.
    ProposeRanges {
        #[command(flatten)]
        connection: Connection,
        /// Revision returned by `read`.
        #[arg(long)]
        revision: String,
        /// Short identifier shown in `TermDraft`'s review overlay.
        #[arg(long, default_value = "termdraft-agent")]
        origin: String,
        /// JSON array of `{ "start", "end", "replacement" }`; `-` reads standard input.
        #[arg(default_value = "-")]
        edits: PathBuf,
    },
}

#[derive(Clone, Debug, Args)]
struct Connection {
    /// Override automatic discovery with a specific session socket.
    #[arg(long, requires = "token")]
    socket: Option<PathBuf>,
    /// Token for an explicitly selected session socket.
    #[arg(long, requires = "socket")]
    token: Option<String>,
}

impl Connection {
    fn resolve(&self) -> anyhow::Result<AgentConnection> {
        match (&self.socket, &self.token) {
            (Some(socket_path), Some(token)) => Ok(AgentConnection {
                socket_path: socket_path.clone(),
                token: token.clone(),
            }),
            (None, None) => Ok(discover_connection()?),
            _ => unreachable!("clap requires socket and token together"),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let action = match Arguments::parse().command {
        AgentCommand::Read(connection) => return send(&connection, AgentAction::Read),
        AgentCommand::Propose {
            connection,
            revision,
            origin,
            source,
        } => (
            connection,
            AgentAction::Propose {
                expected_revision: revision,
                change: ProposalChange::ReplaceSource {
                    source: read_bounded_utf8(&source)?,
                },
                origin: Some(origin),
            },
        ),
        AgentCommand::ProposeRanges {
            connection,
            revision,
            origin,
            edits,
        } => {
            let source = read_bounded_utf8(&edits)?;
            let edits: Vec<RangeEdit> = serde_json::from_str(&source)?;
            (
                connection,
                AgentAction::Propose {
                    expected_revision: revision,
                    change: ProposalChange::ReplaceRanges { edits },
                    origin: Some(origin),
                },
            )
        }
    };
    send(&action.0, action.1)
}

fn send(connection: &Connection, action: AgentAction) -> anyhow::Result<()> {
    let connection = connection.resolve()?;
    let response = request(&connection.socket_path, &connection.token, action)?;
    serde_json::to_writer_pretty(io::stdout().lock(), &response)?;
    println!();
    if response.is_error() {
        std::process::exit(2);
    }
    Ok(())
}

fn read_bounded_utf8(path: &Path) -> anyhow::Result<String> {
    let reader: Box<dyn Read> = if path == Path::new("-") {
        Box::new(io::stdin().lock())
    } else {
        Box::new(File::open(path)?)
    };
    let mut bytes = Vec::new();
    reader
        .take(u64::try_from(MAX_AGENT_SOURCE_BYTES).unwrap() + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_AGENT_SOURCE_BYTES {
        anyhow::bail!("agent proposal exceeds the 16 MiB source limit");
    }
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_cli_uses_discovery_by_default_and_keeps_explicit_overrides_paired() {
        use clap::CommandFactory;

        let command = Arguments::command();
        assert_eq!(command.get_name(), "termdraft-agent");
        assert!(Arguments::try_parse_from(["termdraft-agent", "read"]).is_ok());
        assert!(
            Arguments::try_parse_from(["termdraft-agent", "read", "--socket", "/tmp/session.sock"])
                .is_err()
        );
        assert!(
            Arguments::try_parse_from([
                "termdraft-agent",
                "read",
                "--socket",
                "/tmp/session.sock",
                "--token",
                "secret"
            ])
            .is_ok()
        );
    }

    #[test]
    fn range_input_uses_the_documented_wire_shape() {
        let edits: Vec<RangeEdit> =
            serde_json::from_str(r#"[{"start":0,"end":4,"replacement":"Draft"}]"#).unwrap();

        assert_eq!(
            edits,
            vec![RangeEdit {
                start: 0,
                end: 4,
                replacement: "Draft".to_owned(),
            }]
        );
    }
}
