//! Explicit, same-user local bridge for active-draft agent proposals.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tempfile::{Builder, TempDir};

pub const PROTOCOL_VERSION: u8 = 2;
pub const MAX_AGENT_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_AGENT_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_REQUEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = MAX_AGENT_RESPONSE_BYTES as u64;
const MAX_CONNECTION_BYTES: u64 = 4 * 1024;
const APPLICATION_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const WORKSPACE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const SESSION_ROOT: &str = "/tmp";
const SESSION_PREFIX: &str = "termdraft-agent-";
const CONNECTION_FILE: &str = "connection.json";
const SOCKET_FILE: &str = "session.sock";

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
    ReadWorkspace,
    Propose {
        expected_path: String,
        expected_revision: String,
        change: ProposalChange,
        origin: Option<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWorkspaceDocument {
    pub path: String,
    pub source: String,
    pub revision: String,
    pub dirty: bool,
    pub open: bool,
    pub conflict: bool,
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
    Workspace {
        documents: Vec<AgentWorkspaceDocument>,
        warnings: Vec<String>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConnection {
    pub socket_path: PathBuf,
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredConnection {
    version: u8,
    socket_path: PathBuf,
    token: String,
}

pub struct AgentCall {
    pub action: AgentAction,
    response: Sender<AgentResponse>,
    shutdown: Arc<AtomicBool>,
}

impl AgentCall {
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    pub fn respond(self, response: AgentResponse) {
        let _ = self.response.send(response);
    }
}

pub struct AgentSession {
    shared_root: PathBuf,
    socket_path: PathBuf,
    token: String,
    requests: Receiver<AgentCall>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    _directory: TempDir,
}

impl AgentSession {
    /// Open a private, short-lived Unix socket for one explicitly shared workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when the private directory, socket, permissions, or random token cannot be
    /// created.
    pub fn start(shared_root: PathBuf) -> io::Result<Self> {
        Self::start_in(Path::new(SESSION_ROOT), shared_root)
    }

    fn start_in(root: &Path, shared_root: PathBuf) -> io::Result<Self> {
        let directory = Builder::new().prefix(SESSION_PREFIX).tempdir_in(root)?;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))?;
        let socket_path = directory.path().join(SOCKET_FILE);
        let listener = UnixListener::bind(&socket_path)?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let token = random_token()?;
        write_connection(
            directory.path(),
            &AgentConnection {
                socket_path: socket_path.clone(),
                token: token.clone(),
            },
        )
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot publish private agent connection: {error}"),
            )
        })?;
        let worker_token = token.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = Arc::clone(&shutdown);
        let (request_tx, requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            serve(&listener, &worker_token, &request_tx, &worker_shutdown);
        });

        Ok(Self {
            shared_root,
            socket_path,
            token,
            requests,
            shutdown,
            worker: Some(worker),
            _directory: directory,
        })
    }

    #[must_use]
    pub fn shared_root(&self) -> &Path {
        &self.shared_root
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

/// Discover the one live Agent sharing session opened by the current local user.
///
/// # Errors
///
/// Returns an error when no live session exists, multiple sessions are active, or the temporary
/// session root cannot be read.
pub fn discover_connection() -> io::Result<AgentConnection> {
    discover_connection_in(Path::new(SESSION_ROOT))
}

fn discover_connection_in(root: &Path) -> io::Result<AgentConnection> {
    discover_connection_with(root, |socket_path| {
        UnixStream::connect(socket_path).map(drop)
    })
}

fn discover_connection_with(
    root: &Path,
    mut connect: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<AgentConnection> {
    let mut connections = Vec::new();
    let mut permission_denied = false;
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(SESSION_PREFIX)
        {
            continue;
        }
        let Some(connection) = read_private_connection(&entry.path()) else {
            continue;
        };
        match connect(&connection.socket_path) {
            Ok(()) => connections.push(connection),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                permission_denied = true;
            }
            Err(_) => {}
        }
    }
    if permission_denied {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "TermDraft Agent sharing session found, but access to its Unix socket was denied; allow local socket access and retry",
        ));
    }
    match connections.len() {
        0 => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no active TermDraft Agent sharing session; open Agent sharing in TermDraft",
        )),
        1 => Ok(connections.pop().unwrap()),
        count => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "found {count} active TermDraft Agent sharing sessions; revoke the sessions you do not want to use"
            ),
        )),
    }
}

fn write_connection(directory: &Path, connection: &AgentConnection) -> io::Result<()> {
    let path = directory.join(CONNECTION_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    serde_json::to_writer(
        &mut file,
        &StoredConnection {
            version: PROTOCOL_VERSION,
            socket_path: connection.socket_path.clone(),
            token: connection.token.clone(),
        },
    )
    .map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn read_private_connection(directory: &Path) -> Option<AgentConnection> {
    let directory_metadata = fs::symlink_metadata(directory).ok()?;
    if !directory_metadata.is_dir() || directory_metadata.permissions().mode() & 0o777 != 0o700 {
        return None;
    }

    let path = directory.join(CONNECTION_FILE);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o777 != 0o600 {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .ok()?
        .take(MAX_CONNECTION_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_CONNECTION_BYTES {
        return None;
    }
    let stored: StoredConnection = serde_json::from_slice(&bytes).ok()?;
    if stored.version != PROTOCOL_VERSION
        || stored.socket_path != directory.join(SOCKET_FILE)
        || stored.token.len() != 64
        || !stored
            .token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let socket_metadata = fs::symlink_metadata(&stored.socket_path).ok()?;
    if !socket_metadata.file_type().is_socket()
        || socket_metadata.permissions().mode() & 0o777 != 0o600
    {
        return None;
    }
    Some(AgentConnection {
        socket_path: stored.socket_path,
        token: stored.token,
    })
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
    let read_timeout = if matches!(action, AgentAction::ReadWorkspace) {
        WORKSPACE_RESPONSE_TIMEOUT + Duration::from_secs(5)
    } else {
        Duration::from_secs(10)
    };
    stream.set_read_timeout(Some(read_timeout))?;
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
    shutdown: &Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                handle_connection(stream, token, request_tx, shutdown);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
}

fn handle_connection(
    mut stream: UnixStream,
    token: &str,
    request_tx: &Sender<AgentCall>,
    shutdown: &Arc<AtomicBool>,
) {
    if let Err(error) = stream.set_nonblocking(false) {
        let response = AgentResponse::error(format!("cannot configure agent connection: {error}"));
        let _ = write_response(&mut stream, &response);
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(WORKSPACE_RESPONSE_TIMEOUT));
    let response = read_request(&mut stream, token).and_then(|action| {
        let response_timeout = if matches!(action, AgentAction::ReadWorkspace) {
            WORKSPACE_RESPONSE_TIMEOUT
        } else {
            APPLICATION_RESPONSE_TIMEOUT
        };
        let (response_tx, response_rx) = mpsc::channel();
        request_tx
            .send(AgentCall {
                action,
                response: response_tx,
                shutdown: Arc::clone(shutdown),
            })
            .map_err(|_| "TermDraft closed the sharing session".to_owned())?;
        let deadline = Instant::now() + response_timeout;
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return Err("TermDraft closed the sharing session".to_owned());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("TermDraft did not answer the request in time".to_owned());
            }
            match response_rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(response) => return Ok(response),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("TermDraft closed the sharing session".to_owned());
                }
            }
        }
    });
    let response = response.unwrap_or_else(AgentResponse::error);
    let _ = write_response(&mut stream, &response);
}

fn write_response(stream: &mut UnixStream, response: &AgentResponse) -> io::Result<()> {
    write_response_with_limit(stream, response, MAX_RESPONSE_BYTES)
}

fn write_response_with_limit(
    writer: &mut impl Write,
    response: &AgentResponse,
    limit: u64,
) -> io::Result<()> {
    if json_size(response)? >= limit {
        serde_json::to_writer(
            &mut *writer,
            &AgentResponse::error("agent workspace response exceeds the 64 MiB limit"),
        )
        .map_err(io::Error::other)?;
    } else {
        serde_json::to_writer(&mut *writer, response).map_err(io::Error::other)?;
    }
    writer.write_all(b"\n")?;
    writer.flush()
}

#[derive(Default)]
struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| io::Error::other("serialized agent response is too large"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn json_size(value: &impl Serialize) -> io::Result<u64> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_err(io::Error::other)?;
    Ok(counter.0)
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
    fn private_connection_is_discovered_without_copying_credentials() {
        let root = tempfile::tempdir().unwrap();
        let session =
            AgentSession::start_in(root.path(), PathBuf::from("/workspace/draft.md")).unwrap();

        let connection = discover_connection_in(root.path()).unwrap();
        assert_eq!(connection.socket_path, session.socket_path());
        assert_eq!(connection.token, session.token());

        let (result_tx, result_rx) = mpsc::channel();
        let client = thread::spawn(move || {
            let _ = result_tx.send(request(
                &connection.socket_path,
                &connection.token,
                AgentAction::Read,
            ));
        });
        let deadline = Instant::now() + APPLICATION_RESPONSE_TIMEOUT;
        let call = loop {
            if let Some(call) = session.drain_calls().into_iter().next() {
                break call;
            }
            assert!(Instant::now() < deadline, "agent request was not delivered");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(call.action, AgentAction::Read);
        call.respond(AgentResponse::Document {
            path: "draft.md".to_owned(),
            source: "unsaved".to_owned(),
            revision: "1:abc".to_owned(),
            dirty: true,
        });
        assert!(matches!(
            result_rx.recv().unwrap().unwrap(),
            AgentResponse::Document { dirty: true, .. }
        ));
        client.join().unwrap();

        assert_eq!(
            fs::metadata(
                session
                    .socket_path()
                    .parent()
                    .unwrap()
                    .join(CONNECTION_FILE)
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn discovery_refuses_to_guess_between_live_sessions() {
        let root = tempfile::tempdir().unwrap();
        let _first =
            AgentSession::start_in(root.path(), PathBuf::from("/workspace/first.md")).unwrap();
        let _second =
            AgentSession::start_in(root.path(), PathBuf::from("/workspace/second.md")).unwrap();

        let error = discover_connection_in(root.path()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("2 active"));
    }

    #[test]
    fn discovery_reports_permission_denied_for_a_valid_session() {
        let root = tempfile::tempdir().unwrap();
        let _session =
            AgentSession::start_in(root.path(), PathBuf::from("/workspace/draft.md")).unwrap();

        let error = discover_connection_with(root.path(), |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sandbox denied socket access",
            ))
        })
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("allow local socket access"));
    }

    #[test]
    fn oversized_workspace_response_becomes_a_clear_protocol_error() {
        let response = AgentResponse::Workspace {
            documents: vec![AgentWorkspaceDocument {
                path: "large.md".to_owned(),
                source: "x".repeat(512),
                revision: "0:abc".to_owned(),
                dirty: false,
                open: false,
                conflict: false,
            }],
            warnings: Vec::new(),
        };

        let mut bytes = Vec::new();
        write_response_with_limit(&mut bytes, &response, 128).unwrap();
        let bounded: AgentResponse = serde_json::from_slice(&bytes).unwrap();

        assert!(matches!(
            bounded,
            AgentResponse::Error { ref message } if message.contains("64 MiB")
        ));
    }

    #[test]
    fn dropping_a_session_interrupts_a_pending_workspace_request() {
        let session = AgentSession::start(PathBuf::from("/workspace")).unwrap();
        let socket = session.socket_path().to_path_buf();
        let token = session.token().to_owned();
        let client = thread::spawn(move || request(&socket, &token, AgentAction::ReadWorkspace));
        let deadline = Instant::now() + Duration::from_secs(2);
        let call = loop {
            if let Some(call) = session.drain_calls().into_iter().next() {
                break call;
            }
            assert!(
                Instant::now() < deadline,
                "workspace request was not delivered"
            );
            thread::sleep(Duration::from_millis(5));
        };

        let started = Instant::now();
        drop(session);

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(call.is_cancelled());
        assert!(matches!(
            client.join().unwrap().unwrap(),
            AgentResponse::Error { ref message } if message.contains("closed the sharing session")
        ));
    }

    #[test]
    fn discovery_ignores_stale_and_non_private_descriptors() {
        let root = tempfile::tempdir().unwrap();
        let stale = root.path().join(format!("{SESSION_PREFIX}stale"));
        fs::create_dir(&stale).unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(stale.join(CONNECTION_FILE), b"{}\n").unwrap();
        fs::set_permissions(
            stale.join(CONNECTION_FILE),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        let exposed = root.path().join(format!("{SESSION_PREFIX}exposed"));
        fs::create_dir(&exposed).unwrap();
        fs::set_permissions(&exposed, fs::Permissions::from_mode(0o755)).unwrap();

        let error = discover_connection_in(root.path()).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
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
