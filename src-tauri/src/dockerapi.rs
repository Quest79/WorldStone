//! Asking the Docker engine directly.
//!
//! Every status card is a question for Docker, and starting `docker.exe` to ask costs 300 to 600 ms
//! before Docker is even reached. A refresh asks dozens of questions at once, so cards took seconds.
//! The engine answers the same questions on its own local socket (a named pipe on Windows) in a few
//! milliseconds, so read-only questions are put to its HTTP API there.
//!
//! The `docker` program is still what builds, runs, stops and removes things: that is real work with
//! output the task log shows. This module only reads, plus the two short helpers a read needs (a
//! command run in a running container, and a file read out of a volume).
//!
//! The endpoint is the one the `docker` program itself would use (`DOCKER_HOST`, then
//! `DOCKER_CONTEXT`, then the current context in Docker's config folder), so a status and a build
//! never look at different engines. When that endpoint is not a local named pipe (a remote engine,
//! another operating system), [`ApiError::Unsupported`] says so and callers use the `docker` program.
//!
//! Answers are never kept longer than [`FRESH_FOR`], and never across a `docker` program run by this
//! app ([`invalidate`]), so what a card shows is what Docker said a moment ago.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// How long one question is asked to wait for its answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a shared answer (see [`Coalesced`]) may be reused.
const FRESH_FOR: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// The engine is not on a local named pipe: use the `docker` program.
    Unsupported,
    /// Nothing answered on the engine's pipe (Docker is not running, or is hung).
    Unreachable(String),
    /// The engine answered with an error.
    Http { status: u16, message: String },
    /// The engine's answer could not be understood.
    Protocol(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "The Docker engine is not on a local pipe."),
            Self::Unreachable(why) => write!(f, "Docker did not answer: {why}"),
            Self::Http { status, message } => write!(f, "Docker said {status}: {message}"),
            Self::Protocol(why) => write!(f, "Docker gave an answer that could not be read: {why}"),
        }
    }
}

fn protocol(why: &str) -> ApiError {
    ApiError::Protocol(why.to_string())
}

// ---------------------------------------------------------------------------------------
// Which engine
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Windows named pipe, such as `\\.\pipe\docker_engine`.
    Pipe(String),
    /// Anything else (`tcp://`, `ssh://`, a socket file, an unknown context).
    Other(String),
}

const DEFAULT_HOST: &str = "npipe:////./pipe/docker_engine";

/// Reads a Docker host such as `npipe:////./pipe/docker_engine`.
pub fn parse_host(host: &str) -> Endpoint {
    let host = host.trim();
    match host.strip_prefix("npipe://") {
        Some(path) if !path.is_empty() => Endpoint::Pipe(path.replace('/', "\\")),
        _ => Endpoint::Other(host.to_string()),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn current_context(config_dir: &Path) -> Option<String> {
    let name = read_json(&config_dir.join("config.json"))?.get("currentContext")?.as_str()?.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// The host a named context points at. Docker keeps a context's metadata in a folder named after
/// the SHA-256 of its name.
fn context_host(config_dir: &Path, name: &str) -> Option<String> {
    let digest: String = Sha256::digest(name.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let meta = read_json(&config_dir.join("contexts").join("meta").join(digest).join("meta.json"))?;
    meta.get("Endpoints")?.get("docker")?.get("Host")?.as_str().map(str::to_string)
}

/// The engine the `docker` program would talk to, from its environment and config folder.
pub fn resolve_endpoint(var: &dyn Fn(&str) -> Option<String>, config_dir: Option<&Path>) -> Endpoint {
    let set = |name: &str| var(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    if let Some(host) = set("DOCKER_HOST") {
        return parse_host(&host);
    }
    let context = set("DOCKER_CONTEXT").or_else(|| config_dir.and_then(current_context));
    match context.as_deref() {
        None | Some("default") => parse_host(DEFAULT_HOST),
        Some(name) => match config_dir.and_then(|dir| context_host(dir, name)) {
            Some(host) => parse_host(&host),
            None => Endpoint::Other(format!("docker context {name}")),
        },
    }
}

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DOCKER_CONFIG").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".docker"))
}

/// Read on every question: a few small files, and the person may switch Docker context at any time.
fn current_endpoint() -> Endpoint {
    resolve_endpoint(&|name| std::env::var(name).ok(), config_dir().as_deref())
}

// ---------------------------------------------------------------------------------------
// HTTP on the pipe
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The engine's own message for a failure (`{"message": "..."}`), or the body as it came.
    fn message(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|v| v.get("message")?.as_str().map(str::to_string))
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).trim().to_string())
    }

    /// The response itself when it succeeded, otherwise the engine's error.
    pub fn ok(self) -> Result<Self, ApiError> {
        if self.is_success() {
            Ok(self)
        } else {
            Err(ApiError::Http { status: self.status, message: self.message() })
        }
    }

    pub fn json(&self) -> Result<Value, ApiError> {
        serde_json::from_slice(&self.body).map_err(|e| ApiError::Protocol(format!("not JSON ({e})")))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, ApiError> {
    let mut out = Vec::new();
    loop {
        let eol = find(data, b"\r\n").ok_or_else(|| protocol("a chunked answer was cut short"))?;
        let line = String::from_utf8_lossy(&data[..eol]);
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).map_err(|_| protocol("a chunk had no size"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err(protocol("a chunked answer was cut short"));
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// Reads a whole HTTP/1.1 response (the request asked for the connection to close after it).
pub fn parse_response(raw: &[u8]) -> Result<Response, ApiError> {
    let end = find(raw, b"\r\n\r\n").ok_or_else(|| protocol("the answer ended before its headers did"))?;
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| protocol("the answer had no status"))?;
    let (mut chunked, mut length) = (false, None);
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
                "content-length" => length = value.trim().parse::<usize>().ok(),
                _ => {}
            }
        }
    }
    let rest = &raw[end + 4..];
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some(n) = length {
        rest.get(..n).ok_or_else(|| protocol("the answer was cut short"))?.to_vec()
    } else {
        rest.to_vec()
    };
    Ok(Response { status, body })
}

pub fn build_request(method: &str, path: &str, body: Option<&[u8]>) -> Vec<u8> {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: docker\r\nUser-Agent: worldstone\r\nConnection: close\r\n");
    if let Some(body) = body {
        head.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    } else if method != "GET" && method != "HEAD" {
        head.push_str("Content-Length: 0\r\n");
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    if let Some(body) = body {
        out.extend_from_slice(body);
    }
    out
}

#[cfg(windows)]
async fn pipe_exchange(pipe: &str, request: &[u8], timeout: Duration) -> Result<Vec<u8>, ApiError> {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::windows::named_pipe::ClientOptions,
    };
    /// Every instance of the pipe is serving someone else at this moment.
    const PIPE_BUSY: i32 = 231;
    let unreachable = |e: std::io::Error| ApiError::Unreachable(e.to_string());
    let work = async {
        let started = Instant::now();
        let mut client = loop {
            match ClientOptions::new().open(pipe) {
                Ok(client) => break client,
                Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && started.elapsed() < Duration::from_secs(3) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => return Err(unreachable(e)),
            }
        };
        client.write_all(request).await.map_err(unreachable)?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match client.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                // The engine closing its end is how the answer finishes.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
                Err(e) => return Err(unreachable(e)),
            }
        }
        Ok(out)
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| ApiError::Unreachable("no answer in time".to_string()))?
}

#[cfg(not(windows))]
async fn pipe_exchange(_pipe: &str, _request: &[u8], _timeout: Duration) -> Result<Vec<u8>, ApiError> {
    Err(ApiError::Unsupported)
}

/// Puts one request to the engine and returns whatever it answers, error statuses included.
pub async fn request(method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Response, ApiError> {
    let Endpoint::Pipe(pipe) = current_endpoint() else { return Err(ApiError::Unsupported) };
    let payload = body.map(|b| serde_json::to_vec(b).map_err(|e| ApiError::Protocol(e.to_string()))).transpose()?;
    let raw = pipe_exchange(&pipe, &build_request(method, path, payload.as_deref()), timeout).await?;
    parse_response(&raw)
}

/// A GET whose answer is JSON.
pub async fn get_json(path: &str) -> Result<Value, ApiError> {
    request("GET", path, None, REQUEST_TIMEOUT).await?.ok()?.json()
}

/// Like [`get_json`], but a thing that does not exist (404) is `None`, not an error.
pub async fn find_json(path: &str) -> Result<Option<Value>, ApiError> {
    let response = request("GET", path, None, REQUEST_TIMEOUT).await?;
    if response.status == 404 {
        return Ok(None);
    }
    response.ok()?.json().map(Some)
}

/// Percent-encodes text for a URL path or query value. `keep` lists extra bytes that stay as they are.
pub fn encode(text: &str, keep: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.as_bytes().contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Commands in a container, files out of one
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub code: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Splits Docker's multiplexed stream (an 8-byte header per frame: stream, three zeros, big-endian
/// length) into what the command wrote to stdout and to stderr. A frame cut short keeps what came.
pub fn demux(raw: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut data = raw;
    while data.len() >= 8 {
        let size = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let end = 8usize.saturating_add(size).min(data.len());
        match data[0] {
            1 => out.extend_from_slice(&data[8..end]),
            2 => err.extend_from_slice(&data[8..end]),
            _ => {}
        }
        data = &data[end..];
    }
    (out, err)
}

/// Runs `cmd` in a running container (no terminal, no input) and waits for it. `env` is `KEY=value`
/// lines; they travel to the engine, never through the command line of any program.
pub async fn exec(container: &str, env: &[String], cmd: &[String], timeout: Duration) -> Result<ExecOutput, ApiError> {
    let create = json!({ "AttachStdout": true, "AttachStderr": true, "Tty": false, "Env": env, "Cmd": cmd });
    let created = request("POST", &format!("/containers/{}/exec", encode(container, "")), Some(&create), timeout).await?.ok()?.json()?;
    let id = created.get("Id").and_then(Value::as_str).ok_or_else(|| protocol("no exec id"))?.to_string();
    let start = json!({ "Detach": false, "Tty": false });
    let stream = request("POST", &format!("/exec/{id}/start"), Some(&start), timeout).await?.ok()?;
    let (stdout, stderr) = demux(&stream.body);
    // The stream can end a moment before the engine has recorded how the command exited.
    for _ in 0..300 {
        let info = get_json(&format!("/exec/{id}/json")).await?;
        if info.get("Running").and_then(Value::as_bool) == Some(false) {
            let code = info.get("ExitCode").and_then(Value::as_i64).ok_or_else(|| protocol("no exit code"))?;
            return Ok(ExecOutput { code, stdout, stderr });
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(protocol("the command did not report that it finished"))
}

/// The contents of the first regular file in a tar archive.
pub fn first_file(tar: &[u8]) -> Option<Vec<u8>> {
    let header = tar.get(..512)?;
    if header.iter().all(|b| *b == 0) || !matches!(header[156], 0 | b'0') {
        return None;
    }
    let size_field = std::str::from_utf8(&header[124..136]).ok()?.trim_matches(|c: char| c == '\0' || c == ' ');
    let size = usize::from_str_radix(size_field, 8).ok()?;
    tar.get(512..512 + size).map(<[u8]>::to_vec)
}

/// A regular file out of a container's file system (which includes its volumes), without running
/// anything in it: the container only has to exist. `None` when there is no such file.
pub async fn archive_file(container: &str, path: &str) -> Result<Option<Vec<u8>>, ApiError> {
    let url = format!("/containers/{}/archive?path={}", encode(container, ""), encode(path, "/"));
    let response = request("GET", &url, None, REQUEST_TIMEOUT).await?;
    if response.status == 404 {
        return Ok(None);
    }
    Ok(first_file(&response.ok()?.body))
}

// ---------------------------------------------------------------------------------------
// Freshness
// ---------------------------------------------------------------------------------------

/// Counts the times something this app did may have changed what Docker says. Answers gathered
/// before the latest change are never reused.
static CHANGES: AtomicU64 = AtomicU64::new(0);

/// Says that Docker's state may have changed (a `docker` program ran or is about to). Called from
/// `process.rs` around every run of `docker`.
pub fn invalidate() {
    CHANGES.fetch_add(1, Ordering::SeqCst);
}

struct Entry<T> {
    changes: u64,
    started: Instant,
    value: T,
}

/// One question many callers ask at once (every card asks whether Docker is running): the first
/// caller asks Docker, the others wait for that answer. The answer is reused for a moment, but not
/// past a change made by this app, and never when it was gathered before that change.
pub struct Coalesced<T> {
    changes: &'static AtomicU64,
    slot: tokio::sync::Mutex<Option<Entry<T>>>,
}

impl<T: Clone> Coalesced<T> {
    pub const fn new(changes: &'static AtomicU64) -> Self {
        Self { changes, slot: tokio::sync::Mutex::const_new(None) }
    }

    pub async fn get<F, Fut>(&self, ask: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let mut slot = self.slot.lock().await;
        let changes = self.changes.load(Ordering::SeqCst);
        if let Some(entry) = slot.as_ref() {
            if entry.changes == changes && entry.started.elapsed() < FRESH_FOR {
                return entry.value.clone();
            }
        }
        let started = Instant::now();
        let value = ask().await;
        *slot = Some(Entry { changes, started, value: value.clone() });
        value
    }
}

/// [`Coalesced`] tied to this app's own changes.
pub const fn shared<T: Clone>() -> Coalesced<T> {
    Coalesced::new(&CHANGES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn a_pipe_host_becomes_a_pipe_path() {
        assert_eq!(parse_host("npipe:////./pipe/docker_engine"), Endpoint::Pipe(r"\\.\pipe\docker_engine".to_string()));
        assert_eq!(parse_host("npipe:////./pipe/dockerDesktopLinuxEngine"), Endpoint::Pipe(r"\\.\pipe\dockerDesktopLinuxEngine".to_string()));
        assert!(matches!(parse_host("tcp://10.0.0.5:2375"), Endpoint::Other(_)));
        assert!(matches!(parse_host("ssh://me@box"), Endpoint::Other(_)));
        assert!(matches!(parse_host("npipe://"), Endpoint::Other(_)));
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn without_any_setting_the_default_pipe_is_used() {
        assert_eq!(resolve_endpoint(&no_env, None), Endpoint::Pipe(r"\\.\pipe\docker_engine".to_string()));
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resolve_endpoint(&no_env, Some(dir.path())), Endpoint::Pipe(r"\\.\pipe\docker_engine".to_string()));
    }

    #[test]
    fn docker_host_wins_over_a_context() {
        let env = |name: &str| match name {
            "DOCKER_HOST" => Some("tcp://box:2375".to_string()),
            "DOCKER_CONTEXT" => Some("desktop-linux".to_string()),
            _ => None,
        };
        assert_eq!(resolve_endpoint(&env, None), Endpoint::Other("tcp://box:2375".to_string()));
        let blank = |name: &str| (name == "DOCKER_HOST").then(|| "  ".to_string());
        assert_eq!(resolve_endpoint(&blank, None), Endpoint::Pipe(r"\\.\pipe\docker_engine".to_string()));
    }

    fn write_context(dir: &Path, name: &str, host: &str) {
        let digest: String = Sha256::digest(name.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        let meta = dir.join("contexts").join("meta").join(digest);
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(meta.join("meta.json"), format!(r#"{{"Name":"{name}","Endpoints":{{"docker":{{"Host":"{host}"}}}}}}"#)).unwrap();
    }

    #[test]
    fn the_current_context_in_the_config_folder_is_followed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "\u{feff}{\"currentContext\":\"desktop-linux\"}").unwrap();
        write_context(dir.path(), "desktop-linux", "npipe:////./pipe/dockerDesktopLinuxEngine");
        assert_eq!(resolve_endpoint(&no_env, Some(dir.path())), Endpoint::Pipe(r"\\.\pipe\dockerDesktopLinuxEngine".to_string()));
        let env = |name: &str| (name == "DOCKER_CONTEXT").then(|| "default".to_string());
        assert_eq!(resolve_endpoint(&env, Some(dir.path())), Endpoint::Pipe(r"\\.\pipe\docker_engine".to_string()));
    }

    #[test]
    fn a_context_that_is_not_a_local_pipe_or_not_found_is_not_guessed_at() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), r#"{"currentContext":"remote"}"#).unwrap();
        assert!(matches!(resolve_endpoint(&no_env, Some(dir.path())), Endpoint::Other(_)));
        write_context(dir.path(), "remote", "ssh://me@box");
        assert_eq!(resolve_endpoint(&no_env, Some(dir.path())), Endpoint::Other("ssh://me@box".to_string()));
    }

    #[test]
    fn a_response_with_a_length_is_read() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}EXTRA";
        let r = parse_response(raw).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{\"a\":1}"[..]));
        assert!(matches!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nshort"), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn a_chunked_response_is_put_together() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n7;ext=1\r\n, world\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap().body, b"hello, world");
        let cut = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel";
        assert!(matches!(parse_response(cut), Err(ApiError::Protocol(_))));
        let no_end = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n";
        assert!(matches!(parse_response(no_end), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn a_response_without_length_runs_to_the_end() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.docker.raw-stream\r\n\r\nstream bytes";
        assert_eq!(parse_response(raw).unwrap().body, b"stream bytes");
        assert!(matches!(parse_response(b"garbage"), Err(ApiError::Protocol(_))));
        assert!(matches!(parse_response(b"HTTP/1.1 \r\n\r\n"), Err(ApiError::Protocol(_))));
    }

    #[test]
    fn an_error_carries_the_engines_message() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Length: 35\r\n\r\n{\"message\":\"No such container: db\"}";
        let r = parse_response(raw).unwrap();
        assert!(!r.is_success());
        assert_eq!(r.ok().unwrap_err(), ApiError::Http { status: 404, message: "No such container: db".to_string() });
    }

    #[test]
    fn requests_say_what_they_carry() {
        let get = String::from_utf8(build_request("GET", "/info", None)).unwrap();
        assert!(get.starts_with("GET /info HTTP/1.1\r\n") && get.ends_with("\r\n\r\n") && !get.contains("Content-Length"));
        let post = String::from_utf8(build_request("POST", "/x", Some(b"{}"))).unwrap();
        assert!(post.contains("Content-Length: 2\r\n") && post.ends_with("\r\n\r\n{}"));
        assert!(String::from_utf8(build_request("POST", "/x", None)).unwrap().contains("Content-Length: 0\r\n"));
    }

    #[test]
    fn url_text_is_encoded() {
        assert_eq!(encode("worldstone-db", ""), "worldstone-db");
        assert_eq!(encode("/data/current/record.json", "/"), "/data/current/record.json");
        assert_eq!(encode("a b&c", ""), "a%20b%26c");
        assert_eq!(encode("worldstone/wotlk-world:latest", "/:"), "worldstone/wotlk-world:latest");
    }

    fn frame(kind: u8, text: &str) -> Vec<u8> {
        let mut out = vec![kind, 0, 0, 0];
        out.extend((text.len() as u32).to_be_bytes());
        out.extend(text.as_bytes());
        out
    }

    #[test]
    fn the_multiplexed_stream_is_split_by_stream() {
        let mut raw = frame(1, "one\n");
        raw.extend(frame(2, "warn\n"));
        raw.extend(frame(1, "two\n"));
        let (out, err) = demux(&raw);
        assert_eq!((out.as_slice(), err.as_slice()), (&b"one\ntwo\n"[..], &b"warn\n"[..]));
        // A frame cut short keeps what arrived; a stray header is ignored.
        let mut cut = frame(1, "abcdef");
        cut.truncate(8 + 3);
        assert_eq!(demux(&cut).0, b"abc");
        assert_eq!(demux(&[1, 0, 0]), (Vec::new(), Vec::new()));
    }

    fn tar_of(name: &str, body: &[u8], kind: u8) -> Vec<u8> {
        let mut header = vec![0u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        let size = format!("{:011o}", body.len());
        header[124..135].copy_from_slice(size.as_bytes());
        header[156] = kind;
        let mut out = header;
        out.extend_from_slice(body);
        out.resize(512 + body.len().div_ceil(512) * 512, 0);
        out.extend(vec![0u8; 1024]);
        out
    }

    #[test]
    fn a_file_is_read_out_of_a_tar() {
        assert_eq!(first_file(&tar_of("record.json", b"{\"ok\":true}", b'0')).unwrap(), b"{\"ok\":true}");
        assert_eq!(first_file(&tar_of("empty", b"", 0)).unwrap(), b"");
        // A folder, an empty archive, or a body shorter than its header says is not a file.
        assert_eq!(first_file(&tar_of("dir", b"", b'5')), None);
        assert_eq!(first_file(&[0u8; 1024]), None);
        let mut short = tar_of("f", b"0123456789", b'0');
        short.truncate(512 + 4);
        assert_eq!(first_file(&short), None);
    }

    #[tokio::test]
    async fn one_question_is_asked_once_for_everyone_waiting() {
        static CHANGES: AtomicU64 = AtomicU64::new(0);
        static ASKED: AtomicUsize = AtomicUsize::new(0);
        static CACHE: Coalesced<usize> = Coalesced::new(&CHANGES);
        let ask = || async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            ASKED.fetch_add(1, Ordering::SeqCst) + 1
        };
        let answers = tokio::join!(CACHE.get(ask), CACHE.get(ask), CACHE.get(ask));
        assert_eq!(answers, (1, 1, 1));
        assert_eq!(ASKED.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_change_or_the_passing_of_time_means_asking_again() {
        static CHANGES: AtomicU64 = AtomicU64::new(0);
        static ASKED: AtomicUsize = AtomicUsize::new(0);
        static CACHE: Coalesced<usize> = Coalesced::new(&CHANGES);
        let ask = || async { ASKED.fetch_add(1, Ordering::SeqCst) + 1 };
        assert_eq!(CACHE.get(ask).await, 1);
        assert_eq!(CACHE.get(ask).await, 1);
        CHANGES.fetch_add(1, Ordering::SeqCst);
        assert_eq!(CACHE.get(ask).await, 2);
        tokio::time::sleep(FRESH_FOR + Duration::from_millis(50)).await;
        assert_eq!(CACHE.get(ask).await, 3);
    }

    #[tokio::test]
    async fn an_answer_gathered_before_a_change_is_not_kept() {
        static CHANGES: AtomicU64 = AtomicU64::new(0);
        static CACHE: Coalesced<&'static str> = Coalesced::new(&CHANGES);
        // Docker is asked, something changes while it answers, so the answer is already old.
        let old = CACHE
            .get(|| async {
                CHANGES.fetch_add(1, Ordering::SeqCst);
                "before"
            })
            .await;
        assert_eq!(old, "before");
        assert_eq!(CACHE.get(|| async { "after" }).await, "after");
    }

    /// Needs Docker Desktop running: `cargo test --lib -- --ignored engine_answers`.
    #[tokio::test]
    #[ignore]
    async fn engine_answers() {
        let info = get_json("/info").await.expect("engine info");
        assert_eq!(info["OSType"], "linux");
        let list = get_json("/containers/json?all=1").await.expect("container list");
        assert!(list.is_array());
        assert_eq!(find_json("/containers/worldstone-no-such-thing/json").await.unwrap(), None);
    }
}
