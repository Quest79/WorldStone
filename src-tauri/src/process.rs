//! The one place child processes are spawned.
//!
//! Output is split on `\n` and `\r` so tools that redraw a progress line with a carriage
//! return (git, Docker) arrive as separate lines. Every run supports cancellation and an
//! optional timeout, and never opens a console window on Windows.

use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, watch},
    time::Instant,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Default)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Written to the child's stdin, which is then closed.
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
    /// How long the child gets to exit after being interrupted before it is killed.
    /// Defaults to [`GRACE`].
    pub grace: Option<Duration>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            ..Self::default()
        }
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    #[cfg(test)]
    pub fn grace(mut self, grace: Duration) -> Self {
        self.grace = Some(grace);
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Exited(i32),
    Cancelled,
    TimedOut,
}

/// Turns a byte stream into lines, treating `\n`, `\r` and `\r\n` alike and dropping blanks.
#[derive(Default)]
pub struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                if let Some(line) = self.take() {
                    lines.push(line);
                }
            } else {
                self.pending.push(b);
            }
        }
        lines
    }

    pub fn finish(&mut self) -> Option<String> {
        self.take()
    }

    fn take(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let line = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        Some(line)
    }
}

fn command(spec: &CommandSpec) -> Command {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args).kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP: no console window, and an interrupt sent on
    // cancellation (see `interrupt.rs`) reaches only this child and the tools it starts.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000 | 0x0000_0200);
    cmd
}

/// Starts a program that keeps running on its own (the game): nothing is piped, it is not waited
/// for, and it is not stopped when this app closes. The caller may look at the returned child once
/// to see whether it exited straight away.
pub fn spawn_detached(program: &std::path::Path, cwd: &std::path::Path) -> Result<tokio::process::Child, String> {
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(false);
    // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: not tied to this app's console or interrupts.
    #[cfg(windows)]
    cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    cmd.spawn().map_err(|e| format!("Cannot start {}: {e}", program.display()))
}

async fn pump<R: AsyncRead + Unpin>(mut reader: R, stream: Stream, tx: mpsc::UnboundedSender<(Stream, String)>) {
    let mut buf = [0u8; 8192];
    let mut splitter = LineSplitter::default();
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for line in splitter.push(&buf[..n]) {
                    let _ = tx.send((stream, line));
                }
            }
        }
    }
    if let Some(line) = splitter.finish() {
        let _ = tx.send((stream, line));
    }
}

/// How long a child gets to shut down after an interrupt before it is killed.
const GRACE: Duration = Duration::from_secs(20);

/// Stops a child: interrupt first, so tools can clean up (and tell anything they started on
/// the other side to stop), then kill it if it has not exited within [`GRACE`]. Output the
/// child prints while shutting down is still reported.
async fn stop(
    child: &mut tokio::process::Child,
    grace: Duration,
    rx: &mut mpsc::UnboundedReceiver<(Stream, String)>,
    on_line: &mut (dyn FnMut(Stream, &str) + Send),
) {
    if child.id().is_some_and(crate::interrupt::interrupt) {
        let deadline = Instant::now() + grace;
        loop {
            tokio::select! {
                Some((stream, line)) = rx.recv() => on_line(stream, &line),
                _ = child.wait() => return,
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
    }
    let _ = child.kill().await;
}

/// Marks what Docker said before a `docker` run as old, when the run starts and again however it
/// ends (finished, failed, cancelled or dropped): see `dockerapi::invalidate`.
struct DockerRun(bool);

impl DockerRun {
    fn of(spec: &CommandSpec) -> Self {
        let docker = spec.program == "docker";
        if docker {
            crate::dockerapi::invalidate();
        }
        Self(docker)
    }
}

impl Drop for DockerRun {
    fn drop(&mut self) {
        if self.0 {
            crate::dockerapi::invalidate();
        }
    }
}

/// Runs a command to completion, reporting each output line to `on_line`.
pub async fn run(
    spec: &CommandSpec,
    cancel: &mut watch::Receiver<bool>,
    on_line: &mut (dyn FnMut(Stream, &str) + Send),
) -> Result<Outcome, String> {
    let _docker = DockerRun::of(spec);
    let mut cmd = command(spec);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if spec.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start {}: {e}", spec.program))?;

    if let (Some(bytes), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
        tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    if let Some(out) = child.stdout.take() {
        tokio::spawn(pump(out, Stream::Stdout, tx.clone()));
    }
    if let Some(err) = child.stderr.take() {
        tokio::spawn(pump(err, Stream::Stderr, tx.clone()));
    }
    drop(tx);

    let deadline = spec.timeout.map(|t| Instant::now() + t);
    let timer = async {
        match deadline {
            Some(d) => tokio::time::sleep_until(d).await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(timer);

    let mut cancel_open = true;
    let outcome = loop {
        tokio::select! {
            Some((stream, line)) = rx.recv() => on_line(stream, &line),
            status = child.wait() => {
                let status = status.map_err(|e| e.to_string())?;
                break Outcome::Exited(status.code().unwrap_or(-1));
            }
            changed = cancel.changed(), if cancel_open => match changed {
                Ok(()) if *cancel.borrow() => {
                    stop(&mut child, spec.grace.unwrap_or(GRACE), &mut rx, on_line).await;
                    break Outcome::Cancelled;
                }
                Ok(()) => {}
                Err(_) => cancel_open = false,
            },
            _ = &mut timer => {
                stop(&mut child, spec.grace.unwrap_or(GRACE), &mut rx, on_line).await;
                break Outcome::TimedOut;
            }
        }
    };

    // Flush whatever the process wrote just before it ended.
    while let Ok(Some((stream, line))) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        on_line(stream, &line);
    }
    Ok(outcome)
}

#[derive(Clone, Debug)]
pub struct Probe {
    pub success: bool,
    pub text: String,
}

/// Runs a short command and returns its combined, trimmed output. Never fails: a program
/// that cannot be started reports `success: false` with the reason.
pub async fn probe(program: &str, args: &[&str]) -> Probe {
    capture(&CommandSpec::new(program, args)).await
}

/// Like [`probe`] for a full command: it may set the environment, feed stdin and choose how long
/// to wait (20 seconds by default). Used for questions asked of a running container.
pub async fn capture(spec: &CommandSpec) -> Probe {
    let _docker = DockerRun::of(spec);
    let program = &spec.program;
    let mut cmd = command(spec);
    cmd.stdin(if spec.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let work = async {
        let mut child = cmd.spawn()?;
        if let (Some(bytes), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
            tokio::spawn(async move {
                let _ = stdin.write_all(&bytes).await;
                let _ = stdin.shutdown().await;
            });
        }
        child.wait_with_output().await
    };
    match tokio::time::timeout(spec.timeout.unwrap_or(Duration::from_secs(20)), work).await {
        Err(_) => Probe { success: false, text: format!("{program} did not respond") },
        Ok(Err(e)) => Probe { success: false, text: format!("Cannot start {program}: {e}") },
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !err.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            Probe { success: out.status.success(), text }
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use super::CommandSpec;
    use std::time::Duration;

    /// A command that prints `lines` on stdout and exits with `code`.
    pub fn echo(lines: &[&str], code: i32) -> CommandSpec {
        if cfg!(windows) {
            // No spaces around `&`: cmd would echo them as trailing output.
            let script = lines.iter().map(|l| format!("echo {l}")).collect::<Vec<_>>().join("&");
            CommandSpec::new("cmd", &["/C", &format!("{script}&exit /b {code}")])
        } else {
            let script = lines.iter().map(|l| format!("echo {l}")).collect::<Vec<_>>().join("; ");
            CommandSpec::new("sh", &["-c", &format!("{script}; exit {code}")])
        }
    }

    /// A command that does nothing for about `seconds`. It ignores the interrupt sent on
    /// cancellation, so it is killed once the (short) grace period ends.
    pub fn sleep(seconds: u32) -> CommandSpec {
        let spec = if cfg!(windows) {
            CommandSpec::new("ping", &["-n", &(seconds + 1).to_string(), "127.0.0.1"])
        } else {
            CommandSpec::new("sleep", &[&seconds.to_string()])
        };
        spec.grace(Duration::from_secs(1))
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};

    #[tokio::test]
    async fn a_detached_program_is_started_and_its_early_exit_can_be_seen() {
        let dir = std::env::temp_dir();
        let program = if cfg!(windows) { std::path::PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into())) } else { std::path::PathBuf::from("/bin/true") };
        let mut child = spawn_detached(&program, &dir).expect("starts");
        let status = tokio::time::timeout(Duration::from_secs(20), child.wait()).await.expect("finishes").expect("waits");
        assert!(status.code().is_some());
        assert!(spawn_detached(&dir.join("no-such-program.exe"), &dir).is_err());
    }

    fn lines(input: &[&[u8]]) -> Vec<String> {
        let mut s = LineSplitter::default();
        let mut out: Vec<String> = input.iter().flat_map(|c| s.push(c)).collect();
        out.extend(s.finish());
        out
    }

    #[test]
    fn splits_on_newline_carriage_return_and_crlf() {
        assert_eq!(lines(&[b"a\nb\r\nc\rd"]), ["a", "b", "c", "d"]);
    }

    #[test]
    fn joins_a_line_split_across_reads_and_drops_blanks() {
        assert_eq!(lines(&[b"hel", b"lo\n\n\nwor", b"ld"]), ["hello", "world"]);
    }

    #[test]
    fn progress_redraws_become_separate_lines() {
        assert_eq!(
            lines(&[b"Receiving objects:  10%\rReceiving objects:  20%\r"]),
            ["Receiving objects:  10%", "Receiving objects:  20%"]
        );
    }

    #[tokio::test]
    async fn captures_output_and_exit_code() {
        let (_tx, mut rx) = watch::channel(false);
        let mut seen = Vec::new();
        let out = run(&echo(&["one", "two"], 3), &mut rx, &mut |_, l| seen.push(l.to_string()))
            .await
            .unwrap();
        assert_eq!(out, Outcome::Exited(3));
        assert_eq!(seen, ["one", "two"]);
    }

    #[tokio::test]
    async fn cancellation_stops_a_running_command() {
        let (tx, mut rx) = watch::channel(false);
        let handle = tokio::spawn(async move { run(&sleep(30), &mut rx, &mut |_, _| {}).await });
        tokio::time::sleep(Duration::from_millis(400)).await;
        tx.send(true).unwrap();
        let out = tokio::time::timeout(Duration::from_secs(10), handle).await.unwrap().unwrap().unwrap();
        assert_eq!(out, Outcome::Cancelled);
    }

    /// A hard kill would leave no trace; ping prints its summary only when it receives the
    /// interrupt (and keeps running, so the grace period then ends it). The test process
    /// surviving proves the interrupt reached only the child.
    #[cfg(windows)]
    #[tokio::test]
    async fn cancellation_interrupts_the_command_before_resorting_to_a_kill() {
        let (tx, mut rx) = watch::channel(false);
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            let spec = CommandSpec::new("ping", &["-t", "127.0.0.1"]).grace(Duration::from_secs(2));
            let out = run(&spec, &mut rx, &mut |_, l| seen.push(l.to_string())).await.unwrap();
            (out, seen)
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        tx.send(true).unwrap();
        let (out, seen) = tokio::time::timeout(Duration::from_secs(25), handle).await.unwrap().unwrap();
        assert_eq!(out, Outcome::Cancelled);
        assert!(seen.iter().any(|l| l.contains("Ping statistics")), "the child was interrupted, not just killed: {seen:?}");
    }

    #[tokio::test]
    async fn timeout_stops_a_running_command() {
        let (_tx, mut rx) = watch::channel(false);
        let spec = sleep(30).timeout(Duration::from_millis(300));
        let out = run(&spec, &mut rx, &mut |_, _| {}).await.unwrap();
        assert_eq!(out, Outcome::TimedOut);
    }

    #[tokio::test]
    async fn dropping_the_cancel_sender_does_not_cancel_or_spin() {
        let (tx, mut rx) = watch::channel(false);
        drop(tx);
        let out = run(&echo(&["ok"], 0), &mut rx, &mut |_, _| {}).await.unwrap();
        assert_eq!(out, Outcome::Exited(0));
    }

    #[tokio::test]
    async fn a_missing_program_is_a_readable_error() {
        let (_tx, mut rx) = watch::channel(false);
        let err = run(&CommandSpec::new("definitely-not-a-real-program", &[]), &mut rx, &mut |_, _| {})
            .await
            .unwrap_err();
        assert!(err.contains("Cannot start definitely-not-a-real-program"));
    }

    #[tokio::test]
    async fn stdin_reaches_the_child() {
        let (_tx, mut rx) = watch::channel(false);
        let spec = if cfg!(windows) {
            CommandSpec::new("findstr", &["x*"]).stdin(b"hello\r\n".to_vec())
        } else {
            CommandSpec::new("cat", &[]).stdin(b"hello\n".to_vec())
        };
        let mut seen = Vec::new();
        run(&spec, &mut rx, &mut |_, l| seen.push(l.to_string())).await.unwrap();
        assert_eq!(seen, ["hello"]);
    }
}
