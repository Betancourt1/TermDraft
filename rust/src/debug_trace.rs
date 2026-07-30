//! Lightweight runtime tracing for explicit debug sessions.

use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers};

pub(crate) struct DebugTrace {
    path: PathBuf,
    file: File,
    started_at: Instant,
    last_event: String,
    last_command: String,
    write_error: Option<String>,
}

impl DebugTrace {
    #[must_use]
    pub(crate) fn default_path() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        env::temp_dir().join(format!("termdraft-debug-{timestamp}-{}.log", process::id()))
    }

    pub(crate) fn create(path: &Path, workspace: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path)?;
        let mut trace = Self {
            path: path.to_path_buf(),
            file,
            started_at: Instant::now(),
            last_event: "waiting for input".to_owned(),
            last_command: "none".to_owned(),
            write_error: None,
        };
        trace.write_line(
            "session",
            &format!(
                "start version={} os={} arch={} pid={} workspace={}",
                env!("CARGO_PKG_VERSION"),
                env::consts::OS,
                env::consts::ARCH,
                process::id(),
                workspace.display()
            ),
        );
        trace.write_line(
            "terminal",
            &format!(
                "TERM={} TERM_PROGRAM={} COLORTERM={} XDG_SESSION_TYPE={}",
                environment_value("TERM"),
                environment_value("TERM_PROGRAM"),
                environment_value("COLORTERM"),
                environment_value("XDG_SESSION_TYPE")
            ),
        );
        Ok(trace)
    }

    pub(crate) fn record_event(&mut self, event: &Event, redact_text: bool, context: &str) {
        self.last_event = format!("{} · {context}", event_label(event, redact_text));
        let message = self.last_event.clone();
        self.write_line("event", &message);
    }

    pub(crate) fn record_command(&mut self, command: &str, context: &str) {
        self.last_command = format!("{command} · {context}");
        let message = self.last_command.clone();
        self.write_line("command", &message);
    }

    pub(crate) fn record_error(&mut self, error: &str, context: &str) {
        self.last_event = format!("ERROR {error} · {context}");
        let message = self.last_event.clone();
        self.write_line("error", &message);
    }

    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub(crate) fn last_event(&self) -> &str {
        &self.last_event
    }

    #[must_use]
    pub(crate) fn last_command(&self) -> &str {
        &self.last_command
    }

    fn write_line(&mut self, category: &str, message: &str) {
        if self.write_error.is_some() {
            return;
        }
        let elapsed = self.started_at.elapsed().as_millis();
        if let Err(error) = writeln!(self.file, "{elapsed:>8}ms {category:<8} {message}")
            .and_then(|()| self.file.flush())
        {
            let error = error.to_string();
            self.last_event = format!("debug log write failed · {error}");
            self.write_error = Some(error);
        }
    }
}

impl Drop for DebugTrace {
    fn drop(&mut self) {
        self.write_line(
            "session",
            if std::thread::panicking() {
                "end panicking=true"
            } else {
                "end panicking=false"
            },
        );
    }
}

fn environment_value(name: &str) -> String {
    env::var_os(name).map_or_else(
        || "-".to_owned(),
        |value| {
            format!("{:?}", value.to_string_lossy())
                .replace('\n', " ")
                .replace('\r', " ")
        },
    )
}

fn event_label(event: &Event, redact_text: bool) -> String {
    match event {
        Event::Key(key) => {
            let redact_character = redact_text
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
            let code = match key.code {
                KeyCode::Char(_) if redact_character => "Char(<text>)".to_owned(),
                KeyCode::Char(character) => format!("Char({character:?})"),
                code => format!("{code:?}"),
            };
            format!(
                "key code={code} modifiers={:?} kind={:?} state={:?}",
                key.modifiers, key.kind, key.state
            )
        }
        Event::Mouse(mouse) => format!(
            "mouse kind={:?} column={} row={} modifiers={:?}",
            mouse.kind, mouse.column, mouse.row, mouse.modifiers
        ),
        Event::Paste(text) => format!(
            "paste bytes={} characters={}",
            text.len(),
            text.chars().count()
        ),
        Event::Resize(columns, rows) => format!("resize columns={columns} rows={rows}"),
        Event::FocusGained => "focus gained".to_owned(),
        Event::FocusLost => "focus lost".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ratatui::crossterm::event::{KeyEvent, KeyEventKind};

    use super::*;

    #[test]
    fn trace_redacts_written_text_and_flushes_commands_to_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("debug.log");
        {
            let mut trace = DebugTrace::create(&path, directory.path()).unwrap();
            trace.record_event(
                &Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Char('s'),
                    KeyModifiers::NONE,
                    KeyEventKind::Press,
                )),
                true,
                "mode=WRITE focus=Editor overlay=false",
            );
            trace.record_command("Save", "mode=WRITE focus=Editor overlay=false");

            assert!(trace.last_event().contains("Char(<text>)"));
            assert!(trace.last_command().starts_with("Save"));
        }

        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.contains("session  start version="));
        assert!(contents.contains("event    key code=Char(<text>)"));
        assert!(!contents.contains("Char('s')"));
        assert!(contents.contains("command  Save"));
        assert!(contents.contains("session  end panicking=false"));
    }
}
