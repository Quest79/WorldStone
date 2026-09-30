//! Running the servers (Step 6): start them in order, wait until they answer, stop them so the
//! world is saved, and report what is really running.
//!
//! State is read from Docker, never remembered: each server is a container of the instance's
//! Compose project, and "running" means its own port check passes (the world server only listens
//! once the world has finished loading) and the port also answers on this machine.

use crate::{
    builder, clock,
    compose,
    config::{self, ConfigState},
    database::{self, Credentials, DatabaseState},
    docker::{self, ContainerState, Listed},
    extractor::{self, failure_detail, ExtractState},
    paths::InstancePaths,
    process::{capture, probe, CommandSpec},
    recipe::{Recipe, ServiceSpec},
    tasks::Task,
};
use serde::Serialize;
use std::{
    net::{IpAddr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

/// How long a start may take before it is called a failure. A first start creates the bots'
/// characters and can take a long while on a small machine.
const START_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const POLL_EVERY: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------------------
// One container
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ServiceState {
    /// No container exists.
    Missing,
    /// The container exists and exited normally, or was never started.
    Stopped,
    /// Running, and its port does not answer yet.
    Starting,
    /// Running and answering.
    Running,
    /// Frozen by Docker: the process exists but does nothing until it is resumed.
    Paused,
    /// Running, but its port stopped answering.
    Unresponsive,
    /// Exited with an error, or keeps restarting.
    Crashed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Observed {
    pub state: ServiceState,
    pub exit_code: Option<i64>,
}

/// What Docker's state for a container means for the server it runs.
pub fn observed(container: &ContainerState) -> Observed {
    let code = container.exit_code;
    let state = match (container.status.as_str(), container.health.as_deref()) {
        ("running", Some("healthy")) => ServiceState::Running,
        ("running", Some("unhealthy")) => ServiceState::Unresponsive,
        ("running", _) => ServiceState::Starting,
        ("paused", _) => ServiceState::Paused,
        ("restarting", _) => ServiceState::Crashed,
        ("exited" | "dead", _) if code != 0 => ServiceState::Crashed,
        _ => ServiceState::Stopped,
    };
    Observed { state, exit_code: matches!(container.status.as_str(), "exited" | "dead").then_some(code) }
}

/// When the current run began, in seconds since the epoch; `None` when it never did.
pub fn started(container: &ContainerState) -> Option<u64> {
    clock::parse_rfc3339_utc(&container.started_at)
}

/// What a container is doing and, when it has been started, when this run began.
pub(crate) async fn observe_run(container: &str) -> (Observed, Option<u64>) {
    match docker::container(container).await {
        Some(found) => (observed(&found), started(&found)),
        None => (Observed { state: ServiceState::Missing, exit_code: None }, None),
    }
}

async fn observe(container: &str) -> Observed {
    observe_run(container).await.0
}

/// Whether something answers on the host side of a published port.
async fn port_answers(port: u16) -> bool {
    let Ok(ip) = compose::HOST_ADDRESS.parse::<IpAddr>() else { return false };
    tauri::async_runtime::spawn_blocking(move || TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_secs(3)).is_ok())
        .await
        .unwrap_or(false)
}

/// Removes terminal colour codes and other control characters from a log line.
pub fn clean_log_line(line: &str) -> Option<String> {
    clean_log_line_to(line, 90)
}

/// [`clean_log_line`], cut to `max` characters.
pub fn clean_log_line_to(line: &str, max: usize) -> Option<String> {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while chars.next_if(|c| !c.is_ascii_alphabetic()).is_some() {}
                chars.next();
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    let text = out.trim();
    (!text.is_empty()).then(|| text.chars().take(max).collect())
}

/// When the container's current run began. A container that is started again keeps the log of its
/// earlier runs, so reading only from here on shows what this run is doing, not what the last one
/// ended with.
async fn run_started_at(container: &str) -> Option<String> {
    let text = docker::container(container).await?.started_at;
    (!text.is_empty() && !text.starts_with("0001-")).then_some(text)
}

/// The last `lines` lines a container's current run printed, raw and oldest first.
async fn recent_log(container: &str, lines: u32) -> Option<String> {
    let count = lines.to_string();
    let since = run_started_at(container).await;
    let mut args = vec!["logs", "--tail", count.as_str()];
    if let Some(since) = since.as_deref() {
        args.extend(["--since", since]);
    }
    args.push(container);
    let out = capture(&CommandSpec::new("docker", &args).timeout(Duration::from_secs(15))).await;
    out.success.then_some(out.text)
}

/// What a container printed since a moment (seconds since the epoch), raw and oldest first. Empty
/// when Docker cannot say.
pub(crate) async fn log_since(container: &str, since: u64) -> String {
    let since = since.to_string();
    let out = capture(&CommandSpec::new("docker", &["logs", "--since", &since, container]).timeout(Duration::from_secs(15))).await;
    if out.success {
        out.text
    } else {
        String::new()
    }
}

/// The last `lines` lines of the container's current run; see [`recent_log`].
pub(crate) async fn current_log(container: &str, lines: u32) -> String {
    recent_log(container, lines).await.unwrap_or_default()
}

/// The newest non-empty line a container has printed, cleaned up for showing.
async fn last_log_line(container: &str) -> Option<String> {
    recent_log(container, 6).await?.lines().rev().find_map(clean_log_line)
}

/// The last lines a container printed, oldest first, for explaining why it stopped.
async fn log_tail(container: &str, lines: u32) -> Vec<String> {
    recent_log(container, lines).await.map(|text| text.lines().filter_map(clean_log_line).collect()).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub id: String,
    pub label: String,
    pub container: String,
    pub port: u16,
    pub state: ServiceState,
    pub exit_code: Option<i64>,
    /// Whether the port answers on this machine; only asked when the server says it is running.
    pub reachable: Option<bool>,
    /// How long this run has been going, for a server that is up.
    pub uptime_secs: Option<u64>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ServerState {
    /// Docker could not be asked.
    Unknown,
    /// Nothing is running.
    Stopped,
    Starting,
    /// Every server is running and answering.
    Running,
    /// Every server is frozen; the database stays up.
    Paused,
    /// Some servers run and some do not.
    Partial,
    /// A server crashed or stopped answering.
    Failing,
}

/// The state of the whole from the state of its servers.
pub fn overall(services: &[ServiceState]) -> ServerState {
    use ServiceState::*;
    if services.iter().any(|s| matches!(s, Crashed | Unresponsive)) {
        ServerState::Failing
    } else if services.iter().all(|s| matches!(s, Missing | Stopped)) {
        ServerState::Stopped
    } else if services.iter().all(|s| *s == Running) {
        ServerState::Running
    } else if services.iter().all(|s| *s == Paused) {
        ServerState::Paused
    } else if services.iter().any(|s| *s == Starting) {
        ServerState::Starting
    } else {
        ServerState::Partial
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub state: ServerState,
    pub note: Option<String>,
    /// Where the servers are published on this machine.
    pub host: String,
    pub services: Vec<ServiceStatus>,
    /// A configuration file was changed after a running server started, so the server is still
    /// working from the old one.
    pub restart_needed: bool,
    /// Other instances whose servers are up on a port these servers need. Only one server can
    /// listen on a port, so these have to be stopped first. Only asked while these are not running.
    pub held_by: Vec<String>,
}

/// The WorldStone instances other than `own_project`'s that own one of `holders`.
fn other_instances(own_project: &str, holders: &[&Listed]) -> Vec<String> {
    let mut found: Vec<String> = holders
        .iter()
        .filter_map(|c| c.project.as_deref())
        .filter(|project| *project != own_project)
        .filter_map(|project| project.strip_prefix("worldstone-").map(str::to_string))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The other instances whose servers are up on a host port `recipe` publishes.
pub async fn port_holders(instance: &str, recipe: &Recipe) -> Vec<String> {
    let own = compose::project_name(instance);
    let mut found = Vec::new();
    if let Ok(running) = docker::containers(false).await {
        for spec in &recipe.run.services {
            let holders: Vec<&Listed> = running.iter().filter(|c| c.public_ports.contains(&spec.port)).collect();
            found.extend(other_instances(&own, &holders));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// When the newest configuration file was written, in seconds since the epoch. Files kept in a
/// folder of their own (a module's) count as well.
fn newest_config(paths: &InstancePaths) -> Option<u64> {
    fn newest_in(dir: &std::path::Path, depth: u8) -> Option<u64> {
        let entries: Vec<_> = std::fs::read_dir(dir).ok()?.filter_map(Result::ok).collect();
        let files = entries
            .iter()
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "conf"))
            .filter_map(|entry| entry.metadata().ok()?.modified().ok())
            .filter_map(clock::epoch_of);
        let inner = entries.iter().filter(|entry| depth > 0 && entry.path().is_dir()).filter_map(|entry| newest_in(&entry.path(), depth - 1));
        files.chain(inner).max()
    }
    newest_in(&config::config_dir(paths), 1)
}

/// Whether some server that is up started before the newest configuration was written.
pub fn restart_needed(started: &[Option<u64>], config_written: Option<u64>) -> bool {
    config_written.is_some_and(|written| started.iter().flatten().any(|start| *start < written))
}

/// Reads what Docker says about the servers; starts and changes nothing.
pub async fn status(paths: &InstancePaths, instance: &str, recipe: &Recipe) -> ServerStatus {
    let host = compose::HOST_ADDRESS.to_string();
    if let Err(reason) = builder::engine().await {
        return ServerStatus { state: ServerState::Unknown, note: Some(reason), host, services: Vec::new(), restart_needed: false, held_by: Vec::new() };
    }
    let mut services = Vec::new();
    let mut started = Vec::new();
    for spec in &recipe.run.services {
        let container = compose::service_container(instance, &spec.id);
        let (seen, began) = observe_run(&container).await;
        let up = matches!(seen.state, ServiceState::Running | ServiceState::Starting | ServiceState::Unresponsive | ServiceState::Paused);
        let uptime_secs = began.filter(|_| up).map(|b| clock::now().saturating_sub(b));
        started.push(began.filter(|_| up));
        let reachable = if seen.state == ServiceState::Running { Some(port_answers(spec.port).await) } else { None };
        services.push(ServiceStatus {
            id: spec.id.clone(),
            label: spec.label.clone(),
            container,
            port: spec.port,
            state: seen.state,
            exit_code: seen.exit_code,
            reachable,
            uptime_secs,
        });
    }
    let states: Vec<ServiceState> = services.iter().map(|s| s.state).collect();
    let unreachable: Vec<&str> = services.iter().filter(|s| s.reachable == Some(false)).map(|s| s.label.as_str()).collect();
    let note = (!unreachable.is_empty())
        .then(|| format!("{} answers inside Docker but not on {host}. Check that nothing else uses its port.", unreachable.join(" and ")));
    let restart_needed = restart_needed(&started, newest_config(paths));
    let state = overall(&states);
    let held_by = if state == ServerState::Stopped { port_holders(instance, recipe).await } else { Vec::new() };
    ServerStatus { state, note, host, services, restart_needed, held_by }
}

// ---------------------------------------------------------------------------------------
// Starting
// ---------------------------------------------------------------------------------------

pub(crate) fn compose_spec(paths: &InstancePaths, instance: &str, file: &std::path::Path, args: &[&str], timeout: Duration) -> CommandSpec {
    let project = compose::project_name(instance);
    let file = file.to_string_lossy();
    let mut all = vec!["compose", "--progress", "quiet", "-p", project.as_str(), "-f", file.as_ref()];
    all.extend_from_slice(args);
    CommandSpec::new("docker", &all).cwd(&paths.docker).timeout(timeout)
}

fn explain(what: &str, error: &str, tail: &[String]) -> String {
    match failure_detail(tail) {
        Some(detail) => format!("{what}: {error} Last output: {detail}"),
        None => format!("{what}: {error}"),
    }
}

/// Why a server that should be starting is not, from its exit code and last log lines.
pub fn explain_exit(spec: &ServiceSpec, seen: &Observed, tail: &[String]) -> String {
    let cause = match (seen.state, seen.exit_code) {
        (ServiceState::Missing, _) => "its container is gone".to_string(),
        (_, Some(137)) => "it was killed, most likely for lack of memory. Give Docker Desktop more memory".to_string(),
        (_, Some(code)) => format!("it exited with code {code}"),
        (ServiceState::Crashed, None) => "it keeps restarting".to_string(),
        (_, None) => "it stopped".to_string(),
    };
    let base = format!("The {} did not stay up: {cause}.", spec.label.to_lowercase());
    match failure_detail(tail) {
        Some(detail) => format!("{base} Last output: {detail}"),
        None => base,
    }
}

/// Checks everything a start needs, in the order a person would fix it, and says what is missing.
pub(crate) async fn check_ready(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking Docker, the server image, the game data and the configuration");
    builder::engine().await?;
    if !probe("docker", &["compose", "version", "--short"]).await.success {
        return Err("Docker Compose is not available. Update Docker Desktop.".to_string());
    }
    if let Some(other) = port_holders(instance, recipe).await.first() {
        return Err(format!("The {other} servers are running and use the same ports. Stop them first, then start these."));
    }
    for service in &recipe.run.services {
        if docker::image(&builder::image_ref(instance, recipe, service)).await.is_none() {
            return Err("Build the server first.".to_string());
        }
    }
    if docker::image(&builder::database_image(instance, recipe)).await.is_none() {
        return Err("Check the database first: its image has not been named for this server yet.".to_string());
    }
    if extractor::status(paths, instance, recipe).await.state != ExtractState::Ready {
        return Err("Extract the client data first: the world server cannot run without it.".to_string());
    }
    if Credentials::load(paths)?.is_none() {
        return Err("Set up the database first.".to_string());
    }
    let configured = config::status(paths, recipe);
    match configured.state {
        ConfigState::Ready => Ok(()),
        ConfigState::Unknown => Err(configured.note.unwrap_or_else(|| "The configuration cannot be read.".to_string())),
        ConfigState::NotConfigured => Err("Generate the configuration first.".to_string()),
        ConfigState::Outdated => Err("The configuration is out of date. Generate it again first; that puts back only the settings this app manages.".to_string()),
    }
}

/// Starts the database and the servers and waits until every server answers.
pub async fn start(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    check_ready(task, paths, instance, recipe).await?;
    if !paused_services(instance, recipe).await.is_empty() {
        return Err("The servers are paused. Resume them, or stop them first.".to_string());
    }
    let creds = Credentials::load(paths)?.ok_or_else(|| "Set up the database first.".to_string())?;
    for secret in creds.secrets() {
        task.add_secret(secret);
    }
    let file = compose::write(paths, recipe, instance)?;

    task.step("Starting the database");
    let up_db = compose_spec(paths, instance, &file, &["up", "-d", "--wait", compose::DATABASE_SERVICE], Duration::from_secs(10 * 60));
    task.run(up_db).await.map_err(|e| explain("The database did not start", &e, &task.recent_output()))?;

    let state = database::status(paths, instance, recipe).await;
    match state.state {
        DatabaseState::Ready => {}
        DatabaseState::Outdated => return Err("The database has updates waiting. Run Set up database first; it backs up before changing anything.".to_string()),
        _ => return Err(state.note.unwrap_or_else(|| "The database is not set up completely. Run Set up database first.".to_string())),
    }

    task.step("Starting the servers");
    let ids: Vec<&str> = recipe.run.services.iter().map(|s| s.id.as_str()).collect();
    let mut args = vec!["up", "-d"];
    args.extend(ids);
    let started = task.run(compose_spec(paths, instance, &file, &args, Duration::from_secs(10 * 60))).await;
    if let Err(e) = started {
        return Err(explain("The servers could not be started", &e, &task.recent_output()));
    }
    let waited = wait_until_running(task, instance, recipe, Duration::ZERO).await;
    if waited == Err(crate::tasks::CANCELLED.to_string()) {
        // Stopping the wait must not leave a half-started server behind.
        task.step("Stopping the servers you cancelled");
        let mut args = vec!["stop"];
        args.extend(recipe.run.services.iter().map(|s| s.id.as_str()));
        let _ = task.run(compose_spec(paths, instance, &file, &args, Duration::from_secs(5 * 60))).await;
    }
    waited
}

/// How long after a resume Docker's health verdict from the frozen time may still be showing. The
/// health check cannot run in a frozen container, so Docker marks it unhealthy while it is paused
/// and only replaces that with the next passing probe (every 5 s).
const RESUME_SETTLE: Duration = Duration::from_secs(60);

/// Whether a server in `state` may still come good: it is still starting, or it shows the stale
/// unhealthy verdict of a resume that has not had its next health check yet.
fn may_still_answer(state: ServiceState, settling: bool) -> bool {
    state == ServiceState::Starting || (settling && state == ServiceState::Unresponsive)
}

async fn wait_until_running(task: &mut Task, instance: &str, recipe: &Recipe, settle: Duration) -> Result<(), String> {
    let deadline = Instant::now() + START_TIMEOUT;
    let settled_at = Instant::now() + settle;
    task.step("Waiting for the servers to answer");
    loop {
        let mut waiting: Option<(&ServiceSpec, String)> = None;
        for spec in &recipe.run.services {
            let container = compose::service_container(instance, &spec.id);
            let seen = observe(&container).await;
            match seen.state {
                ServiceState::Running => continue,
                state if may_still_answer(state, Instant::now() < settled_at) => {}
                _ => {
                    let tail = log_tail(&container, 30).await;
                    return Err(task.redact(&explain_exit(spec, &seen, &tail)));
                }
            }
            if waiting.is_none() {
                waiting = Some((spec, container));
            }
        }
        let Some((spec, container)) = waiting else { break };
        let last = last_log_line(&container).await;
        task.status(&match last {
            Some(line) => format!("{} is starting · {line}", spec.label),
            None => format!("{} is starting", spec.label),
        });
        if Instant::now() > deadline {
            let tail = log_tail(&container, 30).await;
            let minutes = START_TIMEOUT.as_secs() / 60;
            return Err(task.redact(&explain(
                &format!("The {} was not answering after {minutes} minutes", spec.label.to_lowercase()),
                "It is still running, so it may only be slow. Check again shortly, or stop it and look at the log.",
                &tail,
            )));
        }
        task.pause(POLL_EVERY).await?;
    }

    task.step("Checking that the ports answer on this machine");
    for spec in &recipe.run.services {
        if !port_answers(spec.port).await {
            return Err(format!(
                "The {} is running but nothing answers on {}:{}. Something else on this computer may be using that port.",
                spec.label.to_lowercase(),
                compose::HOST_ADDRESS,
                spec.port
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Stopping
// ---------------------------------------------------------------------------------------

/// A server that was stopped on request should have exited normally, which for the world server
/// means it saved the game first. Anything else is reported.
pub fn unclean_stops(services: &[(String, Observed)]) -> Vec<String> {
    services
        .iter()
        .filter_map(|(label, seen)| match (seen.state, seen.exit_code) {
            (ServiceState::Missing | ServiceState::Stopped, Some(0) | None) => None,
            (_, Some(137)) => Some(format!("{label} had to be killed after it did not stop in time (exit 137)")),
            (_, Some(code)) => Some(format!("{label} exited with code {code}")),
            (ServiceState::Running | ServiceState::Starting | ServiceState::Unresponsive, None) => Some(format!("{label} is still running")),
            (_, None) => Some(format!("{label} did not report how it stopped")),
        })
        .collect()
}

/// Stops the servers, then the database. Stopping the world server is how the game is saved, so
/// it is given all the time its recipe says it needs.
pub async fn stop(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking Docker");
    builder::engine().await?;
    let file = compose::write(paths, recipe, instance)?;

    // A frozen process cannot save the game or answer a stop, so it is thawed first.
    let frozen = paused_services(instance, recipe).await;
    if !frozen.is_empty() {
        task.step("Resuming the paused servers so they can save");
        let mut args = vec!["unpause"];
        args.extend(frozen.iter().map(String::as_str));
        task.run(compose_spec(paths, instance, &file, &args, Duration::from_secs(60)))
            .await
            .map_err(|e| explain("The paused servers could not be resumed", &e, &task.recent_output()))?;
    }

    let longest = recipe.run.services.iter().map(|s| s.stop_grace_secs).max().unwrap_or(30);
    task.step("Stopping the servers (the world server saves the game first)");
    let mut args = vec!["stop"];
    args.extend(recipe.run.services.iter().rev().map(|s| s.id.as_str()));
    let timeout = Duration::from_secs(u64::from(longest) * 2 + 60);
    task.run(compose_spec(paths, instance, &file, &args, timeout))
        .await
        .map_err(|e| explain("The servers could not be stopped", &e, &task.recent_output()))?;

    let mut seen = Vec::new();
    for spec in &recipe.run.services {
        seen.push((spec.label.clone(), observe(&compose::service_container(instance, &spec.id)).await));
    }

    task.step("Stopping the database");
    task.run(compose_spec(paths, instance, &file, &["stop", compose::DATABASE_SERVICE], Duration::from_secs(5 * 60)))
        .await
        .map_err(|e| explain("The database could not be stopped", &e, &task.recent_output()))?;

    let unclean = unclean_stops(&seen);
    if unclean.is_empty() {
        Ok(())
    } else {
        Err(format!("Everything is stopped, but not cleanly: {}. The last minutes of play may not have been saved.", unclean.join("; ")))
    }
}

/// The ids of the servers Docker has frozen.
async fn paused_services(instance: &str, recipe: &Recipe) -> Vec<String> {
    let mut ids = Vec::new();
    for spec in &recipe.run.services {
        if observe(&compose::service_container(instance, &spec.id)).await.state == ServiceState::Paused {
            ids.push(spec.id.clone());
        }
    }
    ids
}

/// Freezes the servers where they are (`docker pause`): nobody can play until they are resumed, the
/// world stops advancing, and nothing is saved or closed. The database keeps running.
pub async fn pause(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking Docker");
    builder::engine().await?;
    let mut running = Vec::new();
    for spec in &recipe.run.services {
        match observe(&compose::service_container(instance, &spec.id)).await.state {
            ServiceState::Running => running.push(spec.id.clone()),
            ServiceState::Paused => {}
            _ => return Err(format!("The {} is not running, so there is nothing to pause. Start the servers first.", spec.label.to_lowercase())),
        }
    }
    if running.is_empty() {
        return Err("The servers are already paused.".to_string());
    }
    let file = compose::write(paths, recipe, instance)?;
    task.step("Pausing the servers");
    let mut args = vec!["pause"];
    args.extend(running.iter().map(String::as_str));
    task.run(compose_spec(paths, instance, &file, &args, Duration::from_secs(60)))
        .await
        .map_err(|e| explain("The servers could not be paused", &e, &task.recent_output()))?;
    if paused_services(instance, recipe).await.len() == recipe.run.services.len() {
        Ok(())
    } else {
        Err("Docker did not pause every server. Check them on the Servers card.".to_string())
    }
}

/// Lets paused servers carry on, and waits until they answer again.
pub async fn resume(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    task.step("Checking Docker");
    builder::engine().await?;
    let frozen = paused_services(instance, recipe).await;
    if frozen.is_empty() {
        return Err("The servers are not paused.".to_string());
    }
    let file = compose::write(paths, recipe, instance)?;
    task.step("Resuming the servers");
    let mut args = vec!["unpause"];
    args.extend(frozen.iter().map(String::as_str));
    task.run(compose_spec(paths, instance, &file, &args, Duration::from_secs(60)))
        .await
        .map_err(|e| explain("The servers could not be resumed", &e, &task.recent_output()))?;
    wait_until_running(task, instance, recipe, RESUME_SETTLE).await
}

/// Stops and starts again, as one operation.
pub async fn restart(task: &mut Task, paths: &InstancePaths, instance: &str, recipe: &Recipe) -> Result<(), String> {
    // Whatever would stop the start is found out before anything is stopped.
    check_ready(task, paths, instance, recipe).await?;
    stop(task, paths, instance, recipe).await?;
    start(task, paths, instance, recipe).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero() -> Recipe {
        crate::recipe::builtin().unwrap().remove(0)
    }

    fn seen(state: ServiceState, code: Option<i64>) -> Observed {
        Observed { state, exit_code: code }
    }

    #[test]
    fn only_a_resume_forgives_the_unhealthy_verdict_given_while_frozen() {
        assert!(may_still_answer(ServiceState::Starting, false));
        assert!(!may_still_answer(ServiceState::Unresponsive, false), "a normal start fails on it");
        assert!(may_still_answer(ServiceState::Unresponsive, true));
        assert!(!may_still_answer(ServiceState::Crashed, true), "a crash is never forgiven");
        assert!(!may_still_answer(ServiceState::Stopped, true));
    }

    fn cs(status: &str, health: Option<&str>, code: i64, started_at: &str) -> ContainerState {
        ContainerState { status: status.into(), health: health.map(str::to_string), exit_code: code, started_at: started_at.into() }
    }

    #[test]
    fn a_paused_container_is_paused_whatever_its_health_says() {
        assert_eq!(observed(&cs("paused", Some("healthy"), 0, "")).state, ServiceState::Paused);
        assert_eq!(observed(&cs("paused", Some("unhealthy"), 0, "")).state, ServiceState::Paused);
        assert_eq!(overall(&[ServiceState::Paused, ServiceState::Paused]), ServerState::Paused);
        assert_eq!(overall(&[ServiceState::Paused, ServiceState::Running]), ServerState::Partial);
        assert_eq!(overall(&[ServiceState::Paused, ServiceState::Stopped]), ServerState::Partial);
    }

    #[test]
    fn what_docker_says_about_a_container_becomes_a_state() {
        assert_eq!(observed(&cs("running", Some("healthy"), 0, "")).state, ServiceState::Running);
        assert_eq!(observed(&cs("running", Some("starting"), 0, "")).state, ServiceState::Starting);
        assert_eq!(observed(&cs("running", None, 0, "")).state, ServiceState::Starting, "no health check yet means it has not answered");
        assert_eq!(observed(&cs("running", Some("unhealthy"), 0, "")).state, ServiceState::Unresponsive);
        assert_eq!(observed(&cs("restarting", None, 1, "")).state, ServiceState::Crashed);
        assert_eq!(observed(&cs("exited", None, 0, "")), seen(ServiceState::Stopped, Some(0)));
        assert_eq!(observed(&cs("exited", None, 139, "")), seen(ServiceState::Crashed, Some(139)));
        assert_eq!(observed(&cs("created", None, 0, "")), seen(ServiceState::Stopped, None), "an exit code only counts once it exited");
    }

    #[test]
    fn the_start_time_is_absent_when_never_started() {
        assert_eq!(started(&cs("running", Some("healthy"), 0, "2026-09-29T08:21:12.5Z")), Some(1_790_670_072));
        assert_eq!(started(&cs("created", None, 0, "0001-01-01T00:00:00Z")), None);
        assert_eq!(started(&cs("running", Some("healthy"), 0, "")), None);
    }

    #[test]
    fn a_running_server_needs_a_restart_only_when_the_configuration_is_newer_than_it() {
        assert!(restart_needed(&[Some(100), Some(300)], Some(200)));
        assert!(!restart_needed(&[Some(300), Some(400)], Some(200)));
        assert!(!restart_needed(&[None, None], Some(200)), "servers that are not running have nothing to restart");
        assert!(!restart_needed(&[Some(100)], None));
    }

    #[test]
    fn the_whole_is_only_running_when_every_server_is() {
        use ServiceState::*;
        assert_eq!(overall(&[Missing, Missing]), ServerState::Stopped);
        assert_eq!(overall(&[Stopped, Missing]), ServerState::Stopped);
        assert_eq!(overall(&[Running, Running]), ServerState::Running);
        assert_eq!(overall(&[Running, Starting]), ServerState::Starting);
        assert_eq!(overall(&[Running, Stopped]), ServerState::Partial);
        assert_eq!(overall(&[Running, Crashed]), ServerState::Failing);
        assert_eq!(overall(&[Unresponsive, Running]), ServerState::Failing);
        assert_eq!(overall(&[Starting, Crashed]), ServerState::Failing, "a failure is never hidden by a server that is still starting");
    }

    #[test]
    fn only_other_worldstone_instances_are_named_as_holding_a_port() {
        let held = |project: Option<&str>| Listed { name: "c".into(), image_id: "sha256:1".into(), project: project.map(str::to_string), public_ports: vec![3724] };
        let all = [
            held(Some("worldstone-mangos-zero")),
            held(Some("worldstone-azerothcore")),
            held(Some("worldstone-mangos-zero")),
            held(Some("someone-elses-app")),
            held(None),
        ];
        let holders: Vec<&Listed> = all.iter().collect();
        assert_eq!(other_instances("worldstone-azerothcore", &holders), ["mangos-zero"]);
        assert_eq!(other_instances("worldstone-mangos-zero", &holders), ["azerothcore"]);
        assert_eq!(other_instances("worldstone-other", &holders), ["azerothcore", "mangos-zero"]);
        assert!(other_instances("worldstone-azerothcore", &[]).is_empty());
    }

    #[test]
    fn log_lines_are_cleaned_for_showing() {
        assert_eq!(clean_log_line("\u{1b}[32mLoading Creature templates...\u{1b}[0m").as_deref(), Some("Loading Creature templates..."));
        assert_eq!(clean_log_line("  spaced \t out  ").as_deref(), Some("spaced  out"));
        assert_eq!(clean_log_line("\u{1b}[0m   "), None);
        assert_eq!(clean_log_line(&"x".repeat(200)).unwrap().chars().count(), 90);
    }

    #[test]
    fn a_failed_start_says_why_in_words() {
        let recipe = zero();
        let world = recipe.run.services.iter().find(|s| s.id == "mangosd").unwrap();
        let tail = vec!["Can't connect to MySQL".to_string(), "Cannot connect to world database".to_string()];
        let text = explain_exit(world, &seen(ServiceState::Crashed, Some(1)), &tail);
        assert!(text.contains("world server did not stay up") && text.contains("exited with code 1") && text.contains("Cannot connect to world database"), "{text}");
        assert!(explain_exit(world, &seen(ServiceState::Crashed, Some(137)), &[]).contains("memory"));
        assert!(explain_exit(world, &seen(ServiceState::Missing, None), &[]).contains("gone"));
        assert!(explain_exit(world, &seen(ServiceState::Stopped, Some(0)), &[]).contains("exited with code 0"));
    }

    #[test]
    fn a_stop_is_clean_only_when_every_server_exited_normally() {
        let clean = vec![("Login server".to_string(), seen(ServiceState::Stopped, Some(0))), ("World server".to_string(), seen(ServiceState::Stopped, Some(0)))];
        assert!(unclean_stops(&clean).is_empty());
        let killed = vec![("World server".to_string(), seen(ServiceState::Crashed, Some(137)))];
        assert!(unclean_stops(&killed)[0].contains("killed"), "{:?}", unclean_stops(&killed));
        let odd = vec![("Login server".to_string(), seen(ServiceState::Crashed, Some(143)))];
        assert!(unclean_stops(&odd)[0].contains("code 143"));
        let running = vec![("World server".to_string(), seen(ServiceState::Running, None))];
        assert!(unclean_stops(&running)[0].contains("still running"), "{:?}", unclean_stops(&running));
    }

    #[test]
    fn compose_commands_name_the_project_and_run_beside_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::at(dir.path().to_path_buf()).instance("mangos-zero").unwrap();
        let file = compose::file(&paths);
        let spec = compose_spec(&paths, "mangos-zero", &file, &["up", "-d", "realmd"], Duration::from_secs(5));
        assert_eq!(spec.program, "docker");
        assert_eq!(spec.args[..5], ["compose", "--progress", "quiet", "-p", "worldstone-mangos-zero"]);
        assert_eq!(spec.args[5], "-f");
        assert_eq!(spec.args[7..], ["up", "-d", "realmd"]);
        assert_eq!(spec.cwd.as_deref(), Some(paths.docker.as_path()));
    }
}
