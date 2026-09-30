//! One long operation at a time, with a clear event boundary to the UI.
//!
//! A task is begun (failing fast if another is running), reports steps, progress and log
//! lines as events, can be cancelled where that is safe, and always ends in exactly one
//! terminal state. Secrets registered with the manager are redacted from every log line.

use crate::{
    process::{self, CommandSpec, Outcome, Stream},
    progress::{self, Progress},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

/// Error text used to tell a user cancellation from a failure.
pub const CANCELLED: &str = "Cancelled";

/// How many of the latest output lines of a command are kept, to explain why it failed.
const RECENT_LINES: usize = 60;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TaskState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: String,
    pub label: String,
    pub state: TaskState,
    pub step: Option<String>,
    pub progress: Option<Progress>,
    pub message: Option<String>,
    pub cancellable: bool,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Stdout,
    Stderr,
    Error,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub task_id: Option<String>,
    pub level: LogLevel,
    pub text: String,
    pub ts_ms: u64,
}

/// Where events go. The app implements this over Tauri events; tests record them.
pub trait EventSink: Send + Sync {
    fn task(&self, event: &TaskEvent);
    fn log(&self, event: &LogEvent);
}

struct Active {
    id: String,
    label: String,
    cancel: watch::Sender<bool>,
    cancellable: bool,
}

struct Inner {
    sink: Arc<dyn EventSink>,
    active: Mutex<Option<Active>>,
    /// The latest event of the running task, so a UI that reloaded can pick it up again.
    current: Mutex<Option<TaskEvent>>,
    secrets: Mutex<Vec<String>>,
    log_file: PathBuf,
    file_lock: Mutex<()>,
}

#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<Inner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl TaskManager {
    pub fn new(sink: Arc<dyn EventSink>, log_file: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                active: Mutex::new(None),
                current: Mutex::new(None),
                secrets: Mutex::new(Vec::new()),
                log_file,
                file_lock: Mutex::new(()),
            }),
        }
    }

    /// Anything registered here is replaced with `***` in every log line from now on.
    pub fn add_secret(&self, secret: &str) {
        if secret.len() >= 4 {
            self.inner.secrets.lock().unwrap().push(secret.to_string());
        }
    }

    /// The latest state of the running task, or `None` when nothing is running.
    pub fn current(&self) -> Option<TaskEvent> {
        self.inner.current.lock().unwrap().clone()
    }

    /// Sends a task event to the UI, remembering it while the task is still running.
    fn publish(&self, event: TaskEvent) {
        *self.inner.current.lock().unwrap() = (event.state == TaskState::Running).then(|| event.clone());
        self.inner.sink.task(&event);
    }

    /// Signals the running task to stop, if it can be stopped safely.
    pub fn cancel(&self) -> bool {
        match self.inner.active.lock().unwrap().as_ref() {
            Some(a) if a.cancellable => a.cancel.send(true).is_ok(),
            _ => false,
        }
    }

    pub fn begin(&self, label: &str, cancellable: bool) -> Result<Task, String> {
        let mut active = self.inner.active.lock().unwrap();
        if let Some(current) = active.as_ref() {
            return Err(format!("Another operation is running: {}", current.label));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (cancel, cancel_rx) = watch::channel(false);
        *active = Some(Active {
            id: id.clone(),
            label: label.to_string(),
            cancel,
            cancellable,
        });
        drop(active);
        let task = Task {
            id,
            label: label.to_string(),
            manager: self.clone(),
            cancel_rx,
            cancellable,
            step: None,
            recent: VecDeque::new(),
            outcome: None,
            finished: false,
        };
        task.emit(TaskState::Running, None, None);
        self.log(Some(&task.id), LogLevel::Info, &format!("Started: {label}"));
        Ok(task)
    }

    pub fn log(&self, task_id: Option<&str>, level: LogLevel, text: &str) {
        let text = self.redact(text);
        let event = LogEvent {
            task_id: task_id.map(str::to_string),
            level,
            text,
            ts_ms: now_ms(),
        };
        self.inner.sink.log(&event);
        self.append_to_file(&event);
    }

    fn redact(&self, text: &str) -> String {
        let secrets = self.inner.secrets.lock().unwrap();
        secrets.iter().fold(text.to_string(), |t, s| t.replace(s.as_str(), "***"))
    }

    fn append_to_file(&self, event: &LogEvent) {
        let _guard = self.inner.file_lock.lock().unwrap();
        if let Some(parent) = self.inner.log_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Logging must never take the operation down, so write failures are ignored.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.inner.log_file)
        {
            let _ = writeln!(file, "[{}] [{:?}] {}", event.ts_ms, event.level, event.text);
        }
    }

    fn release(&self, id: &str) {
        let mut active = self.inner.active.lock().unwrap();
        if active.as_ref().is_some_and(|a| a.id == id) {
            *active = None;
        }
    }
}

pub struct Task {
    id: String,
    label: String,
    manager: TaskManager,
    cancel_rx: watch::Receiver<bool>,
    cancellable: bool,
    step: Option<String>,
    /// The last lines the most recent command printed (redacted), oldest first.
    recent: VecDeque<String>,
    /// What the operation achieved, in words, shown as its result when it succeeds.
    outcome: Option<String>,
    finished: bool,
}

impl Task {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The last lines printed by the most recent [`Task::run`], oldest first.
    pub fn recent_output(&self) -> Vec<String> {
        self.recent.iter().cloned().collect()
    }

    fn emit(&self, state: TaskState, progress: Option<Progress>, message: Option<String>) {
        self.manager.publish(TaskEvent {
            id: self.id.clone(),
            label: self.label.clone(),
            state,
            step: self.step.clone(),
            progress,
            message,
            cancellable: self.cancellable,
        });
    }

    pub fn info(&self, text: &str) {
        self.manager.log(Some(&self.id), LogLevel::Info, text);
    }

    /// Says what the operation achieved. It is logged, and it is the message of the finished task, so
    /// the person sees what happened (or that nothing needed to) and not just "Finished".
    pub fn outcome(&mut self, text: &str) {
        self.info(text);
        self.outcome = Some(text.to_string());
    }

    /// The text with every registered secret hidden, for messages built from a server's own output.
    pub fn redact(&self, text: &str) -> String {
        self.manager.redact(text)
    }

    /// Hides a secret in everything this and later operations log.
    pub fn add_secret(&self, secret: &str) {
        self.manager.add_secret(secret);
    }

    /// Starts a named step. Clears progress, since the new step has not reported any.
    pub fn step(&mut self, name: &str) {
        self.step = Some(name.to_string());
        self.emit(TaskState::Running, None, None);
        self.info(name);
    }

    /// Shows a live status in place of the current step without writing it to the log, for text
    /// that changes every few seconds (the latest line a starting server printed).
    pub fn status(&mut self, text: &str) {
        self.step = Some(self.manager.redact(text));
        self.emit(TaskState::Running, None, None);
    }

    /// Waits, and gives up early with [`CANCELLED`] if the task is cancelled meanwhile.
    pub async fn pause(&mut self, duration: std::time::Duration) -> Result<(), String> {
        if *self.cancel_rx.borrow() {
            return Err(CANCELLED.to_string());
        }
        tokio::select! {
            _ = tokio::time::sleep(duration) => Ok(()),
            changed = self.cancel_rx.changed() => match changed {
                Ok(()) => Err(CANCELLED.to_string()),
                Err(_) => Ok(()),
            },
        }
    }

    /// Runs a command, streaming its output to the log and its progress to the UI.
    pub async fn run(&mut self, spec: CommandSpec) -> Result<(), String> {
        self.run_tracked(spec, &mut |_| None).await
    }

    /// Like [`Task::run`], and every output line is also offered to `tracker`, which may answer
    /// with a new step text (a live "12 maps done") to show in place of the current step.
    pub async fn run_tracked(
        &mut self,
        spec: CommandSpec,
        tracker: &mut (dyn FnMut(&str) -> Option<String> + Send),
    ) -> Result<(), String> {
        let (manager, id, label, cancellable) =
            (self.manager.clone(), self.id.clone(), self.label.clone(), self.cancellable);
        let mut step = self.step.clone();
        self.recent.clear();
        let recent = &mut self.recent;
        let publish = |step: &Option<String>, progress: Option<Progress>| {
            manager.publish(TaskEvent {
                id: id.clone(),
                label: label.clone(),
                state: TaskState::Running,
                step: step.clone(),
                progress,
                message: None,
                cancellable,
            })
        };
        let mut on_line = |stream: Stream, line: &str| {
            // Git redraws its transfer lines many times, so they only move the bar. Build
            // output is worth reading, so its progress lines are logged as well.
            if let Some(p) = progress::parse(line) {
                return publish(&step, Some(p));
            }
            if let Some(p) = progress::parse_build(line) {
                publish(&step, Some(p));
            }
            let level = if stream == Stream::Stderr { LogLevel::Stderr } else { LogLevel::Stdout };
            manager.log(Some(&id), level, line);
            if recent.len() == RECENT_LINES {
                recent.pop_front();
            }
            recent.push_back(manager.redact(line));
            if let Some(next) = tracker(line) {
                if step.as_deref() != Some(next.as_str()) {
                    step = Some(next);
                    publish(&step, None);
                }
            }
        };
        let outcome = process::run(&spec, &mut self.cancel_rx, &mut on_line).await;
        drop(on_line);
        // Keep the live status as the current step, so later events do not jump back.
        self.step = step;
        match outcome? {
            Outcome::Exited(0) => Ok(()),
            Outcome::Exited(code) => Err(format!("{} failed with exit code {code}. See the log for details.", spec.program)),
            Outcome::Cancelled => Err(CANCELLED.to_string()),
            Outcome::TimedOut => Err(format!("{} did not finish in time.", spec.program)),
        }
    }

    pub fn finish(mut self, result: Result<(), String>) {
        self.finished = true;
        let (state, message) = match &result {
            Ok(()) => (TaskState::Succeeded, self.outcome.clone()),
            Err(e) if e == CANCELLED => (TaskState::Cancelled, None),
            Err(e) => (TaskState::Failed, Some(e.clone())),
        };
        match (&state, &message) {
            (TaskState::Succeeded, _) => self.info(&format!("Finished: {}", self.label)),
            (TaskState::Cancelled, _) => self.info(&format!("Cancelled: {}", self.label)),
            (_, Some(m)) => self.manager.log(Some(&self.id), LogLevel::Error, m),
            _ => {}
        }
        self.manager.release(&self.id);
        self.emit(state, None, message);
    }
}

impl Drop for Task {
    /// A task dropped without `finish` (a panic, say) must not leave the lock held.
    fn drop(&mut self) {
        if !self.finished {
            self.manager.release(&self.id);
            self.emit(TaskState::Failed, None, Some("The operation ended unexpectedly.".into()));
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    #[derive(Default)]
    pub struct Recorder {
        pub tasks: Mutex<Vec<TaskEvent>>,
        pub logs: Mutex<Vec<LogEvent>>,
    }

    impl EventSink for Recorder {
        fn task(&self, event: &TaskEvent) {
            self.tasks.lock().unwrap().push(event.clone());
        }
        fn log(&self, event: &LogEvent) {
            self.logs.lock().unwrap().push(event.clone());
        }
    }

    pub fn manager(dir: &std::path::Path) -> (TaskManager, Arc<Recorder>) {
        let recorder = Arc::new(Recorder::default());
        (TaskManager::new(recorder.clone(), dir.join("Logs").join("worldstone.log")), recorder)
    }
}

#[cfg(test)]
mod tests {
    use super::{test_support::*, *};
    use crate::process::test_support::{echo, sleep};

    fn texts(r: &Recorder) -> Vec<String> {
        r.logs.lock().unwrap().iter().map(|l| l.text.clone()).collect()
    }

    fn last_state(r: &Recorder) -> TaskState {
        r.tasks.lock().unwrap().last().unwrap().state
    }

    #[tokio::test]
    async fn only_one_operation_runs_at_a_time_and_the_lock_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager(dir.path());
        let first = m.begin("First", false).unwrap();
        let err = m.begin("Second", false).err().unwrap();
        assert!(err.contains("First"), "{err}");
        first.finish(Ok(()));
        assert!(m.begin("Second", false).is_ok());
    }

    #[tokio::test]
    async fn a_dropped_task_releases_the_lock_and_reports_failure() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        drop(m.begin("Crashy", false).unwrap());
        assert_eq!(last_state(&r), TaskState::Failed);
        assert!(m.begin("Next", false).is_ok());
    }

    #[tokio::test]
    async fn command_output_is_logged_and_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        let mut task = m.begin("Run", false).unwrap();
        let result = task.run(echo(&["hello"], 2)).await;
        assert!(result.as_ref().unwrap_err().contains("exit code 2"));
        task.finish(result);
        assert!(texts(&r).iter().any(|t| t == "hello"));
        assert_eq!(last_state(&r), TaskState::Failed);
    }

    #[tokio::test]
    async fn the_latest_output_is_kept_for_explaining_a_failure_and_reset_per_command() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager(dir.path());
        m.add_secret("hunter2hunter2");
        let mut task = m.begin("Run", false).unwrap();
        let _ = task.run(echo(&["first", "password=hunter2hunter2"], 1)).await;
        assert_eq!(task.recent_output(), ["first", "password=***"], "kept, and redacted");
        let _ = task.run(echo(&["second"], 0)).await;
        assert_eq!(task.recent_output(), ["second"]);
    }

    #[tokio::test]
    async fn a_reloaded_ui_can_ask_for_the_running_task_and_it_clears_when_the_task_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager(dir.path());
        assert!(m.current().is_none());
        let mut task = m.begin("Build", true).unwrap();
        task.step("Compiling");
        task.run(echo(&["[ 45%] Building CXX object x.o"], 0)).await.unwrap();
        let current = m.current().expect("a task is running");
        assert_eq!((current.label.as_str(), current.step.as_deref(), current.state), ("Build", Some("Compiling"), TaskState::Running));
        assert_eq!(current.progress.map(|p| p.percent), Some(45.0), "the latest progress is kept");
        assert!(current.cancellable);
        task.finish(Ok(()));
        assert!(m.current().is_none());
    }

    #[tokio::test]
    async fn build_progress_moves_the_bar_and_is_still_logged() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        let mut task = m.begin("Build", false).unwrap();
        task.run(echo(&["[ 45%] Building CXX object x.o"], 0)).await.unwrap();
        assert!(r.tasks.lock().unwrap().iter().any(|e| e.progress.as_ref().is_some_and(|p| p.percent == 45.0)));
        assert!(texts(&r).iter().any(|t| t == "[ 45%] Building CXX object x.o"));
    }

    #[tokio::test]
    async fn secrets_never_reach_the_event_stream_or_the_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        m.add_secret("hunter2hunter2");
        let mut task = m.begin("Leaky", false).unwrap();
        let result = task.run(echo(&["password=hunter2hunter2"], 0)).await;
        task.finish(result);
        assert!(texts(&r).iter().all(|t| !t.contains("hunter2")));
        assert!(texts(&r).iter().any(|t| t == "password=***"));
        let file = std::fs::read_to_string(dir.path().join("Logs").join("worldstone.log")).unwrap();
        assert!(!file.contains("hunter2"));
    }

    #[tokio::test]
    async fn cancelling_a_cancellable_task_ends_in_the_cancelled_state() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        let mut task = m.begin("Long", true).unwrap();
        let canceller = m.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            assert!(canceller.cancel());
        });
        let result = task.run(sleep(30)).await;
        assert_eq!(result, Err(CANCELLED.to_string()));
        task.finish(result);
        assert_eq!(last_state(&r), TaskState::Cancelled);
    }

    #[tokio::test]
    async fn a_live_status_replaces_the_step_without_being_logged_and_hides_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        m.add_secret("hunter2hunter2");
        let mut task = m.begin("Start", false).unwrap();
        task.status("Loading hunter2hunter2 spells");
        assert_eq!(m.current().unwrap().step.as_deref(), Some("Loading *** spells"));
        assert!(texts(&r).iter().all(|t| !t.contains("spells")), "a status is not a log line");
    }

    #[tokio::test]
    async fn a_pause_ends_early_when_the_task_is_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager(dir.path());
        let mut task = m.begin("Wait", true).unwrap();
        assert_eq!(task.pause(std::time::Duration::from_millis(10)).await, Ok(()));
        let canceller = m.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        assert_eq!(task.pause(std::time::Duration::from_secs(30)).await, Err(CANCELLED.to_string()));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(task.pause(std::time::Duration::from_secs(30)).await, Err(CANCELLED.to_string()), "and stays cancelled");
    }

    #[tokio::test]
    async fn what_a_task_achieved_is_the_message_of_its_success_and_only_of_its_success() {
        let dir = tempfile::tempdir().unwrap();
        let (m, r) = manager(dir.path());
        let mut task = m.begin("Update", false).unwrap();
        task.outcome("Everything was already up to date.");
        task.finish(Ok(()));
        let done = r.tasks.lock().unwrap().last().unwrap().clone();
        assert_eq!(done.state, TaskState::Succeeded);
        assert_eq!(done.message.as_deref(), Some("Everything was already up to date."));
        assert!(texts(&r).iter().any(|t| t == "Everything was already up to date."), "and it is in the log");

        let mut failed = m.begin("Update", false).unwrap();
        failed.outcome("Nothing to do.");
        failed.finish(Err("Docker is not running".to_string()));
        let done = r.tasks.lock().unwrap().last().unwrap().clone();
        assert_eq!(done.state, TaskState::Failed);
        assert_eq!(done.message.as_deref(), Some("Docker is not running"), "a failure reports the failure");

        let plain = m.begin("Other", false).unwrap();
        plain.finish(Ok(()));
        assert_eq!(r.tasks.lock().unwrap().last().unwrap().message, None);
    }

    #[tokio::test]
    async fn a_task_that_is_not_cancellable_ignores_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager(dir.path());
        let _task = m.begin("Install", false).unwrap();
        assert!(!m.cancel());
    }
}
