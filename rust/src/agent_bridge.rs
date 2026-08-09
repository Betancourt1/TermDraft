//! Explicit, same-user local bridge for active-draft agent proposals.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::{Builder, TempDir};

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_AGENT_SOURCE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const APPLICATION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RangeEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProposalChange {
    ReplaceSource { source: String },
    ReplaceRanges { edits: Vec<RangeEdit> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentAction {
    Read,
    Propose {
        expected_revision: String,
        change: ProposalChange,
        origin: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientRequest {
    version: u8,
    token: String,
    action: AgentAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentResponse {
    Document {
        path: String,
        source: String,
        revision: String,
        dirty: bool,
    },
    PendingReview {
        proposal_id: String,
    },
    Error {
        message: String,
    },
}

impl AgentResponse {
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }
}

pub struct AgentCall {
    pub action: AgentAction,
    response: Sender<AgentResponse>,
}

impl AgentCall {
    pub fn respond(self, response: AgentResponse) {
        let _ = self.response.send(response);
    }
}

pub struct AgentSession {
    shared_path: PathBuf,
    socket_path: PathBuf,
    token: String,
    requests: Receiver<AgentCall>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    _directory: TempDir,
}

impl AgentSession {
    /// Open a private, short-lived Unix socket for one explicitly shared document.
    ///
    /// # Errors
    ///
    /// Returns an error when the private directory, socket, permissions, or random token cannot be
    /// created.
    pub fn start(shared_path: PathBuf) -> io::Result<Self> {
        let directory = Builder::new()
            .prefix("termdraft-agent-")
            .tempdir_in("/tmp")?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        let socket_path = directory.path().join("session.sock");
        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let token = random_token()?;
        let worker_token = token.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let (request_tx, requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            serve(&listener, &worker_token, &request_tx, &worker_shutdown);
        });

        Ok(Self {
            shared_path,
            socket_path,
            token,
            requests,
            shutdown,
            worker: Some(worker),
            _directory: directory,
        })
    }

    #[must_use]
    pub fn shared_path(&self) -> &Path {
        &self.shared_path
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    #[must_use]
    pub fn drain_calls(&self) -> Vec<AgentCall> {
        self.requests.try_iter().collect()
    }
}

impl Drop for AgentSession {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

/// Send one authenticated request through the thin local client protocol.
///
/// # Errors
///
/// Returns an error when the socket cannot be reached, the bounded request or response cannot be
/// transferred, or the response is not valid protocol JSON.
pub fn request(socket_path: &Path, token: &str, action: AgentAction) -> io::Result<AgentResponse> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = ClientRequest {
        version: PROTOCOL_VERSION,
        token: token.to_owned(),
        action,
    };
    serde_json::to_writer(&mut stream, &request).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut bytes = Vec::new();
    BufReader::new(stream.take(MAX_RESPONSE_BYTES + 1)).read_until(b'\n', &mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_RESPONSE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "agent response exceeds the size limit",
        ));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn serve(
    listener: &UnixListener,
    token: &str,
    request_tx: &Sender<AgentCall>,
    shutdown: &AtomicBool,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => handle_connection(stream, token, request_tx),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

fn handle_connection(mut stream: UnixStream, token: &str, request_tx: &Sender<AgentCall>) {
    if let Err(error) = stream.set_nonblocking(false) {
        let response = AgentResponse::error(format!("cannot configure agent connection: {error}"));
        let _ = serde_json::to_writer(&mut stream, &response);
        let _ = stream.write_all(b"\n");
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let response = read_request(&mut stream, token).and_then(|action| {
        let (response_tx, response_rx) = mpsc::channel();
        request_tx
            .send(AgentCall {
                action,
                response: response_tx,
            })
            .map_err(|_| "TermDraft closed the sharing session".to_owned())?;
        response_rx
            .recv_timeout(APPLICATION_RESPONSE_TIMEOUT)
            .map_err(|_| "TermDraft did not answer the request in time".to_owned())
    });
    let response = response.unwrap_or_else(AgentResponse::error);
    let _ = serde_json::to_writer(&mut stream, &response);
    let _ = stream.write_all(b"\n");
    let _ = stream.flush();
}

fn read_request(stream: &mut UnixStream, token: &str) -> Result<AgentAction, String> {
    let mut bytes = Vec::new();
    BufReader::new((&mut *stream).take(MAX_REQUEST_BYTES + 1))
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("cannot read agent request: {error}"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_REQUEST_BYTES {
        return Err("agent request exceeds the size limit".to_owned());
    }
    let request: ClientRequest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid agent request: {error}"))?;
    if request.version != PROTOCOL_VERSION {
        return Err(format!(
            "unsupported agent protocol version {}",
            request.version
        ));
    }
    if !constant_time_eq(request.token.as_bytes(), token.as_bytes()) {
        return Err("agent session token is invalid or revoked".to_owned());
    }
    Ok(request.action)
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use super::*;

    #[test]
    fn private_session_round_trips_an_authenticated_read() {
        let session = AgentSession::start(PathBuf::from("/workspace/draft.md")).unwrap();
        let socket = session.socket_path().to_path_buf();
        let token = session.token().to_owned();
        assert_eq!(
            fs::metadata(socket.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let (result_tx, result_rx) = mpsc::channel();
        let client = thread::spawn(move || {
            let _ = result_tx.send(request(&socket, &token, AgentAction::Read));
        });
        let deadline = Instant::now() + APPLICATION_RESPONSE_TIMEOUT;
        let call = loop {
            if let Some(call) = session.drain_calls().into_iter().next() {
                break call;
            }
            if let Ok(result) = result_rx.try_recv() {
                panic!("agent client ended before delivery: {result:?}");
            }
            assert!(Instant::now() < deadline, "agent request was not delivered");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(call.action, AgentAction::Read);
        call.respond(AgentResponse::Document {
            path: "draft.md".to_owned(),
            source: "unsaved".to_owned(),
            revision: "2:abc".to_owned(),
            dirty: true,
        });

        assert_eq!(
            result_rx.recv().unwrap().unwrap(),
            AgentResponse::Document {
                path: "draft.md".to_owned(),
                source: "unsaved".to_owned(),
                revision: "2:abc".to_owned(),
                dirty: true,
            }
        );
        client.join().unwrap();
    }

    #[test]
    fn invalid_token_never_reaches_the_application() {
        let session = AgentSession::start(PathBuf::from("/workspace/draft.md")).unwrap();
        let response = request(session.socket_path(), "wrong", AgentAction::Read).unwrap();

        assert!(matches!(response, AgentResponse::Error { .. }));
        assert!(session.drain_calls().is_empty());
    }

    #[test]
    fn dropping_a_session_removes_its_endpoint() {
        let socket = {
            let session = AgentSession::start(PathBuf::from("/workspace/draft.md")).unwrap();
            let socket = session.socket_path().to_path_buf();
            assert!(socket.exists());
            socket
        };

        assert!(!socket.exists());
        assert!(!socket.parent().unwrap().exists());
    }
}
